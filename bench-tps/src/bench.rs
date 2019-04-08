use solana_metrics;

use crate::cli::Config;
use rayon::prelude::*;
use solana::cluster_info::FULLNODE_PORT_RANGE;
use solana::contact_info::ContactInfo;
use solana::gen_keys::GenKeys;
use solana::gossip_service::discover_nodes;
use solana_client::thin_client::create_client;
use solana_client::thin_client::ThinClient;
use solana_drone::drone::request_airdrop_transaction;
use solana_metrics::influxdb;
use solana_sdk::async_client::AsyncClient;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, KeypairUtil};
use solana_sdk::sync_client::SyncClient;
use solana_sdk::system_instruction;
use solana_sdk::system_transaction;
use solana_sdk::timing::timestamp;
use solana_sdk::timing::{duration_as_ms, duration_as_s};
use solana_sdk::transaction::Transaction;
use std::cmp;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::process::exit;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::sleep;
use std::thread::Builder;
use std::time::Duration;
use std::time::Instant;

pub struct NodeStats {
    /// Maximum TPS reported by this node
    pub tps: f64,
    /// Total transactions reported by this node
    pub tx: u64,
}

pub const MAX_SPENDS_PER_TX: usize = 4;

pub type SharedTransactions = Arc<RwLock<VecDeque<Vec<(Transaction, u64)>>>>;

