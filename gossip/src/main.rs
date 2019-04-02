//! A command-line executable for monitoring a network's gossip plane.

use clap::{crate_description, crate_name, crate_version, App, Arg};
use solana::gossip_service::discover;
use solana_sdk::pubkey::Pubkey;
use std::error;
use std::net::SocketAddr;
use std::process::exit;

fn pubkey_validator(pubkey: String) -> Result<(), String> {
    match pubkey.parse::<Pubkey>() {
        Ok(_) => Ok(()),
        Err(err) => Err(format!("{:?}", err)),
    }
}

fn main() -> Result<(), Box<dyn error::Error>> {
    solana_logger::setup();
    let mut network_addr = SocketAddr::from(([127, 0, 0, 1], 8001));
    let network_string = network_addr.to_string();
    let matches = App::new(crate_name!())
        .about(crate_description!())
        .version(crate_version!())
        .arg(
            Arg::with_name("network")
                .short("n")
                .long("network")
                .value_name("HOST:PORT")
                .takes_value(true)
                .default_value(&network_string)
                .help("Rendezvous with the network at this gossip entry point; defaults to 127.0.0.1:8001"),
        )
        .arg(
            Arg::with_name("num_nodes")
                .short("N")
                .long("num-nodes")
                .value_name("NUM")
                .takes_value(true)
                .conflicts_with("num_nodes_exactly")
                .help("Wait for at least NUM nodes to converge"),
        )
        .arg(
            Arg::with_name("num_nodes_exactly")
                .short("E")
                .long("num-nodes-exactly")
                .value_name("NUM")
                .takes_value(true)
                .help("Wait for exactly NUM nodes to converge"),
        )
        .arg(
            Arg::with_name("node_pubkey")
                .short("p")
                .long("pubkey")
                .value_name("PUBKEY")
                .takes_value(true)
                .validator(pubkey_validator)
                .help("Public key of a specific node to wait for"),
        )
        .arg(
            Arg::with_name("timeout")
                .long("timeout")
                .value_name("SECS")
                .takes_value(true)
                .help("Seconds to wait for cluster to converge, then exit; default is forever"),
        )
        .get_matches();

    if let Some(addr) = matches.value_of("network") {
        network_addr = addr.parse().unwrap_or_else(|e| {
            eprintln!("failed to parse network: {}", e);
            exit(1)
        });
    }

    let num_nodes_exactly = matches
        .value_of("num_nodes_exactly")
        .map(|num| num.to_string().parse().unwrap());
    let num_nodes = matches
        .value_of("num_nodes")
        .map(|num| num.to_string().parse().unwrap())
        .or(num_nodes_exactly);
    let timeout = matches
        .value_of("timeout")
        .map(|secs| secs.to_string().parse().unwrap());
    let pubkey = matches
        .value_of("node_pubkey")
        .map(|pubkey_str| pubkey_str.parse::<Pubkey>().unwrap());

    let nodes = discover(&network_addr, num_nodes, timeout, pubkey)?;

    if timeout.is_some() {
        if let Some(num) = num_nodes {
            if nodes.len() < num {
                let add = if num_nodes_exactly.is_some() {
                    ""
                } else {
                    " or more"
                };
                eprintln!(
                    "Error: Insufficient nodes discovered.  Expecting {}{}",
                    num, add,
                );
            }
        }
        if let Some(node) = pubkey {
            if nodes.iter().find(|x| x.id == node).is_none() {
                eprintln!("Error: Could not find node {:?}", node);
            }
        }
    }
    if num_nodes_exactly.is_some() && nodes.len() > num_nodes_exactly.unwrap() {
        eprintln!(
            "Error: Extra nodes discovered.  Expecting exactly {}",
            num_nodes_exactly.unwrap()
        );
    }
    Ok(())
}
