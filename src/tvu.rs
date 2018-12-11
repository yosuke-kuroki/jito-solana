//! The `tvu` module implements the Transaction Validation Unit, a
//! 5-stage transaction validation pipeline in software.
//!
//! 1. BlobFetchStage
//! - Incoming blobs are picked up from the TVU sockets and repair socket.
//! 2. RetransmitStage
//! - Blobs are windowed until a contiguous chunk is available.  This stage also repairs and
//! retransmits blobs that are in the queue.
//! 3. ReplayStage
//! - Transactions in blobs are processed and applied to the bank.
//! - TODO We need to verify the signatures in the blobs.
//! 4. LedgerWriteStage
//! - Write the replayed ledger to disk.
//! 5. StorageStage
//! - Generating the keys used to encrypt the ledger and sample it for storage mining.

use crate::bank::Bank;
use crate::blob_fetch_stage::BlobFetchStage;
use crate::cluster_info::ClusterInfo;
use crate::db_ledger::DbLedger;
use crate::ledger_write_stage::LedgerWriteStage;
use crate::replay_stage::{ReplayStage, ReplayStageReturnType};
use crate::retransmit_stage::RetransmitStage;
use crate::service::Service;
use crate::storage_stage::StorageStage;
use solana_sdk::hash::Hash;
use solana_sdk::signature::Keypair;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum TvuReturnType {
    LeaderRotation(u64, u64, Hash),
}

pub struct Tvu {
    fetch_stage: BlobFetchStage,
    retransmit_stage: RetransmitStage,
    replay_stage: ReplayStage,
    ledger_write_stage: LedgerWriteStage,
    storage_stage: StorageStage,
    exit: Arc<AtomicBool>,
}

impl Tvu {
    /// This service receives messages from a leader in the network and processes the transactions
    /// on the bank state.
    /// # Arguments
    /// * `bank` - The bank state.
    /// * `keypair` - Node's key pair for signing
    /// * `vote_account_keypair` - Vote key pair
    /// * `entry_height` - Initial ledger height
    /// * `cluster_info` - The cluster_info state.
    /// * `window` - The window state.
    /// * `fetch_sockets` - my fetch sockets
    /// * `repair_socket` - my repair socket
    /// * `retransmit_socket` - my retransmit socket
    /// * `ledger_path` - path to the ledger file
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        keypair: Arc<Keypair>,
        vote_account_keypair: Arc<Keypair>,
        bank: &Arc<Bank>,
        entry_height: u64,
        last_entry_id: Hash,
        cluster_info: Arc<RwLock<ClusterInfo>>,
        fetch_sockets: Vec<UdpSocket>,
        repair_socket: UdpSocket,
        retransmit_socket: UdpSocket,
        ledger_path: Option<&str>,
        db_ledger: Arc<RwLock<DbLedger>>,
    ) -> Self {
        let exit = Arc::new(AtomicBool::new(false));

        let repair_socket = Arc::new(repair_socket);
        let mut blob_sockets: Vec<Arc<UdpSocket>> =
            fetch_sockets.into_iter().map(Arc::new).collect();
        blob_sockets.push(repair_socket.clone());
        let (fetch_stage, blob_fetch_receiver) =
            BlobFetchStage::new_multi_socket(blob_sockets, exit.clone());

        //TODO
        //the packets coming out of blob_receiver need to be sent to the GPU and verified
        //then sent to the window, which does the erasure coding reconstruction
        let (retransmit_stage, blob_window_receiver) = RetransmitStage::new(
            db_ledger,
            &cluster_info,
            bank.tick_height(),
            entry_height,
            Arc::new(retransmit_socket),
            repair_socket,
            blob_fetch_receiver,
            bank.leader_scheduler.clone(),
        );

        let (replay_stage, ledger_entry_receiver) = ReplayStage::new(
            keypair.clone(),
            vote_account_keypair,
            bank.clone(),
            cluster_info,
            blob_window_receiver,
            exit.clone(),
            entry_height,
            last_entry_id,
        );

        let (ledger_write_stage, storage_entry_receiver) =
            LedgerWriteStage::new(ledger_path, ledger_entry_receiver);

        let storage_stage = StorageStage::new(
            &bank.storage_state,
            storage_entry_receiver,
            ledger_path,
            keypair,
            exit.clone(),
            entry_height,
        );

        Tvu {
            fetch_stage,
            retransmit_stage,
            replay_stage,
            ledger_write_stage,
            storage_stage,
            exit,
        }
    }

    pub fn is_exited(&self) -> bool {
        self.exit.load(Ordering::Relaxed)
    }

    pub fn exit(&self) {
        self.exit.store(true, Ordering::Relaxed);
    }

    pub fn close(self) -> thread::Result<Option<TvuReturnType>> {
        self.fetch_stage.close();
        self.join()
    }
}

