extern crate env_logger;
extern crate getopts;
extern crate isatty;
extern crate serde_json;
extern crate solana;

use getopts::Options;
use isatty::stdin_isatty;
use solana::accountant::Accountant;
use solana::accountant_skel::AccountantSkel;
use solana::crdt::ReplicatedData;
use solana::entry::Entry;
use solana::event::Event;
use solana::historian::Historian;
use solana::signature::{KeyPair, KeyPairUtil};
use std::env;
use std::io::{stdin, stdout, Read};
use std::net::UdpSocket;
use std::process::exit;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::sync_channel;
use std::sync::Arc;

fn print_usage(program: &str, opts: Options) {
    let mut brief = format!("Usage: cat <transaction.log> | {} [options]\n\n", program);
    brief += "  Run a Solana node to handle transactions and\n";
    brief += "  write a new transaction log to stdout.\n";
    brief += "  Takes existing transaction log from stdin.";

    print!("{}", opts.usage(&brief));
}

fn main() {
    env_logger::init().unwrap();
    let mut port = 8000u16;
    let mut opts = Options::new();
    opts.optopt("p", "", "port", "port");
    opts.optflag("h", "help", "print help");
    let args: Vec<String> = env::args().collect();
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{}", e);
            exit(1);
        }
    };
    if matches.opt_present("h") {
        let program = args[0].clone();
        print_usage(&program, opts);
        return;
    }
    if matches.opt_present("p") {
        port = matches.opt_str("p").unwrap().parse().expect("port");
    }
    let serve_addr = format!("0.0.0.0:{}", port);
    let gossip_addr = format!("0.0.0.0:{}", port + 1);
    let replicate_addr = format!("0.0.0.0:{}", port + 2);

    if stdin_isatty() {
        eprintln!("nothing found on stdin, expected a log file");
        exit(1);
    }

    let mut buffer = String::new();
    let num_bytes = stdin().read_to_string(&mut buffer).unwrap();
    if num_bytes == 0 {
        eprintln!("empty file on stdin, expected a log file");
        exit(1);
    }

    eprintln!("Initializing...");
    let mut entries = buffer.lines().map(|line| {
        serde_json::from_str(&line).unwrap_or_else(|e| {
            eprintln!("failed to parse json: {}", e);
            exit(1);
        })
    });

    eprintln!("done parsing...");

    // The first item in the ledger is required to be an entry with zero num_hashes,
    // which implies its id can be used as the ledger's seed.
    let entry0 = entries.next().unwrap();

    // The second item in the ledger is a special transaction where the to and from
    // fields are the same. That entry should be treated as a deposit, not a
    // transfer to oneself.
    let entry1: Entry = entries.next().unwrap();
    let deposit = if let Event::Transaction(ref tr) = entry1.events[0] {
        tr.data.plan.final_payment()
    } else {
        None
    };

    eprintln!("creating accountant...");

    let acc = Accountant::new_from_deposit(&deposit.unwrap());
    acc.register_entry_id(&entry0.id);
    acc.register_entry_id(&entry1.id);

    eprintln!("processing entries...");

    let mut last_id = entry1.id;
    for entry in entries {
        last_id = entry.id;
        acc.process_verified_events(entry.events).unwrap();
        acc.register_entry_id(&last_id);
    }

    eprintln!("creating networking stack...");

    let (input, event_receiver) = sync_channel(10_000);
    let historian = Historian::new(event_receiver, &last_id, Some(1000));
    let exit = Arc::new(AtomicBool::new(false));
    let skel = Arc::new(AccountantSkel::new(acc, input, historian));
    let serve_sock = UdpSocket::bind(&serve_addr).unwrap();
    let gossip_sock = UdpSocket::bind(&gossip_addr).unwrap();
    let replicate_sock = UdpSocket::bind(&replicate_addr).unwrap();
    let pubkey = KeyPair::new().pubkey();
    let d = ReplicatedData::new(
        pubkey,
        gossip_sock.local_addr().unwrap(),
        replicate_sock.local_addr().unwrap(),
        serve_sock.local_addr().unwrap(),
    );
    eprintln!("starting server...");
    let threads =
        AccountantSkel::serve(&skel, d, serve_sock, gossip_sock, exit.clone(), stdout()).unwrap();
    eprintln!("Ready. Listening on {}", serve_addr);
    for t in threads {
        t.join().expect("join");
    }
}
