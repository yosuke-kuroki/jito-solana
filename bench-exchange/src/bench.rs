#![allow(clippy::useless_attribute)]

use crate::order_book::*;
use itertools::izip;
use log::*;
use rayon::prelude::*;
use solana::gen_keys::GenKeys;
use solana_drone::drone::request_airdrop_transaction;
use solana_exchange_api::exchange_instruction;
use solana_exchange_api::exchange_state::*;
use solana_exchange_api::id;
use solana_sdk::client::Client;
use solana_sdk::client::{AsyncClient, SyncClient};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, KeypairUtil};
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;
use std::cmp;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::process::exit;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{mem, thread};

// TODO Chunk length as specified results in a bunch of failures, divide by 10 helps...
// Assume 4MB network buffers, and 512 byte packets
const CHUNK_LEN: usize = 4 * 1024 * 1024 / 512 / 10;

// Maximum system transfers per transaction
const MAX_TRANSFERS_PER_TX: u64 = 4;

// Interval between fetching a new blockhash
const BLOCKHASH_RENEW_PERIOD_S: u64 = 30;

pub type SharedTransactions = Arc<RwLock<VecDeque<Vec<Transaction>>>>;

pub struct Config {
    pub identity: Keypair,
    pub threads: usize,
    pub duration: Duration,
    pub trade_delay: u64,
    pub fund_amount: u64,
    pub batch_size: usize,
    pub account_groups: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            identity: Keypair::new(),
            threads: 4,
            duration: Duration::new(u64::max_value(), 0),
            trade_delay: 0,
            fund_amount: 100_000,
            batch_size: 10,
            account_groups: 100,
        }
    }
}

#[derive(Default)]
pub struct SampleStats {
    /// Maximum TPS reported by this node
    pub tps: f64,
    /// Total time taken for those txs
    pub tx_time: Duration,
    /// Total transactions reported by this node
    pub tx_count: u64,
}

