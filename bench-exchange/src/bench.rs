#![allow(clippy::useless_attribute)]
#![allow(clippy::integer_arithmetic)]

use crate::order_book::*;
use itertools::izip;
use log::*;
use rand::{thread_rng, Rng};
use rayon::prelude::*;
use solana_client::perf_utils::{sample_txs, SampleStats};
use solana_core::gen_keys::GenKeys;
use solana_exchange_program::{exchange_instruction, exchange_state::*, id};
use solana_faucet::faucet::request_airdrop_transaction;
use solana_genesis::Base64Account;
use solana_metrics::datapoint_info;
use solana_sdk::{
    client::{Client, SyncClient},
    commitment_config::CommitmentConfig,
    message::Message,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    timing::{duration_as_ms, duration_as_s},
    transaction::Transaction,
    {system_instruction, system_program},
};
use std::{
    cmp,
    collections::{HashMap, VecDeque},
    fs::File,
    io::prelude::*,
    mem,
    net::SocketAddr,
    path::Path,
    process::exit,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc, RwLock,
    },
    thread::{sleep, Builder},
    time::{Duration, Instant},
};

// TODO Chunk length as specified results in a bunch of failures, divide by 10 helps...
// Assume 4MB network buffers, and 512 byte packets
const FUND_CHUNK_LEN: usize = 4 * 1024 * 1024 / 512;

// Maximum system transfers per transaction
const MAX_TRANSFERS_PER_TX: u64 = 4;

pub type SharedTransactions = Arc<RwLock<VecDeque<Vec<Transaction>>>>;

pub struct Config {
    pub identity: Keypair,
    pub threads: usize,
    pub duration: Duration,
    pub transfer_delay: u64,
    pub fund_amount: u64,
    pub batch_size: usize,
    pub chunk_size: usize,
    pub account_groups: usize,
    pub client_ids_and_stake_file: String,
    pub read_from_client_file: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            identity: Keypair::new(),
            threads: 4,
            duration: Duration::new(u64::max_value(), 0),
            transfer_delay: 0,
            fund_amount: 100_000,
            batch_size: 10,
            chunk_size: 10,
            account_groups: 100,
            client_ids_and_stake_file: String::new(),
            read_from_client_file: false,
        }
    }
}

pub fn create_client_accounts_file(
    client_ids_and_stake_file: &str,
    batch_size: usize,
    account_groups: usize,
    fund_amount: u64,
) {
    let accounts_in_groups = batch_size * account_groups;
    const NUM_KEYPAIR_GROUPS: u64 = 2;
    let total_keys = accounts_in_groups as u64 * NUM_KEYPAIR_GROUPS;

    let keypairs = generate_keypairs(total_keys);

    let mut accounts = HashMap::new();
    keypairs.iter().for_each(|keypair| {
        accounts.insert(
            serde_json::to_string(&keypair.to_bytes().to_vec()).unwrap(),
            Base64Account {
                balance: fund_amount,
                executable: false,
                owner: system_program::id().to_string(),
                data: String::new(),
            },
        );
    });

    let serialized = serde_yaml::to_string(&accounts).unwrap();
    let path = Path::new(&client_ids_and_stake_file);
    let mut file = File::create(path).unwrap();
    file.write_all(&serialized.into_bytes()).unwrap();
}

