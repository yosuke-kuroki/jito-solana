//! Vote program
//! Receive and processes votes from validators

use crate::{
    id,
    vote_state::{self, Vote, VoteAuthorize, VoteInit, VoteState},
};
use log::*;
use num_derive::{FromPrimitive, ToPrimitive};
use serde_derive::{Deserialize, Serialize};
use solana_metrics::inc_new_counter_info;
use solana_sdk::{
    account::{get_signers, KeyedAccount},
    hash::Hash,
    instruction::{AccountMeta, Instruction, InstructionError, WithSigner},
    program_utils::{limited_deserialize, next_keyed_account, DecodeError},
    pubkey::Pubkey,
    system_instruction,
    sysvar::{self, clock::Clock, slot_hashes::SlotHashes, Sysvar},
};
use std::collections::HashSet;
use thiserror::Error;

/// Reasons the stake might have had an error
#[derive(Error, Debug, Clone, PartialEq, FromPrimitive, ToPrimitive)]
pub enum VoteError {
    #[error("vote already recorded or not in slot hashes history")]
    VoteTooOld,

    #[error("vote slots do not match bank history")]
    SlotsMismatch,

    #[error("vote hash does not match bank hash")]
    SlotHashMismatch,

    #[error("vote has no slots, invalid")]
    EmptySlots,

    #[error("vote timestamp not recent")]
    TimestampTooOld,

    #[error("authorized voter has already been changed this epoch")]
    TooSoonToReauthorize,
}

impl<E> DecodeError<E> for VoteError {
    fn type_of() -> &'static str {
        "VoteError"
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum VoteInstruction {
    /// Initialize the VoteState for this `vote account`
    ///    requires VoteInit::node_pubkey signature
    ///
    /// Expects 3 Accounts:
    ///    0 - Uninitialized Vote account
    ///    1 - Rent sysvar
    ///    2 - Clock sysvar
    ///
    InitializeAccount(VoteInit),

    /// Authorize a key to send votes or issue a withdrawal
    ///    requires authorized voter or authorized withdrawer signature,
    ///    depending on which key's being updated
    ///
    /// Expects 2 Accounts:
    ///    0 - Vote account to be updated with the Pubkey for authorization
    ///    1 - Clock sysvar
    ///
    Authorize(Pubkey, VoteAuthorize),

    /// A Vote instruction with recent votes
    ///    requires authorized voter signature
    ///
    /// Expects 3 Accounts:
    ///    0 - Vote account to vote with
    ///    1 - Slot hashes sysvar
    ///    2 - Clock sysvar
    Vote(Vote),

    /// Withdraw some amount of funds
    ///    requires authorized withdrawer signature
    ///
    /// Expects 2 Accounts:
    ///    0 - Vote account to withdraw from
    ///    1 - Destination account for the withdrawal
    Withdraw(u64),

    /// Update the vote account's validator identity (node_pubkey)
    ///    requires authorized withdrawer and new validator identity signature
    ///
    /// Expects 2 Accounts:
    ///    0 - Vote account to be updated with the Pubkey for authorization
    ///    1 - New validator identity (node_pubkey)
    ///
    UpdateValidatorIdentity,

    /// A Vote instruction with recent votes
    ///    requires authorized voter signature
    ///
    /// Expects 3 Accounts:
    ///    0 - Vote account to vote with
    ///    1 - Slot hashes sysvar
    ///    2 - Clock sysvar
    VoteSwitch(Vote, Hash),
}

fn initialize_account(vote_pubkey: &Pubkey, vote_init: &VoteInit) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(sysvar::rent::id(), false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ]
    .with_signer(&vote_init.node_pubkey);

    Instruction::new(
        id(),
        &VoteInstruction::InitializeAccount(*vote_init),
        account_metas,
    )
}

pub fn create_account(
    from_pubkey: &Pubkey,
    vote_pubkey: &Pubkey,
    vote_init: &VoteInit,
    lamports: u64,
) -> Vec<Instruction> {
    let space = VoteState::size_of() as u64;
    let create_ix =
        system_instruction::create_account(from_pubkey, vote_pubkey, lamports, space, &id());
    let init_ix = initialize_account(vote_pubkey, vote_init);
    vec![create_ix, init_ix]
}

