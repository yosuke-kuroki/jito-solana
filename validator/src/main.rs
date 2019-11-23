use bzip2::bufread::BzDecoder;
use clap::{crate_description, crate_name, value_t, value_t_or_exit, App, Arg};
use console::{style, Emoji};
use indicatif::{ProgressBar, ProgressStyle};
use log::*;
use solana_clap_utils::{
    input_parsers::pubkey_of,
    input_validators::{is_keypair, is_pubkey_or_keypair},
    keypair::{
        self, keypair_input, KeypairWithSource, ASK_SEED_PHRASE_ARG,
        SKIP_SEED_PHRASE_VALIDATION_ARG,
    },
};
use solana_client::rpc_client::RpcClient;
use solana_core::{
    cluster_info::{ClusterInfo, Node, VALIDATOR_PORT_RANGE},
    contact_info::ContactInfo,
    gossip_service::GossipService,
    ledger_cleanup_service::DEFAULT_MAX_LEDGER_SLOTS,
    validator::{Validator, ValidatorConfig},
};
use solana_ledger::bank_forks::SnapshotConfig;
use solana_perf::recycler::enable_recycler_warming;
use solana_sdk::{
    clock::Slot,
    hash::Hash,
    pubkey::Pubkey,
    signature::{Keypair, KeypairUtil},
};
use std::{
    fs::{self, File},
    io::{self, Read},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::exit,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    thread::sleep,
    time::{Duration, Instant},
};

fn port_validator(port: String) -> Result<(), String> {
    port.parse::<u16>()
        .map(|_| ())
        .map_err(|e| format!("{:?}", e))
}

fn port_range_validator(port_range: String) -> Result<(), String> {
    if solana_net_utils::parse_port_range(&port_range).is_some() {
        Ok(())
    } else {
        Err("Invalid port range".to_string())
    }
}

fn hash_validator(hash: String) -> Result<(), String> {
    Hash::from_str(&hash)
        .map(|_| ())
        .map_err(|e| format!("{:?}", e))
}

static TRUCK: Emoji = Emoji("🚚 ", "");
static SPARKLE: Emoji = Emoji("✨ ", "");

/// Creates a new process bar for processing that will take an unknown amount of time
fn new_spinner_progress_bar() -> ProgressBar {
    let progress_bar = ProgressBar::new(42);
    progress_bar
        .set_style(ProgressStyle::default_spinner().template("{spinner:.green} {wide_msg}"));
    progress_bar.enable_steady_tick(100);
    progress_bar
}

fn download_tar_bz2(
    rpc_addr: &SocketAddr,
    archive_name: &str,
    download_path: &Path,
    extract: bool,
) -> Result<(), String> {
    let archive_path = download_path.join(archive_name);
    if archive_path.is_file() {
        return Ok(());
    }
    fs::create_dir_all(download_path).map_err(|err| err.to_string())?;

    let temp_archive_path = {
        let mut p = archive_path.clone();
        p.set_extension(".tmp");
        p
    };

    let url = format!("http://{}/{}", rpc_addr, archive_name);
    let download_start = Instant::now();

    let progress_bar = new_spinner_progress_bar();
    progress_bar.set_message(&format!("{}Downloading {}...", TRUCK, url));

    let client = reqwest::Client::new();
    let response = client
        .get(url.as_str())
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|err| format!("Unable to get: {:?}", err))?;
    let download_size = {
        response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|content_length| content_length.to_str().ok())
            .and_then(|content_length| content_length.parse().ok())
            .unwrap_or(0)
    };
    progress_bar.set_length(download_size);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template(&format!(
                "{}{}Downloading {} {}",
                "{spinner:.green} ",
                TRUCK,
                url,
                "[{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})"
            ))
            .progress_chars("=> "),
    );

    struct DownloadProgress<R> {
        progress_bar: ProgressBar,
        response: R,
    }

    impl<R: Read> Read for DownloadProgress<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.response.read(buf).map(|n| {
                self.progress_bar.inc(n as u64);
                n
            })
        }
    }

    let mut source = DownloadProgress {
        progress_bar,
        response,
    };

    let mut file = File::create(&temp_archive_path)
        .map_err(|err| format!("Unable to create {:?}: {:?}", temp_archive_path, err))?;
    std::io::copy(&mut source, &mut file)
        .map_err(|err| format!("Unable to write {:?}: {:?}", temp_archive_path, err))?;

    source.progress_bar.finish_and_clear();
    info!(
        "  {}{}",
        SPARKLE,
        format!(
            "Downloaded {} ({} bytes) in {:?}",
            url,
            download_size,
            Instant::now().duration_since(download_start),
        )
    );

    if extract {
        info!("Extracting {:?}...", archive_path);
        let extract_start = Instant::now();
        let tar_bz2 = File::open(&temp_archive_path)
            .map_err(|err| format!("Unable to open {}: {:?}", archive_name, err))?;
        let tar = BzDecoder::new(std::io::BufReader::new(tar_bz2));
        let mut archive = tar::Archive::new(tar);
        archive
            .unpack(download_path)
            .map_err(|err| format!("Unable to unpack {}: {:?}", archive_name, err))?;
        info!(
            "Extracted {} in {:?}",
            archive_name,
            Instant::now().duration_since(extract_start)
        );
    }
    std::fs::rename(temp_archive_path, archive_path)
        .map_err(|err| format!("Unable to rename: {:?}", err))?;

    Ok(())
}