pub fn do_bench_exchange<T>(clients: Vec<T>, config: Config)
where
    T: 'static + Client + Send + Sync,
{
    let Config {
        identity,
        threads,
        duration,
        transfer_delay,
        fund_amount,
        batch_size,
        chunk_size,
        account_groups,
        client_ids_and_stake_file,
        read_from_client_file,
    } = config;

    info!(
        "Exchange client: threads {} duration {} fund_amount {}",
        threads,
        duration_as_s(&duration),
        fund_amount
    );
    info!(
        "Exchange client: transfer delay {} batch size {} chunk size {}",
        transfer_delay, batch_size, chunk_size
    );

    let accounts_in_groups = batch_size * account_groups;
    const NUM_KEYPAIR_GROUPS: u64 = 2;
    let total_keys = accounts_in_groups as u64 * NUM_KEYPAIR_GROUPS;

    let mut signer_keypairs = if read_from_client_file {
        let path = Path::new(&client_ids_and_stake_file);
        let file = File::open(path).unwrap();

        let accounts: HashMap<String, Base64Account> = serde_yaml::from_reader(file).unwrap();
        accounts
            .into_iter()
            .map(|(keypair, _)| {
                let bytes: Vec<u8> = serde_json::from_str(keypair.as_str()).unwrap();
                Keypair::from_bytes(&bytes).unwrap()
            })
            .collect()
    } else {
        info!("Generating {:?} signer keys", total_keys);
        generate_keypairs(total_keys)
    };

    let trader_signers: Vec<_> = signer_keypairs
        .drain(0..accounts_in_groups)
        .map(Arc::new)
        .collect();
    let swapper_signers: Vec<_> = signer_keypairs
        .drain(0..accounts_in_groups)
        .map(Arc::new)
        .collect();

    let clients: Vec<_> = clients.into_iter().map(Arc::new).collect();
    let client = clients[0].as_ref();

    if !read_from_client_file {
        info!("Fund trader accounts");
        fund_keys(client, &identity, &trader_signers, fund_amount);
        info!("Fund swapper accounts");
        fund_keys(client, &identity, &swapper_signers, fund_amount);
    }

    info!("Generating {:?} account keys", total_keys);
    let mut account_keypairs = generate_keypairs(total_keys);
    let src_keypairs: Vec<_> = account_keypairs.drain(0..accounts_in_groups).collect();
    let src_pubkeys: Vec<Pubkey> = src_keypairs
        .iter()
        .map(|keypair| keypair.pubkey())
        .collect();

    let profit_keypairs: Vec<_> = account_keypairs.drain(0..accounts_in_groups).collect();
    let profit_pubkeys: Vec<Pubkey> = profit_keypairs
        .iter()
        .map(|keypair| keypair.pubkey())
        .collect();

    info!("Create {:?} source token accounts", src_pubkeys.len());
    create_token_accounts(client, &trader_signers, &src_keypairs);
    info!("Create {:?} profit token accounts", profit_pubkeys.len());
    create_token_accounts(client, &swapper_signers, &profit_keypairs);

    // Collect the max transaction rate and total tx count seen (single node only)
    let sample_stats = Arc::new(RwLock::new(Vec::new()));
    let sample_period = 1; // in seconds
    info!("Sampling clients for tps every {} s", sample_period);
    info!(
        "Requesting and swapping trades with {} ms delay per thread...",
        transfer_delay
    );

    let exit_signal = Arc::new(AtomicBool::new(false));
    let shared_txs: SharedTransactions = Arc::new(RwLock::new(VecDeque::new()));
    let total_txs_sent_count = Arc::new(AtomicUsize::new(0));
    let s_threads: Vec<_> = (0..threads)
        .map(|_| {
            let exit_signal = exit_signal.clone();
            let shared_txs = shared_txs.clone();
            let total_txs_sent_count = total_txs_sent_count.clone();
            let client = clients[0].clone();
            Builder::new()
                .name("solana-exchange-transfer".to_string())
                .spawn(move || {
                    do_tx_transfers(&exit_signal, &shared_txs, &total_txs_sent_count, &client)
                })
                .unwrap()
        })
        .collect();

    trace!("Start swapper thread");
    let (swapper_sender, swapper_receiver) = channel();
    let swapper_thread = {
        let exit_signal = exit_signal.clone();
        let shared_txs = shared_txs.clone();
        let client = clients[0].clone();
        Builder::new()
            .name("solana-exchange-swapper".to_string())
            .spawn(move || {
                swapper(
                    &exit_signal,
                    &swapper_receiver,
                    &shared_txs,
                    &swapper_signers,
                    &profit_pubkeys,
                    transfer_delay,
                    batch_size,
                    chunk_size,
                    account_groups,
                    &client,
                )
            })
            .unwrap()
    };

    trace!("Start trader thread");
    let trader_thread = {
        let exit_signal = exit_signal.clone();

        let client = clients[0].clone();
        Builder::new()
            .name("solana-exchange-trader".to_string())
            .spawn(move || {
                trader(
                    &exit_signal,
                    &swapper_sender,
                    &shared_txs,
                    &trader_signers,
                    &src_pubkeys,
                    transfer_delay,
                    batch_size,
                    chunk_size,
                    account_groups,
                    &client,
                )
            })
            .unwrap()
    };

    let sample_threads: Vec<_> = clients
        .iter()
        .map(|client| {
            let exit_signal = exit_signal.clone();
            let sample_stats = sample_stats.clone();
            let client = client.clone();
            Builder::new()
                .name("solana-exchange-sample".to_string())
                .spawn(move || sample_txs(&exit_signal, &sample_stats, sample_period, &client))
                .unwrap()
        })
        .collect();

    sleep(duration);

    info!("Stopping threads");
    exit_signal.store(true, Ordering::Relaxed);
    info!("Wait for trader thread");
    let _ = trader_thread.join();
    info!("Waiting for swapper thread");
    let _ = swapper_thread.join();
    info!("Wait for tx threads");
    for t in s_threads {
        let _ = t.join();
    }
    info!("Wait for sample threads");
    for t in sample_threads {
        let _ = t.join();
    }

    compute_and_report_stats(
        &sample_stats,
        total_txs_sent_count.load(Ordering::Relaxed) as u64,
    );
}

