//! The `tpu` module implements the Transaction Processing Unit, a
//! multi-stage transaction processing pipeline in software.

use crate::{
    banking_stage::BankingStage,
    broadcast_stage::{BroadcastStage, BroadcastStageType, RetransmitSlotsReceiver},
    cluster_info::ClusterInfo,
    cluster_info_vote_listener::{ClusterInfoVoteListener, VerifiedVoteSender, VoteTracker},
    fetch_stage::FetchStage,
    optimistically_confirmed_bank_tracker::BankNotificationSender,
    poh_recorder::{PohRecorder, WorkingBankEntry},
    rpc_subscriptions::RpcSubscriptions,
    sigverify::TransactionSigVerifier,
    sigverify_stage::SigVerifyStage,
};
use crossbeam_channel::unbounded;
use solana_ledger::{blockstore::Blockstore, blockstore_processor::TransactionStatusSender};
use solana_runtime::{
    bank_forks::BankForks,
    vote_sender_types::{ReplayVoteReceiver, ReplayVoteSender},
};
use std::{
    net::UdpSocket,
    sync::{
        atomic::AtomicBool,
        mpsc::{channel, Receiver},
        Arc, Mutex, RwLock,
    },
    thread,
};

pub const DEFAULT_TPU_COALESCE_MS: u64 = 5;

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
        cluster_info: &Arc<ClusterInfo>,
        poh_recorder: &Arc<Mutex<PohRecorder>>,
        entry_receiver: Receiver<WorkingBankEntry>,
        retransmit_slots_receiver: RetransmitSlotsReceiver,
        transactions_sockets: Vec<UdpSocket>,
        tpu_forwards_sockets: Vec<UdpSocket>,
        broadcast_sockets: Vec<UdpSocket>,
        subscriptions: &Arc<RpcSubscriptions>,
        transaction_status_sender: Option<TransactionStatusSender>,
        blockstore: &Arc<Blockstore>,
        broadcast_type: &BroadcastStageType,
        exit: &Arc<AtomicBool>,
        shred_version: u16,
        vote_tracker: Arc<VoteTracker>,
        bank_forks: Arc<RwLock<BankForks>>,
        verified_vote_sender: VerifiedVoteSender,
        replay_vote_receiver: ReplayVoteReceiver,
        replay_vote_sender: ReplayVoteSender,
        bank_notification_sender: Option<BankNotificationSender>,
        tpu_coalesce_ms: u64,
    ) -> Self {
        let (packet_sender, packet_receiver) = channel();
        let fetch_stage = FetchStage::new_with_sender(
            transactions_sockets,
            tpu_forwards_sockets,
            &exit,
            &packet_sender,
            &poh_recorder,
            // At 1024 packets per `Packet`, each packet about MTU size ~1k, this is roughly
            // 20GB
            Some(20_000),
            tpu_coalesce_ms,
        );
        let (verified_sender, verified_receiver) = unbounded();

        let sigverify_stage = {
            let verifier = TransactionSigVerifier::default();
            SigVerifyStage::new(packet_receiver, verified_sender, verifier)
        };

        let (verified_vote_packets_sender, verified_vote_packets_receiver) = unbounded();
        let cluster_info_vote_listener = ClusterInfoVoteListener::new(
            &exit,
            cluster_info.clone(),
            verified_vote_packets_sender,
            &poh_recorder,
            vote_tracker,
            bank_forks,
            subscriptions.clone(),
            verified_vote_sender,
            replay_vote_receiver,
            blockstore.clone(),
            bank_notification_sender,
        );

        let banking_stage = BankingStage::new(
            &cluster_info,
            poh_recorder,
            verified_receiver,
            verified_vote_packets_receiver,
            transaction_status_sender,
            replay_vote_sender,
        );

        let broadcast_stage = broadcast_type.new_broadcast_stage(
            broadcast_sockets,
            cluster_info.clone(),
            entry_receiver,
            retransmit_slots_receiver,
            &exit,
            blockstore,
            shred_version,
        );

        Self {
            fetch_stage,
            sigverify_stage,
            banking_stage,
            cluster_info_vote_listener,
            broadcast_stage,
        }
    }

    pub fn join(self) -> thread::Result<()> {
        let results = vec![
            self.fetch_stage.join(),
            self.sigverify_stage.join(),
            self.cluster_info_vote_listener.join(),
            self.banking_stage.join(),
        ];
        let broadcast_result = self.broadcast_stage.join();
        for result in results {
            result?;
        }
        let _ = broadcast_result?;
        Ok(())
    }
}
