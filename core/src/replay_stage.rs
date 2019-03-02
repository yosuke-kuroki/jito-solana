//! The `replay_stage` replays transactions broadcast by the leader.

use crate::bank_forks::BankForks;
use crate::blocktree::Blocktree;
use crate::blocktree_processor;
use crate::blocktree_processor::BankForksInfo;
use crate::cluster_info::ClusterInfo;
use crate::entry::{Entry, EntryReceiver, EntrySender, EntrySlice};
use crate::leader_schedule_utils;
use crate::packet::BlobError;
use crate::result;
use crate::rpc_subscriptions::RpcSubscriptions;
use crate::service::Service;
use crate::tvu::{TvuRotationInfo, TvuRotationSender};
use solana_metrics::counter::Counter;
use solana_runtime::bank::Bank;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::KeypairUtil;
use solana_sdk::timing::duration_as_ms;
use solana_sdk::vote_transaction::VoteTransaction;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, RwLock};
use std::thread::{self, Builder, JoinHandle};
use std::time::Duration;
use std::time::Instant;

pub const MAX_ENTRY_RECV_PER_ITER: usize = 512;

// Implement a destructor for the ReplayStage thread to signal it exited
// even on panics
struct Finalizer {
    exit_sender: Arc<AtomicBool>,
}

impl Finalizer {
    fn new(exit_sender: Arc<AtomicBool>) -> Self {
        Finalizer { exit_sender }
    }
}
// Implement a destructor for Finalizer.
impl Drop for Finalizer {
    fn drop(&mut self) {
        self.exit_sender.clone().store(true, Ordering::Relaxed);
    }
}

pub struct ReplayStage {
    t_replay: JoinHandle<result::Result<()>>,
    exit: Arc<AtomicBool>,
}

