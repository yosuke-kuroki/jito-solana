use crate::bank_forks::BankForks;
use solana_metrics::datapoint_info;
use solana_runtime::bank::Bank;
use solana_sdk::account::Account;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_vote_api::vote_state::{Lockout, Vote, VoteState, MAX_LOCKOUT_HISTORY};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

pub const VOTE_THRESHOLD_DEPTH: usize = 8;
pub const VOTE_THRESHOLD_SIZE: f64 = 2f64 / 3f64;
pub const MAX_RECENT_VOTES: usize = 16;

#[derive(Default, Debug)]
pub struct StakeLockout {
    lockout: u64,
    stake: u64,
}

impl StakeLockout {
    pub fn lockout(&self) -> u64 {
        self.lockout
    }
    pub fn stake(&self) -> u64 {
        self.stake
    }
}

#[derive(Default)]
pub struct Tower {
    node_pubkey: Pubkey,
    threshold_depth: usize,
    threshold_size: f64,
    lockouts: VoteState,
    recent_votes: VecDeque<Vote>,
}

impl Tower {
    pub fn new(node_pubkey: &Pubkey, vote_account_pubkey: &Pubkey, bank_forks: &BankForks) -> Self {
        let mut tower = Self {
            node_pubkey: *node_pubkey,
            threshold_depth: VOTE_THRESHOLD_DEPTH,
            threshold_size: VOTE_THRESHOLD_SIZE,
            lockouts: VoteState::default(),
            recent_votes: VecDeque::default(),
        };

        tower.initialize_lockouts_from_bank_forks(&bank_forks, vote_account_pubkey);

        tower
    }

    #[cfg(test)]
    pub fn new_for_tests(threshold_depth: usize, threshold_size: f64) -> Self {
        Self {
            threshold_depth,
            threshold_size,
            ..Tower::default()
        }
    }

    pub fn collect_vote_lockouts<F>(
        &self,
        bank_slot: u64,
        vote_accounts: F,
        ancestors: &HashMap<u64, HashSet<u64>>,
    ) -> (HashMap<u64, StakeLockout>, u64)
    where
        F: Iterator<Item = (Pubkey, (u64, Account))>,
    {
        let mut stake_lockouts = HashMap::new();
        let mut total_stake = 0;
        for (key, (lamports, account)) in vote_accounts {
            if lamports == 0 {
                continue;
            }
            let vote_state = VoteState::from(&account);
            if vote_state.is_none() {
                datapoint_warn!(
                    "tower_warn",
                    (
                        "warn",
                        format!("Unable to get vote_state from account {}", key),
                        String
                    ),
                );
                continue;
            }
            let mut vote_state = vote_state.unwrap();

            if key == self.node_pubkey || vote_state.node_pubkey == self.node_pubkey {
                debug!("vote state {:?}", vote_state);
                debug!(
                    "observed slot {}",
                    vote_state.nth_recent_vote(0).map(|v| v.slot).unwrap_or(0) as i64
                );
                debug!("observed root {}", vote_state.root_slot.unwrap_or(0) as i64);
                datapoint_info!(
                    "tower-observed",
                    (
                        "slot",
                        vote_state.nth_recent_vote(0).map(|v| v.slot).unwrap_or(0),
                        i64
                    ),
                    ("root", vote_state.root_slot.unwrap_or(0), i64)
                );
            }
            let start_root = vote_state.root_slot;

            vote_state.process_slot_vote_unchecked(bank_slot);

            for vote in &vote_state.votes {
                Self::update_ancestor_lockouts(&mut stake_lockouts, &vote, ancestors);
            }
            if start_root != vote_state.root_slot {
                if let Some(root) = start_root {
                    let vote = Lockout {
                        confirmation_count: MAX_LOCKOUT_HISTORY as u32,
                        slot: root,
                    };
                    trace!("ROOT: {}", vote.slot);
                    Self::update_ancestor_lockouts(&mut stake_lockouts, &vote, ancestors);
                }
            }
            if let Some(root) = vote_state.root_slot {
                let vote = Lockout {
                    confirmation_count: MAX_LOCKOUT_HISTORY as u32,
                    slot: root,
                };
                Self::update_ancestor_lockouts(&mut stake_lockouts, &vote, ancestors);
            }

            // The last vote in the vote stack is a simulated vote on bank_slot, which
            // we added to the vote stack earlier in this function by calling process_vote().
            // We don't want to update the ancestors stakes of this vote b/c it does not
            // represent an actual vote by the validator.

            // Note: It should not be possible for any vote state in this bank to have
            // a vote for a slot >= bank_slot, so we are guaranteed that the last vote in
            // this vote stack is the simulated vote, so this fetch should be sufficient
            // to find the last unsimulated vote.
            assert_eq!(
                vote_state.nth_recent_vote(0).map(|l| l.slot),
                Some(bank_slot)
            );
            if let Some(vote) = vote_state.nth_recent_vote(1) {
                // Update all the parents of this last vote with the stake of this vote account
                Self::update_ancestor_stakes(&mut stake_lockouts, vote.slot, lamports, ancestors);
            }
            total_stake += lamports;
        }
        (stake_lockouts, total_stake)
    }