fn get_rpc_addr(
    node: &Node,
    identity_keypair: &Arc<Keypair>,
    entrypoint_gossip: &SocketAddr,
) -> (Pubkey, SocketAddr) {
    let mut cluster_info = ClusterInfo::new(node.info.clone(), identity_keypair.clone());
    cluster_info.set_entrypoint(ContactInfo::new_gossip_entry_point(entrypoint_gossip));
    let cluster_info = Arc::new(RwLock::new(cluster_info));

    let exit = Arc::new(AtomicBool::new(false));
    let gossip_service = GossipService::new(
        &cluster_info.clone(),
        None,
        None,
        node.sockets.gossip.try_clone().unwrap(),
        &exit,
    );

    let (id, rpc_addr) = loop {
        info!(
            "Searching for RPC service...\n{}",
            cluster_info.read().unwrap().contact_info_trace()
        );

        let (gossip_peers, rpc_peers) = {
            let cluster_info = cluster_info.read().unwrap();
            (cluster_info.gossip_peers(), cluster_info.rpc_peers())
        };

        let found_entrypoint = gossip_peers
            .iter()
            .any(|contact_info| contact_info.gossip == *entrypoint_gossip);

        if found_entrypoint & !rpc_peers.is_empty() {
            // Prefer the entrypoint's RPC service it it has one, otherwise pick the first RPC
            // service found
            if let Some(contact_info) = rpc_peers
                .iter()
                .find(|contact_info| contact_info.gossip == *entrypoint_gossip)
            {
                break (contact_info.id, contact_info.rpc);
            }
            break (rpc_peers[0].id, rpc_peers[0].rpc);
        }

        sleep(Duration::from_secs(1));
    };

    exit.store(true, Ordering::Relaxed);
    gossip_service.join().unwrap();

    (id, rpc_addr)
}