pub fn do_bench_tps(config: Config) {
    let Config {
        network_addr: network,
        drone_addr,
        id,
        threads,
        thread_batch_sleep_ms,
        num_nodes,
        duration,
        tx_count,
        sustained,
    } = config;

    let nodes = discover_nodes(&network, num_nodes).unwrap_or_else(|err| {
        eprintln!("Failed to discover {} nodes: {:?}", num_nodes, err);
        exit(1);
    });
    if nodes.len() < num_nodes {
        eprintln!(
            "Error: Insufficient nodes discovered.  Expecting {} or more",
            num_nodes
        );
        exit(1);
    }
    let cluster_entrypoint = nodes[0].clone(); // Pick the first node, why not?

    let client = create_client(cluster_entrypoint.client_facing_addr(), FULLNODE_PORT_RANGE);
    let mut barrier_client =
        create_client(cluster_entrypoint.client_facing_addr(), FULLNODE_PORT_RANGE);

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&id.public_key_bytes()[..32]);
    let mut rnd = GenKeys::new(seed);

    println!("Creating {} keypairs...", tx_count * 2);
    let mut total_keys = 0;
    let mut target = tx_count * 2;
    while target > 0 {
        total_keys += target;
        target /= MAX_SPENDS_PER_TX;
    }
    let gen_keypairs = rnd.gen_n_keypairs(total_keys as u64);
    let barrier_source_keypair = Keypair::new();
    let barrier_dest_id = Pubkey::new_rand();

    println!("Get lamports...");
    let num_lamports_per_account = 20;

    // Sample the first keypair, see if it has lamports, if so then resume
    // to avoid lamport loss
    let keypair0_balance = client
        .poll_get_balance(&gen_keypairs.last().unwrap().pubkey())
        .unwrap_or(0);

    if num_lamports_per_account > keypair0_balance {
        let extra = num_lamports_per_account - keypair0_balance;
        let total = extra * (gen_keypairs.len() as u64);
        airdrop_lamports(&client, &drone_addr, &id, total);
        println!("adding more lamports {}", extra);
        fund_keys(&client, &id, &gen_keypairs, extra);
    }
    let start = gen_keypairs.len() - (tx_count * 2) as usize;
    let keypairs = &gen_keypairs[start..];
    airdrop_lamports(&barrier_client, &drone_addr, &barrier_source_keypair, 1);

    println!("Get last ID...");
    let mut blockhash = client.get_recent_blockhash().unwrap();
    println!("Got last ID {:?}", blockhash);

    let first_tx_count = client.get_transaction_count().expect("transaction count");
    println!("Initial transaction count {}", first_tx_count);

    let exit_signal = Arc::new(AtomicBool::new(false));

    // Setup a thread per validator to sample every period
    // collect the max transaction rate and total tx count seen
    let maxes = Arc::new(RwLock::new(Vec::new()));
    let sample_period = 1; // in seconds
    println!("Sampling TPS every {} second...", sample_period);
    let v_threads: Vec<_> = nodes
        .into_iter()
        .map(|v| {
            let exit_signal = exit_signal.clone();
            let maxes = maxes.clone();
            Builder::new()
                .name("solana-client-sample".to_string())
                .spawn(move || {
                    sample_tx_count(&exit_signal, &maxes, first_tx_count, &v, sample_period);
                })
                .unwrap()
        })
        .collect();

    let shared_txs: SharedTransactions = Arc::new(RwLock::new(VecDeque::new()));

    let shared_tx_active_thread_count = Arc::new(AtomicIsize::new(0));
    let total_tx_sent_count = Arc::new(AtomicUsize::new(0));

    let s_threads: Vec<_> = (0..threads)
        .map(|_| {
            let exit_signal = exit_signal.clone();
            let shared_txs = shared_txs.clone();
            let cluster_entrypoint = cluster_entrypoint.clone();
            let shared_tx_active_thread_count = shared_tx_active_thread_count.clone();
            let total_tx_sent_count = total_tx_sent_count.clone();
            Builder::new()
                .name("solana-client-sender".to_string())
                .spawn(move || {
                    do_tx_transfers(
                        &exit_signal,
                        &shared_txs,
                        &cluster_entrypoint,
                        &shared_tx_active_thread_count,
                        &total_tx_sent_count,
                        thread_batch_sleep_ms,
                    );
                })
                .unwrap()
        })
        .collect();

    // generate and send transactions for the specified duration
    let start = Instant::now();
    let mut reclaim_lamports_back_to_source_account = false;
    let mut i = keypair0_balance;
    while start.elapsed() < duration {
        let balance = client.poll_get_balance(&id.pubkey()).unwrap_or(0);
        metrics_submit_lamport_balance(balance);

        // ping-pong between source and destination accounts for each loop iteration
        // this seems to be faster than trying to determine the balance of individual
        // accounts
        let len = tx_count as usize;
        generate_txs(
            &shared_txs,
            &keypairs[..len],
            &keypairs[len..],
            threads,
            reclaim_lamports_back_to_source_account,
            &cluster_entrypoint,
        );
        // In sustained mode overlap the transfers with generation
        // this has higher average performance but lower peak performance
        // in tested environments.
        if !sustained {
            while shared_tx_active_thread_count.load(Ordering::Relaxed) > 0 {
                sleep(Duration::from_millis(100));
            }
        }
        // It's not feasible (would take too much time) to confirm each of the `tx_count / 2`
        // transactions sent by `generate_txs()` so instead send and confirm a single transaction
        // to validate the network is still functional.
        send_barrier_transaction(
            &mut barrier_client,
            &mut blockhash,
            &barrier_source_keypair,
            &barrier_dest_id,
        );

        i += 1;
        if should_switch_directions(num_lamports_per_account, i) {
            reclaim_lamports_back_to_source_account = !reclaim_lamports_back_to_source_account;
        }
    }

    // Stop the sampling threads so it will collect the stats
    exit_signal.store(true, Ordering::Relaxed);

    println!("Waiting for validator threads...");
    for t in v_threads {
        if let Err(err) = t.join() {
            println!("  join() failed with: {:?}", err);
        }
    }

    // join the tx send threads
    println!("Waiting for transmit threads...");
    for t in s_threads {
        if let Err(err) = t.join() {
            println!("  join() failed with: {:?}", err);
        }
    }

    let balance = client.poll_get_balance(&id.pubkey()).unwrap_or(0);
    metrics_submit_lamport_balance(balance);

    compute_and_report_stats(
        &maxes,
        sample_period,
        &start.elapsed(),
        total_tx_sent_count.load(Ordering::Relaxed),
    );
}

fn metrics_submit_lamport_balance(lamport_balance: u64) {
    println!("Token balance: {}", lamport_balance);
    solana_metrics::submit(
        influxdb::Point::new("bench-tps")
            .add_tag("op", influxdb::Value::String("lamport_balance".to_string()))
            .add_field("balance", influxdb::Value::Integer(lamport_balance as i64))
            .to_owned(),
    );
}

