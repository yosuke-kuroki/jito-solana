use solana_runtime::bank::Bank;
use solana_sdk::clock::Slot;
use solana_vote_program::{vote_state::VoteState, vote_state::MAX_LOCKOUT_HISTORY};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender},
    sync::{Arc, RwLock},
    thread::{self, Builder, JoinHandle},
    time::Duration,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BlockCommitment {
    pub commitment: [u64; MAX_LOCKOUT_HISTORY],
}

impl BlockCommitment {
    pub fn increase_confirmation_stake(&mut self, confirmation_count: usize, stake: u64) {
        assert!(confirmation_count > 0 && confirmation_count <= MAX_LOCKOUT_HISTORY);
        self.commitment[confirmation_count - 1] += stake;
    }

    pub fn get_confirmation_stake(&mut self, confirmation_count: usize) -> u64 {
        assert!(confirmation_count > 0 && confirmation_count <= MAX_LOCKOUT_HISTORY);
        self.commitment[confirmation_count - 1]
    }
    #[cfg(test)]
    pub(crate) fn new(commitment: [u64; MAX_LOCKOUT_HISTORY]) -> Self {
        Self { commitment }
    }
}

#[derive(Debug, Default)]
pub struct BlockCommitmentCache {
    block_commitment: HashMap<Slot, BlockCommitment>,
    total_stake: u64,
}

impl BlockCommitmentCache {
    pub fn new(block_commitment: HashMap<Slot, BlockCommitment>, total_stake: u64) -> Self {
        Self {
            block_commitment,
            total_stake,
        }
    }

    pub fn get_block_commitment(&self, slot: Slot) -> Option<&BlockCommitment> {
        self.block_commitment.get(&slot)
    }

    pub fn total_stake(&self) -> u64 {
        self.total_stake
    }

    pub fn get_block_with_depth_commitment(
        &self,
        minimum_depth: usize,
        minimum_stake_percentage: f64,
    ) -> Option<Slot> {
        self.block_commitment
            .iter()
            .filter(|&(_, block_commitment)| {
                let fork_stake_minimum_depth: u64 = block_commitment.commitment[minimum_depth..]
                    .iter()
                    .cloned()
                    .sum();
                fork_stake_minimum_depth as f64 / self.total_stake as f64
                    >= minimum_stake_percentage
            })
            .map(|(slot, _)| *slot)
            .max()
    }

    pub fn get_rooted_block_with_commitment(&self, minimum_stake_percentage: f64) -> Option<u64> {
        self.get_block_with_depth_commitment(MAX_LOCKOUT_HISTORY - 1, minimum_stake_percentage)
    }
}

pub struct CommitmentAggregationData {
    bank: Arc<Bank>,
    total_staked: u64,
}

impl CommitmentAggregationData {
    pub fn new(bank: Arc<Bank>, total_staked: u64) -> Self {
        Self { bank, total_staked }
    }
}

pub struct AggregateCommitmentService {
    t_commitment: JoinHandle<()>,
}

impl AggregateCommitmentService {
    pub fn new(
        exit: &Arc<AtomicBool>,
        block_commitment_cache: Arc<RwLock<BlockCommitmentCache>>,
    ) -> (Sender<CommitmentAggregationData>, Self) {
        let (sender, receiver): (
            Sender<CommitmentAggregationData>,
            Receiver<CommitmentAggregationData>,
        ) = channel();
        let exit_ = exit.clone();
        (
            sender,
            Self {
                t_commitment: Builder::new()
                    .name("solana-aggregate-stake-lockouts".to_string())
                    .spawn(move || loop {
                        if exit_.load(Ordering::Relaxed) {
                            break;
                        }

                        if let Err(RecvTimeoutError::Disconnected) =
                            Self::run(&receiver, &block_commitment_cache, &exit_)
                        {
                            break;
                        }
                    })
                    .unwrap(),
            },
        )
    }