pub fn create_account_with_seed(
    from_pubkey: &Pubkey,
    vote_pubkey: &Pubkey,
    base: &Pubkey,
    seed: &str,
    vote_init: &VoteInit,
    lamports: u64,
) -> Vec<Instruction> {
    let space = VoteState::size_of() as u64;
    let create_ix = system_instruction::create_account_with_seed(
        from_pubkey,
        vote_pubkey,
        base,
        seed,
        lamports,
        space,
        &id(),
    );
    let init_ix = initialize_account(vote_pubkey, vote_init);
    vec![create_ix, init_ix]
}

pub fn authorize(
    vote_pubkey: &Pubkey,
    authorized_pubkey: &Pubkey, // currently authorized
    new_authorized_pubkey: &Pubkey,
    vote_authorize: VoteAuthorize,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ]
    .with_signer(authorized_pubkey);

    Instruction::new(
        id(),
        &VoteInstruction::Authorize(*new_authorized_pubkey, vote_authorize),
        account_metas,
    )
}

pub fn update_validator_identity(
    vote_pubkey: &Pubkey,
    authorized_withdrawer_pubkey: &Pubkey,
    node_pubkey: &Pubkey,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(*node_pubkey, true),
    ]
    .with_signer(authorized_withdrawer_pubkey);

    Instruction::new(
        id(),
        &VoteInstruction::UpdateValidatorIdentity,
        account_metas,
    )
}

pub fn vote(vote_pubkey: &Pubkey, authorized_voter_pubkey: &Pubkey, vote: Vote) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(sysvar::slot_hashes::id(), false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ]
    .with_signer(authorized_voter_pubkey);

    Instruction::new(id(), &VoteInstruction::Vote(vote), account_metas)
}

pub fn vote_switch(
    vote_pubkey: &Pubkey,
    authorized_voter_pubkey: &Pubkey,
    vote: Vote,
    proof_hash: Hash,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(sysvar::slot_hashes::id(), false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ]
    .with_signer(authorized_voter_pubkey);

    Instruction::new(
        id(),
        &VoteInstruction::VoteSwitch(vote, proof_hash),
        account_metas,
    )
}

pub fn withdraw(
    vote_pubkey: &Pubkey,
    authorized_withdrawer_pubkey: &Pubkey,
    lamports: u64,
    to_pubkey: &Pubkey,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new(*to_pubkey, false),
    ]
    .with_signer(authorized_withdrawer_pubkey);

    Instruction::new(id(), &VoteInstruction::Withdraw(lamports), account_metas)
}

