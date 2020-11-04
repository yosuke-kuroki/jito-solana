//! Stakes serve as a cache of stake and vote accounts to derive
//! node stakes
use solana_sdk::{
    account::Account, clock::Epoch, pubkey::Pubkey, sysvar::stake_history::StakeHistory,
};
use solana_stake_program::stake_state::{new_stake_history_entry, Delegation, StakeState};
use solana_vote_program::vote_state::VoteState;
use std::collections::HashMap;

#[derive(Default, Clone, PartialEq, Debug, Deserialize, Serialize, AbiExample)]
pub struct Stakes {
    /// vote accounts
    vote_accounts: HashMap<Pubkey, (u64, Account)>,

    /// stake_delegations
    stake_delegations: HashMap<Pubkey, Delegation>,

    /// unused
    unused: u64,

    /// current epoch, used to calculate current stake
    epoch: Epoch,

    /// history of staking levels
    stake_history: StakeHistory,
}

impl Stakes {
    pub fn history(&self) -> &StakeHistory {
        &self.stake_history
    }
    pub fn clone_with_epoch(&self, next_epoch: Epoch) -> Self {
        let prev_epoch = self.epoch;
        if prev_epoch == next_epoch {
            self.clone()
        } else {
            // wrap up the prev epoch by adding new stake history entry for the prev epoch
            let mut stake_history_upto_prev_epoch = self.stake_history.clone();
            stake_history_upto_prev_epoch.add(
                prev_epoch,
                new_stake_history_entry(
                    prev_epoch,
                    self.stake_delegations
                        .iter()
                        .map(|(_pubkey, stake_delegation)| stake_delegation),
                    Some(&self.stake_history),
                ),
            );

            // refresh the stake distribution of vote accounts for the next epoch, using new stake history
            let vote_accounts_for_next_epoch = self
                .vote_accounts
                .iter()
                .map(|(pubkey, (_stake, account))| {
                    (
                        *pubkey,
                        (
                            self.calculate_stake(
                                pubkey,
                                next_epoch,
                                Some(&stake_history_upto_prev_epoch),
                            ),
                            account.clone(),
                        ),
                    )
                })
                .collect();

            Stakes {
                stake_delegations: self.stake_delegations.clone(),
                unused: self.unused,
                epoch: next_epoch,
                stake_history: stake_history_upto_prev_epoch,
                vote_accounts: vote_accounts_for_next_epoch,
            }
        }
    }

    // sum the stakes that point to the given voter_pubkey
    fn calculate_stake(
        &self,
        voter_pubkey: &Pubkey,
        epoch: Epoch,
        stake_history: Option<&StakeHistory>,
    ) -> u64 {
        self.stake_delegations
            .iter()
            .map(|(_, stake_delegation)| {
                if &stake_delegation.voter_pubkey == voter_pubkey {
                    stake_delegation.stake(epoch, stake_history)
                } else {
                    0
                }
            })
            .sum()
    }

    pub fn vote_balance_and_staked(&self) -> u64 {
        self.stake_delegations
            .iter()
            .map(|(_, stake_delegation)| stake_delegation.stake)
            .sum::<u64>()
            + self
                .vote_accounts
                .iter()
                .map(|(_pubkey, (_staked, vote_account))| vote_account.lamports)
                .sum::<u64>()
    }

    pub fn is_stake(account: &Account) -> bool {
        solana_vote_program::check_id(&account.owner)
            || solana_stake_program::check_id(&account.owner)
                && account.data.len() >= std::mem::size_of::<StakeState>()
    }

    pub fn store(&mut self, pubkey: &Pubkey, account: &Account) -> Option<Account> {
        if solana_vote_program::check_id(&account.owner) {
            let old = self.vote_accounts.remove(pubkey);
            if account.lamports != 0 {
                let stake = old.as_ref().map_or_else(
                    || self.calculate_stake(pubkey, self.epoch, Some(&self.stake_history)),
                    |v| v.0,
                );

                self.vote_accounts.insert(*pubkey, (stake, account.clone()));
            }
            old.map(|(_, account)| account)
        } else if solana_stake_program::check_id(&account.owner) {
            //  old_stake is stake lamports and voter_pubkey from the pre-store() version
            let old_stake = self.stake_delegations.get(pubkey).map(|delegation| {
                (
                    delegation.voter_pubkey,
                    delegation.stake(self.epoch, Some(&self.stake_history)),
                )
            });

            let delegation = StakeState::delegation_from(account);

            let stake = delegation.map(|delegation| {
                (
                    delegation.voter_pubkey,
                    if account.lamports != 0 {
                        delegation.stake(self.epoch, Some(&self.stake_history))
                    } else {
                        0
                    },
                )
            });

            // if adjustments need to be made...
            if stake != old_stake {
                if let Some((voter_pubkey, stake)) = old_stake {
                    self.vote_accounts
                        .entry(voter_pubkey)
                        .and_modify(|e| e.0 -= stake);
                }
                if let Some((voter_pubkey, stake)) = stake {
                    self.vote_accounts
                        .entry(voter_pubkey)
                        .and_modify(|e| e.0 += stake);
                }
            }

            if account.lamports == 0 {
                self.stake_delegations.remove(pubkey);
            } else if let Some(delegation) = delegation {
                self.stake_delegations.insert(*pubkey, delegation);
            }
            None
        } else {
            None
        }
    }