fn check_vote_account(
    rpc_client: &RpcClient,
    vote_pubkey: &Pubkey,
    voting_pubkey: &Pubkey,
    node_pubkey: &Pubkey,
) -> Result<(), String> {
    let found_vote_account = rpc_client
        .get_account(vote_pubkey)
        .map_err(|err| format!("Failed to get vote account: {}", err.to_string()))?;

    if found_vote_account.owner != solana_vote_program::id() {
        return Err(format!(
            "not a vote account (owned by {}): {}",
            found_vote_account.owner, vote_pubkey
        ));
    }

    let found_node_account = rpc_client
        .get_account(node_pubkey)
        .map_err(|err| format!("Failed to get identity account: {}", err.to_string()))?;

    let found_vote_account = solana_vote_program::vote_state::VoteState::from(&found_vote_account);
    if let Some(found_vote_account) = found_vote_account {
        if found_vote_account.authorized_voter != *voting_pubkey {
            return Err(format!(
                "account's authorized voter ({}) does not match to the given voting keypair ({}).",
                found_vote_account.authorized_voter, voting_pubkey
            ));
        }
        if found_vote_account.node_pubkey != *node_pubkey {
            return Err(format!(
                "account's node pubkey ({}) does not match to the given identity keypair ({}).",
                found_vote_account.node_pubkey, node_pubkey
            ));
        }
    } else {
        return Err(format!("invalid vote account data: {}", vote_pubkey));
    }

    // Maybe we can calculate minimum voting fee; rather than 1 lamport
    if found_node_account.lamports <= 1 {
        return Err(format!(
            "unfunded identity account ({}): only {} lamports (needs more fund to vote)",
            node_pubkey, found_node_account.lamports
        ));
    }

    Ok(())
}

fn download_ledger(
    rpc_addr: &SocketAddr,
    ledger_path: &Path,
    no_snapshot_fetch: bool,
) -> Result<(), String> {
    download_tar_bz2(rpc_addr, "genesis.tar.bz2", ledger_path, true)?;

    if !no_snapshot_fetch {
        let snapshot_package = solana_ledger::snapshot_utils::get_snapshot_tar_path(ledger_path);
        if snapshot_package.exists() {
            fs::remove_file(&snapshot_package)
                .unwrap_or_else(|err| warn!("error removing {:?}: {}", snapshot_package, err));
        }
        download_tar_bz2(
            rpc_addr,
            snapshot_package.file_name().unwrap().to_str().unwrap(),
            snapshot_package.parent().unwrap(),
            false,
        )
        .unwrap_or_else(|err| warn!("Unable to fetch snapshot: {:?}", err));
    }

    Ok(())
}

