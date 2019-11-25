//! Stake state
//! * delegate stakes to vote accounts
//! * keep track of rewards
//! * own mining pools

use crate::{config::Config, id, stake_instruction::StakeError};
use serde_derive::{Deserialize, Serialize};
use solana_sdk::{
    account::{Account, KeyedAccount},
    account_utils::State,
    clock::{Clock, Epoch, Slot},
    instruction::InstructionError,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::{
        self,
        stake_history::{StakeHistory, StakeHistoryEntry},
    },
};
use solana_vote_program::vote_state::VoteState;
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
#[allow(clippy::large_enum_variant)]
pub enum StakeState {
    Uninitialized,
    Initialized(Meta),
    Stake(Meta, Stake),
    RewardsPool,
}

impl Default for StakeState {
    fn default() -> Self {
        StakeState::Uninitialized
    }
}

impl StakeState {
    // utility function, used by Stakes, tests
    pub fn from(account: &Account) -> Option<StakeState> {
        account.state().ok()
    }

    pub fn stake_from(account: &Account) -> Option<Stake> {
        Self::from(account).and_then(|state: Self| state.stake())
    }
    pub fn stake(&self) -> Option<Stake> {
        match self {
            StakeState::Stake(_meta, stake) => Some(*stake),
            _ => None,
        }
    }

    pub fn delegation_from(account: &Account) -> Option<Delegation> {
        Self::from(account).and_then(|state: Self| state.delegation())
    }
    pub fn delegation(&self) -> Option<Delegation> {
        match self {
            StakeState::Stake(_meta, stake) => Some(stake.delegation),
            _ => None,
        }
    }

    pub fn authorized_from(account: &Account) -> Option<Authorized> {
        Self::from(account).and_then(|state: Self| state.authorized())
    }

    pub fn authorized(&self) -> Option<Authorized> {
        match self {
            StakeState::Stake(meta, _stake) => Some(meta.authorized),
            StakeState::Initialized(meta) => Some(meta.authorized),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub enum StakeAuthorize {
    Staker,
    Withdrawer,
}

#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub struct Lockup {
    /// slot height at which this stake will allow withdrawal, unless
    ///  to the custodian
    pub slot: Slot,
    /// custodian account, the only account to which this stake will honor a
    ///  withdrawal before lockup expires.  After lockup expires, custodian
    ///  is irrelevant
    pub custodian: Pubkey,
}

#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub struct Authorized {
    pub staker: Pubkey,
    pub withdrawer: Pubkey,
}

#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub struct Meta {
    pub rent_exempt_reserve: u64,
    pub authorized: Authorized,
    pub lockup: Lockup,
}