fn sample_tx_count(
    exit_signal: &Arc<AtomicBool>,
    maxes: &Arc<RwLock<Vec<(SocketAddr, NodeStats)>>>,
    first_tx_count: u64,
    v: &ContactInfo,
    sample_period: u64,
) {
    let client = create_client(v.client_facing_addr(), FULLNODE_PORT_RANGE);
    let mut now = Instant::now();
    let mut initial_tx_count = client.get_transaction_count().expect("transaction count");
    let mut max_tps = 0.0;
    let mut total;

    let log_prefix = format!("{:21}:", v.tpu.to_string());

    loop {
        let tx_count = client.get_transaction_count().expect("transaction count");
        assert!(
            tx_count >= initial_tx_count,
            "expected tx_count({}) >= initial_tx_count({})",
            tx_count,
            initial_tx_count
        );
        let duration = now.elapsed();
        now = Instant::now();
        let sample = tx_count - initial_tx_count;
        initial_tx_count = tx_count;

        let ns = duration.as_secs() * 1_000_000_000 + u64::from(duration.subsec_nanos());
        let tps = (sample * 1_000_000_000) as f64 / ns as f64;
        if tps > max_tps {
            max_tps = tps;
        }
        if tx_count > first_tx_count {
            total = tx_count - first_tx_count;
        } else {
            total = 0;
        }
        println!(
            "{} {:9.2} TPS, Transactions: {:6}, Total transactions: {}",
            log_prefix, tps, sample, total
        );
        sleep(Duration::new(sample_period, 0));

        if exit_signal.load(Ordering::Relaxed) {
            println!("{} Exiting validator thread", log_prefix);
            let stats = NodeStats {
                tps: max_tps,
                tx: total,
            };
            maxes.write().unwrap().push((v.tpu, stats));
            break;
        }
    }
}

/// Send loopback payment of 0 lamports and confirm the network processed it
fn send_barrier_transaction(
    barrier_client: &mut ThinClient,
    blockhash: &mut Hash,
    source_keypair: &Keypair,
    dest_id: &Pubkey,
) {
    let transfer_start = Instant::now();

    let mut poll_count = 0;
    loop {
        if poll_count > 0 && poll_count % 8 == 0 {
            println!(
                "polling for barrier transaction confirmation, attempt {}",
                poll_count
            );
        }

        *blockhash = barrier_client.get_recent_blockhash().unwrap();

        let transaction =
            system_transaction::create_user_account(&source_keypair, dest_id, 0, *blockhash, 0);
        let signature = barrier_client
            .async_send_transaction(transaction)
            .expect("Unable to send barrier transaction");

        let confirmatiom = barrier_client.poll_for_signature(&signature);
        let duration_ms = duration_as_ms(&transfer_start.elapsed());
        if confirmatiom.is_ok() {
            println!("barrier transaction confirmed in {} ms", duration_ms);

            solana_metrics::submit(
                influxdb::Point::new("bench-tps")
                    .add_tag(
                        "op",
                        influxdb::Value::String("send_barrier_transaction".to_string()),
                    )
                    .add_field("poll_count", influxdb::Value::Integer(poll_count))
                    .add_field("duration", influxdb::Value::Integer(duration_ms as i64))
                    .to_owned(),
            );

            // Sanity check that the client balance is still 1
            let balance = barrier_client
                .poll_balance_with_timeout(
                    &source_keypair.pubkey(),
                    &Duration::from_millis(100),
                    &Duration::from_secs(10),
                )
                .expect("Failed to get balance");
            if balance != 1 {
                panic!("Expected an account balance of 1 (balance: {}", balance);
            }
            break;
        }

        // Timeout after 3 minutes.  When running a CPU-only leader+validator+drone+bench-tps on a dev
        // machine, some batches of transactions can take upwards of 1 minute...
        if duration_ms > 1000 * 60 * 3 {
            println!("Error: Couldn't confirm barrier transaction!");
            exit(1);
        }

        let new_blockhash = barrier_client.get_recent_blockhash().unwrap();
        if new_blockhash == *blockhash {
            if poll_count > 0 && poll_count % 8 == 0 {
                println!("blockhash is not advancing, still at {:?}", *blockhash);
            }
        } else {
            *blockhash = new_blockhash;
        }

        poll_count += 1;
    }
}