#[allow(clippy::cognitive_complexity)]
pub fn main() {
    let default_dynamic_port_range =
        &format!("{}-{}", VALIDATOR_PORT_RANGE.0, VALIDATOR_PORT_RANGE.1);

    let matches = App::new(crate_name!()).about(crate_description!())
        .version(solana_clap_utils::version!())
        .arg(
            Arg::with_name("blockstream_unix_socket")
                .long("blockstream")
                .takes_value(true)
                .value_name("UNIX DOMAIN SOCKET")
                .help("Stream entries to this unix domain socket path")
        )
        .arg(
            Arg::with_name(ASK_SEED_PHRASE_ARG.name)
                .long(ASK_SEED_PHRASE_ARG.long)
                .value_name("KEYPAIR NAME")
                .multiple(true)
                .takes_value(true)
                .possible_values(&["identity-keypair", "storage-keypair", "voting-keypair"])
                .help(ASK_SEED_PHRASE_ARG.help),
        )
        .arg(
            Arg::with_name(SKIP_SEED_PHRASE_VALIDATION_ARG.name)
                .long(SKIP_SEED_PHRASE_VALIDATION_ARG.long)
                .requires(ASK_SEED_PHRASE_ARG.name)
                .help(SKIP_SEED_PHRASE_VALIDATION_ARG.help),
        )
        .arg(
            Arg::with_name("identity_keypair")
                .short("i")
                .long("identity-keypair")
                .value_name("PATH")
                .takes_value(true)
                .validator(is_keypair)
                .help("File containing the identity keypair for the validator"),
        )
        .arg(
            Arg::with_name("voting_keypair")
                .long("voting-keypair")
                .value_name("PATH")
                .takes_value(true)
                .validator(is_keypair)
                .help("File containing the authorized voting keypair.  Default is an ephemeral keypair, which may disable voting without --vote-account."),
        )
        .arg(
            Arg::with_name("vote_account")
                .long("vote-account")
                .value_name("PUBKEY")
                .takes_value(true)
                .validator(is_pubkey_or_keypair)
                .help("Public key of the vote account to vote with.  Default is the public key of --voting-keypair"),
        )
        .arg(
            Arg::with_name("storage_keypair")
                .long("storage-keypair")
                .value_name("PATH")
                .takes_value(true)
                .validator(is_keypair)
                .help("File containing the storage account keypair.  Default is an ephemeral keypair"),
        )
        .arg(
            Arg::with_name("init_complete_file")
                .long("init-complete-file")
                .value_name("FILE")
                .takes_value(true)
                .help("Create this file, if it doesn't already exist, once node initialization is complete"),
        )
        .arg(
            Arg::with_name("ledger_path")
                .short("l")
                .long("ledger")
                .value_name("DIR")
                .takes_value(true)
                .required(true)
                .help("Use DIR as persistent ledger location"),
        )
        .arg(
            Arg::with_name("entrypoint")
                .short("n")
                .long("entrypoint")
                .value_name("HOST:PORT")
                .takes_value(true)
                .validator(solana_net_utils::is_host_port)
                .help("Rendezvous with the cluster at this gossip entrypoint"),
        )
        .arg(
            Arg::with_name("no_snapshot_fetch")
                .long("no-snapshot-fetch")
                .takes_value(false)
                .requires("entrypoint")
                .help("Do not attempt to fetch a snapshot from the cluster, start from a local snapshot if present"),
        )
        .arg(
            Arg::with_name("no_genesis_fetch")
                .long("no-genesis-fetch")
                .takes_value(false)
                .requires("entrypoint")
                .help("Do not fetch genesis from the cluster"),
        )
        .arg(
            Arg::with_name("no_voting")
                .long("no-voting")
                .takes_value(false)
                .help("Launch node without voting"),
        )
        .arg(
            Arg::with_name("dev_no_sigverify")
                .long("dev-no-sigverify")
                .takes_value(false)
                .help("Run without signature verification"),
        )
        .arg(
            Arg::with_name("dev_halt_at_slot")
                .long("dev-halt-at-slot")
                .value_name("SLOT")
                .takes_value(true)
                .help("Halt the validator when it reaches the given slot"),
        )
        .arg(
            Arg::with_name("rpc_port")
                .long("rpc-port")
                .value_name("PORT")
                .takes_value(true)
                .validator(port_validator)
                .help("RPC port to use for this node"),
        )
        .arg(
            Arg::with_name("enable_rpc_exit")
                .long("enable-rpc-exit")
                .takes_value(false)
                .help("Enable the JSON RPC 'validatorExit' API.  Only enable in a debug environment"),
        )
        .arg(
            Arg::with_name("rpc_drone_addr")
                .long("rpc-drone-address")
                .value_name("HOST:PORT")
                .takes_value(true)
                .validator(solana_net_utils::is_host_port)
                .help("Enable the JSON RPC 'requestAirdrop' API with this drone address."),
        )
        .arg(
            Arg::with_name("signer_addr")
                .long("vote-signer-address")
                .value_name("HOST:PORT")
                .takes_value(true)
                .hidden(true) // Don't document this argument to discourage its use
                .validator(solana_net_utils::is_host_port)
                .help("Rendezvous with the vote signer at this RPC end point"),
        )
        .arg(
            Arg::with_name("account_paths")
                .long("accounts")
                .value_name("PATHS")
                .takes_value(true)
                .help("Comma separated persistent accounts location"),
        )
        .arg(
            clap::Arg::with_name("gossip_port")
                .long("gossip-port")
                .value_name("PORT")
                .takes_value(true)
                .help("Gossip port number for the node"),
        )
        .arg(
            clap::Arg::with_name("gossip_host")
                .long("gossip-host")
                .value_name("HOST")
                .takes_value(true)
                .conflicts_with("entrypoint")
                .validator(solana_net_utils::is_host)
                .help("Gossip DNS name or IP address for the node when --entrypoint is not provided [default: 127.0.0.1]"),
        )
        .arg(
            clap::Arg::with_name("dynamic_port_range")
                .long("dynamic-port-range")
                .value_name("MIN_PORT-MAX_PORT")
                .takes_value(true)
                .default_value(default_dynamic_port_range)
                .validator(port_range_validator)
                .help("Range to use for dynamically assigned ports"),
        )
        .arg(
            clap::Arg::with_name("snapshot_interval_slots")
                .long("snapshot-interval-slots")
                .value_name("SNAPSHOT_INTERVAL_SLOTS")
                .takes_value(true)
                .default_value("100")
                .help("Number of slots between generating snapshots, 0 to disable snapshots"),
        )
        .arg(
            clap::Arg::with_name("limit_ledger_size")
                .long("limit-ledger-size")
                .takes_value(false)
                .help("drop older slots in the ledger"),
        )
        .arg(
            clap::Arg::with_name("skip_poh_verify")
                .long("skip-poh-verify")
                .takes_value(false)
                .help("Skip ledger verification at node bootup"),
        )
        .arg(
            clap::Arg::with_name("cuda")
                .long("cuda")
                .takes_value(false)
                .help("Use CUDA"),
        )
        .arg(
            Arg::with_name("expected_genesis_hash")
                .long("expected-genesis-hash")
                .value_name("HASH")
                .takes_value(true)
                .validator(hash_validator)
                .help("Require the genesis have this hash"),
        )
        .arg(
            Arg::with_name("logfile")
                .short("o")
                .long("log")
                .value_name("FILE")
                .takes_value(true)
                .help("Redirect logging to the specified file, '-' for standard error"),
        )
        .get_matches();

    let identity_keypair = Arc::new(
        keypair_input(&matches, "identity-keypair")
            .unwrap_or_else(|err| {
                eprintln!("Identity keypair input failed: {}", err);
                exit(1);
            })
            .keypair,
    );
    let KeypairWithSource {
        keypair: voting_keypair,
        source: voting_keypair_source,
    } = keypair_input(&matches, "voting-keypair").unwrap_or_else(|err| {
        eprintln!("Voting keypair input failed: {}", err);
        exit(1);
    });
    let ephemeral_voting_keypair = voting_keypair_source == keypair::Source::Generated;
    let storage_keypair = keypair_input(&matches, "storage-keypair")
        .unwrap_or_else(|err| {
            eprintln!("Storage keypair input failed: {}", err);
            exit(1);
        })
        .keypair;

    let ledger_path = PathBuf::from(matches.value_of("ledger_path").unwrap());
    let entrypoint = matches.value_of("entrypoint");
    let init_complete_file = matches.value_of("init_complete_file");
    let skip_poh_verify = matches.is_present("skip_poh_verify");
    let cuda = matches.is_present("cuda");
    let no_genesis_fetch = matches.is_present("no_genesis_fetch");
    let no_snapshot_fetch = matches.is_present("no_snapshot_fetch");
    let rpc_port = value_t!(matches, "rpc_port", u16);

    let mut validator_config = ValidatorConfig::default();
    validator_config.dev_sigverify_disabled = matches.is_present("dev_no_sigverify");
    validator_config.dev_halt_at_slot = value_t!(matches, "dev_halt_at_slot", Slot).ok();

    validator_config.rpc_config.enable_validator_exit = matches.is_present("enable_rpc_exit");

    validator_config.rpc_config.drone_addr = matches.value_of("rpc_drone_addr").map(|address| {
        solana_net_utils::parse_host_port(address).expect("failed to parse drone address")
    });

    let dynamic_port_range =
        solana_net_utils::parse_port_range(matches.value_of("dynamic_port_range").unwrap())
            .expect("invalid dynamic_port_range");

    if let Some(account_paths) = matches.value_of("account_paths") {
        validator_config.account_paths = Some(account_paths.to_string());
    } else {
        validator_config.account_paths =
            Some(ledger_path.join("accounts").to_str().unwrap().to_string());
    }

    let snapshot_interval_slots = value_t_or_exit!(matches, "snapshot_interval_slots", usize);
    let snapshot_path = ledger_path.clone().join("snapshot");
    fs::create_dir_all(&snapshot_path).unwrap_or_else(|err| {
        eprintln!(
            "Failed to create snapshots directory {:?}: {}",
            snapshot_path, err
        );
        exit(1);
    });

    validator_config.snapshot_config = Some(SnapshotConfig {
        snapshot_interval_slots: if snapshot_interval_slots > 0 {
            snapshot_interval_slots
        } else {
            std::usize::MAX
        },
        snapshot_path,
        snapshot_package_output_path: ledger_path.clone(),
    });

    if matches.is_present("limit_ledger_size") {
        validator_config.max_ledger_slots = Some(DEFAULT_MAX_LEDGER_SLOTS);
    }

    if matches.value_of("signer_addr").is_some() {
        warn!("--vote-signer-address ignored");
    }

    validator_config.blockstream_unix_socket = matches
        .value_of("blockstream_unix_socket")
        .map(PathBuf::from);

    validator_config.expected_genesis_hash = matches
        .value_of("expected_genesis_hash")
        .map(|s| Hash::from_str(&s).unwrap());

    println!(
        "{} {}",
        style(crate_name!()).bold(),
        solana_clap_utils::version!()
    );

    let _log_redirect = {
        #[cfg(unix)]
        {
            let default_logfile = format!(
                "solana-validator-{}-{}.log",
                identity_keypair.pubkey(),
                chrono::Utc::now().format("%Y%m%d-%H%M%S")
            );
            let logfile = matches.value_of("logfile").unwrap_or(&default_logfile);

            if logfile == "-" {
                None
            } else {
                println!("log file: {}", logfile);
                Some(gag::Redirect::stderr(File::create(logfile).unwrap_or_else(
                    |err| {
                        eprintln!("Unable to create {}: {:?}", logfile, err);
                        exit(1);
                    },
                )))
            }
        }
        #[cfg(not(unix))]
        {
            println!("logging to a file is not supported on this platform");
            ()
        }
    };

    solana_logger::setup_with_filter(
        &[
            "solana=info", /* info logging for all solana modules */
            "rpc=trace",   /* json_rpc request/response logging */
        ]
        .join(","),
    );

    if matches.is_present("no_voting") {
        validator_config.voting_disabled = true;
    }

    let vote_account = pubkey_of(&matches, "vote_account").unwrap_or_else(|| {
        // Disable voting because normal (=not bootstrapping) validator rejects
        // non-voting accounts (= ephemeral keypairs).
        if ephemeral_voting_keypair && entrypoint.is_some() {
            warn!("Disabled voting due to the use of ephemeral key for vote account");
            validator_config.voting_disabled = true;
        };
        voting_keypair.pubkey()
    });

    solana_metrics::set_host_id(identity_keypair.pubkey().to_string());
    solana_metrics::set_panic_hook("validator");

    if cuda {
        solana_perf::perf_libs::init_cuda();
        enable_recycler_warming();
    }

    let entrypoint_addr = matches.value_of("entrypoint").map(|entrypoint| {
        solana_net_utils::parse_host_port(entrypoint).unwrap_or_else(|e| {
            eprintln!("failed to parse entrypoint address: {}", e);
            exit(1);
        })
    });

    let gossip_host = if let Some(entrypoint_addr) = entrypoint_addr {
        solana_net_utils::get_public_ip_addr(&entrypoint_addr).unwrap_or_else(|err| {
            eprintln!(
                "Failed to contact cluster entrypoint {}: {}",
                entrypoint_addr, err
            );
            exit(1);
        })
    } else {
        solana_net_utils::parse_host(matches.value_of("gossip_host").unwrap_or("127.0.0.1"))
            .unwrap_or_else(|err| {
                eprintln!("Error: {}", err);
                exit(1);
            })
    };

    let gossip_addr = SocketAddr::new(
        gossip_host,
        value_t!(matches, "gossip_port", u16).unwrap_or_else(|_| {
            solana_net_utils::find_available_port_in_range((0, 1))
                .expect("unable to find an available gossip port")
        }),
    );

    let cluster_entrypoint = entrypoint_addr
        .as_ref()
        .map(ContactInfo::new_gossip_entry_point);

    let mut tcp_ports = vec![];
    let mut node =
        Node::new_with_external_ip(&identity_keypair.pubkey(), &gossip_addr, dynamic_port_range);
    if let Ok(rpc_port) = rpc_port {
        let rpc_pubsub_port = rpc_port + 1;
        node.info.rpc = SocketAddr::new(node.info.gossip.ip(), rpc_port);
        node.info.rpc_pubsub = SocketAddr::new(node.info.gossip.ip(), rpc_pubsub_port);
        tcp_ports = vec![rpc_port, rpc_pubsub_port];
    };

    if let Some(ref cluster_entrypoint) = cluster_entrypoint {
        let udp_sockets = [
            &node.sockets.gossip,
            &node.sockets.broadcast,
            &node.sockets.repair,
        ];

        let mut tcp_listeners: Vec<(_, _)> = tcp_ports
            .iter()
            .map(|port| {
                (
                    *port,
                    TcpListener::bind(&SocketAddr::from(([0, 0, 0, 0], *port))).unwrap_or_else(
                        |err| {
                            error!("Unable to bind to tcp/{}: {}", port, err);
                            std::process::exit(1);
                        },
                    ),
                )
            })
            .collect();
        if let Some(ip_echo) = &node.sockets.ip_echo {
            let ip_echo = ip_echo.try_clone().expect("unable to clone tcp_listener");
            tcp_listeners.push((node.info.gossip.port(), ip_echo));
        }

        solana_net_utils::verify_reachable_ports(
            &cluster_entrypoint.gossip,
            tcp_listeners,
            &udp_sockets,
        );

        if !no_genesis_fetch {
            let (rpc_node_id, rpc_addr) =
                get_rpc_addr(&node, &identity_keypair, &cluster_entrypoint.gossip);
            info!("Using RPC from node {}: {:?}", rpc_node_id, rpc_addr);
            let rpc_client = RpcClient::new_socket(rpc_addr);
            let rpc_version = rpc_client.get_version().unwrap_or_else(|err| {
                error!("Failed to get version: {}", err);
                exit(1);
            });
            info!("RPC node version: {}", rpc_version.solana_core);

            download_ledger(&rpc_addr, &ledger_path, no_snapshot_fetch).unwrap_or_else(|err| {
                error!("Failed to initialize ledger: {}", err);
                exit(1);
            });

            let genesis_hash = rpc_client.get_genesis_hash().unwrap_or_else(|err| {
                error!("Failed to get genesis hash: {}", err);
                exit(1);
            });

            if let Some(expected_genesis_hash) = validator_config.expected_genesis_hash {
                if expected_genesis_hash != genesis_hash {
                    error!(
                        "Genesis hash mismatch: expected {} but local genesis hash is {}",
                        expected_genesis_hash, genesis_hash,
                    );
                    exit(1);
                }
            }
            validator_config.expected_genesis_hash = Some(genesis_hash);

            if !validator_config.voting_disabled {
                check_vote_account(
                    &rpc_client,
                    &vote_account,
                    &voting_keypair.pubkey(),
                    &identity_keypair.pubkey(),
                )
                .unwrap_or_else(|err| {
                    error!("Failed to check vote account: {}", err);
                    exit(1);
                });
            }
        }
    }

    if !ledger_path.is_dir() {
        error!(
            "ledger directory does not exist or is not accessible: {:?}",
            ledger_path
        );
        exit(1);
    }

    let validator = Validator::new(
        node,
        &identity_keypair,
        &ledger_path,
        &vote_account,
        &Arc::new(voting_keypair),
        &Arc::new(storage_keypair),
        cluster_entrypoint.as_ref(),
        !skip_poh_verify,
        &validator_config,
    );

    if let Some(filename) = init_complete_file {
        File::create(filename).unwrap_or_else(|_| {
            error!("Unable to create: {}", filename);
            exit(1);
        });
    }
    info!("Validator initialized");
    validator.join().expect("validator exit");
    info!("Validator exiting..");
}
