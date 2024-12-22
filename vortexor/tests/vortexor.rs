use {
    crossbeam_channel::unbounded,
    solana_net_utils::VALIDATOR_PORT_RANGE,
    solana_sdk::{net::DEFAULT_TPU_COALESCE, pubkey::Pubkey, signature::Keypair, signer::Signer},
    solana_streamer::{
        nonblocking::{
            quic::{DEFAULT_MAX_CONNECTIONS_PER_IPADDR_PER_MINUTE, DEFAULT_MAX_STREAMS_PER_MS},
            testing_utilities::check_multiple_streams,
        },
        quic::{MAX_STAKED_CONNECTIONS, MAX_UNSTAKED_CONNECTIONS},
        streamer::StakedNodes,
    },
    solana_vortexor::{
        cli::{DEFAULT_MAX_QUIC_CONNECTIONS_PER_PEER, DEFAULT_NUM_QUIC_ENDPOINTS},
        vortexor::Vortexor,
    },
    std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, RwLock,
        },
    },
};

#[tokio::test(flavor = "multi_thread")]
async fn test_vortexor() {
    solana_logger::setup();

    let bind_address = solana_net_utils::parse_host("127.0.0.1").expect("invalid bind_address");
    let keypair = Keypair::new();
    let exit = Arc::new(AtomicBool::new(false));

    let (tpu_sender, tpu_receiver) = unbounded();
    let (tpu_fwd_sender, tpu_fwd_receiver) = unbounded();
    let tpu_sockets = Vortexor::create_tpu_sockets(
        bind_address,
        VALIDATOR_PORT_RANGE,
        DEFAULT_NUM_QUIC_ENDPOINTS.try_into().unwrap(),
    );

    let tpu_address = tpu_sockets.tpu_quic[0].local_addr().unwrap();
    let tpu_fwd_address = tpu_sockets.tpu_quic_fwd[0].local_addr().unwrap();

    let stakes = HashMap::from([(keypair.pubkey(), 10000)]);
    let staked_nodes = Arc::new(RwLock::new(StakedNodes::new(
        Arc::new(stakes),
        HashMap::<Pubkey, u64>::default(), // overrides
    )));

    let vortexor = Vortexor::create_vortexor(
        tpu_sockets,
        staked_nodes,
        tpu_sender,
        tpu_fwd_sender,
        DEFAULT_MAX_QUIC_CONNECTIONS_PER_PEER.try_into().unwrap(),
        MAX_STAKED_CONNECTIONS.try_into().unwrap(),
        MAX_UNSTAKED_CONNECTIONS.try_into().unwrap(),
        MAX_STAKED_CONNECTIONS
            .saturating_add(MAX_UNSTAKED_CONNECTIONS)
            .try_into()
            .unwrap(), // max_fwd_staked_connections
        0, // max_fwd_unstaked_connections
        DEFAULT_MAX_STREAMS_PER_MS,
        DEFAULT_MAX_CONNECTIONS_PER_IPADDR_PER_MINUTE,
        DEFAULT_TPU_COALESCE,
        &keypair,
        exit.clone(),
    );

    check_multiple_streams(tpu_receiver, tpu_address, Some(&keypair)).await;
    check_multiple_streams(tpu_fwd_receiver, tpu_fwd_address, Some(&keypair)).await;

    exit.store(true, Ordering::Relaxed);
    vortexor.join().unwrap();
}