    pub fn vote_accounts(&self) -> &HashMap<Pubkey, (u64, Account)> {
        &self.vote_accounts
    }

    pub fn stake_delegations(&self) -> &HashMap<Pubkey, Delegation> {
        &self.stake_delegations
    }

    pub fn highest_staked_node(&self) -> Option<Pubkey> {
        self.vote_accounts
            .iter()
            .max_by(|(_ak, av), (_bk, bv)| av.0.cmp(&bv.0))
            .and_then(|(_k, (_stake, account))| VoteState::from(account))
            .map(|vote_state| vote_state.node_pubkey)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use solana_sdk::{pubkey::Pubkey, rent::Rent};
    use solana_stake_program::stake_state;
    use solana_vote_program::vote_state::{self, VoteState};

    //  set up some dummies for a staked node     ((     vote      )  (     stake     ))
    pub fn create_staked_node_accounts(stake: u64) -> ((Pubkey, Account), (Pubkey, Account)) {
        let vote_pubkey = solana_sdk::pubkey::new_rand();
        let vote_account =
            vote_state::create_account(&vote_pubkey, &solana_sdk::pubkey::new_rand(), 0, 1);
        (
            (vote_pubkey, vote_account),
            create_stake_account(stake, &vote_pubkey),
        )
    }

    //   add stake to a vote_pubkey                               (   stake    )
    pub fn create_stake_account(stake: u64, vote_pubkey: &Pubkey) -> (Pubkey, Account) {
        let stake_pubkey = solana_sdk::pubkey::new_rand();
        (
            stake_pubkey,
            stake_state::create_account(
                &stake_pubkey,
                &vote_pubkey,
                &vote_state::create_account(&vote_pubkey, &solana_sdk::pubkey::new_rand(), 0, 1),
                &Rent::free(),
                stake,
            ),
        )
    }

    pub fn create_warming_staked_node_accounts(
        stake: u64,
        epoch: Epoch,
    ) -> ((Pubkey, Account), (Pubkey, Account)) {
        let vote_pubkey = solana_sdk::pubkey::new_rand();
        let vote_account =
            vote_state::create_account(&vote_pubkey, &solana_sdk::pubkey::new_rand(), 0, 1);
        (
            (vote_pubkey, vote_account),
            create_warming_stake_account(stake, epoch, &vote_pubkey),
        )
    }

    //   add stake to a vote_pubkey                               (   stake    )
    pub fn create_warming_stake_account(
        stake: u64,
        epoch: Epoch,
        vote_pubkey: &Pubkey,
    ) -> (Pubkey, Account) {
        let stake_pubkey = solana_sdk::pubkey::new_rand();
        (
            stake_pubkey,
            stake_state::create_account_with_activation_epoch(
                &stake_pubkey,
                &vote_pubkey,
                &vote_state::create_account(&vote_pubkey, &solana_sdk::pubkey::new_rand(), 0, 1),
                &Rent::free(),
                stake,
                epoch,
            ),
        )
    }

    #[test]
    fn test_stakes_basic() {
        for i in 0..4 {
            let mut stakes = Stakes::default();
            stakes.epoch = i;

            let ((vote_pubkey, vote_account), (stake_pubkey, mut stake_account)) =
                create_staked_node_accounts(10);

            stakes.store(&vote_pubkey, &vote_account);
            stakes.store(&stake_pubkey, &stake_account);
            let stake = StakeState::stake_from(&stake_account).unwrap();
            {
                let vote_accounts = stakes.vote_accounts();
                assert!(vote_accounts.get(&vote_pubkey).is_some());
                assert_eq!(
                    vote_accounts.get(&vote_pubkey).unwrap().0,
                    stake.stake(i, None)
                );
            }

            stake_account.lamports = 42;
            stakes.store(&stake_pubkey, &stake_account);
            {
                let vote_accounts = stakes.vote_accounts();
                assert!(vote_accounts.get(&vote_pubkey).is_some());
                assert_eq!(
                    vote_accounts.get(&vote_pubkey).unwrap().0,
                    stake.stake(i, None)
                ); // stays old stake, because only 10 is activated
            }

            // activate more
            let (_stake_pubkey, mut stake_account) = create_stake_account(42, &vote_pubkey);
            stakes.store(&stake_pubkey, &stake_account);
            let stake = StakeState::stake_from(&stake_account).unwrap();
            {
                let vote_accounts = stakes.vote_accounts();
                assert!(vote_accounts.get(&vote_pubkey).is_some());
                assert_eq!(
                    vote_accounts.get(&vote_pubkey).unwrap().0,
                    stake.stake(i, None)
                ); // now stake of 42 is activated
            }

            stake_account.lamports = 0;
            stakes.store(&stake_pubkey, &stake_account);
            {
                let vote_accounts = stakes.vote_accounts();
                assert!(vote_accounts.get(&vote_pubkey).is_some());
                assert_eq!(vote_accounts.get(&vote_pubkey).unwrap().0, 0);
            }
        }
    }

    #[test]
    fn test_stakes_highest() {
        let mut stakes = Stakes::default();

        assert_eq!(stakes.highest_staked_node(), None);

        let ((vote_pubkey, vote_account), (stake_pubkey, stake_account)) =
            create_staked_node_accounts(10);

        stakes.store(&vote_pubkey, &vote_account);
        stakes.store(&stake_pubkey, &stake_account);

        let ((vote11_pubkey, vote11_account), (stake11_pubkey, stake11_account)) =
            create_staked_node_accounts(20);

        stakes.store(&vote11_pubkey, &vote11_account);
        stakes.store(&stake11_pubkey, &stake11_account);

        let vote11_node_pubkey = VoteState::from(&vote11_account).unwrap().node_pubkey;

        assert_eq!(stakes.highest_staked_node(), Some(vote11_node_pubkey))
    }

    #[test]
    fn test_stakes_vote_account_disappear_reappear() {
        let mut stakes = Stakes::default();
        stakes.epoch = 4;

        let ((vote_pubkey, mut vote_account), (stake_pubkey, stake_account)) =
            create_staked_node_accounts(10);

        stakes.store(&vote_pubkey, &vote_account);
        stakes.store(&stake_pubkey, &stake_account);

        {
            let vote_accounts = stakes.vote_accounts();
            assert!(vote_accounts.get(&vote_pubkey).is_some());
            assert_eq!(vote_accounts.get(&vote_pubkey).unwrap().0, 10);
        }

        vote_account.lamports = 0;
        stakes.store(&vote_pubkey, &vote_account);

        {
            let vote_accounts = stakes.vote_accounts();
            assert!(vote_accounts.get(&vote_pubkey).is_none());
        }
        vote_account.lamports = 1;
        stakes.store(&vote_pubkey, &vote_account);

        {
            let vote_accounts = stakes.vote_accounts();
            assert!(vote_accounts.get(&vote_pubkey).is_some());
            assert_eq!(vote_accounts.get(&vote_pubkey).unwrap().0, 10);
        }
    }

    #[test]
    fn test_stakes_change_delegate() {
        let mut stakes = Stakes::default();
        stakes.epoch = 4;

        let ((vote_pubkey, vote_account), (stake_pubkey, stake_account)) =
            create_staked_node_accounts(10);

        let ((vote_pubkey2, vote_account2), (_stake_pubkey2, stake_account2)) =
            create_staked_node_accounts(10);

        stakes.store(&vote_pubkey, &vote_account);
        stakes.store(&vote_pubkey2, &vote_account2);

        // delegates to vote_pubkey
        stakes.store(&stake_pubkey, &stake_account);

        let stake = StakeState::stake_from(&stake_account).unwrap();

        {
            let vote_accounts = stakes.vote_accounts();
            assert!(vote_accounts.get(&vote_pubkey).is_some());
            assert_eq!(
                vote_accounts.get(&vote_pubkey).unwrap().0,
                stake.stake(stakes.epoch, Some(&stakes.stake_history))
            );
            assert!(vote_accounts.get(&vote_pubkey2).is_some());
            assert_eq!(vote_accounts.get(&vote_pubkey2).unwrap().0, 0);
        }

        // delegates to vote_pubkey2
        stakes.store(&stake_pubkey, &stake_account2);

        {
            let vote_accounts = stakes.vote_accounts();
            assert!(vote_accounts.get(&vote_pubkey).is_some());
            assert_eq!(vote_accounts.get(&vote_pubkey).unwrap().0, 0);
            assert!(vote_accounts.get(&vote_pubkey2).is_some());
            assert_eq!(
                vote_accounts.get(&vote_pubkey2).unwrap().0,
                stake.stake(stakes.epoch, Some(&stakes.stake_history))
            );
        }
    }
    #[test]
    fn test_stakes_multiple_stakers() {
        let mut stakes = Stakes::default();
        stakes.epoch = 4;

        let ((vote_pubkey, vote_account), (stake_pubkey, stake_account)) =
            create_staked_node_accounts(10);

        let (stake_pubkey2, stake_account2) = create_stake_account(10, &vote_pubkey);

        stakes.store(&vote_pubkey, &vote_account);

        // delegates to vote_pubkey
        stakes.store(&stake_pubkey, &stake_account);
        stakes.store(&stake_pubkey2, &stake_account2);

        {
            let vote_accounts = stakes.vote_accounts();
            assert!(vote_accounts.get(&vote_pubkey).is_some());
            assert_eq!(vote_accounts.get(&vote_pubkey).unwrap().0, 20);
        }
    }
    #[test]
    fn test_clone_with_epoch() {
        let mut stakes = Stakes::default();

        let ((vote_pubkey, vote_account), (stake_pubkey, stake_account)) =
            create_staked_node_accounts(10);

        stakes.store(&vote_pubkey, &vote_account);
        stakes.store(&stake_pubkey, &stake_account);
        let stake = StakeState::stake_from(&stake_account).unwrap();

        {
            let vote_accounts = stakes.vote_accounts();
            assert_eq!(
                vote_accounts.get(&vote_pubkey).unwrap().0,
                stake.stake(stakes.epoch, Some(&stakes.stake_history))
            );
        }
        let stakes = stakes.clone_with_epoch(3);
        {
            let vote_accounts = stakes.vote_accounts();
            assert_eq!(
                vote_accounts.get(&vote_pubkey).unwrap().0,
                stake.stake(stakes.epoch, Some(&stakes.stake_history))
            );
        }
    }

    #[test]
    fn test_stakes_not_delegate() {
        let mut stakes = Stakes::default();
        stakes.epoch = 4;

        let ((vote_pubkey, vote_account), (stake_pubkey, stake_account)) =
            create_staked_node_accounts(10);

        stakes.store(&vote_pubkey, &vote_account);
        stakes.store(&stake_pubkey, &stake_account);

        {
            let vote_accounts = stakes.vote_accounts();
            assert!(vote_accounts.get(&vote_pubkey).is_some());
            assert_eq!(vote_accounts.get(&vote_pubkey).unwrap().0, 10);
        }

        // not a stake account, and whacks above entry
        stakes.store(
            &stake_pubkey,
            &Account::new(1, 0, &solana_stake_program::id()),
        );
        {
            let vote_accounts = stakes.vote_accounts();
            assert!(vote_accounts.get(&vote_pubkey).is_some());
            assert_eq!(vote_accounts.get(&vote_pubkey).unwrap().0, 0);
        }
    }

    #[test]
    fn test_vote_balance_and_staked_empty() {
        let stakes = Stakes::default();
        assert_eq!(stakes.vote_balance_and_staked(), 0);
    }

    #[test]
    fn test_vote_balance_and_staked_normal() {
        let mut stakes = Stakes::default();
        impl Stakes {
            pub fn vote_balance_and_warmed_staked(&self) -> u64 {
                self.vote_accounts
                    .iter()
                    .map(|(_pubkey, (staked, account))| staked + account.lamports)
                    .sum()
            }
        }

        let genesis_epoch = 0;
        let ((vote_pubkey, vote_account), (stake_pubkey, stake_account)) =
            create_warming_staked_node_accounts(10, genesis_epoch);
        stakes.store(&vote_pubkey, &vote_account);
        stakes.store(&stake_pubkey, &stake_account);

        assert_eq!(stakes.vote_balance_and_staked(), 11);
        assert_eq!(stakes.vote_balance_and_warmed_staked(), 1);

        for (epoch, expected_warmed_stake) in ((genesis_epoch + 1)..=3).zip(&[2, 3, 4]) {
            stakes = stakes.clone_with_epoch(epoch);
            // vote_balance_and_staked() always remain to return same lamports
            // while vote_balance_and_warmed_staked() gradually increases
            assert_eq!(stakes.vote_balance_and_staked(), 11);
            assert_eq!(
                stakes.vote_balance_and_warmed_staked(),
                *expected_warmed_stake
            );
        }
    }
}