pub fn process_instruction(
    _program_id: &Pubkey,
    keyed_accounts: &[KeyedAccount],
    data: &[u8],
) -> Result<(), InstructionError> {
    trace!("process_instruction: {:?}", data);
    trace!("keyed_accounts: {:?}", keyed_accounts);

    let signers: HashSet<Pubkey> = get_signers(keyed_accounts);

    let keyed_accounts = &mut keyed_accounts.iter();
    let me = &mut next_keyed_account(keyed_accounts)?;

    match limited_deserialize(data)? {
        VoteInstruction::InitializeAccount(vote_init) => {
            sysvar::rent::verify_rent_exemption(me, next_keyed_account(keyed_accounts)?)?;
            vote_state::initialize_account(
                me,
                &vote_init,
                &signers,
                &Clock::from_keyed_account(next_keyed_account(keyed_accounts)?)?,
            )
        }
        VoteInstruction::Authorize(voter_pubkey, vote_authorize) => vote_state::authorize(
            me,
            &voter_pubkey,
            vote_authorize,
            &signers,
            &Clock::from_keyed_account(next_keyed_account(keyed_accounts)?)?,
        ),
        VoteInstruction::UpdateValidatorIdentity => vote_state::update_validator_identity(
            me,
            next_keyed_account(keyed_accounts)?.unsigned_key(),
            &signers,
        ),
        VoteInstruction::Vote(vote) | VoteInstruction::VoteSwitch(vote, _) => {
            inc_new_counter_info!("vote-native", 1);
            vote_state::process_vote(
                me,
                &SlotHashes::from_keyed_account(next_keyed_account(keyed_accounts)?)?,
                &Clock::from_keyed_account(next_keyed_account(keyed_accounts)?)?,
                &vote,
                &signers,
            )
        }
        VoteInstruction::Withdraw(lamports) => {
            let to = next_keyed_account(keyed_accounts)?;
            vote_state::withdraw(me, lamports, to, &signers)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{account::Account, rent::Rent};
    use std::cell::RefCell;

    // these are for 100% coverage in this file
    #[test]
    fn test_vote_process_instruction_decode_bail() {
        assert_eq!(
            super::process_instruction(&Pubkey::default(), &[], &[],),
            Err(InstructionError::NotEnoughAccountKeys),
        );
    }

    fn process_instruction(instruction: &Instruction) -> Result<(), InstructionError> {
        let mut accounts: Vec<_> = instruction
            .accounts
            .iter()
            .map(|meta| {
                RefCell::new(if sysvar::clock::check_id(&meta.pubkey) {
                    Clock::default().create_account(1)
                } else if sysvar::slot_hashes::check_id(&meta.pubkey) {
                    SlotHashes::default().create_account(1)
                } else if sysvar::rent::check_id(&meta.pubkey) {
                    Rent::free().create_account(1)
                } else {
                    Account::default()
                })
            })
            .collect();

        for _ in 0..instruction.accounts.len() {
            accounts.push(RefCell::new(Account::default()));
        }
        {
            let keyed_accounts: Vec<_> = instruction
                .accounts
                .iter()
                .zip(accounts.iter())
                .map(|(meta, account)| KeyedAccount::new(&meta.pubkey, meta.is_signer, account))
                .collect();
            super::process_instruction(&Pubkey::default(), &keyed_accounts, &instruction.data)
        }
    }

    #[test]
    fn test_vote_process_instruction() {
        let instructions = create_account(
            &Pubkey::default(),
            &Pubkey::default(),
            &VoteInit::default(),
            100,
        );
        assert_eq!(
            process_instruction(&instructions[1]),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction(&vote(
                &Pubkey::default(),
                &Pubkey::default(),
                Vote::default(),
            )),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction(&vote_switch(
                &Pubkey::default(),
                &Pubkey::default(),
                Vote::default(),
                Hash::default(),
            )),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction(&authorize(
                &Pubkey::default(),
                &Pubkey::default(),
                &Pubkey::default(),
                VoteAuthorize::Voter,
            )),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction(&update_validator_identity(
                &Pubkey::default(),
                &Pubkey::default(),
                &Pubkey::default(),
            )),
            Err(InstructionError::InvalidAccountData),
        );

        assert_eq!(
            process_instruction(&withdraw(
                &Pubkey::default(),
                &Pubkey::default(),
                0,
                &Pubkey::default()
            )),
            Err(InstructionError::InvalidAccountData),
        );
    }

    #[test]
    fn test_minimum_balance() {
        let rent = solana_sdk::rent::Rent::default();
        let minimum_balance = rent.minimum_balance(VoteState::size_of());
        // golden, may need updating when vote_state grows
        assert!(minimum_balance as f64 / 10f64.powf(9.0) < 0.04)
    }

    #[test]
    fn test_custom_error_decode() {
        use num_traits::FromPrimitive;
        fn pretty_err<T>(err: InstructionError) -> String
        where
            T: 'static + std::error::Error + DecodeError<T> + FromPrimitive,
        {
            if let InstructionError::Custom(code) = err {
                let specific_error: T = T::decode_custom_error_to_enum(code).unwrap();
                format!(
                    "{:?}: {}::{:?} - {}",
                    err,
                    T::type_of(),
                    specific_error,
                    specific_error,
                )
            } else {
                "".to_string()
            }
        }
        assert_eq!(
            "Custom(0): VoteError::VoteTooOld - vote already recorded or not in slot hashes history",
            pretty_err::<VoteError>(VoteError::VoteTooOld.into())
        )
    }
}