    pub fn is_slot_confirmed(
        &self,
        slot: u64,
        lockouts: &HashMap<u64, StakeLockout>,
        total_staked: u64,
    ) -> bool {
        lockouts
            .get(&slot)
            .map(|lockout| (lockout.stake as f64 / total_staked as f64) > self.threshold_size)
            .unwrap_or(false)
    }

    pub fn record_vote(&mut self, slot: u64, hash: Hash) -> Option<u64> {
        let root_slot = self.lockouts.root_slot;
        let vote = Vote { slot, hash };
        self.lockouts.process_vote_unchecked(&vote);

        // vote_state doesn't keep around the hashes, so we save them in recent_votes
        self.recent_votes.push_back(vote);
        let slots = self
            .lockouts
            .votes
            .iter()
            .skip(self.lockouts.votes.len().saturating_sub(MAX_RECENT_VOTES))
            .map(|vote| vote.slot)
            .collect::<Vec<_>>();
        self.recent_votes
            .retain(|vote| slots.iter().any(|slot| vote.slot == *slot));

        datapoint_info!(
            "tower-vote",
            ("latest", slot, i64),
            ("root", self.lockouts.root_slot.unwrap_or(0), i64)
        );
        if root_slot != self.lockouts.root_slot {
            Some(self.lockouts.root_slot.unwrap())
        } else {
            None
        }
    }

    pub fn recent_votes(&self) -> Vec<Vote> {
        self.recent_votes.iter().cloned().collect::<Vec<_>>()
    }

    pub fn root(&self) -> Option<u64> {
        self.lockouts.root_slot
    }

    pub fn calculate_weight(&self, stake_lockouts: &HashMap<u64, StakeLockout>) -> u128 {
        let mut sum = 0u128;
        let root_slot = self.lockouts.root_slot.unwrap_or(0);
        for (slot, stake_lockout) in stake_lockouts {
            if self.lockouts.root_slot.is_some() && *slot <= root_slot {
                continue;
            }
            sum += u128::from(stake_lockout.lockout) * u128::from(stake_lockout.stake)
        }
        sum
    }

    pub fn has_voted(&self, slot: u64) -> bool {
        for vote in &self.lockouts.votes {
            if vote.slot == slot {
                return true;
            }
        }
        false
    }

    pub fn is_locked_out(&self, slot: u64, descendants: &HashMap<u64, HashSet<u64>>) -> bool {
        let mut lockouts = self.lockouts.clone();
        lockouts.process_slot_vote_unchecked(slot);
        for vote in &lockouts.votes {
            if vote.slot == slot {
                continue;
            }
            if !descendants[&vote.slot].contains(&slot) {
                return true;
            }
        }
        if let Some(root) = lockouts.root_slot {
            !descendants[&root].contains(&slot)
        } else {
            false
        }
    }

    pub fn check_vote_stake_threshold(
        &self,
        slot: u64,
        stake_lockouts: &HashMap<u64, StakeLockout>,
        total_staked: u64,
    ) -> bool {
        let mut lockouts = self.lockouts.clone();
        lockouts.process_slot_vote_unchecked(slot);
        let vote = lockouts.nth_recent_vote(self.threshold_depth);
        if let Some(vote) = vote {
            if let Some(fork_stake) = stake_lockouts.get(&vote.slot) {
                (fork_stake.stake as f64 / total_staked as f64) > self.threshold_size
            } else {
                false
            }
        } else {
            true
        }
    }