fn generate_txs(
    shared_txs: &SharedTransactions,
    source: &[Keypair],
    dest: &[Keypair],
    threads: usize,
    reclaim: bool,
    contact_info: &ContactInfo,
) {
    let client = create_client(contact_info.client_facing_addr(), FULLNODE_PORT_RANGE);
    let blockhash = client.get_recent_blockhash().unwrap();
    let tx_count = source.len();
    println!("Signing transactions... {} (reclaim={})", tx_count, reclaim);
    let signing_start = Instant::now();

    let pairs: Vec<_> = if !reclaim {
        source.iter().zip(dest.iter()).collect()
    } else {
        dest.iter().zip(source.iter()).collect()
    };
    let transactions: Vec<_> = pairs
        .par_iter()
        .map(|(id, keypair)| {
            (
                system_transaction::create_user_account(id, &keypair.pubkey(), 1, blockhash, 0),
                timestamp(),
            )
        })
        .collect();

    let duration = signing_start.elapsed();
    let ns = duration.as_secs() * 1_000_000_000 + u64::from(duration.subsec_nanos());
    let bsps = (tx_count) as f64 / ns as f64;
    let nsps = ns as f64 / (tx_count) as f64;
    println!(
        "Done. {:.2} thousand signatures per second, {:.2} us per signature, {} ms total time, {}",
        bsps * 1_000_000_f64,
        nsps / 1_000_f64,
        duration_as_ms(&duration),
        blockhash,
    );
    solana_metrics::submit(
        influxdb::Point::new("bench-tps")
            .add_tag("op", influxdb::Value::String("generate_txs".to_string()))
            .add_field(
                "duration",
                influxdb::Value::Integer(duration_as_ms(&duration) as i64),
            )
            .to_owned(),
    );

    let sz = transactions.len() / threads;
    let chunks: Vec<_> = transactions.chunks(sz).collect();
    {
        let mut shared_txs_wl = shared_txs.write().unwrap();
        for chunk in chunks {
            shared_txs_wl.push_back(chunk.to_vec());
        }
    }
}

fn do_tx_transfers(
    exit_signal: &Arc<AtomicBool>,
    shared_txs: &SharedTransactions,
    contact_info: &ContactInfo,
    shared_tx_thread_count: &Arc<AtomicIsize>,
    total_tx_sent_count: &Arc<AtomicUsize>,
    thread_batch_sleep_ms: usize,
) {
    let client = create_client(contact_info.client_facing_addr(), FULLNODE_PORT_RANGE);
    loop {
        if thread_batch_sleep_ms > 0 {
            sleep(Duration::from_millis(thread_batch_sleep_ms as u64));
        }
        let txs;
        {
            let mut shared_txs_wl = shared_txs.write().unwrap();
            txs = shared_txs_wl.pop_front();
        }
        if let Some(txs0) = txs {
            shared_tx_thread_count.fetch_add(1, Ordering::Relaxed);
            println!(
                "Transferring 1 unit {} times... to {}",
                txs0.len(),
                contact_info.tpu
            );
            let tx_len = txs0.len();
            let transfer_start = Instant::now();
            for tx in txs0 {
                let now = timestamp();
                if now > tx.1 && now - tx.1 > 1000 * 30 {
                    continue;
                }
                client.async_send_transaction(tx.0).unwrap();
            }
            shared_tx_thread_count.fetch_add(-1, Ordering::Relaxed);
            total_tx_sent_count.fetch_add(tx_len, Ordering::Relaxed);
            println!(
                "Tx send done. {} ms {} tps",
                duration_as_ms(&transfer_start.elapsed()),
                tx_len as f32 / duration_as_s(&transfer_start.elapsed()),
            );
            solana_metrics::submit(
                influxdb::Point::new("bench-tps")
                    .add_tag("op", influxdb::Value::String("do_tx_transfers".to_string()))
                    .add_field(
                        "duration",
                        influxdb::Value::Integer(duration_as_ms(&transfer_start.elapsed()) as i64),
                    )
                    .add_field("count", influxdb::Value::Integer(tx_len as i64))
                    .to_owned(),
            );
        }
        if exit_signal.load(Ordering::Relaxed) {
            break;
        }
    }
}

fn verify_funding_transfer(client: &ThinClient, tx: &Transaction, amount: u64) -> bool {
    for a in &tx.message().account_keys[1..] {
        if client.get_balance(a).unwrap_or(0) >= amount {
            return true;
        }
    }

    false
}

