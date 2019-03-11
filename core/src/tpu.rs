//! The `tpu` module implements the Transaction Processing Unit, a
//! multi-stage transaction processing pipeline in software.

use crate::banking_stage::BankingStage;
use crate::blocktree::Blocktree;
use crate::broadcast_stage::BroadcastStage;
use crate::cluster_info::ClusterInfo;
use crate::cluster_info_vote_listener::ClusterInfoVoteListener;
use crate::fetch_stage::FetchStage;
use crate::poh_recorder::{PohRecorder, WorkingBankEntries};
use crate::service::Service;
use crate::sigverify_stage::SigVerifyStage;
use solana_sdk::pubkey::Pubkey;
use std::net::UdpSocket;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

pub struct Tpu {
    fetch_stage: FetchStage,
    sigverify_stage: SigVerifyStage,
    banking_stage: BankingStage,
    cluster_info_vote_listener: ClusterInfoVoteListener,
    broadcast_stage: BroadcastStage,
}

impl Tpu {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: &Pubkey,
        cluster_info: &Arc<RwLock<ClusterInfo>>,
        poh_recorder: &Arc<Mutex<PohRecorder>>,
        entry_receiver: Receiver<WorkingBankEntries>,
        transactions_sockets: Vec<UdpSocket>,
        tpu_via_blobs_sockets: Vec<UdpSocket>,
        broadcast_socket: UdpSocket,
        sigverify_disabled: bool,
        blocktree: &Arc<Blocktree>,
        exit: &Arc<AtomicBool>,
    ) -> Self {
        cluster_info.write().unwrap().set_leader(id);

        let (packet_sender, packet_receiver) = channel();
        let fetch_stage = FetchStage::new_with_sender(
            transactions_sockets,
            tpu_via_blobs_sockets,
            &exit,
            &packet_sender.clone(),
        );
        let cluster_info_vote_listener =
            ClusterInfoVoteListener::new(&exit, cluster_info.clone(), packet_sender);

        let (sigverify_stage, verified_receiver) =
            SigVerifyStage::new(packet_receiver, sigverify_disabled);

        let banking_stage = BankingStage::new(&cluster_info, poh_recorder, verified_receiver);

        let broadcast_stage = BroadcastStage::new(
            broadcast_socket,
            cluster_info.clone(),
            entry_receiver,
            &exit,
            blocktree,
        );

        Self {
            fetch_stage,
            sigverify_stage,
            banking_stage,
            cluster_info_vote_listener,
            broadcast_stage,
        }
    }
}

impl Service for Tpu {
    type JoinReturnType = ();

    fn join(self) -> thread::Result<()> {
        let mut results = vec![];
        results.push(self.fetch_stage.join());
        results.push(self.sigverify_stage.join());
        results.push(self.cluster_info_vote_listener.join());
        results.push(self.banking_stage.join());
        let broadcast_result = self.broadcast_stage.join();
        for result in results {
            result?;
        }
        let _ = broadcast_result?;
        Ok(())
    }
}