fn do_tx_transfers<T>(
    exit_signal: &Arc<AtomicBool>,
    shared_txs: &SharedTransactions,
    total_txs_sent_count: &Arc<AtomicUsize>,
    client: &Arc<T>,
) where
    T: Client,
{
    loop {
        let txs;
        {
            let mut shared_txs_wl = shared_txs.write().unwrap();
            txs = shared_txs_wl.pop_front();
        }
        if let Some(txs0) = txs {
            let n = txs0.len();

            let now = Instant::now();
            for tx in txs0 {
                client.async_send_transaction(tx).expect("Transfer");
            }
            let duration = now.elapsed();

            total_txs_sent_count.fetch_add(n, Ordering::Relaxed);
            datapoint_info!(
                "bench-exchange-do_tx_transfers",
                ("duration", duration_as_ms(&duration), i64),
                ("count", n, i64)
            );
        }
        if exit_signal.load(Ordering::Relaxed) {
            return;
        }
    }
}

struct TradeInfo {
    trade_account: Pubkey,
    order_info: OrderInfo,
}
#[allow(clippy::too_many_arguments)]
fn swapper<T>(
    exit_signal: &Arc<AtomicBool>,
    receiver: &Receiver<Vec<TradeInfo>>,
    shared_txs: &SharedTransactions,
    signers: &[Arc<Keypair>],
    profit_pubkeys: &[Pubkey],
    transfer_delay: u64,
    batch_size: usize,
    chunk_size: usize,
    account_groups: usize,
    client: &Arc<T>,
) where
    T: Client,
{
    let mut order_book = OrderBook::default();
    let mut account_group: usize = 0;

    let mut txs = 0;
    let mut total_txs = 0;
    let mut now = Instant::now();
    let start_time = now;
    let mut total_elapsed = start_time.elapsed();

    // Chunks may have been dropped and we don't want to wait a long time
    // for each time, Back-off each time we fail to confirm a chunk
    const CHECK_TX_TIMEOUT_MAX_MS: u64 = 15000;
    const CHECK_TX_DELAY_MS: u64 = 100;
    let mut max_tries = CHECK_TX_TIMEOUT_MAX_MS / CHECK_TX_DELAY_MS;

    // If we dump too many chunks maybe we are just waiting on a back-log
    // rather than a series of dropped packets, reset to max waits
    const MAX_DUMPS: u64 = 50;
    let mut dumps = 0;

    'outer: loop {
        if let Ok(trade_infos) = receiver.try_recv() {
            let mut tries = 0;
            let mut trade_index = 0;
            while client
                .get_balance_with_commitment(
                    &trade_infos[trade_index].trade_account,
                    CommitmentConfig::processed(),
                )
                .unwrap_or(0)
                == 0
            {
                tries += 1;
                if tries >= max_tries {
                    if exit_signal.load(Ordering::Relaxed) {
                        break 'outer;
                    }
                    error!("Give up and dump batch");
                    if dumps >= MAX_DUMPS {
                        error!("Max batches dumped, reset wait back-off");
                        max_tries = CHECK_TX_TIMEOUT_MAX_MS / CHECK_TX_DELAY_MS;
                        dumps = 0;
                    } else {
                        dumps += 1;
                        max_tries /= 2;
                    }
                    continue 'outer;
                }
                debug!("{} waiting for trades batch to clear", tries);
                sleep(Duration::from_millis(CHECK_TX_DELAY_MS));
                trade_index = thread_rng().gen_range(0, trade_infos.len());
            }
            max_tries = CHECK_TX_TIMEOUT_MAX_MS / CHECK_TX_DELAY_MS;
            dumps = 0;

            trade_infos.iter().for_each(|info| {
                order_book
                    .push(info.trade_account, info.order_info)
                    .expect("Failed to push to order_book");
            });
            let mut swaps = Vec::new();
            while let Some((to, from)) = order_book.pop() {
                swaps.push((to, from));
                if swaps.len() >= batch_size {
                    break;
                }
            }
            let swaps_size = swaps.len();

            let mut to_swap = vec![];
            let start = account_group * swaps_size as usize;
            let end = account_group * swaps_size as usize + batch_size as usize;
            for (signer, swap, profit) in izip!(
                signers[start..end].iter(),
                swaps,
                profit_pubkeys[start..end].iter(),
            ) {
                to_swap.push((signer, swap, profit));
            }
            account_group = (account_group + 1) % account_groups as usize;

            let (blockhash, _fee_calculator, _last_valid_slot) = client
                .get_recent_blockhash_with_commitment(CommitmentConfig::processed())
                .expect("Failed to get blockhash");
            let to_swap_txs: Vec<_> = to_swap
                .par_iter()
                .map(|(signer, swap, profit)| {
                    let s: &Keypair = &signer;
                    let owner = &signer.pubkey();
                    let instruction = exchange_instruction::swap_request(
                        owner,
                        &swap.0.pubkey,
                        &swap.1.pubkey,
                        &profit,
                    );
                    let message = Message::new(&[instruction], Some(&s.pubkey()));
                    Transaction::new(&[s], message, blockhash)
                })
                .collect();

            txs += to_swap_txs.len() as u64;
            total_txs += to_swap_txs.len() as u64;
            total_elapsed = start_time.elapsed();
            let duration = now.elapsed();
            if duration_as_s(&duration) >= 1_f32 {
                now = Instant::now();
                let tps = txs as f32 / duration_as_s(&duration);
                info!(
                    "Swapper {:9.2} TPS, Transactions: {:6}, Total transactions: {} over {} s",
                    tps,
                    txs,
                    total_txs,
                    total_elapsed.as_secs(),
                );
                txs = 0;
            }

            datapoint_info!("bench-exchange-swaps", ("count", to_swap_txs.len(), i64));

            let chunks: Vec<_> = to_swap_txs.chunks(chunk_size).collect();
            {
                let mut shared_txs_wl = shared_txs.write().unwrap();
                for chunk in chunks {
                    shared_txs_wl.push_back(chunk.to_vec());
                }
            }
            // Throttle the swapper so it doesn't try to catchup unbridled
            sleep(Duration::from_millis(transfer_delay / 2));
        }

        if exit_signal.load(Ordering::Relaxed) {
            break 'outer;
        }
    }
    info!(
        "Swapper sent {} at {:9.2} TPS",
        total_txs,
        total_txs as f32 / duration_as_s(&total_elapsed)
    );
    assert_eq!(
        order_book.get_num_outstanding().0 + order_book.get_num_outstanding().1,
        0
    );
}