impl ReplayStage {
    #[allow(clippy::new_ret_no_self, clippy::too_many_arguments)]
    pub fn new<T>(
        my_id: Pubkey,
        voting_keypair: Option<Arc<T>>,
        blocktree: Arc<Blocktree>,
        bank_forks: &Arc<RwLock<BankForks>>,
        _bank_forks_info: &[BankForksInfo],
        cluster_info: Arc<RwLock<ClusterInfo>>,
        exit: Arc<AtomicBool>,
        to_leader_sender: &TvuRotationSender,
        ledger_signal_receiver: Receiver<bool>,
        subscriptions: &Arc<RpcSubscriptions>,
    ) -> (Self, Receiver<(u64, Pubkey)>, EntryReceiver)
    where
        T: 'static + KeypairUtil + Send + Sync,
    {
        let (forward_entry_sender, forward_entry_receiver) = channel();
        let (slot_full_sender, slot_full_receiver) = channel();
        trace!("replay stage");
        let exit_ = exit.clone();
        let to_leader_sender = to_leader_sender.clone();
        let subscriptions = subscriptions.clone();
        let bank_forks = bank_forks.clone();

        let mut progress = HashMap::new();

        // Start the replay stage loop
        let t_replay = Builder::new()
            .name("solana-replay-stage".to_string())
            .spawn(move || {
                let _exit = Finalizer::new(exit_.clone());
                loop {
                    let now = Instant::now();
                    // Stop getting entries if we get exit signal
                    if exit_.load(Ordering::Relaxed) {
                        break;
                    }
                    Self::generate_new_bank_forks(&blocktree, &mut bank_forks.write().unwrap());
                    let live_bank_ids = bank_forks.read().unwrap().active_banks();
                    trace!("live banks {:?}", live_bank_ids);
                    let mut votable: Vec<u64> = vec![];
                    for bank_id in live_bank_ids {
                        let bank = bank_forks.read().unwrap().get(bank_id).unwrap().clone();
                        if !Self::is_tpu(&bank, my_id) {
                            Self::replay_blocktree_into_bank(
                                &bank,
                                &blocktree,
                                &mut progress,
                                &forward_entry_sender,
                            )?;
                        }
                        let max_tick_height = (bank_id + 1) * bank.ticks_per_slot() - 1;
                        if bank.tick_height() == max_tick_height {
                            bank.freeze();
                            votable.push(bank_id);
                            progress.remove(&bank_id);
                            let id = leader_schedule_utils::slot_leader_at(bank.slot(), &bank);
                            if let Err(e) = slot_full_sender.send((bank.slot(), id)) {
                                info!("{} slot_full alert failed: {:?}", my_id, e);
                            }
                        }
                    }
                    // TODO: fork selection
                    // vote on the latest one for now
                    votable.sort();

                    if let Some(latest_slot_vote) = votable.last() {
                        let parent = bank_forks
                            .read()
                            .unwrap()
                            .get(*latest_slot_vote)
                            .unwrap()
                            .clone();
                        let next_slot = *latest_slot_vote + 1;
                        let next_leader = leader_schedule_utils::slot_leader_at(next_slot, &parent);
                        cluster_info.write().unwrap().set_leader(next_leader);

                        subscriptions.notify_subscribers(&parent);

                        if let Some(ref voting_keypair) = voting_keypair {
                            let keypair = voting_keypair.as_ref();
                            let vote = VoteTransaction::new_vote(
                                keypair,
                                *latest_slot_vote,
                                parent.last_blockhash(),
                                0,
                            );
                            cluster_info.write().unwrap().push_vote(vote);
                        }
                        if next_leader == my_id {
                            let tpu_bank = Bank::new_from_parent(&parent, my_id, next_slot);
                            bank_forks.write().unwrap().insert(next_slot, tpu_bank);
                        }
                        debug!(
                            "to_leader_sender: me: {} next_slot: {} next_leader: {}",
                            my_id, next_slot, next_leader
                        );
                        to_leader_sender.send(TvuRotationInfo {
                            tick_height: parent.tick_height(),
                            blockhash: parent.last_blockhash(),
                            slot: next_slot,
                            leader_id: next_leader,
                        })?;
                    }
                    inc_new_counter_info!(
                        "replicate_stage-duration",
                        duration_as_ms(&now.elapsed()) as usize
                    );
                    let timer = Duration::from_millis(100);
                    let result = ledger_signal_receiver.recv_timeout(timer);
                    match result {
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(_) => break,
                        Ok(_) => debug!("blocktree signal"),
                    };
                }
                Ok(())
            })
            .unwrap();
        (
            Self { t_replay, exit },
            slot_full_receiver,
            forward_entry_receiver,
        )
    }

    pub fn replay_blocktree_into_bank(
        bank: &Bank,
        blocktree: &Blocktree,
        progress: &mut HashMap<u64, (Hash, usize)>,
        forward_entry_sender: &EntrySender,
    ) -> result::Result<()> {
        let (entries, num) = Self::load_blocktree_entries(bank, blocktree, progress)?;
        let len = entries.len();
        let result =
            Self::replay_entries_into_bank(bank, entries, progress, forward_entry_sender, num);
        if result.is_ok() {
            trace!("verified entries {}", len);
            inc_new_counter_info!("replicate-stage_process_entries", len);
        } else {
            info!("debug to verify entries {}", len);
            //TODO: mark this fork as failed
            inc_new_counter_info!("replicate-stage_failed_process_entries", len);
        }
        Ok(())
    }

    pub fn load_blocktree_entries(
        bank: &Bank,
        blocktree: &Blocktree,
        progress: &mut HashMap<u64, (Hash, usize)>,
    ) -> result::Result<(Vec<Entry>, usize)> {
        let bank_id = bank.slot();
        let bank_progress = &mut progress
            .entry(bank_id)
            .or_insert((bank.last_blockhash(), 0));
        blocktree.get_slot_entries_with_blob_count(bank_id, bank_progress.1 as u64, None)
    }

