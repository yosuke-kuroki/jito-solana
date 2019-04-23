//! The `retransmit_stage` retransmits blobs between validators

use crate::bank_forks::BankForks;
use crate::blocktree::Blocktree;
use crate::cluster_info::{
    compute_retransmit_peers, ClusterInfo, GROW_LAYER_CAPACITY, NEIGHBORHOOD_SIZE,
};
use crate::leader_schedule_cache::LeaderScheduleCache;
use crate::result::{Error, Result};
use crate::service::Service;
use crate::staking_utils;
use crate::streamer::BlobReceiver;
use crate::window_service::WindowService;
use solana_metrics::counter::Counter;
use solana_metrics::{influxdb, submit};
use solana_sdk::hash::Hash;
use std::net::UdpSocket;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, RwLock};
use std::thread::{self, Builder, JoinHandle};
use std::time::Duration;

fn retransmit(
    bank_forks: &Arc<RwLock<BankForks>>,
    leader_schedule_cache: &Arc<LeaderScheduleCache>,
    cluster_info: &Arc<RwLock<ClusterInfo>>,
    r: &BlobReceiver,
    sock: &UdpSocket,
) -> Result<()> {
    let timer = Duration::new(1, 0);
    let mut blobs = r.recv_timeout(timer)?;
    while let Ok(mut nq) = r.try_recv() {
        blobs.append(&mut nq);
    }

    submit(
        influxdb::Point::new("retransmit-stage")
            .add_field("count", influxdb::Value::Integer(blobs.len() as i64))
            .to_owned(),
    );
    let r_bank = bank_forks.read().unwrap().working_bank();
    let bank_epoch = r_bank.get_stakers_epoch(r_bank.slot());
    let (neighbors, children) = compute_retransmit_peers(
        &staking_utils::delegated_stakes_at_epoch(&r_bank, bank_epoch).unwrap(),
        cluster_info,
        NEIGHBORHOOD_SIZE,
        NEIGHBORHOOD_SIZE,
        GROW_LAYER_CAPACITY,
    );
    for blob in &blobs {
        let leader = leader_schedule_cache
            .slot_leader_at_else_compute(blob.read().unwrap().slot(), r_bank.as_ref());
        if blob.read().unwrap().meta.forward {
            ClusterInfo::retransmit_to(&cluster_info, &neighbors, blob, leader, sock, true)?;
            ClusterInfo::retransmit_to(&cluster_info, &children, blob, leader, sock, false)?;
        } else {
            ClusterInfo::retransmit_to(&cluster_info, &children, blob, leader, sock, true)?;
        }
    }
    Ok(())
}

/// Service to retransmit messages from the leader or layer 1 to relevant peer nodes.
/// See `cluster_info` for network layer definitions.
/// # Arguments
/// * `sock` - Socket to read from.  Read timeout is set to 1.
/// * `exit` - Boolean to signal system exit.
/// * `cluster_info` - This structure needs to be updated and populated by the bank and via gossip.
/// * `recycler` - Blob recycler.
/// * `r` - Receive channel for blobs to be retransmitted to all the layer 1 nodes.
fn retransmitter(
    sock: Arc<UdpSocket>,
    bank_forks: Arc<RwLock<BankForks>>,
    leader_schedule_cache: &Arc<LeaderScheduleCache>,
    cluster_info: Arc<RwLock<ClusterInfo>>,
    r: BlobReceiver,
) -> JoinHandle<()> {
    let bank_forks = bank_forks.clone();
    let leader_schedule_cache = leader_schedule_cache.clone();
    Builder::new()
        .name("solana-retransmitter".to_string())
        .spawn(move || {
            trace!("retransmitter started");
            loop {
                if let Err(e) = retransmit(
                    &bank_forks,
                    &leader_schedule_cache,
                    &cluster_info,
                    &r,
                    &sock,
                ) {
                    match e {
                        Error::RecvTimeoutError(RecvTimeoutError::Disconnected) => break,
                        Error::RecvTimeoutError(RecvTimeoutError::Timeout) => (),
                        _ => {
                            inc_new_counter_info!("streamer-retransmit-error", 1, 1);
                        }
                    }
                }
            }
            trace!("exiting retransmitter");
        })
        .unwrap()
}

pub struct RetransmitStage {
    thread_hdls: Vec<JoinHandle<()>>,
    window_service: WindowService,
}

impl RetransmitStage {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        bank_forks: Arc<RwLock<BankForks>>,
        leader_schedule_cache: &Arc<LeaderScheduleCache>,
        blocktree: Arc<Blocktree>,
        cluster_info: &Arc<RwLock<ClusterInfo>>,
        retransmit_socket: Arc<UdpSocket>,
        repair_socket: Arc<UdpSocket>,
        fetch_stage_receiver: BlobReceiver,
        exit: &Arc<AtomicBool>,
        genesis_blockhash: &Hash,
    ) -> Self {
        let (retransmit_sender, retransmit_receiver) = channel();

        let t_retransmit = retransmitter(
            retransmit_socket,
            bank_forks.clone(),
            leader_schedule_cache,
            cluster_info.clone(),
            retransmit_receiver,
        );
        let window_service = WindowService::new(
            Some(bank_forks),
            Some(leader_schedule_cache.clone()),
            blocktree,
            cluster_info.clone(),
            fetch_stage_receiver,
            retransmit_sender,
            repair_socket,
            exit,
            None,
            genesis_blockhash,
        );

        let thread_hdls = vec![t_retransmit];
        Self {
            thread_hdls,
            window_service,
        }
    }
}

impl Service for RetransmitStage {
    type JoinReturnType = ();

    fn join(self) -> thread::Result<()> {
        for thread_hdl in self.thread_hdls {
            thread_hdl.join()?;
        }
        self.window_service.join()?;
        Ok(())
    }
}