    pub fn aggregate_stake_lockouts(
        root: Option<u64>,
        ancestors: &HashMap<u64, HashSet<u64>>,
        stake_lockouts: HashMap<u64, StakeLockout>,
    ) -> HashMap<u64, u128> {
        let mut stake_weighted_lockouts: HashMap<u64, u128> = HashMap::new();
        for (fork, lockout) in stake_lockouts.iter() {
            if root.is_none() || *fork >= root.unwrap() {
                let mut slot_with_ancestors = vec![*fork];
                slot_with_ancestors.extend(ancestors.get(&fork).unwrap_or(&HashSet::new()));
                for slot in slot_with_ancestors {
                    if root.is_none() || slot >= root.unwrap() {
                        let entry = stake_weighted_lockouts.entry(slot).or_default();
                        *entry += u128::from(lockout.lockout) * u128::from(lockout.stake);
                    }
                }
            }
        }
        stake_weighted_lockouts
    }

    /// Update lockouts for all the ancestors
    fn update_ancestor_lockouts(
        stake_lockouts: &mut HashMap<u64, StakeLockout>,
        vote: &Lockout,
        ancestors: &HashMap<u64, HashSet<u64>>,
    ) {
        let mut slot_with_ancestors = vec![vote.slot];
        slot_with_ancestors.extend(ancestors.get(&vote.slot).unwrap_or(&HashSet::new()));
        for slot in slot_with_ancestors {
            let entry = &mut stake_lockouts.entry(slot).or_default();
            entry.lockout += vote.lockout();
        }
    }

    /// Update stake for all the ancestors.
    /// Note, stake is the same for all the ancestor.
    fn update_ancestor_stakes(
        stake_lockouts: &mut HashMap<u64, StakeLockout>,
        slot: u64,
        lamports: u64,
        ancestors: &HashMap<u64, HashSet<u64>>,
    ) {
        let mut slot_with_ancestors = vec![slot];
        slot_with_ancestors.extend(ancestors.get(&slot).unwrap_or(&HashSet::new()));
        for slot in slot_with_ancestors {
            let entry = &mut stake_lockouts.entry(slot).or_default();
            entry.stake += lamports;
        }
    }

    fn bank_weight(&self, bank: &Bank, ancestors: &HashMap<u64, HashSet<u64>>) -> u128 {
        let (stake_lockouts, _) =
            self.collect_vote_lockouts(bank.slot(), bank.vote_accounts().into_iter(), ancestors);
        self.calculate_weight(&stake_lockouts)
    }

    fn find_heaviest_bank(&self, bank_forks: &BankForks) -> Option<Arc<Bank>> {
        let ancestors = bank_forks.ancestors();
        let mut bank_weights: Vec<_> = bank_forks
            .frozen_banks()
            .values()
            .map(|b| {
                (
                    self.bank_weight(b, &ancestors),
                    b.parents().len(),
                    b.clone(),
                )
            })
            .collect();
        bank_weights.sort_by_key(|b| (b.0, b.1));
        bank_weights.pop().map(|b| b.2)
    }