#[allow(clippy::too_many_arguments)]
fn trader<T>(
    exit_signal: &Arc<AtomicBool>,
    sender: &Sender<Vec<TradeInfo>>,
    shared_txs: &SharedTransactions,
    signers: &[Arc<Keypair>],
    srcs: &[Pubkey],
    transfer_delay: u64,
    batch_size: usize,
    chunk_size: usize,
    account_groups: usize,
    client: &Arc<T>,
) where
    T: Client,
{
    // TODO Hard coded for now
    let pair = AssetPair::default();
    let tokens = 1;
    let price = 1000;
    let mut account_group: usize = 0;

    let mut txs = 0;
    let mut total_txs = 0;
    let mut now = Instant::now();
    let start_time = now;
    let mut total_elapsed = start_time.elapsed();

    loop {
        let trade_keys = generate_keypairs(batch_size as u64);

        let mut trades = vec![];
        let mut trade_infos = vec![];
        let start = account_group * batch_size as usize;
        let end = account_group * batch_size as usize + batch_size as usize;
        let mut side = OrderSide::Ask;
        for (signer, trade, src) in izip!(
            signers[start..end].iter(),
            trade_keys,
            srcs[start..end].iter(),
        ) {
            side = if side == OrderSide::Ask {
                OrderSide::Bid
            } else {
                OrderSide::Ask
            };
            let order_info = OrderInfo {
                /// Owner of the trade order
                owner: Pubkey::default(), // don't care
                side,
                pair,
                tokens,
                price,
                tokens_settled: 0,
            };
            trade_infos.push(TradeInfo {
                trade_account: trade.pubkey(),
                order_info,
            });
            trades.push((signer, trade, side, src));
        }
        account_group = (account_group + 1) % account_groups as usize;

        let (blockhash, _fee_calculator, _last_valid_slot) = client
            .get_recent_blockhash_with_commitment(CommitmentConfig::processed())
            .expect("Failed to get blockhash");

        trades.chunks(chunk_size).for_each(|chunk| {
            let trades_txs: Vec<_> = chunk
                .par_iter()
                .map(|(owner, trade, side, src)| {
                    let owner_pubkey = &owner.pubkey();
                    let trade_pubkey = &trade.pubkey();
                    let space = mem::size_of::<ExchangeState>() as u64;
                    let instructions = [
                        system_instruction::create_account(
                            owner_pubkey,
                            trade_pubkey,
                            1,
                            space,
                            &id(),
                        ),
                        exchange_instruction::trade_request(
                            owner_pubkey,
                            trade_pubkey,
                            *side,
                            pair,
                            tokens,
                            price,
                            src,
                        ),
                    ];
                    let message = Message::new(&instructions, Some(&owner_pubkey));
                    Transaction::new(&[owner.as_ref(), trade], message, blockhash)
                })
                .collect();

            {
                txs += chunk_size as u64;
                total_txs += chunk_size as u64;
                total_elapsed = start_time.elapsed();
                let duration = now.elapsed();
                if duration_as_s(&duration) >= 1_f32 {
                    now = Instant::now();
                    let tps = txs as f32 / duration_as_s(&duration);
                    info!(
                        "Trader  {:9.2} TPS, Transactions: {:6}, Total transactions: {} over {} s",
                        tps,
                        txs,
                        total_txs,
                        total_elapsed.as_secs(),
                    );
                    txs = 0;
                }

                datapoint_info!("bench-exchange-trades", ("count", trades_txs.len(), i64));

                {
                    let mut shared_txs_wl = shared_txs
                        .write()
                        .expect("Failed to send tx to transfer threads");
                    shared_txs_wl.push_back(trades_txs);
                }
            }
            if transfer_delay > 0 {
                sleep(Duration::from_millis(transfer_delay));
            }
        });

        if exit_signal.load(Ordering::Relaxed) {
            info!(
                "Trader sent {} at {:9.2} TPS",
                total_txs,
                total_txs as f32 / duration_as_s(&total_elapsed)
            );
            return;
        }

        // TODO chunk the trade infos and send them when the batch is sent
        sender
            .send(trade_infos)
            .expect("Failed to send trades to swapper");
    }
}