impl Meta {
    pub fn auto(authorized: &Pubkey) -> Self {
        Self {
            authorized: Authorized::auto(authorized),
            rent_exempt_reserve: Rent::default().minimum_balance(std::mem::size_of::<StakeState>()),
            ..Meta::default()
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub struct Delegation {
    /// to whom the stake is delegated
    pub voter_pubkey: Pubkey,
    /// activated stake amount, set at delegate_stake() time
    pub stake: u64,
    /// epoch at which this stake was activated, std::Epoch::MAX if is a bootstrap stake
    pub activation_epoch: Epoch,
    /// epoch the stake was deactivated, std::Epoch::MAX if not deactivated
    pub deactivation_epoch: Epoch,
    /// how much stake we can activate per-epoch as a fraction of currently effective stake
    pub warmup_cooldown_rate: f64,
}

impl Default for Delegation {
    fn default() -> Self {
        Self {
            voter_pubkey: Pubkey::default(),
            stake: 0,
            activation_epoch: 0,
            deactivation_epoch: std::u64::MAX,
            warmup_cooldown_rate: Config::default().warmup_cooldown_rate,
        }
    }
}

impl Delegation {
    pub fn new(
        voter_pubkey: &Pubkey,
        stake: u64,
        activation_epoch: Epoch,
        warmup_cooldown_rate: f64,
    ) -> Self {
        Self {
            voter_pubkey: *voter_pubkey,
            stake,
            activation_epoch,
            warmup_cooldown_rate,
            ..Delegation::default()
        }
    }
    pub fn is_bootstrap(&self) -> bool {
        self.activation_epoch == std::u64::MAX
    }

    pub fn stake(&self, epoch: Epoch, history: Option<&StakeHistory>) -> u64 {
        self.stake_activating_and_deactivating(epoch, history).0
    }

    fn stake_activating_and_deactivating(
        &self,
        epoch: Epoch,
        history: Option<&StakeHistory>,
    ) -> (u64, u64, u64) {
        // first, calculate an effective stake and activating number
        let (stake, activating) = self.stake_and_activating(epoch, history);

        // then de-activate some portion if necessary
        if epoch < self.deactivation_epoch {
            (stake, activating, 0) // not deactivated
        } else if epoch == self.deactivation_epoch {
            (stake, 0, stake.min(self.stake)) // can only deactivate what's activated
        } else if let Some((history, mut entry)) = history.and_then(|history| {
            history
                .get(&self.deactivation_epoch)
                .map(|entry| (history, entry))
        }) {
            // && epoch > self.deactivation_epoch
            let mut effective_stake = stake;
            let mut next_epoch = self.deactivation_epoch;

            // loop from my activation epoch until the current epoch
            //   summing up my entitlement
            loop {
                if entry.deactivating == 0 {
                    break;
                }
                // I'm trying to get to zero, how much of the deactivation in stake
                //   this account is entitled to take
                let weight = effective_stake as f64 / entry.deactivating as f64;

                // portion of activating stake in this epoch I'm entitled to
                effective_stake = effective_stake.saturating_sub(
                    ((weight * entry.effective as f64 * self.warmup_cooldown_rate) as u64).max(1),
                );

                if effective_stake == 0 {
                    break;
                }

                next_epoch += 1;
                if next_epoch >= epoch {
                    break;
                }
                if let Some(next_entry) = history.get(&next_epoch) {
                    entry = next_entry;
                } else {
                    break;
                }
            }
            (effective_stake, 0, effective_stake)
        } else {
            // no history or I've dropped out of history, so  fully deactivated
            (0, 0, 0)
        }
    }

    fn stake_and_activating(&self, epoch: Epoch, history: Option<&StakeHistory>) -> (u64, u64) {
        if self.is_bootstrap() {
            (self.stake, 0)
        } else if epoch == self.activation_epoch {
            (0, self.stake)
        } else if epoch < self.activation_epoch {
            (0, 0)
        } else if let Some((history, mut entry)) = history.and_then(|history| {
            history
                .get(&self.activation_epoch)
                .map(|entry| (history, entry))
        }) {
            // && !is_bootstrap() && epoch > self.activation_epoch
            let mut effective_stake = 0;
            let mut next_epoch = self.activation_epoch;

            // loop from my activation epoch until the current epoch
            //   summing up my entitlement
            loop {
                if entry.activating == 0 {
                    break;
                }
                // how much of the growth in stake this account is
                //  entitled to take
                let weight = (self.stake - effective_stake) as f64 / entry.activating as f64;

                // portion of activating stake in this epoch I'm entitled to
                effective_stake +=
                    ((weight * entry.effective as f64 * self.warmup_cooldown_rate) as u64).max(1);

                if effective_stake >= self.stake {
                    effective_stake = self.stake;
                    break;
                }

                next_epoch += 1;
                if next_epoch >= epoch || next_epoch >= self.deactivation_epoch {
                    break;
                }
                if let Some(next_entry) = history.get(&next_epoch) {
                    entry = next_entry;
                } else {
                    break;
                }
            }
            (effective_stake, self.stake - effective_stake)
        } else {
            // no history or I've dropped out of history, so assume fully activated
            (self.stake, 0)
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub struct Stake {
    pub delegation: Delegation,
    /// the epoch when voter_pubkey was most recently set
    pub voter_pubkey_epoch: Epoch,
    /// credits observed is credits from vote account state when delegated or redeemed
    pub credits_observed: u64,
    /// history of prior delegates and the epoch ranges for which
    ///  they were set, circular buffer
    pub prior_delegates: [(Pubkey, Epoch, Epoch, Slot); MAX_PRIOR_DELEGATES],
    /// next pointer
    pub prior_delegates_idx: usize,
}

const MAX_PRIOR_DELEGATES: usize = 32; // this is how many epochs a stake is exposed to a slashing condition

impl Default for Stake {
    fn default() -> Self {
        Self {
            delegation: Delegation::default(),
            voter_pubkey_epoch: 0,
            credits_observed: 0,
            prior_delegates: <[(Pubkey, Epoch, Epoch, Slot); MAX_PRIOR_DELEGATES]>::default(),
            prior_delegates_idx: MAX_PRIOR_DELEGATES - 1,
        }
    }
}

impl Authorized {
    pub fn auto(authorized: &Pubkey) -> Self {
        Self {
            staker: *authorized,
            withdrawer: *authorized,
        }
    }
    pub fn check(
        &self,
        signers: &HashSet<Pubkey>,
        stake_authorize: StakeAuthorize,
    ) -> Result<(), InstructionError> {
        match stake_authorize {
            StakeAuthorize::Staker if signers.contains(&self.staker) => Ok(()),
            StakeAuthorize::Withdrawer if signers.contains(&self.withdrawer) => Ok(()),
            _ => Err(InstructionError::MissingRequiredSignature),
        }
    }

    pub fn authorize(
        &mut self,
        signers: &HashSet<Pubkey>,
        new_authorized: &Pubkey,
        stake_authorize: StakeAuthorize,
    ) -> Result<(), InstructionError> {
        self.check(signers, stake_authorize)?;
        match stake_authorize {
            StakeAuthorize::Staker => self.staker = *new_authorized,
            StakeAuthorize::Withdrawer => self.withdrawer = *new_authorized,
        }
        Ok(())
    }
}

impl Stake {
    pub fn stake(&self, epoch: Epoch, history: Option<&StakeHistory>) -> u64 {
        self.delegation.stake(epoch, history)
    }
    /// for a given stake and vote_state, calculate what distributions and what updates should be made
    /// returns a tuple in the case of a payout of:
    ///   * voter_rewards to be distributed
    ///   * staker_rewards to be distributed
    ///   * new value for credits_observed in the stake
    //  returns None if there's no payout or if any deserved payout is < 1 lamport
    fn calculate_rewards(
        &self,
        point_value: f64,
        vote_state: &VoteState,
        stake_history: Option<&StakeHistory>,
    ) -> Option<(u64, u64, u64)> {
        if self.credits_observed >= vote_state.credits() {
            return None;
        }

        let mut credits_observed = self.credits_observed;
        let mut total_rewards = 0f64;
        for (epoch, credits, prev_credits) in vote_state.epoch_credits() {
            // figure out how much this stake has seen that
            //   for which the vote account has a record
            let epoch_credits = if self.credits_observed < *prev_credits {
                // the staker observed the entire epoch
                credits - prev_credits
            } else if self.credits_observed < *credits {
                // the staker registered sometime during the epoch, partial credit
                credits - credits_observed
            } else {
                // the staker has already observed/redeemed this epoch, or activated
                //  after this epoch
                0
            };

            total_rewards +=
                (self.delegation.stake(*epoch, stake_history) * epoch_credits) as f64 * point_value;

            // don't want to assume anything about order of the iterator...
            credits_observed = credits_observed.max(*credits);
        }
        // don't bother trying to collect fractional lamports
        if total_rewards < 1f64 {
            return None;
        }

        let (voter_rewards, staker_rewards, is_split) = vote_state.commission_split(total_rewards);

        if (voter_rewards < 1f64 || staker_rewards < 1f64) && is_split {
            // don't bother trying to collect fractional lamports
            return None;
        }

        Some((
            voter_rewards as u64,
            staker_rewards as u64,
            credits_observed,
        ))
    }

    fn redelegate(
        &mut self,
        voter_pubkey: &Pubkey,
        vote_state: &VoteState,
        clock: &Clock,
    ) -> Result<(), StakeError> {
        // only one re-delegation supported per epoch
        if self.voter_pubkey_epoch == clock.epoch {
            return Err(StakeError::TooSoonToRedelegate);
        }

        // remember prior delegate and when we switched, to support later slashing
        self.prior_delegates_idx += 1;
        self.prior_delegates_idx %= MAX_PRIOR_DELEGATES;

        self.prior_delegates[self.prior_delegates_idx] = (
            self.delegation.voter_pubkey,
            self.voter_pubkey_epoch,
            clock.epoch,
            clock.slot,
        );

        self.delegation.voter_pubkey = *voter_pubkey;
        self.voter_pubkey_epoch = clock.epoch;
        self.credits_observed = vote_state.credits();
        Ok(())
    }

    fn split(&mut self, lamports: u64) -> Result<Self, StakeError> {
        if lamports > self.delegation.stake {
            return Err(StakeError::InsufficientStake);
        }
        self.delegation.stake -= lamports;
        let new = Self {
            delegation: Delegation {
                stake: lamports,
                ..self.delegation
            },
            ..*self
        };
        Ok(new)
    }

    fn new(
        stake: u64,
        voter_pubkey: &Pubkey,
        vote_state: &VoteState,
        activation_epoch: Epoch,
        config: &Config,
    ) -> Self {
        Self {
            delegation: Delegation::new(
                voter_pubkey,
                stake,
                activation_epoch,
                config.warmup_cooldown_rate,
            ),
            voter_pubkey_epoch: activation_epoch,
            credits_observed: vote_state.credits(),
            ..Stake::default()
        }
    }

    fn deactivate(&mut self, epoch: Epoch) -> Result<(), StakeError> {
        if self.delegation.deactivation_epoch != std::u64::MAX {
            Err(StakeError::AlreadyDeactivated)
        } else {
            self.delegation.deactivation_epoch = epoch;
            Ok(())
        }
    }
}

pub trait StakeAccount {
    fn initialize(
        &mut self,
        authorized: &Authorized,
        lockup: &Lockup,
        rent: &Rent,
    ) -> Result<(), InstructionError>;
    fn authorize(
        &mut self,
        authority: &Pubkey,
        stake_authorize: StakeAuthorize,
        signers: &HashSet<Pubkey>,
    ) -> Result<(), InstructionError>;
    fn delegate_stake(
        &mut self,
        vote_account: &KeyedAccount,
        clock: &sysvar::clock::Clock,
        config: &Config,
        signers: &HashSet<Pubkey>,
    ) -> Result<(), InstructionError>;
    fn deactivate_stake(
        &mut self,
        clock: &sysvar::clock::Clock,
        signers: &HashSet<Pubkey>,
    ) -> Result<(), InstructionError>;
    fn redeem_vote_credits(
        &mut self,
        vote_account: &mut KeyedAccount,
        rewards_account: &mut KeyedAccount,
        rewards: &sysvar::rewards::Rewards,
        stake_history: &sysvar::stake_history::StakeHistory,
    ) -> Result<(), InstructionError>;
    fn split(
        &mut self,
        lamports: u64,
        split_stake: &mut KeyedAccount,
        signers: &HashSet<Pubkey>,
    ) -> Result<(), InstructionError>;
    fn withdraw(
        &mut self,
        lamports: u64,
        to: &mut KeyedAccount,
        clock: &sysvar::clock::Clock,
        stake_history: &sysvar::stake_history::StakeHistory,
        signers: &HashSet<Pubkey>,
    ) -> Result<(), InstructionError>;
}

impl<'a> StakeAccount for KeyedAccount<'a> {
    fn initialize(
        &mut self,
        authorized: &Authorized,
        lockup: &Lockup,
        rent: &Rent,
    ) -> Result<(), InstructionError> {
        if let StakeState::Uninitialized = self.state()? {
            let rent_exempt_reserve = rent.minimum_balance(self.account.data.len());

            if rent_exempt_reserve < self.account.lamports {
                self.set_state(&StakeState::Initialized(Meta {
                    rent_exempt_reserve,
                    authorized: *authorized,
                    lockup: *lockup,
                }))
            } else {
                Err(InstructionError::InsufficientFunds)
            }
        } else {
            Err(InstructionError::InvalidAccountData)
        }
    }

    /// Authorize the given pubkey to manage stake (deactivate, withdraw). This may be called
    /// multiple times, but will implicitly withdraw authorization from the previously authorized
    /// staker. The default staker is the owner of the stake account's pubkey.
    fn authorize(
        &mut self,
        authority: &Pubkey,
        stake_authorize: StakeAuthorize,
        signers: &HashSet<Pubkey>,
    ) -> Result<(), InstructionError> {
        match self.state()? {
            StakeState::Stake(mut meta, stake) => {
                meta.authorized
                    .authorize(signers, authority, stake_authorize)?;
                self.set_state(&StakeState::Stake(meta, stake))
            }
            StakeState::Initialized(mut meta) => {
                meta.authorized
                    .authorize(signers, authority, stake_authorize)?;
                self.set_state(&StakeState::Initialized(meta))
            }
            _ => Err(InstructionError::InvalidAccountData),
        }
    }
    fn delegate_stake(
        &mut self,
        vote_account: &KeyedAccount,
        clock: &sysvar::clock::Clock,
        config: &Config,
        signers: &HashSet<Pubkey>,
    ) -> Result<(), InstructionError> {
        match self.state()? {
            StakeState::Initialized(meta) => {
                meta.authorized.check(signers, StakeAuthorize::Staker)?;
                let stake = Stake::new(
                    self.account
                        .lamports
                        .saturating_sub(meta.rent_exempt_reserve), // can't stake the rent ;)
                    vote_account.unsigned_key(),
                    &vote_account.state()?,
                    clock.epoch,
                    config,
                );
                self.set_state(&StakeState::Stake(meta, stake))
            }
            StakeState::Stake(meta, mut stake) => {
                meta.authorized.check(signers, StakeAuthorize::Staker)?;
                stake.redelegate(vote_account.unsigned_key(), &vote_account.state()?, &clock)?;
                self.set_state(&StakeState::Stake(meta, stake))
            }
            _ => Err(InstructionError::InvalidAccountData),
        }
    }
    fn deactivate_stake(
        &mut self,
        clock: &sysvar::clock::Clock,
        signers: &HashSet<Pubkey>,
    ) -> Result<(), InstructionError> {
        if let StakeState::Stake(meta, mut stake) = self.state()? {
            meta.authorized.check(signers, StakeAuthorize::Staker)?;
            stake.deactivate(clock.epoch)?;

            self.set_state(&StakeState::Stake(meta, stake))
        } else {
            Err(InstructionError::InvalidAccountData)
        }
    }
    fn redeem_vote_credits(
        &mut self,
        vote_account: &mut KeyedAccount,
        rewards_account: &mut KeyedAccount,
        rewards: &sysvar::rewards::Rewards,
        stake_history: &sysvar::stake_history::StakeHistory,
    ) -> Result<(), InstructionError> {
        if let (StakeState::Stake(meta, mut stake), StakeState::RewardsPool) =
            (self.state()?, rewards_account.state()?)
        {
            let vote_state: VoteState = vote_account.state()?;

            // the only valid use of current voter_pubkey, redelegation breaks
            //  rewards redemption for previous voter_pubkey
            if stake.delegation.voter_pubkey != *vote_account.unsigned_key() {
                return Err(InstructionError::InvalidArgument);
            }

            if let Some((voters_reward, stakers_reward, credits_observed)) = stake
                .calculate_rewards(
                    rewards.validator_point_value,
                    &vote_state,
                    Some(stake_history),
                )
            {
                if rewards_account.account.lamports < (stakers_reward + voters_reward) {
                    return Err(InstructionError::UnbalancedInstruction);
                }
                rewards_account.account.lamports -= stakers_reward + voters_reward;

                self.account.lamports += stakers_reward;
                vote_account.account.lamports += voters_reward;

                stake.credits_observed = credits_observed;
                stake.delegation.stake += stakers_reward;

                self.set_state(&StakeState::Stake(meta, stake))
            } else {
                // not worth collecting
                Err(StakeError::NoCreditsToRedeem.into())
            }
        } else {
            Err(InstructionError::InvalidAccountData)
        }
    }

    fn split(
        &mut self,
        lamports: u64,
        split: &mut KeyedAccount,
        signers: &HashSet<Pubkey>,
    ) -> Result<(), InstructionError> {
        if let StakeState::Uninitialized = split.state()? {
            // verify enough account lamports
            if lamports > self.account.lamports {
                return Err(InstructionError::InsufficientFunds);
            }

            match self.state()? {
                StakeState::Stake(meta, mut stake) => {
                    meta.authorized.check(signers, StakeAuthorize::Staker)?;

                    // verify enough lamports for rent in new stake with the split
                    if split.account.lamports + lamports < meta.rent_exempt_reserve
                        // verify enough lamports left in previous stake
                        || lamports + meta.rent_exempt_reserve > self.account.lamports
                    {
                        return Err(InstructionError::InsufficientFunds);
                    }

                    // split the stake, subtract rent_exempt_balance unless
                    //  the destination account already has those lamports
                    //  in place.
                    // this could represent a small loss of staked lamports
                    //  if the split account starts out with a zero balance
                    let split_stake = stake.split(
                        lamports
                            - meta
                                .rent_exempt_reserve
                                .saturating_sub(split.account.lamports),
                    )?;

                    self.set_state(&StakeState::Stake(meta, stake))?;
                    split.set_state(&StakeState::Stake(meta, split_stake))?;
                }
                StakeState::Initialized(meta) => {
                    meta.authorized.check(signers, StakeAuthorize::Staker)?;

                    // enough lamports for rent in new stake
                    if lamports < meta.rent_exempt_reserve
                    // verify enough lamports left in previous stake
                        || lamports + meta.rent_exempt_reserve > self.account.lamports
                    {
                        return Err(InstructionError::InsufficientFunds);
                    }

                    split.set_state(&StakeState::Initialized(meta))?;
                }
                StakeState::Uninitialized => {
                    if !signers.contains(&self.unsigned_key()) {
                        return Err(InstructionError::MissingRequiredSignature);
                    }
                }
                _ => return Err(InstructionError::InvalidAccountData),
            }

            split.account.lamports += lamports;
            self.account.lamports -= lamports;
            Ok(())
        } else {
            Err(InstructionError::InvalidAccountData)
        }
    }

    fn withdraw(
        &mut self,
        lamports: u64,
        to: &mut KeyedAccount,
        clock: &sysvar::clock::Clock,
        stake_history: &sysvar::stake_history::StakeHistory,
        signers: &HashSet<Pubkey>,
    ) -> Result<(), InstructionError> {
        let (lockup, reserve, is_staked) = match self.state()? {
            StakeState::Stake(meta, stake) => {
                meta.authorized.check(signers, StakeAuthorize::Withdrawer)?;
                // if we have a deactivation epoch and we're in cooldown
                let staked = if clock.epoch >= stake.delegation.deactivation_epoch {
                    stake.delegation.stake(clock.epoch, Some(stake_history))
                } else {
                    // Assume full stake if the stake account hasn't been
                    //  de-activated, because in the future the exposed stake
                    //  might be higher than stake.stake() due to warmup
                    stake.delegation.stake
                };

                (meta.lockup, staked + meta.rent_exempt_reserve, staked != 0)
            }
            StakeState::Initialized(meta) => {
                meta.authorized.check(signers, StakeAuthorize::Withdrawer)?;

                (meta.lockup, meta.rent_exempt_reserve, false)
            }
            StakeState::Uninitialized => {
                if !signers.contains(&self.unsigned_key()) {
                    return Err(InstructionError::MissingRequiredSignature);
                }
                (Lockup::default(), 0, false) // no lockup, no restrictions
            }
            _ => return Err(InstructionError::InvalidAccountData),
        };

        // verify that lockup has expired or that the withdrawal is going back
        //   to the custodian
        if lockup.slot > clock.slot && lockup.custodian != *to.unsigned_key() {
            return Err(StakeError::LockupInForce.into());
        }

        // if the stake is active, we mustn't allow the account to go away
        if is_staked // line coverage for branch coverage
            && lamports + reserve > self.account.lamports
        {
            return Err(InstructionError::InsufficientFunds);
        }

        if lamports != self.account.lamports // not a full withdrawal
            && lamports + reserve > self.account.lamports
        {
            assert!(!is_staked);
            return Err(InstructionError::InsufficientFunds);
        }

        self.account.lamports -= lamports;
        to.account.lamports += lamports;
        Ok(())
    }
}

// utility function, used by runtime::Stakes, tests
pub fn new_stake_history_entry<'a, I>(
    epoch: Epoch,
    stakes: I,
    history: Option<&StakeHistory>,
) -> StakeHistoryEntry
where
    I: Iterator<Item = &'a Delegation>,
{
    // whatever the stake says they  had for the epoch
    //  and whatever the were still waiting for
    fn add(a: (u64, u64, u64), b: (u64, u64, u64)) -> (u64, u64, u64) {
        (a.0 + b.0, a.1 + b.1, a.2 + b.2)
    }
    let (effective, activating, deactivating) = stakes.fold((0, 0, 0), |sum, stake| {
        add(sum, stake.stake_activating_and_deactivating(epoch, history))
    });

    StakeHistoryEntry {
        effective,
        activating,
        deactivating,
    }
}

// utility function, used by Bank, tests, genesis
pub fn create_account(
    authorized: &Pubkey,
    voter_pubkey: &Pubkey,
    vote_account: &Account,
    rent: &Rent,
    lamports: u64,
) -> Account {
    let mut stake_account = Account::new(lamports, std::mem::size_of::<StakeState>(), &id());

    let vote_state = VoteState::from(vote_account).expect("vote_state");
    let rent_exempt_reserve = rent.minimum_balance(std::mem::size_of::<StakeState>());
    stake_account
        .set_state(&StakeState::Stake(
            Meta {
                rent_exempt_reserve,
                authorized: Authorized {
                    staker: *authorized,
                    withdrawer: *authorized,
                },
                lockup: Lockup::default(),
            },
            Stake::new(
                lamports - rent_exempt_reserve, // underflow is an error, assert!(lamports> rent_exempt_reserve);
                voter_pubkey,
                &vote_state,
                std::u64::MAX,
                &Config::default(),
            ),
        ))
        .expect("set_state");

    stake_account
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id;
    use solana_sdk::{account::Account, pubkey::Pubkey, system_program};
    use solana_vote_program::vote_state;

    #[test]
    fn test_stake_state_stake_from_fail() {
        let mut stake_account = Account::new(0, std::mem::size_of::<StakeState>(), &id());

        stake_account
            .set_state(&StakeState::default())
            .expect("set_state");

        assert_eq!(StakeState::stake_from(&stake_account), None);
    }

    #[test]
    fn test_stake_is_bootstrap() {
        assert_eq!(
            Delegation {
                activation_epoch: std::u64::MAX,
                ..Delegation::default()
            }
            .is_bootstrap(),
            true
        );
        assert_eq!(
            Delegation {
                activation_epoch: 0,
                ..Delegation::default()
            }
            .is_bootstrap(),
            false
        );
    }

    #[test]
    fn test_stake_delegate_stake() {
        let mut clock = sysvar::clock::Clock {
            epoch: 1,
            ..sysvar::clock::Clock::default()
        };

        let vote_pubkey = Pubkey::new_rand();
        let mut vote_state = VoteState::default();
        for i in 0..1000 {
            vote_state.process_slot_vote_unchecked(i);
        }

        let mut vote_account =
            vote_state::create_account(&vote_pubkey, &Pubkey::new_rand(), 0, 100);
        let mut vote_keyed_account = KeyedAccount::new(&vote_pubkey, false, &mut vote_account);
        vote_keyed_account.set_state(&vote_state).unwrap();

        let stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;
        let mut stake_account = Account::new_data_with_space(
            stake_lamports,
            &StakeState::Initialized(Meta {
                authorized: Authorized {
                    staker: stake_pubkey,
                    withdrawer: stake_pubkey,
                },
                ..Meta::default()
            }),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        // unsigned keyed account
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, false, &mut stake_account);

        {
            let stake_state: StakeState = stake_keyed_account.state().unwrap();
            assert_eq!(
                stake_state,
                StakeState::Initialized(Meta {
                    authorized: Authorized {
                        staker: stake_pubkey,
                        withdrawer: stake_pubkey,
                    },
                    ..Meta::default()
                })
            );
        }

        let mut signers = HashSet::default();
        assert_eq!(
            stake_keyed_account.delegate_stake(
                &vote_keyed_account,
                &clock,
                &Config::default(),
                &signers,
            ),
            Err(InstructionError::MissingRequiredSignature)
        );

        // signed keyed account
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        signers.insert(stake_pubkey);
        assert!(stake_keyed_account
            .delegate_stake(&vote_keyed_account, &clock, &Config::default(), &signers,)
            .is_ok());

        // verify that delegate_stake() looks right, compare against hand-rolled
        let stake = StakeState::stake_from(&stake_keyed_account.account).unwrap();
        assert_eq!(
            stake,
            Stake {
                delegation: Delegation {
                    voter_pubkey: vote_pubkey,
                    stake: stake_lamports,
                    activation_epoch: clock.epoch,
                    deactivation_epoch: std::u64::MAX,
                    ..Delegation::default()
                },
                voter_pubkey_epoch: clock.epoch,
                credits_observed: vote_state.credits(),
                ..Stake::default()
            }
        );
        // verify that delegate_stake can be called twice, 2nd is redelegate
        assert_eq!(
            stake_keyed_account.delegate_stake(
                &vote_keyed_account,
                &clock,
                &Config::default(),
                &signers
            ),
            Err(StakeError::TooSoonToRedelegate.into())
        );

        clock.epoch += 1;
        // verify that delegate_stake can be called twice, 2nd is redelegate
        assert!(stake_keyed_account
            .delegate_stake(&vote_keyed_account, &clock, &Config::default(), &signers)
            .is_ok());

        // verify that non-stakes fail delegate_stake()
        let stake_state = StakeState::RewardsPool;

        stake_keyed_account.set_state(&stake_state).unwrap();
        assert!(stake_keyed_account
            .delegate_stake(&vote_keyed_account, &clock, &Config::default(), &signers)
            .is_err());
    }

    #[test]
    fn test_stake_redelegate() {
        // what a freshly delegated stake looks like
        let mut stake = Stake {
            delegation: Delegation {
                voter_pubkey: Pubkey::new_rand(),
                ..Delegation::default()
            },
            voter_pubkey_epoch: 0,
            ..Stake::default()
        };
        // verify that redelegation works when epoch is changing, that
        //  wraparound works, and that the stake is delegated
        //  to the most recent vote account
        for epoch in 1..=MAX_PRIOR_DELEGATES + 2 {
            let voter_pubkey = Pubkey::new_rand();
            assert_eq!(
                stake.redelegate(
                    &voter_pubkey,
                    &VoteState::default(),
                    &sysvar::clock::Clock {
                        epoch: epoch as u64,
                        ..sysvar::clock::Clock::default()
                    },
                ),
                Ok(())
            );
            assert_eq!(
                stake.redelegate(
                    &voter_pubkey,
                    &VoteState::default(),
                    &sysvar::clock::Clock {
                        epoch: epoch as u64,
                        ..sysvar::clock::Clock::default()
                    },
                ),
                Err(StakeError::TooSoonToRedelegate)
            );
            assert_eq!(stake.delegation.voter_pubkey, voter_pubkey);
        }
    }

    fn create_stake_history_from_delegations(
        bootstrap: Option<u64>,
        epochs: std::ops::Range<Epoch>,
        delegations: &[Delegation],
    ) -> StakeHistory {
        let mut stake_history = StakeHistory::default();

        let bootstrap_delegation = if let Some(bootstrap) = bootstrap {
            vec![Delegation {
                activation_epoch: std::u64::MAX,
                stake: bootstrap,
                ..Delegation::default()
            }]
        } else {
            vec![]
        };

        for epoch in epochs {
            let entry = new_stake_history_entry(
                epoch,
                delegations.iter().chain(bootstrap_delegation.iter()),
                Some(&stake_history),
            );
            stake_history.add(epoch, entry);
        }

        stake_history
    }

    #[test]
    fn test_stake_activating_and_deactivating() {
        let stake = Delegation {
            stake: 1_000,
            activation_epoch: 0, // activating at zero
            deactivation_epoch: 5,
            ..Delegation::default()
        };

        // save this off so stake.config.warmup_rate changes don't break this test
        let increment = (1_000 as f64 * stake.warmup_cooldown_rate) as u64;

        let mut stake_history = StakeHistory::default();
        // assert that this stake follows step function if there's no history
        assert_eq!(
            stake.stake_activating_and_deactivating(stake.activation_epoch, Some(&stake_history)),
            (0, stake.stake, 0)
        );
        for epoch in stake.activation_epoch + 1..stake.deactivation_epoch {
            assert_eq!(
                stake.stake_activating_and_deactivating(epoch, Some(&stake_history)),
                (stake.stake, 0, 0)
            );
        }
        // assert that this stake is full deactivating
        assert_eq!(
            stake.stake_activating_and_deactivating(stake.deactivation_epoch, Some(&stake_history)),
            (stake.stake, 0, stake.stake)
        );
        // assert that this stake is fully deactivated if there's no history
        assert_eq!(
            stake.stake_activating_and_deactivating(
                stake.deactivation_epoch + 1,
                Some(&stake_history)
            ),
            (0, 0, 0)
        );

        stake_history.add(
            0u64, // entry for zero doesn't have my activating amount
            StakeHistoryEntry {
                effective: 1_000,
                activating: 0,
                ..StakeHistoryEntry::default()
            },
        );
        // assert that this stake is broken, because above setup is broken
        assert_eq!(
            stake.stake_activating_and_deactivating(1, Some(&stake_history)),
            (0, stake.stake, 0)
        );

        stake_history.add(
            0u64, // entry for zero has my activating amount
            StakeHistoryEntry {
                effective: 1_000,
                activating: 1_000,
                ..StakeHistoryEntry::default()
            },
            // no entry for 1, so this stake gets shorted
        );
        // assert that this stake is broken, because above setup is broken
        assert_eq!(
            stake.stake_activating_and_deactivating(2, Some(&stake_history)),
            (increment, stake.stake - increment, 0)
        );

        // start over, test deactivation edge cases
        let mut stake_history = StakeHistory::default();

        stake_history.add(
            stake.deactivation_epoch, // entry for zero doesn't have my de-activating amount
            StakeHistoryEntry {
                effective: 1_000,
                activating: 0,
                ..StakeHistoryEntry::default()
            },
        );
        // assert that this stake is broken, because above setup is broken
        assert_eq!(
            stake.stake_activating_and_deactivating(
                stake.deactivation_epoch + 1,
                Some(&stake_history)
            ),
            (stake.stake, 0, stake.stake) // says "I'm still waiting for deactivation"
        );

        // put in my initial deactivating amount, but don't put in an entry for next
        stake_history.add(
            stake.deactivation_epoch, // entry for zero has my de-activating amount
            StakeHistoryEntry {
                effective: 1_000,
                deactivating: 1_000,
                ..StakeHistoryEntry::default()
            },
        );
        // assert that this stake is broken, because above setup is broken
        assert_eq!(
            stake.stake_activating_and_deactivating(
                stake.deactivation_epoch + 2,
                Some(&stake_history)
            ),
            (stake.stake - increment, 0, stake.stake - increment) // hung, should be lower
        );
    }

    #[test]
    fn test_stop_activating_after_deactivation() {
        solana_logger::setup();
        let stake = Delegation {
            stake: 1_000,
            activation_epoch: 0,
            deactivation_epoch: 3,
            ..Delegation::default()
        };

        let base_stake = 1_000;
        let mut stake_history = StakeHistory::default();
        let mut effective = base_stake;
        let other_activation = 100;
        let mut other_activations = vec![0];

        // Build a stake history where the test staker always consumes all of the available warm
        // up and cool down stake. However, simulate other stakers beginning to activate during
        // the test staker's deactivation.
        for epoch in 0..=stake.deactivation_epoch + 1 {
            let (activating, deactivating) = if epoch < stake.deactivation_epoch {
                (stake.stake + base_stake - effective, 0)
            } else {
                let other_activation_sum: u64 = other_activations.iter().sum();
                let deactivating = effective - base_stake - other_activation_sum;
                (other_activation, deactivating)
            };

            stake_history.add(
                epoch,
                StakeHistoryEntry {
                    effective,
                    activating,
                    deactivating,
                },
            );

            if epoch < stake.deactivation_epoch {
                let increase = (effective as f64 * stake.warmup_cooldown_rate) as u64;
                effective += increase.min(activating);
                other_activations.push(0);
            } else {
                let decrease = (effective as f64 * stake.warmup_cooldown_rate) as u64;
                effective -= decrease.min(deactivating);
                effective += other_activation;
                other_activations.push(other_activation);
            }
        }

        for epoch in 0..=stake.deactivation_epoch + 1 {
            let history = stake_history.get(&epoch).unwrap();
            let other_activations: u64 = other_activations[..=epoch as usize].iter().sum();
            let expected_stake = history.effective - base_stake - other_activations;
            let (expected_activating, expected_deactivating) = if epoch < stake.deactivation_epoch {
                (history.activating, 0)
            } else {
                (0, history.deactivating)
            };
            assert_eq!(
                stake.stake_activating_and_deactivating(epoch, Some(&stake_history)),
                (expected_stake, expected_activating, expected_deactivating)
            );
        }
    }

    #[test]
    fn test_stake_warmup_cooldown_sub_integer_moves() {
        let delegations = [Delegation {
            stake: 2,
            activation_epoch: 0, // activating at zero
            deactivation_epoch: 5,
            ..Delegation::default()
        }];
        // give 2 epochs of cooldown
        let epochs = 7;
        // make boostrap stake smaller than warmup so warmup/cooldownn
        //  increment is always smaller than 1
        let bootstrap = (delegations[0].warmup_cooldown_rate * 100.0 / 2.0) as u64;
        let stake_history =
            create_stake_history_from_delegations(Some(bootstrap), 0..epochs, &delegations);
        let mut max_stake = 0;
        let mut min_stake = 2;

        for epoch in 0..epochs {
            let stake = delegations
                .iter()
                .map(|delegation| delegation.stake(epoch, Some(&stake_history)))
                .sum::<u64>();
            max_stake = max_stake.max(stake);
            min_stake = min_stake.min(stake);
        }
        assert_eq!(max_stake, 2);
        assert_eq!(min_stake, 0);
    }

    #[test]
    fn test_stake_warmup_cooldown() {
        let delegations = [
            Delegation {
                // never deactivates
                stake: 1_000,
                activation_epoch: std::u64::MAX,
                ..Delegation::default()
            },
            Delegation {
                stake: 1_000,
                activation_epoch: 0,
                deactivation_epoch: 9,
                ..Delegation::default()
            },
            Delegation {
                stake: 1_000,
                activation_epoch: 1,
                deactivation_epoch: 6,
                ..Delegation::default()
            },
            Delegation {
                stake: 1_000,
                activation_epoch: 2,
                deactivation_epoch: 5,
                ..Delegation::default()
            },
            Delegation {
                stake: 1_000,
                activation_epoch: 2,
                deactivation_epoch: 4,
                ..Delegation::default()
            },
            Delegation {
                stake: 1_000,
                activation_epoch: 4,
                deactivation_epoch: 4,
                ..Delegation::default()
            },
        ];
        // chosen to ensure that the last activated stake (at 4) finishes
        //  warming up and cooling down
        //  a stake takes 2.0f64.log(1.0 + STAKE_WARMUP_RATE) epochs to warm up or cool down
        //  when all alone, but the above overlap a lot
        let epochs = 20;

        let stake_history = create_stake_history_from_delegations(None, 0..epochs, &delegations);

        let mut prev_total_effective_stake = delegations
            .iter()
            .map(|delegation| delegation.stake(0, Some(&stake_history)))
            .sum::<u64>();

        // uncomment and add ! for fun with graphing
        // eprintln("\n{:8} {:8} {:8}", "   epoch", "   total", "   delta");
        for epoch in 1..epochs {
            let total_effective_stake = delegations
                .iter()
                .map(|delegation| delegation.stake(epoch, Some(&stake_history)))
                .sum::<u64>();

            let delta = if total_effective_stake > prev_total_effective_stake {
                total_effective_stake - prev_total_effective_stake
            } else {
                prev_total_effective_stake - total_effective_stake
            };

            // uncomment and add ! for fun with graphing
            //eprint("{:8} {:8} {:8} ", epoch, total_effective_stake, delta);
            //(0..(total_effective_stake as usize / (stakes.len() * 5))).for_each(|_| eprint("#"));
            //eprintln();

            assert!(
                delta
                    <= ((prev_total_effective_stake as f64 * Config::default().warmup_cooldown_rate) as u64)
                        .max(1)
            );

            prev_total_effective_stake = total_effective_stake;
        }
    }

    #[test]
    fn test_stake_initialize() {
        let stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;
        let mut stake_account =
            Account::new(stake_lamports, std::mem::size_of::<StakeState>(), &id());

        // unsigned keyed account
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, false, &mut stake_account);
        let custodian = Pubkey::new_rand();

        // not enough balance for rent...
        assert_eq!(
            stake_keyed_account.initialize(
                &Authorized::default(),
                &Lockup::default(),
                &Rent {
                    lamports_per_byte_year: 42,
                    ..Rent::default()
                }
            ),
            Err(InstructionError::InsufficientFunds)
        );

        // this one works, as is uninit
        assert_eq!(
            stake_keyed_account.initialize(
                &Authorized::auto(&stake_pubkey),
                &Lockup { slot: 1, custodian },
                &Rent::default(),
            ),
            Ok(())
        );
        // check that we see what we expect
        assert_eq!(
            StakeState::from(&stake_keyed_account.account).unwrap(),
            StakeState::Initialized(Meta {
                lockup: Lockup { slot: 1, custodian },
                ..Meta::auto(&stake_pubkey)
            })
        );

        // 2nd time fails, can't move it from anything other than uninit->init
        assert_eq!(
            stake_keyed_account.initialize(
                &Authorized::default(),
                &Lockup::default(),
                &Rent::default()
            ),
            Err(InstructionError::InvalidAccountData)
        );
    }

    #[test]
    fn test_deactivate_stake() {
        let stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;
        let mut stake_account = Account::new_data_with_space(
            stake_lamports,
            &StakeState::Initialized(Meta::auto(&stake_pubkey)),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let clock = sysvar::clock::Clock {
            epoch: 1,
            ..sysvar::clock::Clock::default()
        };

        // signed keyed account but not staked yet
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        let signers = vec![stake_pubkey].into_iter().collect();
        assert_eq!(
            stake_keyed_account.deactivate_stake(&clock, &signers),
            Err(InstructionError::InvalidAccountData)
        );

        // Staking
        let vote_pubkey = Pubkey::new_rand();
        let mut vote_account =
            vote_state::create_account(&vote_pubkey, &Pubkey::new_rand(), 0, 100);
        let mut vote_keyed_account = KeyedAccount::new(&vote_pubkey, false, &mut vote_account);
        vote_keyed_account.set_state(&VoteState::default()).unwrap();
        assert_eq!(
            stake_keyed_account.delegate_stake(
                &vote_keyed_account,
                &clock,
                &Config::default(),
                &signers
            ),
            Ok(())
        );

        // no signers fails
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, false, &mut stake_account);
        assert_eq!(
            stake_keyed_account.deactivate_stake(&clock, &HashSet::default()),
            Err(InstructionError::MissingRequiredSignature)
        );

        // Deactivate after staking
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        assert_eq!(
            stake_keyed_account.deactivate_stake(&clock, &signers),
            Ok(())
        );

        // verify that deactivate() only works once
        assert_eq!(
            stake_keyed_account.deactivate_stake(&clock, &signers),
            Err(StakeError::AlreadyDeactivated.into())
        );
    }

    #[test]
    fn test_withdraw_stake() {
        let stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;
        let mut stake_account = Account::new_data_with_space(
            stake_lamports,
            &StakeState::Uninitialized,
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let mut clock = sysvar::clock::Clock::default();

        let to = Pubkey::new_rand();
        let mut to_account = Account::new(1, 0, &system_program::id());
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);

        // no signers, should fail
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, false, &mut stake_account);
        assert_eq!(
            stake_keyed_account.withdraw(
                stake_lamports,
                &mut to_keyed_account,
                &clock,
                &StakeHistory::default(),
                &HashSet::default(),
            ),
            Err(InstructionError::MissingRequiredSignature)
        );

        // signed keyed account and uninitialized should work
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);
        let signers = vec![stake_pubkey].into_iter().collect();
        assert_eq!(
            stake_keyed_account.withdraw(
                stake_lamports,
                &mut to_keyed_account,
                &clock,
                &StakeHistory::default(),
                &signers,
            ),
            Ok(())
        );
        assert_eq!(stake_account.lamports, 0);

        // reset balance
        stake_account.lamports = stake_lamports;

        // lockup
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        let custodian = Pubkey::new_rand();
        stake_keyed_account
            .initialize(
                &Authorized::auto(&stake_pubkey),
                &Lockup { slot: 0, custodian },
                &Rent::default(),
            )
            .unwrap();

        // signed keyed account and locked up, more than available should fail
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);
        assert_eq!(
            stake_keyed_account.withdraw(
                stake_lamports + 1,
                &mut to_keyed_account,
                &clock,
                &StakeHistory::default(),
                &signers,
            ),
            Err(InstructionError::InsufficientFunds)
        );

        // Stake some lamports (available lamports for withdrawals will reduce to zero)
        let vote_pubkey = Pubkey::new_rand();
        let mut vote_account =
            vote_state::create_account(&vote_pubkey, &Pubkey::new_rand(), 0, 100);
        let mut vote_keyed_account = KeyedAccount::new(&vote_pubkey, false, &mut vote_account);
        vote_keyed_account.set_state(&VoteState::default()).unwrap();
        assert_eq!(
            stake_keyed_account.delegate_stake(
                &vote_keyed_account,
                &clock,
                &Config::default(),
                &signers,
            ),
            Ok(())
        );

        // simulate rewards
        stake_account.lamports += 10;
        // withdrawal before deactivate works for rewards amount
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);
        assert_eq!(
            stake_keyed_account.withdraw(
                10,
                &mut to_keyed_account,
                &clock,
                &StakeHistory::default(),
                &signers,
            ),
            Ok(())
        );

        // simulate rewards
        stake_account.lamports += 10;
        // withdrawal of rewards fails if not in excess of stake
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);
        assert_eq!(
            stake_keyed_account.withdraw(
                10 + 1,
                &mut to_keyed_account,
                &clock,
                &StakeHistory::default(),
                &signers
            ),
            Err(InstructionError::InsufficientFunds)
        );

