use clap::{crate_description, crate_name, crate_version, App, Arg};
use console::style;
use solana_clap_utils::input_validators::is_keypair;
use solana_core::{
    archiver::Archiver,
    cluster_info::{Node, VALIDATOR_PORT_RANGE},
    contact_info::ContactInfo,
};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    signature::{read_keypair_file, Keypair, KeypairUtil},
};
use std::{net::SocketAddr, path::PathBuf, process::exit, sync::Arc};

fn main() {
    solana_logger::setup();

    let matches = App::new(crate_name!())
        .about(crate_description!())
        .version(crate_version!())
        .arg(
            Arg::with_name("identity")
                .short("i")
                .long("identity")
                .value_name("PATH")
                .takes_value(true)
                .validator(is_keypair)
                .help("File containing an identity (keypair)"),
        )
        .arg(
            Arg::with_name("entrypoint")
                .short("n")
                .long("entrypoint")
                .value_name("HOST:PORT")
                .takes_value(true)
                .required(true)
                .validator(solana_net_utils::is_host_port)
                .help("Rendezvous with the cluster at this entry point"),
        )
        .arg(
            Arg::with_name("ledger")
                .short("l")
                .long("ledger")
                .value_name("DIR")
                .takes_value(true)
                .required(true)
                .help("use DIR as persistent ledger location"),
        )
        .arg(
            Arg::with_name("storage_keypair")
                .short("s")
                .long("storage-keypair")
                .value_name("PATH")
                .takes_value(true)
                .required(true)
                .validator(is_keypair)
                .help("File containing the storage account keypair"),
        )
        .get_matches();

    let ledger_path = PathBuf::from(matches.value_of("ledger").unwrap());

    let keypair = if let Some(identity) = matches.value_of("identity") {
        read_keypair_file(identity).unwrap_or_else(|err| {
            eprintln!("{}: Unable to open keypair file: {}", err, identity);
            exit(1);
        })
    } else {
        Keypair::new()
    };
    let storage_keypair = if let Some(storage_keypair) = matches.value_of("storage_keypair") {
        read_keypair_file(storage_keypair).unwrap_or_else(|err| {
            eprintln!("{}: Unable to open keypair file: {}", err, storage_keypair);
            exit(1);
        })
    } else {
        Keypair::new()
    };

    let entrypoint_addr = matches
        .value_of("entrypoint")
        .map(|entrypoint| {
            solana_net_utils::parse_host_port(entrypoint)
                .expect("failed to parse entrypoint address")
        })
        .unwrap();

    let gossip_addr = {
        let ip = solana_net_utils::get_public_ip_addr(&entrypoint_addr).unwrap();
        let mut addr = SocketAddr::new(ip, 0);
        addr.set_ip(solana_net_utils::get_public_ip_addr(&entrypoint_addr).unwrap());
        addr
    };
    let node =
        Node::new_archiver_with_external_ip(&keypair.pubkey(), &gossip_addr, VALIDATOR_PORT_RANGE);

    println!(
        "{} version {} (branch={}, commit={})",
        style(crate_name!()).bold(),
        crate_version!(),
        option_env!("CI_BRANCH").unwrap_or("unknown"),
        option_env!("CI_COMMIT").unwrap_or("unknown")
    );
    solana_metrics::set_host_id(keypair.pubkey().to_string());
    println!(
        "replicating the data with keypair={:?} gossip_addr={:?}",
        keypair.pubkey(),
        gossip_addr
    );

    let entrypoint_info = ContactInfo::new_gossip_entry_point(&entrypoint_addr);
    let archiver = Archiver::new(
        &ledger_path,
        node,
        entrypoint_info,
        Arc::new(keypair),
        Arc::new(storage_keypair),
        CommitmentConfig::recent(),
    )
    .unwrap();

    archiver.join();
}