fn verify_transaction<T>(sync_client: &T, tx: &Transaction) -> bool
where
    T: SyncClient + ?Sized,
{
    for s in &tx.signatures {
        if let Ok(Some(r)) =
            sync_client.get_signature_status_with_commitment(s, CommitmentConfig::processed())
        {
            match r {
                Ok(_) => {
                    return true;
                }
                Err(e) => {
                    info!("error: {:?}", e);
                }
            }
        }
    }
    false
}

fn verify_funding_transfer<T: SyncClient + ?Sized>(
    client: &T,
    tx: &Transaction,
    amount: u64,
) -> bool {
    if verify_transaction(client, tx) {
        for a in &tx.message().account_keys[1..] {
            if client
                .get_balance_with_commitment(a, CommitmentConfig::processed())
                .unwrap_or(0)
                >= amount
            {
                return true;
            }
        }
    }
    false
}

pub fn fund_keys<T: Client>(client: &T, source: &Keypair, dests: &[Arc<Keypair>], lamports: u64) {
    let total = lamports * (dests.len() as u64 + 1);
    let mut funded: Vec<(&Keypair, u64)> = vec![(source, total)];
    let mut notfunded: Vec<&Arc<Keypair>> = dests.iter().collect();

    info!(
        "  Funding {} keys with {} lamports each",
        dests.len(),
        lamports
    );
    while !notfunded.is_empty() {
        if funded.is_empty() {
            panic!("No funded accounts left to fund remaining");
        }
        let mut new_funded: Vec<(&Keypair, u64)> = vec![];
        let mut to_fund = vec![];
        debug!("  Creating from... {}", funded.len());
        for f in &mut funded {
            let max_units = cmp::min(
                cmp::min(notfunded.len() as u64, MAX_TRANSFERS_PER_TX),
                (f.1 - lamports) / lamports,
            );
            if max_units == 0 {
                continue;
            }
            let per_unit = ((f.1 - lamports) / lamports / max_units) * lamports;
            f.1 -= per_unit * max_units;
            let start = notfunded.len() - max_units as usize;
            let moves: Vec<_> = notfunded[start..]
                .iter()
                .map(|k| (k.pubkey(), per_unit))
                .collect();
            notfunded[start..]
                .iter()
                .for_each(|k| new_funded.push((k, per_unit)));
            notfunded.truncate(start);
            if !moves.is_empty() {
                to_fund.push((f.0, moves));
            }
        }

        to_fund.chunks(FUND_CHUNK_LEN).for_each(|chunk| {
            #[allow(clippy::clone_double_ref)] // sigh
            let mut to_fund_txs: Vec<_> = chunk
                .par_iter()
                .map(|(k, m)| {
                    let instructions = system_instruction::transfer_many(&k.pubkey(), &m);
                    let message = Message::new(&instructions, Some(&k.pubkey()));
                    (k.clone(), Transaction::new_unsigned(message))
                })
                .collect();

            let mut retries = 0;
            let amount = chunk[0].1[0].1;
            while !to_fund_txs.is_empty() {
                let receivers: usize = to_fund_txs
                    .iter()
                    .map(|(_, tx)| tx.message().instructions.len())
                    .sum();

                debug!(
                    "  {} to {} in {} txs",
                    if retries == 0 {
                        "  Transferring"
                    } else {
                        "  Retrying"
                    },
                    receivers,
                    to_fund_txs.len(),
                );

                let (blockhash, _fee_calculator, _last_valid_slot) = client
                    .get_recent_blockhash_with_commitment(CommitmentConfig::processed())
                    .expect("blockhash");
                to_fund_txs.par_iter_mut().for_each(|(k, tx)| {
                    tx.sign(&[*k], blockhash);
                });
                to_fund_txs.iter().for_each(|(_, tx)| {
                    client.async_send_transaction(tx.clone()).expect("transfer");
                });

                let mut waits = 0;
                loop {
                    sleep(Duration::from_millis(200));
                    to_fund_txs.retain(|(_, tx)| !verify_funding_transfer(client, &tx, amount));
                    if to_fund_txs.is_empty() {
                        break;
                    }
                    debug!(
                        "    {} transactions outstanding, {:?} waits",
                        to_fund_txs.len(),
                        waits
                    );
                    waits += 1;
                    if waits >= 5 {
                        break;
                    }
                }
                if !to_fund_txs.is_empty() {
                    retries += 1;
                    debug!("  Retry {:?}", retries);
                    if retries >= 10 {
                        error!("fund_keys: Too many retries ({}), give up", retries);
                        exit(1);
                    }
                }
            }
        });
        funded.append(&mut new_funded);
        funded.retain(|(k, b)| {
            client
                .get_balance_with_commitment(&k.pubkey(), CommitmentConfig::processed())
                .unwrap_or(0)
                > lamports
                && *b > lamports
        });
        debug!("  Funded: {} left: {}", funded.len(), notfunded.len());
    }
}