    pub fn replay_entries_into_bank(
        bank: &Bank,
        entries: Vec<Entry>,
        progress: &mut HashMap<u64, (Hash, usize)>,
        forward_entry_sender: &EntrySender,
        num: usize,
    ) -> result::Result<()> {
        let bank_progress = &mut progress
            .entry(bank.slot())
            .or_insert((bank.last_blockhash(), 0));
        let result = Self::verify_and_process_entries(&bank, &entries, &bank_progress.0);
        bank_progress.1 += num;
        if let Some(last_entry) = entries.last() {
            bank_progress.0 = last_entry.hash;
        }
        if result.is_ok() {
            forward_entry_sender.send(entries)?;
        }
        result
    }

    pub fn is_tpu(bank: &Bank, my_id: Pubkey) -> bool {
        my_id == leader_schedule_utils::slot_leader(&bank)
    }

    pub fn close(self) -> thread::Result<()> {
        self.exit();
        self.join()
    }

    pub fn exit(&self) {
        self.exit.store(true, Ordering::Relaxed);
    }

    pub fn verify_and_process_entries(
        bank: &Bank,
        entries: &[Entry],
        last_entry: &Hash,
    ) -> result::Result<()> {
        if !entries.verify(last_entry) {
            trace!(
                "entry verification failed {} {} {} {}",
                entries.len(),
                bank.tick_height(),
                last_entry,
                bank.last_blockhash()
            );
            return Err(result::Error::BlobError(BlobError::VerificationFailed));
        }
        blocktree_processor::process_entries(bank, entries)?;

        Ok(())
    }

    fn generate_new_bank_forks(blocktree: &Blocktree, forks: &mut BankForks) {
        // Find the next slot that chains to the old slot
        let frozen_banks = forks.frozen_banks();
        let frozen_bank_ids: Vec<u64> = frozen_banks.keys().cloned().collect();
        trace!("generate new forks {:?}", frozen_bank_ids);
        let next_slots = blocktree
            .get_slots_since(&frozen_bank_ids)
            .expect("Db error");
        for (parent_id, children) in next_slots {
            let parent_bank = frozen_banks
                .get(&parent_id)
                .expect("missing parent in bank forks")
                .clone();
            for child_id in children {
                let new_fork = forks.get(child_id).is_none();
                if new_fork {
                    let leader = leader_schedule_utils::slot_leader_at(child_id, &parent_bank);
                    trace!("new fork:{} parent:{}", child_id, parent_id);
                    forks.insert(
                        child_id,
                        Bank::new_from_parent(&parent_bank, leader, child_id),
                    );
                }
            }
        }
    }
}

impl Service for ReplayStage {
    type JoinReturnType = ();