pub fn do_bench_exchange<F, T>(client_ctors: Vec<F>, config: Config)
where
    F: Fn() -> T,
    F: 'static + std::marker::Sync + std::marker::Send,
    T: Client,
{
    let Config {
        identity,
        threads,
        duration,
        trade_delay,
        fund_amount,
        batch_size,
        account_groups,
    } = config;
    let accounts_in_groups = batch_size * account_groups;
    let exit_signal = Arc::new(AtomicBool::new(false));
    let client_ctors: Vec<_> = client_ctors.into_iter().map(Arc::new).collect();
    let client = client_ctors[0]();

    let total_keys = accounts_in_groups as u64 * 5;
    info!("Generating {:?} keys", total_keys);
    let mut keypairs = generate_keypairs(total_keys);
    let trader_signers: Vec<_> = keypairs
        .drain(0..accounts_in_groups)
        .map(Arc::new)
        .collect();
    let swapper_signers: Vec<_> = keypairs
        .drain(0..accounts_in_groups)
        .map(Arc::new)
        .collect();
    let src_pubkeys: Vec<_> = keypairs
        .drain(0..accounts_in_groups)
        .map(|keypair| keypair.pubkey())
        .collect();
    let dst_pubkeys: Vec<_> = keypairs
        .drain(0..accounts_in_groups)
        .map(|keypair| keypair.pubkey())
        .collect();
    let profit_pubkeys: Vec<_> = keypairs
        .drain(0..accounts_in_groups)
        .map(|keypair| keypair.pubkey())
        .collect();

    info!("Fund trader accounts");
    fund_keys(&client, &identity, &trader_signers, fund_amount);
    info!("Fund swapper accounts");
    fund_keys(&client, &identity, &swapper_signers, fund_amount);

    info!("Create {:?} source token accounts", src_pubkeys.len());
    create_token_accounts(&client, &trader_signers, &src_pubkeys);
    info!("Create {:?} destination token accounts", dst_pubkeys.len());
    create_token_accounts(&client, &trader_signers, &dst_pubkeys);
    info!("Create {:?} profit token accounts", profit_pubkeys.len());
    create_token_accounts(&client, &swapper_signers, &profit_pubkeys);

    // Collect the max transaction rate and total tx count seen (single node only)
    let sample_stats = Arc::new(RwLock::new(Vec::new()));
    let sample_period = 1; // in seconds
    info!("Sampling clients for tps every {} s", sample_period);

    let sample_threads: Vec<_> = client_ctors
        .iter()
        .map(|ctor| {
            let exit_signal = exit_signal.clone();
            let sample_stats = sample_stats.clone();
            let client_ctor = ctor.clone();
            thread::spawn(move || {
                sample_tx_count(&exit_signal, &sample_stats, sample_period, &client_ctor)
            })
        })
        .collect();

    let shared_txs: SharedTransactions = Arc::new(RwLock::new(VecDeque::new()));
    let shared_tx_active_thread_count = Arc::new(AtomicIsize::new(0));
    let total_tx_sent_count = Arc::new(AtomicUsize::new(0));
    let s_threads: Vec<_> = (0..threads)
        .map(|_| {
            let exit_signal = exit_signal.clone();
            let shared_txs = shared_txs.clone();
            let shared_tx_active_thread_count = shared_tx_active_thread_count.clone();
            let total_tx_sent_count = total_tx_sent_count.clone();
            let client_ctor = client_ctors[0].clone();
            thread::spawn(move || {
                do_tx_transfers(
                    &exit_signal,
                    &shared_txs,
                    &shared_tx_active_thread_count,
                    &total_tx_sent_count,
                    &client_ctor,
                )
            })
        })
        .collect();

    trace!("Start swapper thread");
    let (swapper_sender, swapper_receiver) = channel();
    let swapper_thread = {
        let exit_signal = exit_signal.clone();
        let shared_txs = shared_txs.clone();
        let shared_tx_active_thread_count = shared_tx_active_thread_count.clone();
        let client_ctor = client_ctors[0].clone();
        thread::spawn(move || {
            swapper(
                &exit_signal,
                &swapper_receiver,
                &shared_txs,
                &shared_tx_active_thread_count,
                &swapper_signers,
                &profit_pubkeys,
                batch_size,
                account_groups,
                &client_ctor,
            )
        })
    };

    trace!("Start trader thread");
    let trader_thread = {
        let exit_signal = exit_signal.clone();
        let shared_txs = shared_txs.clone();
        let shared_tx_active_thread_count = shared_tx_active_thread_count.clone();
        let client_ctor = client_ctors[0].clone();
        thread::spawn(move || {
            trader(
                &exit_signal,
                &swapper_sender,
                &shared_txs,
                &shared_tx_active_thread_count,
                &trader_signers,
                &src_pubkeys,
                &dst_pubkeys,
                trade_delay,
                batch_size,
                account_groups,
                &client_ctor,
            )
        })
    };

    info!("Requesting and swapping trades");
    sleep(duration);

    exit_signal.store(true, Ordering::Relaxed);
    let _ = trader_thread.join();
    let _ = swapper_thread.join();
    for t in s_threads {
        let _ = t.join();
    }
    for t in sample_threads {
        let _ = t.join();
    }

    compute_and_report_stats(&sample_stats, total_tx_sent_count.load(Ordering::Relaxed));
}