pub fn create_token_accounts<T: Client>(
    client: &T,
    signers: &[Arc<Keypair>],
    accounts: &[Keypair],
) {
    let mut notfunded: Vec<(&Arc<Keypair>, &Keypair)> = signers.iter().zip(accounts).collect();

    while !notfunded.is_empty() {
        notfunded.chunks(FUND_CHUNK_LEN).for_each(|chunk| {
            let mut to_create_txs: Vec<_> = chunk
                .par_iter()
                .map(|(from_keypair, new_keypair)| {
                    let owner_pubkey = &from_keypair.pubkey();
                    let space = mem::size_of::<ExchangeState>() as u64;
                    let create_ix = system_instruction::create_account(
                        owner_pubkey,
                        &new_keypair.pubkey(),
                        1,
                        space,
                        &id(),
                    );
                    let request_ix =
                        exchange_instruction::account_request(owner_pubkey, &new_keypair.pubkey());
                    let message = Message::new(&[create_ix, request_ix], Some(&owner_pubkey));
                    (
                        (from_keypair, new_keypair),
                        Transaction::new_unsigned(message),
                    )
                })
                .collect();

            let accounts: usize = to_create_txs
                .iter()
                .map(|(_, tx)| tx.message().instructions.len() / 2)
                .sum();

            debug!(
                "  Creating {} accounts in {} txs",
                accounts,
                to_create_txs.len(),
            );

            let mut retries = 0;
            while !to_create_txs.is_empty() {
                let (blockhash, _fee_calculator, _last_valid_slot) = client
                    .get_recent_blockhash_with_commitment(CommitmentConfig::processed())
                    .expect("Failed to get blockhash");
                to_create_txs
                    .par_iter_mut()
                    .for_each(|((from_keypair, to_keypair), tx)| {
                        tx.sign(&[from_keypair.as_ref(), to_keypair], blockhash);
                    });
                to_create_txs.iter().for_each(|(_, tx)| {
                    client.async_send_transaction(tx.clone()).expect("transfer");
                });

                let mut waits = 0;
                while !to_create_txs.is_empty() {
                    sleep(Duration::from_millis(200));
                    to_create_txs.retain(|(_, tx)| !verify_transaction(client, &tx));
                    if to_create_txs.is_empty() {
                        break;
                    }
                    info!(
                        "    {} transactions outstanding, waits {:?}",
                        to_create_txs.len(),
                        waits
                    );
                    waits += 1;
                    if waits >= 5 {
                        break;
                    }
                }

                if !to_create_txs.is_empty() {
                    retries += 1;
                    info!("  Retry {:?} {} txes left", retries, to_create_txs.len());
                    if retries >= 20 {
                        error!(
                            "create_token_accounts: Too many retries ({}), give up",
                            retries
                        );
                        exit(1);
                    }
                }
            }
        });

        let mut new_notfunded: Vec<(&Arc<Keypair>, &Keypair)> = vec![];
        for f in &notfunded {
            if client
                .get_balance_with_commitment(&f.1.pubkey(), CommitmentConfig::processed())
                .unwrap_or(0)
                == 0
            {
                new_notfunded.push(*f)
            }
        }
        notfunded = new_notfunded;
        debug!("  Left: {}", notfunded.len());
    }
}