impl Service for Tvu {
    type JoinReturnType = Option<TvuReturnType>;

    fn join(self) -> thread::Result<Option<TvuReturnType>> {
        self.retransmit_stage.join()?;
        self.fetch_stage.join()?;
        self.ledger_write_stage.join()?;
        self.storage_stage.join()?;
        match self.replay_stage.join()? {
            Some(ReplayStageReturnType::LeaderRotation(
                tick_height,
                entry_height,
                last_entry_id,
            )) => Ok(Some(TvuReturnType::LeaderRotation(
                tick_height,
                entry_height,
                last_entry_id,
            ))),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::bank::Bank;
    use crate::cluster_info::{ClusterInfo, Node};
    use crate::db_ledger::DbLedger;
    use crate::entry::Entry;
    use crate::gossip_service::GossipService;
    use crate::leader_scheduler::LeaderScheduler;
    use crate::ledger::get_tmp_ledger_path;
    use crate::logger;
    use crate::mint::Mint;
    use crate::packet::SharedBlob;
    use crate::service::Service;
    use crate::streamer;
    use crate::tvu::Tvu;
    use crate::window::{self, SharedWindow};
    use bincode::serialize;
    use rocksdb::{Options, DB};
    use solana_sdk::hash::Hash;
    use solana_sdk::signature::{Keypair, KeypairUtil};
    use solana_sdk::system_transaction::SystemTransaction;
    use solana_sdk::transaction::Transaction;
    use std::fs::remove_dir_all;
    use std::net::UdpSocket;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::{Arc, RwLock};
    use std::time::Duration;

    fn new_ncp(
        cluster_info: Arc<RwLock<ClusterInfo>>,
        gossip: UdpSocket,
        exit: Arc<AtomicBool>,
    ) -> (GossipService, SharedWindow) {
        let window = Arc::new(RwLock::new(window::default_window()));
        let gossip_service = GossipService::new(&cluster_info, window.clone(), None, gossip, exit);
        (gossip_service, window)
    }

    /// Test that message sent from leader to target1 and replayed to target2
    #[test]
    #[ignore]
    fn test_replay() {
        logger::setup();
        let leader = Node::new_localhost();
        let target1_keypair = Keypair::new();
        let target1 = Node::new_localhost_with_pubkey(target1_keypair.pubkey());
        let target2 = Node::new_localhost();
        let exit = Arc::new(AtomicBool::new(false));

        //start cluster_info_l
        let mut cluster_info_l = ClusterInfo::new(leader.info.clone());
        cluster_info_l.set_leader(leader.info.id);

        let cref_l = Arc::new(RwLock::new(cluster_info_l));
        let dr_l = new_ncp(cref_l, leader.sockets.gossip, exit.clone());

        //start cluster_info2
        let mut cluster_info2 = ClusterInfo::new(target2.info.clone());
        cluster_info2.insert_info(leader.info.clone());
        cluster_info2.set_leader(leader.info.id);
        let leader_id = leader.info.id;
        let cref2 = Arc::new(RwLock::new(cluster_info2));
        let dr_2 = new_ncp(cref2, target2.sockets.gossip, exit.clone());

        // setup some blob services to send blobs into the socket
        // to simulate the source peer and get blobs out of the socket to
        // simulate target peer
        let (s_reader, r_reader) = channel();
        let blob_sockets: Vec<Arc<UdpSocket>> =
            target2.sockets.tvu.into_iter().map(Arc::new).collect();

        let t_receiver = streamer::blob_receiver(blob_sockets[0].clone(), exit.clone(), s_reader);

        // simulate leader sending messages
        let (s_responder, r_responder) = channel();
        let t_responder = streamer::responder(
            "test_replay",
            Arc::new(leader.sockets.retransmit),
            r_responder,
        );

        let starting_balance = 10_000;
        let mint = Mint::new(starting_balance);
        let tvu_addr = target1.info.tvu;
        let leader_scheduler = Arc::new(RwLock::new(LeaderScheduler::from_bootstrap_leader(
            leader_id,
        )));
        let mut bank = Bank::new(&mint);
        bank.leader_scheduler = leader_scheduler;
        let bank = Arc::new(bank);

        //start cluster_info1
        let mut cluster_info1 = ClusterInfo::new(target1.info.clone());
        cluster_info1.insert_info(leader.info.clone());
        cluster_info1.set_leader(leader.info.id);
        let cref1 = Arc::new(RwLock::new(cluster_info1));
        let dr_1 = new_ncp(cref1.clone(), target1.sockets.gossip, exit.clone());

        let vote_account_keypair = Arc::new(Keypair::new());
        let mut cur_hash = Hash::default();
        let db_ledger_path = get_tmp_ledger_path("test_replay");
        let db_ledger =
            DbLedger::open(&db_ledger_path).expect("Expected to successfully open ledger");
        let tvu = Tvu::new(
            Arc::new(target1_keypair),
            vote_account_keypair,
            &bank,
            0,
            cur_hash,
            cref1,
            target1.sockets.tvu,
            target1.sockets.repair,
            target1.sockets.retransmit,
            None,
            Arc::new(RwLock::new(db_ledger)),
        );

        let mut alice_ref_balance = starting_balance;
        let mut msgs = Vec::new();
        let mut blob_idx = 0;
        let num_transfers = 10;
        let transfer_amount = 501;
        let bob_keypair = Keypair::new();
        for i in 0..num_transfers {
            let entry0 = Entry::new(&cur_hash, 0, i, vec![]);
            cur_hash = entry0.id;
            bank.register_tick(&cur_hash);
            let entry_tick0 = Entry::new(&cur_hash, 0, i + 1, vec![]);
            cur_hash = entry_tick0.id;

            let tx0 = Transaction::system_new(
                &mint.keypair(),
                bob_keypair.pubkey(),
                transfer_amount,
                cur_hash,
            );
            bank.register_tick(&cur_hash);
            let entry_tick1 = Entry::new(&cur_hash, 0, i + 1, vec![]);
            cur_hash = entry_tick1.id;
            let entry1 = Entry::new(&cur_hash, 0, i + num_transfers, vec![tx0]);
            bank.register_tick(&entry1.id);
            let entry_tick2 = Entry::new(&entry1.id, 0, i + 1, vec![]);
            cur_hash = entry_tick2.id;

            alice_ref_balance -= transfer_amount;

            for entry in vec![entry0, entry_tick0, entry_tick1, entry1, entry_tick2] {
                let b = SharedBlob::default();
                {
                    let mut w = b.write().unwrap();
                    w.set_index(blob_idx).unwrap();
                    blob_idx += 1;
                    w.set_id(&leader_id).unwrap();

                    let serialized_entry = serialize(&entry).unwrap();

                    w.data_mut()[..serialized_entry.len()].copy_from_slice(&serialized_entry);
                    w.set_size(serialized_entry.len());
                    w.meta.set_addr(&tvu_addr);
                }
                msgs.push(b);
            }
        }

        // send the blobs into the socket
        s_responder.send(msgs).expect("send");
        drop(s_responder);

        // receive retransmitted messages
        let timer = Duration::new(1, 0);
        while let Ok(_msg) = r_reader.recv_timeout(timer) {
            trace!("got msg");
        }

        let alice_balance = bank.get_balance(&mint.keypair().pubkey());
        assert_eq!(alice_balance, alice_ref_balance);

        let bob_balance = bank.get_balance(&bob_keypair.pubkey());
        assert_eq!(bob_balance, starting_balance - alice_ref_balance);

        tvu.close().expect("close");
        exit.store(true, Ordering::Relaxed);
        dr_l.0.join().expect("join");
        dr_2.0.join().expect("join");
        dr_1.0.join().expect("join");
        t_receiver.join().expect("join");
        t_responder.join().expect("join");
        DB::destroy(&Options::default(), &db_ledger_path)
            .expect("Expected successful database destuction");
        let _ignored = remove_dir_all(&db_ledger_path);
    }
}