    fn run(
        receiver: &Receiver<CommitmentAggregationData>,
        block_commitment_cache: &RwLock<BlockCommitmentCache>,
        exit: &Arc<AtomicBool>,
    ) -> Result<(), RecvTimeoutError> {
        loop {
            if exit.load(Ordering::Relaxed) {
                return Ok(());
            }

            let mut aggregation_data = receiver.recv_timeout(Duration::from_secs(1))?;

            while let Ok(new_data) = receiver.try_recv() {
                aggregation_data = new_data;
            }

            let ancestors = aggregation_data.bank.status_cache_ancestors();
            if ancestors.is_empty() {
                continue;
            }

            let block_commitment = Self::aggregate_commitment(&ancestors, &aggregation_data.bank);

            let mut new_block_commitment =
                BlockCommitmentCache::new(block_commitment, aggregation_data.total_staked);

            let mut w_block_commitment_cache = block_commitment_cache.write().unwrap();

            std::mem::swap(&mut *w_block_commitment_cache, &mut new_block_commitment);
        }
    }

    pub fn aggregate_commitment(ancestors: &[Slot], bank: &Bank) -> HashMap<Slot, BlockCommitment> {
        assert!(!ancestors.is_empty());

        // Check ancestors is sorted
        for a in ancestors.windows(2) {
            assert!(a[0] < a[1]);
        }

        let mut commitment = HashMap::new();
        for (_, (lamports, account)) in bank.vote_accounts().into_iter() {
            if lamports == 0 {
                continue;
            }
            let vote_state = VoteState::from(&account);
            if vote_state.is_none() {
                continue;
            }

            let vote_state = vote_state.unwrap();
            Self::aggregate_commitment_for_vote_account(
                &mut commitment,
                &vote_state,
                ancestors,
                lamports,
            );
        }

        commitment
    }

    fn aggregate_commitment_for_vote_account(
        commitment: &mut HashMap<Slot, BlockCommitment>,
        vote_state: &VoteState,
        ancestors: &[Slot],
        lamports: u64,
    ) {
        assert!(!ancestors.is_empty());
        let mut ancestors_index = 0;
        if let Some(root) = vote_state.root_slot {
            for (i, a) in ancestors.iter().enumerate() {
                if *a <= root {
                    commitment
                        .entry(*a)
                        .or_insert_with(BlockCommitment::default)
                        .increase_confirmation_stake(MAX_LOCKOUT_HISTORY, lamports);
                } else {
                    ancestors_index = i;
                    break;
                }
            }
        }

        for vote in &vote_state.votes {
            while ancestors[ancestors_index] <= vote.slot {
                commitment
                    .entry(ancestors[ancestors_index])
                    .or_insert_with(BlockCommitment::default)
                    .increase_confirmation_stake(vote.confirmation_count as usize, lamports);
                ancestors_index += 1;

                if ancestors_index == ancestors.len() {
                    return;
                }
            }
        }
    }