fn compute_and_report_stats(maxes: &Arc<RwLock<Vec<(String, SampleStats)>>>, total_txs_sent: u64) {
    let mut max_txs = 0;
    let mut max_elapsed = Duration::new(0, 0);
    info!("|       Max TPS | Total Transactions");
    info!("+---------------+--------------------");

    for (_sock, stats) in maxes.read().unwrap().iter() {
        let maybe_flag = match stats.txs {
            0 => "!!!!!",
            _ => "",
        };

        info!("| {:13.2} | {} {}", stats.tps, stats.txs, maybe_flag);

        if stats.elapsed > max_elapsed {
            max_elapsed = stats.elapsed;
        }
        if stats.txs > max_txs {
            max_txs = stats.txs;
        }
    }
    info!("+---------------+--------------------");

    if max_txs >= total_txs_sent {
        info!(
            "Warning: Average TPS might be under reported, there were no txs sent for a portion of the duration"
        );
        max_txs = total_txs_sent;
    }
    info!(
        "{} txs outstanding when test ended (lag) ({:.2}%)",
        total_txs_sent - max_txs,
        (total_txs_sent - max_txs) as f64 / total_txs_sent as f64 * 100_f64
    );
    info!(
        "\tAverage TPS: {:.2}",
        max_txs as f32 / max_elapsed.as_secs() as f32
    );
}