        // deactivate the stake before withdrawal
        assert_eq!(
            stake_keyed_account.deactivate_stake(&clock, &signers),
            Ok(())
        );
        // simulate time passing
        clock.epoch += 100;

        // Try to withdraw more than what's available
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);
        assert_eq!(
            stake_keyed_account.withdraw(
                stake_lamports + 10 + 1,
                &mut to_keyed_account,
                &clock,
                &StakeHistory::default(),
                &signers
            ),
            Err(InstructionError::InsufficientFunds)
        );

        // Try to withdraw all lamports
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);
        assert_eq!(
            stake_keyed_account.withdraw(
                stake_lamports + 10,
                &mut to_keyed_account,
                &clock,
                &StakeHistory::default(),
                &signers
            ),
            Ok(())
        );
        assert_eq!(stake_account.lamports, 0);
    }

    #[test]
    fn test_withdraw_stake_before_warmup() {
        let stake_pubkey = Pubkey::new_rand();
        let total_lamports = 100;
        let stake_lamports = 42;
        let mut stake_account = Account::new_data_with_space(
            total_lamports,
            &StakeState::Initialized(Meta::auto(&stake_pubkey)),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let clock = sysvar::clock::Clock::default();
        let mut future = sysvar::clock::Clock::default();
        future.epoch += 16;

        let to = Pubkey::new_rand();
        let mut to_account = Account::new(1, 0, &system_program::id());
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);

        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);

        // Stake some lamports (available lamports for withdrawals will reduce)
        let vote_pubkey = Pubkey::new_rand();
        let mut vote_account =
            vote_state::create_account(&vote_pubkey, &Pubkey::new_rand(), 0, 100);
        let mut vote_keyed_account = KeyedAccount::new(&vote_pubkey, false, &mut vote_account);
        vote_keyed_account.set_state(&VoteState::default()).unwrap();
        let signers = vec![stake_pubkey].into_iter().collect();
        assert_eq!(
            stake_keyed_account.delegate_stake(
                &vote_keyed_account,
                &future,
                &Config::default(),
                &signers,
            ),
            Ok(())
        );

        let stake_history = create_stake_history_from_delegations(
            None,
            0..future.epoch,
            &[StakeState::stake_from(&stake_keyed_account.account)
                .unwrap()
                .delegation],
        );

        // Try to withdraw stake
        assert_eq!(
            stake_keyed_account.withdraw(
                total_lamports - stake_lamports + 1,
                &mut to_keyed_account,
                &clock,
                &stake_history,
                &signers,
            ),
            Err(InstructionError::InsufficientFunds)
        );
    }

    #[test]
    fn test_withdraw_stake_invalid_state() {
        let stake_pubkey = Pubkey::new_rand();
        let total_lamports = 100;
        let mut stake_account = Account::new_data_with_space(
            total_lamports,
            &StakeState::RewardsPool,
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let to = Pubkey::new_rand();
        let mut to_account = Account::new(1, 0, &system_program::id());
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        let signers = vec![stake_pubkey].into_iter().collect();
        assert_eq!(
            stake_keyed_account.withdraw(
                total_lamports,
                &mut to_keyed_account,
                &sysvar::clock::Clock::default(),
                &StakeHistory::default(),
                &signers,
            ),
            Err(InstructionError::InvalidAccountData)
        );
    }

    #[test]
    fn test_withdraw_lockup() {
        let stake_pubkey = Pubkey::new_rand();
        let custodian = Pubkey::new_rand();
        let total_lamports = 100;
        let mut stake_account = Account::new_data_with_space(
            total_lamports,
            &StakeState::Initialized(Meta {
                lockup: Lockup { slot: 1, custodian },
                ..Meta::auto(&stake_pubkey)
            }),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let to = Pubkey::new_rand();
        let mut to_account = Account::new(1, 0, &system_program::id());
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);

        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);

        let mut clock = sysvar::clock::Clock::default();

        let signers = vec![stake_pubkey].into_iter().collect();

        // lockup is still in force, can't withdraw
        assert_eq!(
            stake_keyed_account.withdraw(
                total_lamports,
                &mut to_keyed_account,
                &clock,
                &StakeHistory::default(),
                &signers,
            ),
            Err(StakeError::LockupInForce.into())
        );

        // but we *can* send to the custodian
        let mut custodian_account = Account::new(1, 0, &system_program::id());
        let mut custodian_keyed_account =
            KeyedAccount::new(&custodian, false, &mut custodian_account);
        assert_eq!(
            stake_keyed_account.withdraw(
                total_lamports,
                &mut custodian_keyed_account,
                &clock,
                &StakeHistory::default(),
                &signers,
            ),
            Ok(())
        );
        // reset balance
        stake_keyed_account.account.lamports = total_lamports;

        // lockup has expired
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);
        clock.slot += 1;
        assert_eq!(
            stake_keyed_account.withdraw(
                total_lamports,
                &mut to_keyed_account,
                &clock,
                &StakeHistory::default(),
                &signers,
            ),
            Ok(())
        );
    }

    #[test]
    fn test_stake_state_calculate_rewards() {
        let mut vote_state = VoteState::default();
        // assume stake.stake() is right
        // bootstrap means fully-vested stake at epoch 0
        let mut stake = Stake::new(
            1,
            &Pubkey::default(),
            &vote_state,
            std::u64::MAX,
            &Config::default(),
        );

        // this one can't collect now, credits_observed == vote_state.credits()
        assert_eq!(
            None,
            stake.calculate_rewards(1_000_000_000.0, &vote_state, None)
        );

        // put 2 credits in at epoch 0
        vote_state.increment_credits(0);
        vote_state.increment_credits(0);

        // this one can't collect now, no epoch credits have been saved off
        //   even though point value is huuge
        assert_eq!(
            None,
            stake.calculate_rewards(1_000_000_000_000.0, &vote_state, None)
        );

        // put 1 credit in epoch 1, pushes the 2 above into a redeemable state
        vote_state.increment_credits(1);

        // this one should be able to collect exactly 2
        assert_eq!(
            Some((0, stake.delegation.stake * 2, 2)),
            stake.calculate_rewards(1.0, &vote_state, None)
        );

        stake.credits_observed = 1;
        // this one should be able to collect exactly 1 (only observed one)
        assert_eq!(
            Some((0, stake.delegation.stake * 1, 2)),
            stake.calculate_rewards(1.0, &vote_state, None)
        );

        stake.credits_observed = 2;
        // this one should be able to collect none because credits_observed >= credits in a
        //  redeemable state (the 2 credits in epoch 0)
        assert_eq!(None, stake.calculate_rewards(1.0, &vote_state, None));

        // put 1 credit in epoch 2, pushes the 1 for epoch 1 to redeemable
        vote_state.increment_credits(2);
        // this one should be able to collect 1 now, one credit by a stake of 1
        assert_eq!(
            Some((0, stake.delegation.stake * 1, 3)),
            stake.calculate_rewards(1.0, &vote_state, None)
        );

        stake.credits_observed = 0;
        // this one should be able to collect everything from t=0 a warmed up stake of 2
        // (2 credits at stake of 1) + (1 credit at a stake of 2)
        assert_eq!(
            Some((
                0,
                stake.delegation.stake * 1 + stake.delegation.stake * 2,
                3
            )),
            stake.calculate_rewards(1.0, &vote_state, None)
        );

        // same as above, but is a really small commission out of 32 bits,
        //  verify that None comes back on small redemptions where no one gets paid
        vote_state.commission = 1;
        assert_eq!(
            None, // would be Some((0, 2 * 1 + 1 * 2, 3)),
            stake.calculate_rewards(1.0, &vote_state, None)
        );
        vote_state.commission = std::u8::MAX - 1;
        assert_eq!(
            None, // would be pSome((0, 2 * 1 + 1 * 2, 3)),
            stake.calculate_rewards(1.0, &vote_state, None)
        );
    }

    #[test]
    fn test_stake_redeem_vote_credits() {
        let clock = sysvar::clock::Clock::default();
        let mut rewards = sysvar::rewards::Rewards::default();
        rewards.validator_point_value = 100.0;

        let rewards_pool_pubkey = Pubkey::new_rand();
        let mut rewards_pool_account = Account::new_data(
            std::u64::MAX,
            &StakeState::RewardsPool,
            &crate::rewards_pools::id(),
        )
        .unwrap();
        let mut rewards_pool_keyed_account =
            KeyedAccount::new(&rewards_pool_pubkey, false, &mut rewards_pool_account);

        let stake_pubkey = Pubkey::default();
        let stake_lamports = 100;
        let mut stake_account = Account::new_data_with_space(
            stake_lamports,
            &StakeState::Initialized(Meta::auto(&stake_pubkey)),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);

        let vote_pubkey = Pubkey::new_rand();
        let mut vote_account =
            vote_state::create_account(&vote_pubkey, &Pubkey::new_rand(), 0, 100);
        let mut vote_keyed_account = KeyedAccount::new(&vote_pubkey, false, &mut vote_account);

        // not delegated yet, deserialization fails
        assert_eq!(
            stake_keyed_account.redeem_vote_credits(
                &mut vote_keyed_account,
                &mut rewards_pool_keyed_account,
                &rewards,
                &StakeHistory::default(),
            ),
            Err(InstructionError::InvalidAccountData)
        );
        let signers = vec![stake_pubkey].into_iter().collect();
        // delegate the stake
        assert!(stake_keyed_account
            .delegate_stake(&vote_keyed_account, &clock, &Config::default(), &signers)
            .is_ok());

        let stake_history = create_stake_history_from_delegations(
            Some(100),
            0..10,
            &[StakeState::stake_from(&stake_keyed_account.account)
                .unwrap()
                .delegation],
        );

        // no credits to claim
        assert_eq!(
            stake_keyed_account.redeem_vote_credits(
                &mut vote_keyed_account,
                &mut rewards_pool_keyed_account,
                &rewards,
                &stake_history,
            ),
            Err(StakeError::NoCreditsToRedeem.into())
        );

        // in this call, we've swapped rewards and vote, deserialization of rewards_pool fails
        assert_eq!(
            stake_keyed_account.redeem_vote_credits(
                &mut rewards_pool_keyed_account,
                &mut vote_keyed_account,
                &rewards,
                &StakeHistory::default(),
            ),
            Err(InstructionError::InvalidAccountData)
        );

        let mut vote_account =
            vote_state::create_account(&vote_pubkey, &Pubkey::new_rand(), 0, 100);

        let mut vote_state = VoteState::from(&vote_account).unwrap();
        // split credits 3:1 between staker and voter
        vote_state.commission = std::u8::MAX / 4;
        // put in some credits in epoch 0 for which we should have a non-zero stake
        for _i in 0..100 {
            vote_state.increment_credits(1);
        }
        vote_state.increment_credits(2);

        vote_state.to(&mut vote_account).unwrap();
        let mut vote_keyed_account = KeyedAccount::new(&vote_pubkey, false, &mut vote_account);

        // some credits to claim, but rewards pool empty (shouldn't ever happen)
        rewards_pool_keyed_account.account.lamports = 1;
        assert_eq!(
            stake_keyed_account.redeem_vote_credits(
                &mut vote_keyed_account,
                &mut rewards_pool_keyed_account,
                &rewards,
                &StakeHistory::default(),
            ),
            Err(InstructionError::UnbalancedInstruction)
        );
        rewards_pool_keyed_account.account.lamports = std::u64::MAX;

        // finally! some credits to claim
        let stake_account_balance = stake_keyed_account.account.lamports;
        let vote_account_balance = vote_keyed_account.account.lamports;
        assert_eq!(
            stake_keyed_account.redeem_vote_credits(
                &mut vote_keyed_account,
                &mut rewards_pool_keyed_account,
                &rewards,
                &stake_history,
            ),
            Ok(())
        );
        let staker_rewards = stake_keyed_account.account.lamports - stake_account_balance;
        let voter_commission = vote_keyed_account.account.lamports - vote_account_balance;
        assert!(voter_commission > 0);
        assert!(staker_rewards > 0);
        assert!(
            staker_rewards / 3 > voter_commission,
            "rewards should be split ~3:1"
        );
        // verify rewards are added to stake
        let stake = StakeState::stake_from(&stake_keyed_account.account).unwrap();
        assert_eq!(stake.delegation.stake, stake_keyed_account.account.lamports);

        let wrong_vote_pubkey = Pubkey::new_rand();
        let mut wrong_vote_keyed_account =
            KeyedAccount::new(&wrong_vote_pubkey, false, &mut vote_account);

        // wrong voter_pubkey...
        assert_eq!(
            stake_keyed_account.redeem_vote_credits(
                &mut wrong_vote_keyed_account,
                &mut rewards_pool_keyed_account,
                &rewards,
                &stake_history,
            ),
            Err(InstructionError::InvalidArgument)
        );
    }

    #[test]
    fn test_authorize_uninit() {
        let stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;
        let mut stake_account = Account::new_data_with_space(
            stake_lamports,
            &StakeState::default(),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        let signers = vec![stake_pubkey].into_iter().collect();
        assert_eq!(
            stake_keyed_account.authorize(&stake_pubkey, StakeAuthorize::Staker, &signers),
            Err(InstructionError::InvalidAccountData)
        );
    }

    #[test]
    fn test_authorize_lockup() {
        let stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;
        let mut stake_account = Account::new_data_with_space(
            stake_lamports,
            &StakeState::Initialized(Meta::auto(&stake_pubkey)),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let to = Pubkey::new_rand();
        let mut to_account = Account::new(1, 0, &system_program::id());
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);

        let clock = sysvar::clock::Clock::default();
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);

        let stake_pubkey0 = Pubkey::new_rand();
        let signers = vec![stake_pubkey].into_iter().collect();
        assert_eq!(
            stake_keyed_account.authorize(&stake_pubkey0, StakeAuthorize::Staker, &signers),
            Ok(())
        );
        assert_eq!(
            stake_keyed_account.authorize(&stake_pubkey0, StakeAuthorize::Withdrawer, &signers),
            Ok(())
        );
        if let StakeState::Initialized(Meta { authorized, .. }) =
            StakeState::from(&stake_keyed_account.account).unwrap()
        {
            assert_eq!(authorized.staker, stake_pubkey0);
            assert_eq!(authorized.withdrawer, stake_pubkey0);
        } else {
            assert!(false);
        }

        // A second authorization signed by the stake_keyed_account should fail
        let stake_pubkey1 = Pubkey::new_rand();
        assert_eq!(
            stake_keyed_account.authorize(&stake_pubkey1, StakeAuthorize::Staker, &signers),
            Err(InstructionError::MissingRequiredSignature)
        );

        let signers0 = vec![stake_pubkey0].into_iter().collect();

        // Test a second authorization by the newly authorized pubkey
        let stake_pubkey2 = Pubkey::new_rand();
        assert_eq!(
            stake_keyed_account.authorize(&stake_pubkey2, StakeAuthorize::Staker, &signers0),
            Ok(())
        );
        if let StakeState::Initialized(Meta { authorized, .. }) =
            StakeState::from(&stake_keyed_account.account).unwrap()
        {
            assert_eq!(authorized.staker, stake_pubkey2);
        }

        assert_eq!(
            stake_keyed_account.authorize(&stake_pubkey2, StakeAuthorize::Withdrawer, &signers0,),
            Ok(())
        );
        if let StakeState::Initialized(Meta { authorized, .. }) =
            StakeState::from(&stake_keyed_account.account).unwrap()
        {
            assert_eq!(authorized.staker, stake_pubkey2);
        }

        let signers2 = vec![stake_pubkey2].into_iter().collect();

        // Test that withdrawal to account fails without authorized withdrawer
        assert_eq!(
            stake_keyed_account.withdraw(
                stake_lamports,
                &mut to_keyed_account,
                &clock,
                &StakeHistory::default(),
                &signers, // old signer
            ),
            Err(InstructionError::MissingRequiredSignature)
        );

        // Test a successful action by the currently authorized withdrawer
        let mut to_keyed_account = KeyedAccount::new(&to, false, &mut to_account);
        assert_eq!(
            stake_keyed_account.withdraw(
                stake_lamports,
                &mut to_keyed_account,
                &clock,
                &StakeHistory::default(),
                &signers2,
            ),
            Ok(())
        );
    }

    #[test]
    fn test_split_source_uninitialized() {
        let stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;
        let mut stake_account = Account::new_data_with_space(
            stake_lamports,
            &StakeState::Uninitialized,
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let split_stake_pubkey = Pubkey::new_rand();
        let mut split_stake_account = Account::new_data_with_space(
            0,
            &StakeState::Uninitialized,
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, false, &mut stake_account);
        let mut split_stake_keyed_account =
            KeyedAccount::new(&split_stake_pubkey, false, &mut split_stake_account);

        // no signers should fail
        assert_eq!(
            stake_keyed_account.split(
                stake_lamports / 2,
                &mut split_stake_keyed_account,
                &HashSet::default() // no signers
            ),
            Err(InstructionError::MissingRequiredSignature)
        );

        // this should work
        let signers = vec![stake_pubkey].into_iter().collect();
        assert_eq!(
            stake_keyed_account.split(stake_lamports / 2, &mut split_stake_keyed_account, &signers),
            Ok(())
        );
        assert_eq!(
            stake_keyed_account.account.lamports,
            split_stake_keyed_account.account.lamports
        );
    }

    #[test]
    fn test_split_split_not_uninitialized() {
        let stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;
        let mut stake_account = Account::new_data_with_space(
            stake_lamports,
            &StakeState::Stake(Meta::auto(&stake_pubkey), Stake::just_stake(stake_lamports)),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let split_stake_pubkey = Pubkey::new_rand();
        let mut split_stake_account = Account::new_data_with_space(
            0,
            &StakeState::Initialized(Meta::auto(&stake_pubkey)),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let signers = vec![stake_pubkey].into_iter().collect();
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        let mut split_stake_keyed_account =
            KeyedAccount::new(&split_stake_pubkey, true, &mut split_stake_account);
        assert_eq!(
            stake_keyed_account.split(stake_lamports / 2, &mut split_stake_keyed_account, &signers),
            Err(InstructionError::InvalidAccountData)
        );
    }
    impl Stake {
        fn just_stake(stake: u64) -> Self {
            Self {
                delegation: Delegation {
                    stake,
                    ..Delegation::default()
                },
                ..Stake::default()
            }
        }
    }

    #[test]
    fn test_split_more_than_staked() {
        let stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;
        let mut stake_account = Account::new_data_with_space(
            stake_lamports,
            &StakeState::Stake(
                Meta::auto(&stake_pubkey),
                Stake::just_stake(stake_lamports / 2 - 1),
            ),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let split_stake_pubkey = Pubkey::new_rand();
        let mut split_stake_account = Account::new_data_with_space(
            0,
            &StakeState::Uninitialized,
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let signers = vec![stake_pubkey].into_iter().collect();
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        let mut split_stake_keyed_account =
            KeyedAccount::new(&split_stake_pubkey, true, &mut split_stake_account);
        assert_eq!(
            stake_keyed_account.split(stake_lamports / 2, &mut split_stake_keyed_account, &signers),
            Err(StakeError::InsufficientStake.into())
        );
    }

    #[test]
    fn test_split_with_rent() {
        let stake_pubkey = Pubkey::new_rand();
        let split_stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;
        let rent_exempt_reserve = 10;
        let signers = vec![stake_pubkey].into_iter().collect();

        let meta = Meta {
            authorized: Authorized::auto(&stake_pubkey),
            rent_exempt_reserve,
            ..Meta::default()
        };

        // test splitting both an Initialized stake and a Staked stake
        for state in &[
            StakeState::Initialized(meta),
            StakeState::Stake(meta, Stake::just_stake(stake_lamports)),
        ] {
            let mut stake_account = Account::new_data_with_space(
                stake_lamports,
                state,
                std::mem::size_of::<StakeState>(),
                &id(),
            )
            .expect("stake_account");

            let mut stake_keyed_account =
                KeyedAccount::new(&stake_pubkey, true, &mut stake_account);

            let mut split_stake_account = Account::new_data_with_space(
                0,
                &StakeState::Uninitialized,
                std::mem::size_of::<StakeState>(),
                &id(),
            )
            .expect("stake_account");

            let mut split_stake_keyed_account =
                KeyedAccount::new(&split_stake_pubkey, true, &mut split_stake_account);

            // not enough to make a stake account
            assert_eq!(
                stake_keyed_account.split(
                    rent_exempt_reserve - 1,
                    &mut split_stake_keyed_account,
                    &signers
                ),
                Err(InstructionError::InsufficientFunds)
            );

            // doesn't leave enough for initial stake
            assert_eq!(
                stake_keyed_account.split(
                    (stake_lamports - rent_exempt_reserve) + 1,
                    &mut split_stake_keyed_account,
                    &signers
                ),
                Err(InstructionError::InsufficientFunds)
            );

            // split account already has way enough lamports
            split_stake_keyed_account.account.lamports = 1_000;
            assert_eq!(
                stake_keyed_account.split(
                    stake_lamports - rent_exempt_reserve,
                    &mut split_stake_keyed_account,
                    &signers
                ),
                Ok(())
            );

            // verify no stake leakage in the case of a stake
            if let StakeState::Stake(meta, stake) = state {
                assert_eq!(
                    split_stake_keyed_account.state(),
                    Ok(StakeState::Stake(
                        *meta,
                        Stake {
                            delegation: Delegation {
                                stake: stake_lamports - rent_exempt_reserve,
                                ..stake.delegation
                            },
                            ..*stake
                        }
                    ))
                );
                //                assert_eq!(
                //                    stake_keyed_account.state(),
                //                    Ok(StakeState::Stake(*meta, Stake { stake: 0, ..*stake }))
                //                );
                assert_eq!(stake_keyed_account.account.lamports, rent_exempt_reserve);
                assert_eq!(
                    split_stake_keyed_account.account.lamports,
                    1_000 + stake_lamports - rent_exempt_reserve
                );
            }
        }
    }

    #[test]
    fn test_split() {
        let stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;

        let split_stake_pubkey = Pubkey::new_rand();
        let signers = vec![stake_pubkey].into_iter().collect();

        // test splitting both an Initialized stake and a Staked stake
        for state in &[
            StakeState::Initialized(Meta::auto(&stake_pubkey)),
            StakeState::Stake(Meta::auto(&stake_pubkey), Stake::just_stake(stake_lamports)),
        ] {
            let mut split_stake_account = Account::new_data_with_space(
                0,
                &StakeState::Uninitialized,
                std::mem::size_of::<StakeState>(),
                &id(),
            )
            .expect("stake_account");

            let mut split_stake_keyed_account =
                KeyedAccount::new(&split_stake_pubkey, true, &mut split_stake_account);

            let mut stake_account = Account::new_data_with_space(
                stake_lamports,
                state,
                std::mem::size_of::<StakeState>(),
                &id(),
            )
            .expect("stake_account");
            let mut stake_keyed_account =
                KeyedAccount::new(&stake_pubkey, true, &mut stake_account);

            // split more than available fails
            assert_eq!(
                stake_keyed_account.split(
                    stake_lamports + 1,
                    &mut split_stake_keyed_account,
                    &signers
                ),
                Err(InstructionError::InsufficientFunds)
            );

            // should work
            assert_eq!(
                stake_keyed_account.split(
                    stake_lamports / 2,
                    &mut split_stake_keyed_account,
                    &signers
                ),
                Ok(())
            );
            // no lamport leakage
            assert_eq!(
                stake_keyed_account.account.lamports + split_stake_keyed_account.account.lamports,
                stake_lamports
            );

            match state {
                StakeState::Initialized(_) => {
                    assert_eq!(Ok(*state), split_stake_keyed_account.state());
                    assert_eq!(Ok(*state), stake_keyed_account.state());
                }
                StakeState::Stake(meta, stake) => {
                    assert_eq!(
                        Ok(StakeState::Stake(
                            *meta,
                            Stake {
                                delegation: Delegation {
                                    stake: stake_lamports / 2,
                                    ..stake.delegation
                                },
                                ..*stake
                            }
                        )),
                        split_stake_keyed_account.state()
                    );
                    assert_eq!(
                        Ok(StakeState::Stake(
                            *meta,
                            Stake {
                                delegation: Delegation {
                                    stake: stake_lamports / 2,
                                    ..stake.delegation
                                },
                                ..*stake
                            }
                        )),
                        stake_keyed_account.state()
                    );
                }
                _ => unreachable!(),
            }

            // reset
            stake_keyed_account.account.lamports = stake_lamports;
        }
    }

    #[test]
    fn test_authorize_delegated_stake() {
        let stake_pubkey = Pubkey::new_rand();
        let stake_lamports = 42;
        let mut stake_account = Account::new_data_with_space(
            stake_lamports,
            &StakeState::Initialized(Meta::auto(&stake_pubkey)),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .expect("stake_account");

        let mut clock = sysvar::clock::Clock::default();

        let vote_pubkey = Pubkey::new_rand();
        let mut vote_account =
            vote_state::create_account(&vote_pubkey, &Pubkey::new_rand(), 0, 100);
        let vote_keyed_account = KeyedAccount::new(&vote_pubkey, false, &mut vote_account);

        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, true, &mut stake_account);
        let signers = vec![stake_pubkey].into_iter().collect();
        stake_keyed_account
            .delegate_stake(&vote_keyed_account, &clock, &Config::default(), &signers)
            .unwrap();

        let new_staker_pubkey = Pubkey::new_rand();
        assert_eq!(
            stake_keyed_account.authorize(&new_staker_pubkey, StakeAuthorize::Staker, &signers),
            Ok(())
        );
        let authorized = StakeState::authorized_from(&stake_keyed_account.account).unwrap();
        assert_eq!(authorized.staker, new_staker_pubkey);

        let other_pubkey = Pubkey::new_rand();
        let other_signers = vec![other_pubkey].into_iter().collect();

        // Use unsigned stake_keyed_account to test other signers
        let mut stake_keyed_account = KeyedAccount::new(&stake_pubkey, false, &mut stake_account);

        let new_voter_pubkey = Pubkey::new_rand();
        let vote_state = VoteState::default();
        let mut new_vote_account =
            vote_state::create_account(&new_voter_pubkey, &Pubkey::new_rand(), 0, 100);
        let mut new_vote_keyed_account =
            KeyedAccount::new(&new_voter_pubkey, false, &mut new_vote_account);
        new_vote_keyed_account.set_state(&vote_state).unwrap();

        // time passes, so we can re-delegate
        clock.epoch += 1;
        // Random other account should fail
        assert_eq!(
            stake_keyed_account.delegate_stake(
                &new_vote_keyed_account,
                &clock,
                &Config::default(),
                &other_signers,
            ),
            Err(InstructionError::MissingRequiredSignature)
        );

        let new_signers = vec![new_staker_pubkey].into_iter().collect();
        // Authorized staker should succeed
        assert_eq!(
            stake_keyed_account.delegate_stake(
                &new_vote_keyed_account,
                &clock,
                &Config::default(),
                &new_signers
            ),
            Ok(())
        );
        let stake = StakeState::stake_from(&stake_keyed_account.account).unwrap();
        assert_eq!(stake.delegation.voter_pubkey, new_voter_pubkey);

        // Test another staking action
        assert_eq!(
            stake_keyed_account.deactivate_stake(&clock, &new_signers),
            Ok(())
        );
    }
}