/// fund the dests keys by spending all of the source keys into MAX_SPENDS_PER_TX
/// on every iteration.  This allows us to replay the transfers because the source is either empty,
/// or full
fn fund_keys(client: &ThinClient, source: &Keypair, dests: &[Keypair], lamports: u64) {
    let total = lamports * dests.len() as u64;
    let mut funded: Vec<(&Keypair, u64)> = vec![(source, total)];
    let mut notfunded: Vec<&Keypair> = dests.iter().collect();

    println!("funding keys {}", dests.len());
    while !notfunded.is_empty() {
        let mut new_funded: Vec<(&Keypair, u64)> = vec![];
        let mut to_fund = vec![];
        println!("creating from... {}", funded.len());
        for f in &mut funded {
            let max_units = cmp::min(notfunded.len(), MAX_SPENDS_PER_TX);
            if max_units == 0 {
                break;
            }
            let start = notfunded.len() - max_units;
            let per_unit = f.1 / (max_units as u64);
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

        // try to transfer a "few" at a time with recent blockhash
        //  assume 4MB network buffers, and 512 byte packets
        const FUND_CHUNK_LEN: usize = 4 * 1024 * 1024 / 512;

        to_fund.chunks(FUND_CHUNK_LEN).for_each(|chunk| {
            let mut tries = 0;

            // this set of transactions just initializes us for bookkeeping
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

            let amount = chunk[0].1[0].1;

            while !to_fund_txs.is_empty() {
                let receivers = to_fund_txs
                    .iter()
                    .fold(0, |len, (_, tx)| len + tx.message().instructions.len());

                println!(
                    "{} {} to {} in {} txs",
                    if tries == 0 {
                        "transferring"
                    } else {
                        " retrying"
                    },
                    amount,
                    receivers,
                    to_fund_txs.len(),
                );

                let blockhash = client.get_recent_blockhash().unwrap();

                // re-sign retained to_fund_txes with updated blockhash
                to_fund_txs.par_iter_mut().for_each(|(k, tx)| {
                    tx.sign(&[*k], blockhash);
                });

                to_fund_txs.iter().for_each(|(_, tx)| {
                    client.async_send_transaction(tx.clone()).expect("transfer");
                });

                // retry anything that seems to have dropped through cracks
                //  again since these txs are all or nothing, they're fine to
                //  retry
                to_fund_txs.retain(|(_, tx)| !verify_funding_transfer(client, &tx, amount));

                tries += 1;
            }
            println!("transferred");
        });
        println!("funded: {} left: {}", new_funded.len(), notfunded.len());
        funded = new_funded;
    }
}

fn airdrop_lamports(client: &ThinClient, drone_addr: &SocketAddr, id: &Keypair, tx_count: u64) {
    let starting_balance = client.poll_get_balance(&id.pubkey()).unwrap_or(0);
    metrics_submit_lamport_balance(starting_balance);
    println!("starting balance {}", starting_balance);

    if starting_balance < tx_count {
        let airdrop_amount = tx_count - starting_balance;
        println!(
            "Airdropping {:?} lamports from {} for {}",
            airdrop_amount,
            drone_addr,
            id.pubkey(),
        );

        let blockhash = client.get_recent_blockhash().unwrap();
        match request_airdrop_transaction(&drone_addr, &id.pubkey(), airdrop_amount, blockhash) {
            Ok(transaction) => {
                let signature = client.async_send_transaction(transaction).unwrap();
                client.poll_for_signature(&signature).unwrap();
            }
            Err(err) => {
                panic!(
                    "Error requesting airdrop: {:?} to addr: {:?} amount: {}",
                    err, drone_addr, airdrop_amount
                );
            }
        };

        let current_balance = client.poll_get_balance(&id.pubkey()).unwrap_or_else(|e| {
            println!("airdrop error {}", e);
            starting_balance
        });
        println!("current balance {}...", current_balance);

        metrics_submit_lamport_balance(current_balance);
        if current_balance - starting_balance != airdrop_amount {
            println!(
                "Airdrop failed! {} {} {}",
                id.pubkey(),
                current_balance,
                starting_balance
            );
            exit(1);
        }
    }
}

fn compute_and_report_stats(
    maxes: &Arc<RwLock<Vec<(SocketAddr, NodeStats)>>>,
    sample_period: u64,
    tx_send_elapsed: &Duration,
    total_tx_send_count: usize,
) {
    // Compute/report stats
    let mut max_of_maxes = 0.0;
    let mut max_tx_count = 0;
    let mut nodes_with_zero_tps = 0;
    let mut total_maxes = 0.0;
    println!(" Node address        |       Max TPS | Total Transactions");
    println!("---------------------+---------------+--------------------");

    for (sock, stats) in maxes.read().unwrap().iter() {
        let maybe_flag = match stats.tx {
            0 => "!!!!!",
            _ => "",
        };

        println!(
            "{:20} | {:13.2} | {} {}",
            (*sock).to_string(),
            stats.tps,
            stats.tx,
            maybe_flag
        );

        if stats.tps == 0.0 {
            nodes_with_zero_tps += 1;
        }
        total_maxes += stats.tps;

        if stats.tps > max_of_maxes {
            max_of_maxes = stats.tps;
        }
        if stats.tx > max_tx_count {
            max_tx_count = stats.tx;
        }
    }

    if total_maxes > 0.0 {
        let num_nodes_with_tps = maxes.read().unwrap().len() - nodes_with_zero_tps;
        let average_max = total_maxes / num_nodes_with_tps as f64;
        println!(
            "\nAverage max TPS: {:.2}, {} nodes had 0 TPS",
            average_max, nodes_with_zero_tps
        );
    }

    println!(
        "\nHighest TPS: {:.2} sampling period {}s max transactions: {} clients: {} drop rate: {:.2}",
        max_of_maxes,
        sample_period,
        max_tx_count,
        maxes.read().unwrap().len(),
        (total_tx_send_count as u64 - max_tx_count) as f64 / total_tx_send_count as f64,
    );
    println!(
        "\tAverage TPS: {}",
        max_tx_count as f32 / duration_as_s(tx_send_elapsed)
    );
}

// First transfer 3/4 of the lamports to the dest accounts
// then ping-pong 1/4 of the lamports back to the other account
// this leaves 1/4 lamport buffer in each account
fn should_switch_directions(num_lamports_per_account: u64, i: u64) -> bool {
    i % (num_lamports_per_account / 4) == 0 && (i >= (3 * num_lamports_per_account) / 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana::fullnode::FullnodeConfig;
    use solana::local_cluster::LocalCluster;
    use solana_drone::drone::run_local_drone;
    use std::sync::mpsc::channel;

    #[test]
    fn test_switch_directions() {
        assert_eq!(should_switch_directions(20, 0), false);
        assert_eq!(should_switch_directions(20, 1), false);
        assert_eq!(should_switch_directions(20, 14), false);
        assert_eq!(should_switch_directions(20, 15), true);
        assert_eq!(should_switch_directions(20, 16), false);
        assert_eq!(should_switch_directions(20, 19), false);
        assert_eq!(should_switch_directions(20, 20), true);
        assert_eq!(should_switch_directions(20, 21), false);
        assert_eq!(should_switch_directions(20, 99), false);
        assert_eq!(should_switch_directions(20, 100), true);
        assert_eq!(should_switch_directions(20, 101), false);
    }

    #[test]
    #[ignore]
    fn test_bench_tps() {
        let fullnode_config = FullnodeConfig::default();
        const NUM_NODES: usize = 1;
        let cluster =
            LocalCluster::new_with_config(&[999_990; NUM_NODES], 2_000_000, &fullnode_config, &[]);

        let drone_keypair = Keypair::new();
        cluster.transfer(&cluster.funding_keypair, &drone_keypair.pubkey(), 1_000_000);

        let (addr_sender, addr_receiver) = channel();
        run_local_drone(drone_keypair, addr_sender, None);
        let drone_addr = addr_receiver.recv_timeout(Duration::from_secs(2)).unwrap();

        let mut cfg = Config::default();
        cfg.network_addr = cluster.entry_point_info.gossip;
        cfg.drone_addr = drone_addr;
        cfg.tx_count = 100;
        cfg.duration = Duration::from_secs(5);
        cfg.num_nodes = NUM_NODES;

        do_bench_tps(cfg);
    }
}