fn sample_tx_count<F, T>(
    exit_signal: &Arc<AtomicBool>,
    sample_stats: &Arc<RwLock<Vec<SampleStats>>>,
    sample_period: u64,
    client_ctor: &Arc<F>,
) where
    F: Fn() -> T,
    T: Client,
{
    let client = client_ctor();
    let mut max_tps = 0.0;
    let mut total_tx_time;
    let mut total_tx_count;
    let mut now = Instant::now();
    let start_time = now;
    let mut initial_tx_count = client.get_transaction_count().expect("transaction count");
    let first_tx_count = initial_tx_count;

    loop {
        let tx_count = client.get_transaction_count().expect("transaction count");
        let duration = now.elapsed();
        now = Instant::now();
        assert!(
            tx_count >= initial_tx_count,
            "expected tx_count({}) >= initial_tx_count({})",
            tx_count,
            initial_tx_count
        );
        let sample = tx_count - initial_tx_count;
        initial_tx_count = tx_count;

        let ns = duration.as_secs() * 1_000_000_000 + u64::from(duration.subsec_nanos());
        let tps = (sample * 1_000_000_000) as f64 / ns as f64;
        if tps > max_tps {
            max_tps = tps;
        }
        total_tx_time = start_time.elapsed();
        total_tx_count = tx_count - first_tx_count;
        trace!(
            "Sampler {:9.2} TPS, Transactions: {:6}, Total transactions: {} over {} s",
            tps,
            sample,
            total_tx_count,
            total_tx_time.as_secs(),
        );

        if exit_signal.load(Ordering::Relaxed) {
            let stats = SampleStats {
                tps: max_tps,
                tx_time: total_tx_time,
                tx_count: total_tx_count,
            };
            sample_stats.write().unwrap().push(stats);
            break;
        }
        sleep(Duration::from_secs(sample_period));
    }
}

fn do_tx_transfers<F, T>(
    exit_signal: &Arc<AtomicBool>,
    shared_txs: &SharedTransactions,
    shared_tx_thread_count: &Arc<AtomicIsize>,
    total_tx_sent_count: &Arc<AtomicUsize>,
    client_ctor: &Arc<F>,
) where
    F: Fn() -> T,
    T: Client,
{
    let client = client_ctor();
    let async_client: &AsyncClient = &client;
    let mut stats = Stats::default();
    loop {
        let txs;
        {
            let mut shared_txs_wl = shared_txs.write().unwrap();
            txs = shared_txs_wl.pop_front();
        }
        match txs {
            Some(txs0) => {
                let n = txs0.len();

                shared_tx_thread_count.fetch_add(1, Ordering::Relaxed);
                let now = Instant::now();
                for tx in txs0 {
                    async_client.async_send_transaction(tx).expect("Transfer");
                }
                let duration = now.elapsed();
                shared_tx_thread_count.fetch_add(-1, Ordering::Relaxed);

                total_tx_sent_count.fetch_add(n, Ordering::Relaxed);
                stats.total += n as u64;
                let sent_ns =
                    duration.as_secs() * 1_000_000_000 + u64::from(duration.subsec_nanos());
                stats.sent_ns += sent_ns;
                let rate = (n as f64 / sent_ns as f64) * 1_000_000_000_f64;
                if rate > stats.sent_peak_rate {
                    stats.sent_peak_rate = rate;
                }
                trace!("  tx {:?} sent     {:.2}/s", n, rate);
            }
            None => {
                if exit_signal.load(Ordering::Relaxed) {
                    info!(
                        "  Thread Transferred {} Txs, avg {:.2}/s peak {:.2}/s",
                        stats.total,
                        (stats.total as f64 / stats.sent_ns as f64) * 1_000_000_000_f64,
                        stats.sent_peak_rate,
                    );
                    break;
                }
            }
        }
    }
}

#[derive(Default)]
struct Stats {
    total: u64,
    keygen_ns: u64,
    keygen_peak_rate: f64,
    sign_ns: u64,
    sign_peak_rate: f64,
    sent_ns: u64,
    sent_peak_rate: f64,
}

