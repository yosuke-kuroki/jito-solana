use crate::{
    consensus::{SwitchForkDecision, Tower},
    latest_validator_votes_for_frozen_banks::LatestValidatorVotesForFrozenBanks,
    progress_map::ProgressMap,
    replay_stage::HeaviestForkFailures,
};
use solana_runtime::{bank::Bank, bank_forks::BankForks};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

pub(crate) struct SelectVoteAndResetForkResult {
    pub vote_bank: Option<(Arc<Bank>, SwitchForkDecision)>,
    pub reset_bank: Option<Arc<Bank>>,
    pub heaviest_fork_failures: Vec<HeaviestForkFailures>,
}

pub(crate) trait ForkChoice {
    type ForkChoiceKey;
    fn compute_bank_stats(
        &mut self,
        bank: &Bank,
        tower: &Tower,
        latest_validator_votes_for_frozen_banks: &mut LatestValidatorVotesForFrozenBanks,
    );

    // Returns:
    // 1) The heaviest overall bank
    // 2) The heaviest bank on the same fork as the last vote (doesn't require a
    // switching proof to vote for)
    fn select_forks(
        &self,
        frozen_banks: &[Arc<Bank>],
        tower: &Tower,
        progress: &ProgressMap,
        ancestors: &HashMap<u64, HashSet<u64>>,
        bank_forks: &RwLock<BankForks>,
    ) -> (Arc<Bank>, Option<Arc<Bank>>);

    fn mark_fork_invalid_candidate(&mut self, invalid_slot: &Self::ForkChoiceKey);

    fn mark_fork_valid_candidate(&mut self, valid_slot: &Self::ForkChoiceKey);
}