    fn join(self) -> thread::Result<()> {
        self.t_replay.join().map(|_| ())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::blocktree::create_new_tmp_ledger;
    use crate::cluster_info::{ClusterInfo, Node};
    use crate::entry::create_ticks;
    use crate::entry::{next_entry_mut, Entry};
    use crate::fullnode::new_banks_from_blocktree;
    use crate::replay_stage::ReplayStage;
    use crate::result::Error;
    use solana_sdk::genesis_block::GenesisBlock;
    use solana_sdk::hash::Hash;
    use solana_sdk::signature::{Keypair, KeypairUtil};
    use std::fs::remove_dir_all;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::channel;
    use std::sync::{Arc, RwLock};

    #[test]
    fn test_vote_error_replay_stage_correctness() {
        solana_logger::setup();
        // Set up dummy node to host a ReplayStage
        let my_keypair = Keypair::new();
        let my_id = my_keypair.pubkey();
        let my_node = Node::new_localhost_with_pubkey(my_id);

        // Create keypair for the leader
        let leader_id = Keypair::new().pubkey();

        let (genesis_block, _mint_keypair) = GenesisBlock::new_with_leader(10_000, leader_id, 500);

        let (my_ledger_path, _blockhash) = create_new_tmp_ledger!(&genesis_block);

        // Set up the cluster info
        let cluster_info_me = Arc::new(RwLock::new(ClusterInfo::new(my_node.info.clone())));

        // Set up the replay stage
        let exit = Arc::new(AtomicBool::new(false));
        let voting_keypair = Arc::new(Keypair::new());
        let (to_leader_sender, _to_leader_receiver) = channel();
        {
            let (bank_forks, bank_forks_info, blocktree, l_receiver) =
                new_banks_from_blocktree(&my_ledger_path, None);
            let bank = bank_forks.working_bank();

            let blocktree = Arc::new(blocktree);
            let (replay_stage, _slot_full_receiver, ledger_writer_recv) = ReplayStage::new(
                my_keypair.pubkey(),
                Some(voting_keypair.clone()),
                blocktree.clone(),
                &Arc::new(RwLock::new(bank_forks)),
                &bank_forks_info,
                cluster_info_me.clone(),
                exit.clone(),
                &to_leader_sender,
                l_receiver,
                &Arc::new(RpcSubscriptions::default()),
            );

            let keypair = voting_keypair.as_ref();
            let vote = VoteTransaction::new_vote(keypair, 0, bank.last_blockhash(), 0);
            cluster_info_me.write().unwrap().push_vote(vote);

            info!("Send ReplayStage an entry, should see it on the ledger writer receiver");
            let next_tick = create_ticks(1, bank.last_blockhash());
            blocktree.write_entries(1, 0, 0, next_tick.clone()).unwrap();

            let received_tick = ledger_writer_recv
                .recv()
                .expect("Expected to receive an entry on the ledger writer receiver");

            assert_eq!(next_tick[0], received_tick[0]);

            replay_stage
                .close()
                .expect("Expect successful ReplayStage exit");
        }
        let _ignored = remove_dir_all(&my_ledger_path);
    }

    #[test]
    fn test_replay_stage_poh_ok_entry_receiver() {
        let (forward_entry_sender, forward_entry_receiver) = channel();
        let genesis_block = GenesisBlock::new(10_000).0;
        let bank = Arc::new(Bank::new(&genesis_block));
        let mut blockhash = bank.last_blockhash();
        let mut entries = Vec::new();
        for _ in 0..5 {
            let entry = next_entry_mut(&mut blockhash, 1, vec![]); //just ticks
            entries.push(entry);
        }

        let mut progress = HashMap::new();
        let res = ReplayStage::replay_entries_into_bank(
            &bank,
            entries.clone(),
            &mut progress,
            &forward_entry_sender,
            0,
        );
        assert!(res.is_ok(), "replay failed {:?}", res);
        let res = forward_entry_receiver.try_recv();
        match res {
            Ok(_) => (),
            Err(e) => assert!(false, "Entries were not sent correctly {:?}", e),
        }
    }

    #[test]
    fn test_replay_stage_poh_error_entry_receiver() {
        let (forward_entry_sender, forward_entry_receiver) = channel();
        let mut entries = Vec::new();
        for _ in 0..5 {
            let entry = Entry::new(&mut Hash::default(), 1, vec![]); //just broken entries
            entries.push(entry);
        }

        let genesis_block = GenesisBlock::new(10_000).0;
        let bank = Arc::new(Bank::new(&genesis_block));
        let mut progress = HashMap::new();
        let res = ReplayStage::replay_entries_into_bank(
            &bank,
            entries.clone(),
            &mut progress,
            &forward_entry_sender,
            0,
        );

        match res {
            Ok(_) => assert!(false, "Should have failed because entries are broken"),
            Err(Error::BlobError(BlobError::VerificationFailed)) => (),
            Err(e) => assert!(
                false,
                "Should have failed because with blob error, instead, got {:?}",
                e
            ),
        }
        assert!(forward_entry_receiver.try_recv().is_err());
    }
}