    pub fn join(self) -> thread::Result<()> {
        self.t_commitment.join()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genesis_utils::{create_genesis_config, GenesisConfigInfo};
    use solana_sdk::pubkey::Pubkey;
    use solana_stake_program::stake_state;
    use solana_vote_program::vote_state::{self, VoteStateVersions};

    #[test]
    fn test_block_commitment() {
        let mut cache = BlockCommitment::default();
        assert_eq!(cache.get_confirmation_stake(1), 0);
        cache.increase_confirmation_stake(1, 10);
        assert_eq!(cache.get_confirmation_stake(1), 10);
        cache.increase_confirmation_stake(1, 20);
        assert_eq!(cache.get_confirmation_stake(1), 30);
    }

    #[test]
    fn test_get_block_with_depth_commitment() {
        // Build BlockCommitmentCache with votes at depths 0 and 1 for 2 slots
        let mut cache0 = BlockCommitment::default();
        cache0.increase_confirmation_stake(1, 15);
        cache0.increase_confirmation_stake(2, 25);

        let mut cache1 = BlockCommitment::default();
        cache1.increase_confirmation_stake(1, 10);
        cache1.increase_confirmation_stake(2, 20);

        let mut block_commitment = HashMap::new();
        block_commitment.entry(0).or_insert(cache0.clone());
        block_commitment.entry(1).or_insert(cache1.clone());
        let block_commitment_cache = BlockCommitmentCache::new(block_commitment, 50);

        // Neither slot has rooted votes
        assert_eq!(
            block_commitment_cache.get_rooted_block_with_commitment(0.1),
            None
        );
        // Neither slot meets the minimum level of commitment 0.6 at depth 1
        assert_eq!(
            block_commitment_cache.get_block_with_depth_commitment(1, 0.6),
            None
        );
        // Only slot 0 meets the minimum level of commitment 0.5 at depth 1
        assert_eq!(
            block_commitment_cache.get_block_with_depth_commitment(1, 0.5),
            Some(0)
        );
        // If multiple slots meet the minimum level of commitment, method should return the most recent
        assert_eq!(
            block_commitment_cache.get_block_with_depth_commitment(1, 0.4),
            Some(1)
        );
        // If multiple slots meet the minimum level of commitment, method should return the most recent
        assert_eq!(
            block_commitment_cache.get_block_with_depth_commitment(0, 0.6),
            Some(1)
        );
        // Neither slot meets the minimum level of commitment 0.9 at depth 0
        assert_eq!(
            block_commitment_cache.get_block_with_depth_commitment(0, 0.9),
            None
        );
    }

    #[test]
    fn test_get_rooted_block_with_commitment() {
        // Build BlockCommitmentCache with rooted votes
        let mut cache0 = BlockCommitment::new([0; MAX_LOCKOUT_HISTORY]);
        cache0.increase_confirmation_stake(MAX_LOCKOUT_HISTORY, 40);
        cache0.increase_confirmation_stake(MAX_LOCKOUT_HISTORY - 1, 10);
        let mut cache1 = BlockCommitment::new([0; MAX_LOCKOUT_HISTORY]);
        cache1.increase_confirmation_stake(MAX_LOCKOUT_HISTORY, 30);
        cache1.increase_confirmation_stake(MAX_LOCKOUT_HISTORY - 1, 10);
        cache1.increase_confirmation_stake(MAX_LOCKOUT_HISTORY - 2, 10);

        let mut block_commitment = HashMap::new();
        block_commitment.entry(0).or_insert(cache0.clone());
        block_commitment.entry(1).or_insert(cache1.clone());
        let block_commitment_cache = BlockCommitmentCache::new(block_commitment, 50);

        // Only slot 0 meets the minimum level of commitment 0.66 at root
        assert_eq!(
            block_commitment_cache.get_rooted_block_with_commitment(0.66),
            Some(0)
        );
        // If multiple slots meet the minimum level of commitment, method should return the most recent
        assert_eq!(
            block_commitment_cache.get_rooted_block_with_commitment(0.6),
            Some(1)
        );
        // Neither slot meets the minimum level of commitment 0.9 at root
        assert_eq!(
            block_commitment_cache.get_rooted_block_with_commitment(0.9),
            None
        );
    }

    #[test]
    fn test_aggregate_commitment_for_vote_account_1() {
        let ancestors = vec![3, 4, 5, 7, 9, 11];
        let mut commitment = HashMap::new();
        let lamports = 5;
        let mut vote_state = VoteState::default();

        let root = ancestors.last().unwrap();
        vote_state.root_slot = Some(*root);
        AggregateCommitmentService::aggregate_commitment_for_vote_account(
            &mut commitment,
            &vote_state,
            &ancestors,
            lamports,
        );

        for a in ancestors {
            let mut expected = BlockCommitment::default();
            expected.increase_confirmation_stake(MAX_LOCKOUT_HISTORY, lamports);
            assert_eq!(*commitment.get(&a).unwrap(), expected);
        }
    }

    #[test]
    fn test_aggregate_commitment_for_vote_account_2() {
        let ancestors = vec![3, 4, 5, 7, 9, 11];
        let mut commitment = HashMap::new();
        let lamports = 5;
        let mut vote_state = VoteState::default();

        let root = ancestors[2];
        vote_state.root_slot = Some(root);
        vote_state.process_slot_vote_unchecked(*ancestors.last().unwrap());
        AggregateCommitmentService::aggregate_commitment_for_vote_account(
            &mut commitment,
            &vote_state,
            &ancestors,
            lamports,
        );

        for a in ancestors {
            if a <= root {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(MAX_LOCKOUT_HISTORY, lamports);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(1, lamports);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            }
        }
    }

    #[test]
    fn test_aggregate_commitment_for_vote_account_3() {
        let ancestors = vec![3, 4, 5, 7, 9, 10, 11];
        let mut commitment = HashMap::new();
        let lamports = 5;
        let mut vote_state = VoteState::default();

        let root = ancestors[2];
        vote_state.root_slot = Some(root);
        assert!(ancestors[4] + 2 >= ancestors[6]);
        vote_state.process_slot_vote_unchecked(ancestors[4]);
        vote_state.process_slot_vote_unchecked(ancestors[6]);
        AggregateCommitmentService::aggregate_commitment_for_vote_account(
            &mut commitment,
            &vote_state,
            &ancestors,
            lamports,
        );

        for (i, a) in ancestors.iter().enumerate() {
            if *a <= root {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(MAX_LOCKOUT_HISTORY, lamports);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else if i <= 4 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(2, lamports);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else if i <= 6 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(1, lamports);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            }
        }
    }

    #[test]
    fn test_aggregate_commitment_validity() {
        let ancestors = vec![3, 4, 5, 7, 9, 10, 11];
        let GenesisConfigInfo {
            mut genesis_config, ..
        } = create_genesis_config(10_000);

        let sk1 = Pubkey::new_rand();
        let pk1 = Pubkey::new_rand();
        let mut vote_account1 = vote_state::create_account(&pk1, &Pubkey::new_rand(), 0, 100);
        let stake_account1 =
            stake_state::create_account(&sk1, &pk1, &vote_account1, &genesis_config.rent, 100);
        let sk2 = Pubkey::new_rand();
        let pk2 = Pubkey::new_rand();
        let mut vote_account2 = vote_state::create_account(&pk2, &Pubkey::new_rand(), 0, 50);
        let stake_account2 =
            stake_state::create_account(&sk2, &pk2, &vote_account2, &genesis_config.rent, 50);

        genesis_config.accounts.extend(vec![
            (pk1, vote_account1.clone()),
            (sk1, stake_account1),
            (pk2, vote_account2.clone()),
            (sk2, stake_account2),
        ]);

        // Create bank
        let bank = Arc::new(Bank::new(&genesis_config));

        let mut vote_state1 = VoteState::from(&vote_account1).unwrap();
        vote_state1.process_slot_vote_unchecked(3);
        vote_state1.process_slot_vote_unchecked(5);
        let versioned = VoteStateVersions::Current(Box::new(vote_state1));
        VoteState::to(&versioned, &mut vote_account1).unwrap();
        bank.store_account(&pk1, &vote_account1);

        let mut vote_state2 = VoteState::from(&vote_account2).unwrap();
        vote_state2.process_slot_vote_unchecked(9);
        vote_state2.process_slot_vote_unchecked(10);
        let versioned = VoteStateVersions::Current(Box::new(vote_state2));
        VoteState::to(&versioned, &mut vote_account2).unwrap();
        bank.store_account(&pk2, &vote_account2);

        let commitment = AggregateCommitmentService::aggregate_commitment(&ancestors, &bank);

        for a in ancestors {
            if a <= 3 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(2, 150);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else if a <= 5 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(1, 100);
                expected.increase_confirmation_stake(2, 50);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else if a <= 9 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(2, 50);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else if a <= 10 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(1, 50);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else {
                assert!(commitment.get(&a).is_none());
            }
        }
    }
}