struct TradeInfo {
    trade_account: Pubkey,
    order_info: TradeOrderInfo,
}
#[allow(clippy::too_many_arguments)]
fn swapper<F, T>(
    exit_signal: &Arc<AtomicBool>,
    receiver: &Receiver<Vec<TradeInfo>>,
    shared_txs: &SharedTransactions,
    shared_tx_active_thread_count: &Arc<AtomicIsize>,
    signers: &[Arc<Keypair>],
    profit_pubkeys: &[Pubkey],
    batch_size: usize,
    account_groups: usize,
    client_ctor: &Arc<F>,
) where
    F: Fn() -> T,
    T: Client,
{
    let client = client_ctor();
    let mut stats = Stats::default();
    let mut order_book = OrderBook::default();
    let mut account_group: usize = 0;
    let mut one_more_time = true;
    let mut blockhash = client
        .get_recent_blockhash()
        .expect("Failed to get blockhash");
    let mut blockhash_now = UNIX_EPOCH;
    'outer: loop {
        if let Ok(trade_infos) = receiver.try_recv() {
            let mut tries = 0;
            while client
                .get_balance(&trade_infos[0].trade_account)
                .unwrap_or(0)
                == 0
            {
                tries += 1;
                if tries > 10 {
                    debug!("Give up waiting, dump batch");
                    continue 'outer;
                }
                debug!("{} waiting for trades batch to clear", tries);
                sleep(Duration::from_millis(100));
            }

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
            stats.total += swaps_size as u64;

            let now = Instant::now();
            let swap_keys = generate_keypairs(swaps_size as u64);

            let mut to_swap = vec![];
            let start = account_group * swaps_size as usize;
            let end = account_group * swaps_size as usize + batch_size as usize;
            for (signer, swap, swap_key, profit) in izip!(
                signers[start..end].iter(),
                swaps,
                swap_keys,
                profit_pubkeys[start..end].iter(),
            ) {
                to_swap.push((signer, swap_key, swap, profit));
            }
            account_group = (account_group + 1) % account_groups as usize;
            let duration = now.elapsed();
            let keypair_ns =
                duration.as_secs() * 1_000_000_000 + u64::from(duration.subsec_nanos());
            let rate = (swaps_size as f64 / keypair_ns as f64) * 1_000_000_000_f64;
            stats.keygen_ns += keypair_ns;
            if rate > stats.keygen_peak_rate {
                stats.keygen_peak_rate = rate;
            }
            trace!("sw {:?} keypairs {:.2} /s", swaps_size, rate);

            let now = Instant::now();

            // Don't get a blockhash every time
            if SystemTime::now()
                .duration_since(blockhash_now)
                .unwrap()
                .as_secs()
                > BLOCKHASH_RENEW_PERIOD_S
            {
                blockhash = client
                    .get_recent_blockhash()
                    .expect("Failed to get blockhash");
                blockhash_now = SystemTime::now();
            }

            let to_swap_txs: Vec<_> = to_swap
                .par_iter()
                .map(|(signer, swap_key, swap, profit)| {
                    let s: &Keypair = &signer;
                    let owner = &signer.pubkey();
                    let space = mem::size_of::<ExchangeState>() as u64;
                    Transaction::new_signed_instructions(
                        &[s],
                        vec![
                            system_instruction::create_account(
                                owner,
                                &swap_key.pubkey(),
                                1,
                                space,
                                &id(),
                            ),
                            exchange_instruction::swap_request(
                                owner,
                                &swap_key.pubkey(),
                                &swap.0.pubkey,
                                &swap.1.pubkey,
                                &swap.0.info.dst_account,
                                &swap.1.info.dst_account,
                                &profit,
                            ),
                        ],
                        blockhash,
                    )
                })
                .collect();
            let duration = now.elapsed();
            let sign_ns = duration.as_secs() * 1_000_000_000 + u64::from(duration.subsec_nanos());
            let n = to_swap_txs.len();
            let rate = (n as f64 / sign_ns as f64) * 1_000_000_000_f64;
            stats.sign_ns += sign_ns;
            if rate > stats.sign_peak_rate {
                stats.sign_peak_rate = rate;
            }
            trace!("  sw {:?} signed   {:.2} /s ", n, rate);

            let chunks: Vec<_> = to_swap_txs.chunks(CHUNK_LEN).collect();
            {
                let mut shared_txs_wl = shared_txs.write().unwrap();
                for chunk in chunks {
                    shared_txs_wl.push_back(chunk.to_vec());
                }
            }
        }

        while shared_tx_active_thread_count.load(Ordering::Relaxed) > 0 {
            sleep(Duration::from_millis(100));
        }

        if exit_signal.load(Ordering::Relaxed) {
            if !one_more_time {
                info!("{} Swaps with batch size {}", stats.total, batch_size);
                info!(
                    "  Keygen avg {:.2}/s peak {:.2}/s",
                    (stats.total as f64 / stats.keygen_ns as f64) * 1_000_000_000_f64,
                    stats.keygen_peak_rate
                );
                info!(
                    "  Signed avg {:.2}/s peak {:.2}/s",
                    (stats.total as f64 / stats.sign_ns as f64) * 1_000_000_000_f64,
                    stats.sign_peak_rate
                );
                assert_eq!(
                    order_book.get_num_outstanding().0 + order_book.get_num_outstanding().1,
                    0
                );
                break;
            }
            // Grab any outstanding trades
            sleep(Duration::from_secs(2));
            one_more_time = false;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn trader<F, T>(
    exit_signal: &Arc<AtomicBool>,
    sender: &Sender<Vec<TradeInfo>>,
    shared_txs: &SharedTransactions,
    shared_tx_active_thread_count: &Arc<AtomicIsize>,
    signers: &[Arc<Keypair>],
    srcs: &[Pubkey],
    dsts: &[Pubkey],
    delay: u64,
    batch_size: usize,
    account_groups: usize,
    client_ctor: &Arc<F>,
) where
    F: Fn() -> T,
    T: Client,
{
    let client = client_ctor();
    let mut stats = Stats::default();

    // TODO Hard coded for now
    let pair = TokenPair::AB;
    let tokens = 1;
    let price = 1000;
    let mut account_group: usize = 0;
    let mut blockhash = client
        .get_recent_blockhash()
        .expect("Failed to get blockhash");
    let mut blockhash_now = UNIX_EPOCH;

    loop {
        let now = Instant::now();
        let trade_keys = generate_keypairs(batch_size as u64);

        stats.total += batch_size as u64;

        let mut trades = vec![];
        let mut trade_infos = vec![];
        let start = account_group * batch_size as usize;
        let end = account_group * batch_size as usize + batch_size as usize;
        let mut direction = Direction::To;
        for (signer, trade, src, dst) in izip!(
            signers[start..end].iter(),
            trade_keys,
            srcs[start..end].iter(),
            dsts[start..end].iter()
        ) {
            direction = if direction == Direction::To {
                Direction::From
            } else {
                Direction::To
            };
            let order_info = TradeOrderInfo {
                /// Owner of the trade order
                owner: Pubkey::default(), // don't care
                direction,
                pair,
                tokens,
                price,
                src_account: Pubkey::default(), // don't care
                dst_account: *dst,
            };
            trade_infos.push(TradeInfo {
                trade_account: trade.pubkey(),
                order_info,
            });
            trades.push((signer, trade.pubkey(), direction, src, dst));
        }
        account_group = (account_group + 1) % account_groups as usize;
        let duration = now.elapsed();
        let keypair_ns = duration.as_secs() * 1_000_000_000 + u64::from(duration.subsec_nanos());
        let rate = (batch_size as f64 / keypair_ns as f64) * 1_000_000_000_f64;
        stats.keygen_ns += keypair_ns;
        if rate > stats.keygen_peak_rate {
            stats.keygen_peak_rate = rate;
        }
        trace!("sw {:?} keypairs {:.2} /s", batch_size, rate);

        trades.chunks(CHUNK_LEN).for_each(|chunk| {
            let now = Instant::now();

            // Don't get a blockhash every time
            if SystemTime::now()
                .duration_since(blockhash_now)
                .unwrap()
                .as_secs()
                > BLOCKHASH_RENEW_PERIOD_S
            {
                blockhash = client
                    .get_recent_blockhash()
                    .expect("Failed to get blockhash");
                blockhash_now = SystemTime::now();
            }

            let trades_txs: Vec<_> = chunk
                .par_iter()
                .map(|(signer, trade, direction, src, dst)| {
                    let s: &Keypair = &signer;
                    let owner = &signer.pubkey();
                    let space = mem::size_of::<ExchangeState>() as u64;
                    Transaction::new_signed_instructions(
                        &[s],
                        vec![
                            system_instruction::create_account(owner, trade, 1, space, &id()),
                            exchange_instruction::trade_request(
                                owner, trade, *direction, pair, tokens, price, src, dst,
                            ),
                        ],
                        blockhash,
                    )
                })
                .collect();
            let duration = now.elapsed();
            let sign_ns = duration.as_secs() * 1_000_000_000 + u64::from(duration.subsec_nanos());
            let n = trades_txs.len();
            let rate = (n as f64 / sign_ns as f64) * 1_000_000_000_f64;
            stats.sign_ns += sign_ns;
            if rate > stats.sign_peak_rate {
                stats.sign_peak_rate = rate;
            }
            trace!("  sw {:?} signed   {:.2} /s ", n, rate);

            let chunks: Vec<_> = trades_txs.chunks(CHUNK_LEN).collect();
            {
                let mut shared_txs_wl = shared_txs
                    .write()
                    .expect("Failed to send tx to transfer threads");
                for chunk in chunks {
                    shared_txs_wl.push_back(chunk.to_vec());
                }
            }

            if delay > 0 {
                sleep(Duration::from_millis(delay));
            }
        });

        sender
            .send(trade_infos)
            .expect("Failed to send trades to swapper");

        while shared_tx_active_thread_count.load(Ordering::Relaxed) > 0 {
            sleep(Duration::from_millis(100));
        }

        if exit_signal.load(Ordering::Relaxed) {
            info!("{} Trades with batch size {}", stats.total, batch_size);
            info!(
                "  Keygen avg {:.2}/s peak {:.2}/s",
                (stats.total as f64 / stats.keygen_ns as f64) * 1_000_000_000_f64,
                stats.keygen_peak_rate
            );
            info!(
                "  Signed avg {:.2}/s peak {:.2}/s",
                (stats.total as f64 / stats.sign_ns as f64) * 1_000_000_000_f64,
                stats.sign_peak_rate
            );
            break;
        }
    }
}

fn verify_transfer<T>(sync_client: &T, tx: &Transaction) -> bool
where
    T: SyncClient + ?Sized,
{
    for s in &tx.signatures {
        if let Ok(Some(_)) = sync_client.get_signature_status(s) {
            return true;
        }
    }
    false
}

pub fn fund_keys(client: &Client, source: &Keypair, dests: &[Arc<Keypair>], lamports: u64) {
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

        to_fund.chunks(CHUNK_LEN).for_each(|chunk| {
            #[allow(clippy::clone_double_ref)] // sigh
            let mut to_fund_txs: Vec<_> = chunk
                .par_iter()
                .map(|(k, m)| {
                    (
                        k.clone(),
                        Transaction::new_unsigned_instructions(system_instruction::transfer_many(
                            &k.pubkey(),
                            &m,
                        )),
                    )
                })
                .collect();

            let mut retries = 0;
            while !to_fund_txs.is_empty() {
                let receivers = to_fund_txs
                    .iter()
                    .fold(0, |len, (_, tx)| len + tx.message().instructions.len());

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

                let blockhash = client.get_recent_blockhash().expect("blockhash");
                to_fund_txs.par_iter_mut().for_each(|(k, tx)| {
                    tx.sign(&[*k], blockhash);
                });
                to_fund_txs.iter().for_each(|(_, tx)| {
                    client.async_send_transaction(tx.clone()).expect("transfer");
                });

                let mut waits = 0;
                loop {
                    sleep(Duration::from_millis(50));
                    to_fund_txs.retain(|(_, tx)| !verify_transfer(client, &tx));
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
                        error!("  Too many retries, give up");
                        exit(1);
                    }
                }
            }
        });
        funded.append(&mut new_funded);
        funded.retain(|(k, b)| {
            client.get_balance(&k.pubkey()).unwrap_or(0) > lamports && *b > lamports
        });
        debug!("  Funded: {} left: {}", funded.len(), notfunded.len());
    }
}