    fn initialize_lockouts_from_bank_forks(
        &mut self,
        bank_forks: &BankForks,
        vote_account_pubkey: &Pubkey,
    ) {
        if let Some(bank) = self.find_heaviest_bank(bank_forks) {
            if let Some((_stake, vote_account)) = bank.vote_accounts().get(vote_account_pubkey) {
                let vote_state = VoteState::deserialize(&vote_account.data)
                    .expect("vote_account isn't a VoteState?");
                assert_eq!(
                    vote_state.node_pubkey, self.node_pubkey,
                    "vote account's node_pubkey doesn't match",
                );
                self.lockouts = vote_state;
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn gen_stakes(stake_votes: &[(u64, &[u64])]) -> Vec<(Pubkey, (u64, Account))> {
        let mut stakes = vec![];
        for (lamports, votes) in stake_votes {
            let mut account = Account::default();
            account.data = vec![0; 1024];
            account.lamports = *lamports;
            let mut vote_state = VoteState::default();
            for slot in *votes {
                vote_state.process_slot_vote_unchecked(*slot);
            }
            vote_state
                .serialize(&mut account.data)
                .expect("serialize state");
            stakes.push((Pubkey::new_rand(), (*lamports, account)));
        }
        stakes
    }

    #[test]
    fn test_collect_vote_lockouts_sums() {
        //two accounts voting for slot 0 with 1 token staked
        let accounts = gen_stakes(&[(1, &[0]), (1, &[0])]);
        let tower = Tower::new_for_tests(0, 0.67);
        let ancestors = vec![(1, vec![0].into_iter().collect()), (0, HashSet::new())]
            .into_iter()
            .collect();
        let (staked_lockouts, total_staked) =
            tower.collect_vote_lockouts(1, accounts.into_iter(), &ancestors);
        assert_eq!(staked_lockouts[&0].stake, 2);
        assert_eq!(staked_lockouts[&0].lockout, 2 + 2 + 4 + 4);
        assert_eq!(total_staked, 2);
    }

    #[test]
    fn test_collect_vote_lockouts_root() {
        let votes: Vec<u64> = (0..MAX_LOCKOUT_HISTORY as u64).into_iter().collect();
        //two accounts voting for slot 0 with 1 token staked
        let accounts = gen_stakes(&[(1, &votes), (1, &votes)]);
        let mut tower = Tower::new_for_tests(0, 0.67);
        let mut ancestors = HashMap::new();
        for i in 0..(MAX_LOCKOUT_HISTORY + 1) {
            tower.record_vote(i as u64, Hash::default());
            ancestors.insert(i as u64, (0..i as u64).into_iter().collect());
        }
        assert_eq!(tower.lockouts.root_slot, Some(0));
        let (staked_lockouts, _total_staked) = tower.collect_vote_lockouts(
            MAX_LOCKOUT_HISTORY as u64,
            accounts.into_iter(),
            &ancestors,
        );
        for i in 0..MAX_LOCKOUT_HISTORY {
            assert_eq!(staked_lockouts[&(i as u64)].stake, 2);
        }
        // should be the sum of all the weights for root
        assert!(staked_lockouts[&0].lockout > (2 * (1 << MAX_LOCKOUT_HISTORY)));
    }

    #[test]
    fn test_calculate_weight_skips_root() {
        let mut tower = Tower::new_for_tests(0, 0.67);
        tower.lockouts.root_slot = Some(1);
        let stakes = vec![
            (
                0,
                StakeLockout {
                    stake: 1,
                    lockout: 8,
                },
            ),
            (
                1,
                StakeLockout {
                    stake: 1,
                    lockout: 8,
                },
            ),
        ]
        .into_iter()
        .collect();
        assert_eq!(tower.calculate_weight(&stakes), 0u128);
    }

    #[test]
    fn test_calculate_weight() {
        let tower = Tower::new_for_tests(0, 0.67);
        let stakes = vec![(
            0,
            StakeLockout {
                stake: 1,
                lockout: 8,
            },
        )]
        .into_iter()
        .collect();
        assert_eq!(tower.calculate_weight(&stakes), 8u128);
    }

    #[test]
    fn test_check_vote_threshold_without_votes() {
        let tower = Tower::new_for_tests(1, 0.67);
        let stakes = vec![(
            0,
            StakeLockout {
                stake: 1,
                lockout: 8,
            },
        )]
        .into_iter()
        .collect();
        assert!(tower.check_vote_stake_threshold(0, &stakes, 2));
    }

    #[test]
    fn test_aggregate_stake_lockouts() {
        let mut tower = Tower::new_for_tests(0, 0.67);
        tower.lockouts.root_slot = Some(1);
        let stakes = vec![
            (
                0,
                StakeLockout {
                    stake: 1,
                    lockout: 32,
                },
            ),
            (
                1,
                StakeLockout {
                    stake: 1,
                    lockout: 24,
                },
            ),
            (
                2,
                StakeLockout {
                    stake: 1,
                    lockout: 16,
                },
            ),
            (
                3,
                StakeLockout {
                    stake: 1,
                    lockout: 8,
                },
            ),
        ]
        .into_iter()
        .collect();

        let ancestors = vec![
            (0, HashSet::new()),
            (1, vec![0].into_iter().collect()),
            (2, vec![0, 1].into_iter().collect()),
            (3, vec![0, 1, 2].into_iter().collect()),
        ]
        .into_iter()
        .collect();
        let stake_weighted_lockouts =
            Tower::aggregate_stake_lockouts(tower.root(), &ancestors, stakes);
        assert!(stake_weighted_lockouts.get(&0).is_none());
        assert_eq!(*stake_weighted_lockouts.get(&1).unwrap(), 8 + 16 + 24);
        assert_eq!(*stake_weighted_lockouts.get(&2).unwrap(), 8 + 16);
        assert_eq!(*stake_weighted_lockouts.get(&3).unwrap(), 8);
    }

    #[test]
    fn test_is_slot_confirmed_not_enough_stake_failure() {
        let tower = Tower::new_for_tests(1, 0.67);
        let stakes = vec![(
            0,
            StakeLockout {
                stake: 1,
                lockout: 8,
            },
        )]
        .into_iter()
        .collect();
        assert!(!tower.is_slot_confirmed(0, &stakes, 2));
    }

    #[test]
    fn test_is_slot_confirmed_unknown_slot() {
        let tower = Tower::new_for_tests(1, 0.67);
        let stakes = HashMap::new();
        assert!(!tower.is_slot_confirmed(0, &stakes, 2));
    }

    #[test]
    fn test_is_slot_confirmed_pass() {
        let tower = Tower::new_for_tests(1, 0.67);
        let stakes = vec![(
            0,
            StakeLockout {
                stake: 2,
                lockout: 8,
            },
        )]
        .into_iter()
        .collect();
        assert!(tower.is_slot_confirmed(0, &stakes, 2));
    }

    #[test]
    fn test_is_locked_out_empty() {
        let tower = Tower::new_for_tests(0, 0.67);
        let descendants = HashMap::new();
        assert!(!tower.is_locked_out(0, &descendants));
    }

    #[test]
    fn test_is_locked_out_root_slot_child_pass() {
        let mut tower = Tower::new_for_tests(0, 0.67);
        let descendants = vec![(0, vec![1].into_iter().collect())]
            .into_iter()
            .collect();
        tower.lockouts.root_slot = Some(0);
        assert!(!tower.is_locked_out(1, &descendants));
    }

    #[test]
    fn test_is_locked_out_root_slot_sibling_fail() {
        let mut tower = Tower::new_for_tests(0, 0.67);
        let descendants = vec![(0, vec![1].into_iter().collect())]
            .into_iter()
            .collect();
        tower.lockouts.root_slot = Some(0);
        assert!(tower.is_locked_out(2, &descendants));
    }

    #[test]
    fn test_check_already_voted() {
        let mut tower = Tower::new_for_tests(0, 0.67);
        tower.record_vote(0, Hash::default());
        assert!(tower.has_voted(0));
        assert!(!tower.has_voted(1));
    }

    #[test]
    fn test_is_locked_out_double_vote() {
        let mut tower = Tower::new_for_tests(0, 0.67);
        let descendants = vec![(0, vec![1].into_iter().collect()), (1, HashSet::new())]
            .into_iter()
            .collect();
        tower.record_vote(0, Hash::default());
        tower.record_vote(1, Hash::default());
        assert!(tower.is_locked_out(0, &descendants));
    }

    #[test]
    fn test_is_locked_out_child() {
        let mut tower = Tower::new_for_tests(0, 0.67);
        let descendants = vec![(0, vec![1].into_iter().collect())]
            .into_iter()
            .collect();
        tower.record_vote(0, Hash::default());
        assert!(!tower.is_locked_out(1, &descendants));
    }

    #[test]
    fn test_is_locked_out_sibling() {
        let mut tower = Tower::new_for_tests(0, 0.67);
        let descendants = vec![
            (0, vec![1, 2].into_iter().collect()),
            (1, HashSet::new()),
            (2, HashSet::new()),
        ]
        .into_iter()
        .collect();
        tower.record_vote(0, Hash::default());
        tower.record_vote(1, Hash::default());
        assert!(tower.is_locked_out(2, &descendants));
    }

    #[test]
    fn test_is_locked_out_last_vote_expired() {
        let mut tower = Tower::new_for_tests(0, 0.67);
        let descendants = vec![(0, vec![1, 4].into_iter().collect()), (1, HashSet::new())]
            .into_iter()
            .collect();
        tower.record_vote(0, Hash::default());
        tower.record_vote(1, Hash::default());
        assert!(!tower.is_locked_out(4, &descendants));
        tower.record_vote(4, Hash::default());
        assert_eq!(tower.lockouts.votes[0].slot, 0);
        assert_eq!(tower.lockouts.votes[0].confirmation_count, 2);
        assert_eq!(tower.lockouts.votes[1].slot, 4);
        assert_eq!(tower.lockouts.votes[1].confirmation_count, 1);
    }

    #[test]
    fn test_check_vote_threshold_below_threshold() {
        let mut tower = Tower::new_for_tests(1, 0.67);
        let stakes = vec![(
            0,
            StakeLockout {
                stake: 1,
                lockout: 8,
            },
        )]
        .into_iter()
        .collect();
        tower.record_vote(0, Hash::default());
        assert!(!tower.check_vote_stake_threshold(1, &stakes, 2));
    }
    #[test]
    fn test_check_vote_threshold_above_threshold() {
        let mut tower = Tower::new_for_tests(1, 0.67);
        let stakes = vec![(
            0,
            StakeLockout {
                stake: 2,
                lockout: 8,
            },
        )]
        .into_iter()
        .collect();
        tower.record_vote(0, Hash::default());
        assert!(tower.check_vote_stake_threshold(1, &stakes, 2));
    }

    #[test]
    fn test_check_vote_threshold_above_threshold_after_pop() {
        let mut tower = Tower::new_for_tests(1, 0.67);
        let stakes = vec![(
            0,
            StakeLockout {
                stake: 2,
                lockout: 8,
            },
        )]
        .into_iter()
        .collect();
        tower.record_vote(0, Hash::default());
        tower.record_vote(1, Hash::default());
        tower.record_vote(2, Hash::default());
        assert!(tower.check_vote_stake_threshold(6, &stakes, 2));
    }

    #[test]
    fn test_check_vote_threshold_above_threshold_no_stake() {
        let mut tower = Tower::new_for_tests(1, 0.67);
        let stakes = HashMap::new();
        tower.record_vote(0, Hash::default());
        assert!(!tower.check_vote_stake_threshold(1, &stakes, 2));
    }

    #[test]
    fn test_lockout_is_updated_for_entire_branch() {
        let mut stake_lockouts = HashMap::new();
        let vote = Lockout {
            slot: 2,
            confirmation_count: 1,
        };
        let set: HashSet<u64> = vec![0u64, 1u64].into_iter().collect();
        let mut ancestors = HashMap::new();
        ancestors.insert(2, set);
        let set: HashSet<u64> = vec![0u64].into_iter().collect();
        ancestors.insert(1, set);
        Tower::update_ancestor_lockouts(&mut stake_lockouts, &vote, &ancestors);
        assert_eq!(stake_lockouts[&0].lockout, 2);
        assert_eq!(stake_lockouts[&1].lockout, 2);
        assert_eq!(stake_lockouts[&2].lockout, 2);
    }

    #[test]
    fn test_lockout_is_updated_for_slot_or_lower() {
        let mut stake_lockouts = HashMap::new();
        let set: HashSet<u64> = vec![0u64, 1u64].into_iter().collect();
        let mut ancestors = HashMap::new();
        ancestors.insert(2, set);
        let set: HashSet<u64> = vec![0u64].into_iter().collect();
        ancestors.insert(1, set);
        let vote = Lockout {
            slot: 2,
            confirmation_count: 1,
        };
        Tower::update_ancestor_lockouts(&mut stake_lockouts, &vote, &ancestors);
        let vote = Lockout {
            slot: 1,
            confirmation_count: 2,
        };
        Tower::update_ancestor_lockouts(&mut stake_lockouts, &vote, &ancestors);
        assert_eq!(stake_lockouts[&0].lockout, 2 + 4);
        assert_eq!(stake_lockouts[&1].lockout, 2 + 4);
        assert_eq!(stake_lockouts[&2].lockout, 2);
    }

    #[test]
    fn test_stake_is_updated_for_entire_branch() {
        let mut stake_lockouts = HashMap::new();
        let mut account = Account::default();
        account.lamports = 1;
        let set: HashSet<u64> = vec![0u64, 1u64].into_iter().collect();
        let ancestors: HashMap<u64, HashSet<u64>> = [(2u64, set)].into_iter().cloned().collect();
        Tower::update_ancestor_stakes(&mut stake_lockouts, 2, account.lamports, &ancestors);
        assert_eq!(stake_lockouts[&0].stake, 1);
        assert_eq!(stake_lockouts[&1].stake, 1);
        assert_eq!(stake_lockouts[&2].stake, 1);
    }

    #[test]
    fn test_check_vote_threshold_forks() {
        // Create the ancestor relationships
        let ancestors = (0..=(VOTE_THRESHOLD_DEPTH + 1) as u64)
            .map(|slot| {
                let slot_parents: HashSet<_> = (0..slot).collect();
                (slot, slot_parents)
            })
            .collect();

        // Create votes such that
        // 1) 3/4 of the stake has voted on slot: VOTE_THRESHOLD_DEPTH - 2, lockout: 2
        // 2) 1/4 of the stake has voted on slot: VOTE_THRESHOLD_DEPTH, lockout: 2^9
        let total_stake = 4;
        let threshold_size = 0.67;
        let threshold_stake = (f64::ceil(total_stake as f64 * threshold_size)) as u64;
        let tower_votes: Vec<u64> = (0..VOTE_THRESHOLD_DEPTH as u64).collect();
        let accounts = gen_stakes(&[
            (threshold_stake, &[(VOTE_THRESHOLD_DEPTH - 2) as u64]),
            (total_stake - threshold_stake, &tower_votes[..]),
        ]);

        // Initialize tower
        let mut tower = Tower::new_for_tests(VOTE_THRESHOLD_DEPTH, threshold_size);

        // CASE 1: Record the first VOTE_THRESHOLD tower votes for fork 2. We want to
        // evaluate a vote on slot VOTE_THRESHOLD_DEPTH. The nth most recent vote should be
        // for slot 0, which is common to all account vote states, so we should pass the
        // threshold check
        let vote_to_evaluate = VOTE_THRESHOLD_DEPTH as u64;
        for vote in &tower_votes {
            tower.record_vote(*vote, Hash::default());
        }
        let (staked_lockouts, total_staked) =
            tower.collect_vote_lockouts(vote_to_evaluate, accounts.clone().into_iter(), &ancestors);
        assert!(tower.check_vote_stake_threshold(vote_to_evaluate, &staked_lockouts, total_staked));

        // CASE 2: Now we want to evaluate a vote for slot VOTE_THRESHOLD_DEPTH + 1. This slot
        // will expire the vote in one of the vote accounts, so we should have insufficient
        // stake to pass the threshold
        let vote_to_evaluate = VOTE_THRESHOLD_DEPTH as u64 + 1;
        let (staked_lockouts, total_staked) =
            tower.collect_vote_lockouts(vote_to_evaluate, accounts.into_iter(), &ancestors);
        assert!(!tower.check_vote_stake_threshold(
            vote_to_evaluate,
            &staked_lockouts,
            total_staked
        ));
    }

    fn vote_and_check_recent(num_votes: usize) {
        let mut tower = Tower::new_for_tests(1, 0.67);
        let start = num_votes.saturating_sub(MAX_RECENT_VOTES);
        let expected: Vec<_> = (start..num_votes)
            .map(|i| Vote::new(i as u64, Hash::default()))
            .collect();
        for i in 0..num_votes {
            tower.record_vote(i as u64, Hash::default());
        }
        assert_eq!(expected, tower.recent_votes())
    }

    #[test]
    fn test_recent_votes_full() {
        vote_and_check_recent(MAX_LOCKOUT_HISTORY)
    }

    #[test]
    fn test_recent_votes_empty() {
        vote_and_check_recent(0)
    }

    #[test]
    fn test_recent_votes_exact() {
        vote_and_check_recent(MAX_RECENT_VOTES)
    }
}
