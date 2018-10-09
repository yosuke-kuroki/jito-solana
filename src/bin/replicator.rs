#[macro_use]
extern crate clap;
extern crate getopts;
extern crate serde_json;
#[macro_use]
extern crate solana;

use clap::{App, Arg};
use solana::chacha::chacha_cbc_encrypt_files;
use solana::cluster_info::Node;
use solana::fullnode::Config;
use solana::ledger::LEDGER_DATA_FILE;
use solana::logger;
use solana::replicator::{sample_file, Replicator};
use solana::signature::{Keypair, KeypairUtil};
use std::fs::File;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

fn main() {
    logger::setup();

    let matches = App::new("replicator")
        .version(crate_version!())
        .arg(
            Arg::with_name("identity")
                .short("i")
                .long("identity")
                .value_name("PATH")
                .takes_value(true)
                .help("Run with the identity found in FILE"),
        ).arg(
            Arg::with_name("network")
                .short("n")
                .long("network")
                .value_name("HOST:PORT")
                .takes_value(true)
                .help("Rendezvous with the network at this gossip entry point"),
        ).arg(
            Arg::with_name("ledger")
                .short("l")
                .long("ledger")
                .value_name("DIR")
                .takes_value(true)
                .required(true)
                .help("use DIR as persistent ledger location"),
        ).get_matches();

    let ledger_path = matches.value_of("ledger");

    let (keypair, ncp) = if let Some(i) = matches.value_of("identity") {
        let path = i.to_string();
        if let Ok(file) = File::open(path.clone()) {
            let parse: serde_json::Result<Config> = serde_json::from_reader(file);
            if let Ok(data) = parse {
                (data.keypair(), data.node_info.contact_info.ncp)
            } else {
                eprintln!("failed to parse {}", path);
                exit(1);
            }
        } else {
            eprintln!("failed to read {}", path);
            exit(1);
        }
    } else {
        (Keypair::new(), socketaddr!([127, 0, 0, 1], 8700))
    };

    let node = Node::new_with_external_ip(keypair.pubkey(), &ncp);

    println!(
        "replicating the data with keypair: {:?} ncp:{:?}",
        keypair.pubkey(),
        ncp
    );
    println!("my node: {:?}", node);

    let exit = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));

    let network_addr = matches
        .value_of("network")
        .map(|network| network.parse().expect("failed to parse network address"));

    // TODO: ask network what slice we should store
    let entry_height = 0;

    let replicator = Replicator::new(
        entry_height,
        5,
        &exit,
        ledger_path,
        node,
        network_addr,
        done.clone(),
    );

    while !done.load(Ordering::Relaxed) {
        sleep(Duration::from_millis(100));
    }

    println!("Done downloading ledger");

    let ledger_path = Path::new(ledger_path.unwrap());
    let ledger_data_file = ledger_path.join(LEDGER_DATA_FILE);
    let ledger_data_file_encrypted = ledger_path.join(format!("{}.enc", LEDGER_DATA_FILE));
    let key = "abc123";

    if let Err(e) = chacha_cbc_encrypt_files(
        &ledger_data_file,
        &ledger_data_file_encrypted,
        key.to_string(),
    ) {
        println!("Error while encrypting ledger: {:?}", e);
        return;
    }

    println!("Done encrypting the ledger");

    let sampling_offsets = [0, 1, 2, 3];

    match sample_file(&ledger_data_file_encrypted, &sampling_offsets) {
        Ok(hash) => println!("sampled hash: {}", hash),
        Err(e) => println!("Error occurred while sampling: {:?}", e),
    }

    replicator.join();
}