fn generate_keypairs(num: u64) -> Vec<Keypair> {
    let mut seed = [0_u8; 32];
    seed.copy_from_slice(&Keypair::new().pubkey().as_ref());
    let mut rnd = GenKeys::new(seed);
    rnd.gen_n_keypairs(num)
}

pub fn airdrop_lamports<T: Client>(
    client: &T,
    faucet_addr: &SocketAddr,
    id: &Keypair,
    amount: u64,
) {
    let balance = client.get_balance_with_commitment(&id.pubkey(), CommitmentConfig::processed());
    let balance = balance.unwrap_or(0);
    if balance >= amount {
        return;
    }

    let amount_to_drop = amount - balance;

    info!(
        "Airdropping {:?} lamports from {} for {}",
        amount_to_drop,
        faucet_addr,
        id.pubkey(),
    );

    let mut tries = 0;
    loop {
        let (blockhash, _fee_calculator, _last_valid_slot) = client
            .get_recent_blockhash_with_commitment(CommitmentConfig::processed())
            .expect("Failed to get blockhash");
        match request_airdrop_transaction(&faucet_addr, &id.pubkey(), amount_to_drop, blockhash) {
            Ok(transaction) => {
                let signature = client.async_send_transaction(transaction).unwrap();

                for _ in 0..30 {
                    if let Ok(Some(_)) = client.get_signature_status_with_commitment(
                        &signature,
                        CommitmentConfig::processed(),
                    ) {
                        break;
                    }
                    sleep(Duration::from_millis(100));
                }
                if client
                    .get_balance_with_commitment(&id.pubkey(), CommitmentConfig::processed())
                    .unwrap_or(0)
                    >= amount
                {
                    break;
                }
            }
            Err(err) => {
                panic!(
                    "Error requesting airdrop: {:?} to addr: {:?} amount: {}",
                    err, faucet_addr, amount
                );
            }
        };
        debug!("  Retry...");
        tries += 1;
        if tries > 50 {
            error!("airdrop_lamports: Too many retries ({}), give up", tries);
            exit(1);
        }
        sleep(Duration::from_secs(2));
    }
}