pub fn create_token_accounts(client: &Client, signers: &[Arc<Keypair>], accounts: &[Pubkey]) {
    let mut notfunded: Vec<(&Arc<Keypair>, &Pubkey)> = signers.iter().zip(accounts).collect();

    while !notfunded.is_empty() {
        notfunded.chunks(CHUNK_LEN).for_each(|chunk| {
            let mut to_create_txs: Vec<_> = chunk
                .par_iter()
                .map(|(signer, new)| {
                    let owner_id = &signer.pubkey();
                    let space = mem::size_of::<ExchangeState>() as u64;
                    let create_ix =
                        system_instruction::create_account(owner_id, new, 1, space, &id());
                    let request_ix = exchange_instruction::account_request(owner_id, new);
                    (
                        signer,
                        Transaction::new_unsigned_instructions(vec![create_ix, request_ix]),
                    )
                })
                .collect();

            let accounts = to_create_txs
                .iter()
                .fold(0, |len, (_, tx)| len + tx.message().instructions.len() / 2);

            debug!(
                "  Creating {} accounts in {} txs",
                accounts,
                to_create_txs.len(),
            );

            let mut retries = 0;
            while !to_create_txs.is_empty() {
                let blockhash = client
                    .get_recent_blockhash()
                    .expect("Failed to get blockhash");
                to_create_txs.par_iter_mut().for_each(|(k, tx)| {
                    let kp: &Keypair = k;
                    tx.sign(&[kp], blockhash);
                });
                to_create_txs.iter().for_each(|(_, tx)| {
                    client.async_send_transaction(tx.clone()).expect("transfer");
                });

                let mut waits = 0;
                while !to_create_txs.is_empty() {
                    sleep(Duration::from_millis(50));
                    to_create_txs.retain(|(_, tx)| !verify_transfer(client, &tx));
                    if to_create_txs.is_empty() {
                        break;
                    }
                    debug!(
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
                    debug!("  Retry {:?}", retries);
                    if retries >= 10 {
                        error!("  Too many retries, give up");
                        exit(1);
                    }
                }
            }
        });

        let mut new_notfunded: Vec<(&Arc<Keypair>, &Pubkey)> = vec![];
        for f in &notfunded {
            if client.get_balance(&f.1).unwrap_or(0) == 0 {
                new_notfunded.push(*f)
            }
        }
        notfunded = new_notfunded;
        debug!("  Left: {}", notfunded.len());
    }
}

fn compute_and_report_stats(maxes: &Arc<RwLock<Vec<(SampleStats)>>>, total_tx_send_count: usize) {
    let mut max_tx_count = 0;
    let mut max_tx_time = Duration::new(0, 0);
    info!("|       Max TPS | Total Transactions");
    info!("+---------------+--------------------");

    for stats in maxes.read().unwrap().iter() {
        let maybe_flag = match stats.tx_count {
            0 => "!!!!!",
            _ => "",
        };

        info!("| {:13.2} | {} {}", stats.tps, stats.tx_count, maybe_flag);

        if stats.tx_time > max_tx_time {
            max_tx_time = stats.tx_time;
        }
        if stats.tx_count > max_tx_count {
            max_tx_count = stats.tx_count;
        }
    }
    info!("+---------------+--------------------");

    if max_tx_count > total_tx_send_count as u64 {
        error!(
            "{} more transactions sampled ({}) then were sent ({})",
            max_tx_count - total_tx_send_count as u64,
            max_tx_count,
            total_tx_send_count
        );
    } else {
        info!(
            "{} txs dropped ({:.2}%)",
            total_tx_send_count as u64 - max_tx_count,
            (total_tx_send_count as u64 - max_tx_count) as f64 / total_tx_send_count as f64
                * 100_f64
        );
    }
    info!(
        "\tAverage TPS: {}",
        max_tx_count as f32 / max_tx_time.as_secs() as f32
    );
}

fn generate_keypairs(num: u64) -> Vec<Keypair> {
    let mut seed = [0_u8; 32];
    seed.copy_from_slice(&Keypair::new().pubkey().as_ref());
    let mut rnd = GenKeys::new(seed);
    rnd.gen_n_keypairs(num)
}

pub fn airdrop_lamports(client: &Client, drone_addr: &SocketAddr, id: &Keypair, amount: u64) {
    let balance = client.get_balance(&id.pubkey());
    let balance = balance.unwrap_or(0);
    if balance > amount {
        return;
    }

    let amount_to_drop = amount - balance;

    info!(
        "Airdropping {:?} lamports from {} for {}",
        amount_to_drop,
        drone_addr,
        id.pubkey(),
    );

    let mut tries = 0;
    loop {
        let blockhash = client
            .get_recent_blockhash()
            .expect("Failed to get blockhash");
        match request_airdrop_transaction(&drone_addr, &id.pubkey(), amount_to_drop, blockhash) {
            Ok(transaction) => {
                let signature = client.async_send_transaction(transaction).unwrap();

                if let Ok(Some(_)) = client.get_signature_status(&signature) {
                    break;
                }
            }
            Err(err) => {
                panic!(
                    "Error requesting airdrop: {:?} to addr: {:?} amount: {}",
                    err, drone_addr, amount
                );
            }
        };
        debug!("  Retry...");
        tries += 1;
        if tries > 50 {
            error!("Too many retries, give up");
            exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana::cluster_info::FULLNODE_PORT_RANGE;
    use solana::fullnode::FullnodeConfig;
    use solana::gossip_service::discover_nodes;
    use solana::local_cluster::{ClusterConfig, LocalCluster};
    use solana_client::thin_client::create_client;
    use solana_client::thin_client::ThinClient;
    use solana_drone::drone::run_local_drone;
    use std::sync::mpsc::channel;

    #[test]
    #[ignore] // TODO Issue #3825
    fn test_exchange_local_cluster() {
        solana_logger::setup();

        const NUM_NODES: usize = 1;
        let fullnode_config = FullnodeConfig::default();

        let mut config = Config::default();
        config.identity = Keypair::new();
        config.threads = 4;
        config.duration = Duration::from_secs(5);
        config.fund_amount = 100_000;
        config.trade_delay = 0;
        config.batch_size = 10;
        config.account_groups = 100;
        let Config {
            fund_amount,
            batch_size,
            account_groups,
            ..
        } = config;
        let accounts_in_groups = batch_size * account_groups;

        let cluster = LocalCluster::new(&ClusterConfig {
            node_stakes: vec![100_000; NUM_NODES],
            cluster_lamports: 100_000_000_000_000,
            fullnode_config,
            native_instruction_processors: [(
                "solana_exchange_program".to_string(),
                solana_exchange_api::id(),
            )]
            .to_vec(),
            ..ClusterConfig::default()
        });

        let drone_keypair = Keypair::new();
        cluster.transfer(
            &cluster.funding_keypair,
            &drone_keypair.pubkey(),
            2_000_000_000_000,
        );

        let (addr_sender, addr_receiver) = channel();
        run_local_drone(drone_keypair, addr_sender, Some(1_000_000_000_000));
        let drone_addr = addr_receiver.recv_timeout(Duration::from_secs(2)).unwrap();

        info!("Connecting to the cluster");
        let nodes =
            discover_nodes(&cluster.entry_point_info.gossip, NUM_NODES).unwrap_or_else(|err| {
                error!("Failed to discover {} nodes: {:?}", NUM_NODES, err);
                exit(1);
            });
        if nodes.len() < NUM_NODES {
            error!(
                "Error: Insufficient nodes discovered.  Expecting {} or more",
                NUM_NODES
            );
            exit(1);
        }
        let client_ctors: Vec<_> = nodes
            .iter()
            .map(|node| {
                let cluster_entrypoint = node.clone();
                let cluster_addrs = cluster_entrypoint.client_facing_addr();
                let client_ctor =
                    move || -> ThinClient { create_client(cluster_addrs, FULLNODE_PORT_RANGE) };
                client_ctor
            })
            .collect();

        let client = client_ctors[0]();
        airdrop_lamports(
            &client,
            &drone_addr,
            &config.identity,
            fund_amount * (accounts_in_groups + 1) as u64 * 2,
        );

        do_bench_exchange(client_ctors, config);
    }
}
