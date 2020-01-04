use crate::native_loader;
use crate::system_instruction_processor;
use serde::{Deserialize, Serialize};
use solana_sdk::account::{create_keyed_readonly_accounts, Account, KeyedAccount};
use solana_sdk::clock::Epoch;
use solana_sdk::instruction::{CompiledInstruction, InstructionError};
use solana_sdk::instruction_processor_utils;
use solana_sdk::loader_instruction::LoaderInstruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::system_program;
use solana_sdk::transaction::TransactionError;
use std::collections::HashMap;
use std::io::Write;
use std::sync::RwLock;

#[cfg(unix)]
use libloading::os::unix::*;
#[cfg(windows)]
use libloading::os::windows::*;

/// Return true if the slice has any duplicate elements
pub fn has_duplicates<T: PartialEq>(xs: &[T]) -> bool {
    // Note: This is an O(n^2) algorithm, but requires no heap allocations. The benchmark
    // `bench_has_duplicates` in benches/message_processor.rs shows that this implementation is
    // ~50 times faster than using HashSet for very short slices.
    for i in 1..xs.len() {
        if xs[i..].contains(&xs[i - 1]) {
            return true;
        }
    }
    false
}

/// Get mut references to a subset of elements.
fn get_subset_unchecked_mut<'a, T>(
    xs: &'a mut [T],
    indexes: &[u8],
) -> Result<Vec<&'a mut T>, InstructionError> {
    // Since the compiler doesn't know the indexes are unique, dereferencing
    // multiple mut elements is assumed to be unsafe. If, however, all
    // indexes are unique, it's perfectly safe. The returned elements will share
    // the liftime of the input slice.

    // Make certain there are no duplicate indexes. If there are, return an error
    // because we can't return multiple mut references to the same element.
    if has_duplicates(indexes) {
        return Err(InstructionError::DuplicateAccountIndex);
    }

    Ok(indexes
        .iter()
        .map(|i| {
            let ptr = &mut xs[*i as usize] as *mut T;
            unsafe { &mut *ptr }
        })
        .collect())
}

// The relevant state of an account before an Instruction executes, used
// to verify account integrity after the Instruction completes
pub struct PreInstructionAccount {
    pub is_writable: bool,
    pub lamports: u64,
    pub data_len: usize,
    pub data: Option<Vec<u8>>,
    pub owner: Pubkey,
    pub executable: bool,
    pub rent_epoch: Epoch,
}
impl PreInstructionAccount {
    pub fn new(account: &Account, is_writable: bool, copy_data: bool) -> Self {
        Self {
            is_writable,
            lamports: account.lamports,
            data_len: account.data.len(),
            data: if copy_data {
                Some(account.data.clone())
            } else {
                None
            },
            owner: account.owner,
            executable: account.executable,
            rent_epoch: account.rent_epoch,
        }
    }
}
pub fn need_account_data_checked(program_id: &Pubkey, owner: &Pubkey, is_writable: bool) -> bool {
    // For accounts not assigned to the program, the data may not change.
    program_id != owner
    // Read-only account data may not change.
    || !is_writable
}
pub fn verify_account_changes(
    program_id: &Pubkey,
    pre: &PreInstructionAccount,
    post: &Account,
) -> Result<(), InstructionError> {
    // Verify the transaction

    // Only the owner of the account may change owner and
    //   only if the account is writable and
    //   only if the data is zero-initialized or empty
    if pre.owner != post.owner
        && (!pre.is_writable // line coverage used to get branch coverage
            || *program_id != pre.owner // line coverage used to get branch coverage
            || !is_zeroed(&post.data))
    {
        return Err(InstructionError::ModifiedProgramId);
    }

    // An account not assigned to the program cannot have its balance decrease.
    if *program_id != pre.owner // line coverage used to get branch coverage
        && pre.lamports > post.lamports
    {
        return Err(InstructionError::ExternalAccountLamportSpend);
    }

    // The balance of read-only accounts may not change.
    if !pre.is_writable // line coverage used to get branch coverage
        && pre.lamports != post.lamports
    {
        return Err(InstructionError::ReadonlyLamportChange);
    }

    // Only the system program can change the size of the data
    //  and only if the system program owns the account
    if pre.data_len != post.data.len()
        && (!system_program::check_id(program_id) // line coverage used to get branch coverage
            || !system_program::check_id(&pre.owner))
    {
        return Err(InstructionError::AccountDataSizeChanged);
    }

    if need_account_data_checked(&pre.owner, program_id, pre.is_writable) {
        match &pre.data {
            Some(data) if *data == post.data => (),
            _ => {
                if !pre.is_writable {
                    return Err(InstructionError::ReadonlyDataModified);
                } else {
                    return Err(InstructionError::ExternalAccountDataModified);
                }
            }
        }
    }

    // executable is one-way (false->true) and only the account owner may set it.
    if pre.executable != post.executable
        && (!pre.is_writable // line coverage used to get branch coverage
            || pre.executable // line coverage used to get branch coverage
            || *program_id != pre.owner)
    {
        return Err(InstructionError::ExecutableModified);
    }

    // No one modifies rent_epoch (yet).
    if pre.rent_epoch != post.rent_epoch {
        return Err(InstructionError::RentEpochModified);
    }

    Ok(())
}

/// Return instruction data to pass to process_instruction().
/// When a loader is detected, the instruction data is wrapped with a LoaderInstruction
/// to signal to the loader that the instruction data should be used as arguments when
/// invoking a "main()" function.
fn get_loader_instruction_data<'a>(
    loaders: &[(Pubkey, Account)],
    ix_data: &'a [u8],
    loader_ix_data: &'a mut Vec<u8>,
) -> &'a [u8] {
    if loaders.len() > 1 {
        let ix = LoaderInstruction::InvokeMain {
            data: ix_data.to_vec(),
        };
        let ix_data = bincode::serialize(&ix).unwrap();
        loader_ix_data.write_all(&ix_data).unwrap();
        loader_ix_data
    } else {
        ix_data
    }
}

pub type ProcessInstruction =
    fn(&Pubkey, &mut [KeyedAccount], &[u8]) -> Result<(), InstructionError>;

pub type SymbolCache = RwLock<HashMap<Vec<u8>, Symbol<instruction_processor_utils::Entrypoint>>>;

#[derive(Serialize, Deserialize)]
pub struct MessageProcessor {
    #[serde(skip)]
    instruction_processors: Vec<(Pubkey, ProcessInstruction)>,
    #[serde(skip)]
    symbol_cache: SymbolCache,
}

impl Default for MessageProcessor {
    fn default() -> Self {
        let instruction_processors: Vec<(Pubkey, ProcessInstruction)> = vec![(
            system_program::id(),
            system_instruction_processor::process_instruction,
        )];

        Self {
            instruction_processors,
            symbol_cache: RwLock::new(HashMap::new()),
        }
    }
}

impl MessageProcessor {
    /// Add a static entrypoint to intercept instructions before the dynamic loader.
    pub fn add_instruction_processor(
        &mut self,
        program_id: Pubkey,
        process_instruction: ProcessInstruction,
    ) {
        self.instruction_processors
            .push((program_id, process_instruction));
    }

    /// Process an instruction
    /// This method calls the instruction's program entrypoint method
    fn process_instruction(
        &self,
        message: &Message,
        instruction: &CompiledInstruction,
        executable_accounts: &mut [(Pubkey, Account)],
        program_accounts: &mut [&mut Account],
    ) -> Result<(), InstructionError> {
        let program_id = instruction.program_id(&message.account_keys);

        let mut loader_ix_data = vec![];
        let ix_data = get_loader_instruction_data(
            executable_accounts,
            &instruction.data,
            &mut loader_ix_data,
        );

        let mut keyed_accounts = create_keyed_readonly_accounts(executable_accounts);
        let mut keyed_accounts2: Vec<_> = instruction
            .accounts
            .iter()
            .map(|&index| {
                let index = index as usize;
                let key = &message.account_keys[index];
                let is_writable = message.is_writable(index);
                (
                    key,
                    index < message.header.num_required_signatures as usize,
                    is_writable,
                )
            })
            .zip(program_accounts.iter_mut())
            .map(|((key, is_signer, is_writable), account)| {
                if is_writable {
                    KeyedAccount::new(key, is_signer, account)
                } else {
                    KeyedAccount::new_readonly(key, is_signer, account)
                }
            })
            .collect();
        keyed_accounts.append(&mut keyed_accounts2);

        assert!(
            keyed_accounts[0].account.executable,
            "loader not executable"
        );

        let loader_id = keyed_accounts[0].unsigned_key();
        for (id, process_instruction) in &self.instruction_processors {
            if id == loader_id {
                return process_instruction(&program_id, &mut keyed_accounts[1..], &ix_data);
            }
        }

        native_loader::invoke_entrypoint(
            &program_id,
            &mut keyed_accounts,
            ix_data,
            &self.symbol_cache,
        )
    }

    fn sum_account_lamports(accounts: &mut [&mut Account]) -> u128 {
        accounts.iter().map(|a| u128::from(a.lamports)).sum()
    }

    /// Execute an instruction
    /// This method calls the instruction's program entrypoint method and verifies that the result of
    /// the call does not violate the bank's accounting rules.
    /// The accounts are committed back to the bank only if this function returns Ok(_).
    fn execute_instruction(
        &self,
        message: &Message,
        instruction: &CompiledInstruction,
        executable_accounts: &mut [(Pubkey, Account)],
        program_accounts: &mut [&mut Account],
    ) -> Result<(), InstructionError> {
        assert_eq!(instruction.accounts.len(), program_accounts.len());
        let program_id = instruction.program_id(&message.account_keys);
        // Copy only what we need to verify after instruction processing
        let pre_accounts: Vec<_> = program_accounts
            .iter_mut()
            .enumerate()
            .map(|(i, account)| {
                let is_writable = message.is_writable(instruction.accounts[i] as usize);
                PreInstructionAccount::new(
                    account,
                    is_writable,
                    need_account_data_checked(&account.owner, program_id, is_writable),
                )
            })
            .collect();
        // Sum total lamports before instruction processing
        let pre_total = Self::sum_account_lamports(program_accounts);

        self.process_instruction(message, instruction, executable_accounts, program_accounts)?;

        // Verify the instruction
        for (pre_account, post_account) in pre_accounts.iter().zip(program_accounts.iter()) {
            verify_account_changes(&program_id, pre_account, post_account)?;
        }
        // The total sum of all the lamports in all the accounts cannot change.
        let post_total = Self::sum_account_lamports(program_accounts);
        if pre_total != post_total {
            return Err(InstructionError::UnbalancedInstruction);
        }
        Ok(())
    }

    /// Process a message.
    /// This method calls each instruction in the message over the set of loaded Accounts
    /// The accounts are committed back to the bank only if every instruction succeeds
    pub fn process_message(
        &self,
        message: &Message,
        loaders: &mut [Vec<(Pubkey, Account)>],
        accounts: &mut [Account],
    ) -> Result<(), TransactionError> {
        for (instruction_index, instruction) in message.instructions.iter().enumerate() {
            let executable_index = message
                .program_position(instruction.program_id_index as usize)
                .ok_or(TransactionError::InvalidAccountIndex)?;
            let executable_accounts = &mut loaders[executable_index];
            let mut program_accounts = get_subset_unchecked_mut(accounts, &instruction.accounts)
                .map_err(|err| TransactionError::InstructionError(instruction_index as u8, err))?;
            // TODO: `get_subset_unchecked_mut` panics on an index out of bounds if an executable
            // account is also included as a regular account for an instruction, because the
            // executable account is not passed in as part of the accounts slice
            self.execute_instruction(
                message,
                instruction,
                executable_accounts,
                &mut program_accounts,
            )
            .map_err(|err| TransactionError::InstructionError(instruction_index as u8, err))?;
        }
        Ok(())
    }
}

pub const ZEROS_LEN: usize = 1024;
static ZEROS: [u8; ZEROS_LEN] = [0; ZEROS_LEN];
pub fn is_zeroed(buf: &[u8]) -> bool {
    let mut chunks = buf.chunks_exact(ZEROS_LEN);

    chunks.all(|chunk| chunk == &ZEROS[..])
        && chunks.remainder() == &ZEROS[..chunks.remainder().len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::instruction::{AccountMeta, Instruction, InstructionError};
    use solana_sdk::message::Message;
    use solana_sdk::native_loader::create_loadable_account;

    #[test]
    fn test_is_zeroed() {
        let mut buf = [0; ZEROS_LEN];
        assert_eq!(is_zeroed(&buf), true);
        buf[0] = 1;
        assert_eq!(is_zeroed(&buf), false);

        let mut buf = [0; ZEROS_LEN - 1];
        assert_eq!(is_zeroed(&buf), true);
        buf[0] = 1;
        assert_eq!(is_zeroed(&buf), false);

        let mut buf = [0; ZEROS_LEN + 1];
        assert_eq!(is_zeroed(&buf), true);
        buf[0] = 1;
        assert_eq!(is_zeroed(&buf), false);

        let buf = vec![];
        assert_eq!(is_zeroed(&buf), true);
    }

    #[test]
    fn test_has_duplicates() {
        assert!(!has_duplicates(&[1, 2]));
        assert!(has_duplicates(&[1, 2, 1]));
    }

    #[test]
    fn test_get_subset_unchecked_mut() {
        assert_eq!(
            get_subset_unchecked_mut(&mut [7, 8], &[0]).unwrap(),
            vec![&mut 7]
        );
        assert_eq!(
            get_subset_unchecked_mut(&mut [7, 8], &[0, 1]).unwrap(),
            vec![&mut 7, &mut 8]
        );
    }

    #[test]
    fn test_get_subset_unchecked_mut_duplicate_index() {
        // This panics, because it assumes duplicate detection is done elsewhere.
        assert_eq!(
            get_subset_unchecked_mut(&mut [7, 8], &[0, 0]).unwrap_err(),
            InstructionError::DuplicateAccountIndex
        );
    }

    #[test]
    #[should_panic]
    fn test_get_subset_unchecked_mut_out_of_bounds() {
        // This panics, because it assumes bounds validation is done elsewhere.
        get_subset_unchecked_mut(&mut [7, 8], &[2]).unwrap();
    }

    #[test]
    fn test_verify_account_changes_owner() {
        fn change_owner(
            ix: &Pubkey,
            pre: &Pubkey,
            post: &Pubkey,
            is_writable: bool,
        ) -> Result<(), InstructionError> {
            verify_account_changes(
                &ix,
                &PreInstructionAccount::new(
                    &Account::new(0, 0, pre),
                    is_writable,
                    need_account_data_checked(pre, ix, is_writable),
                ),
                &Account::new(0, 0, post),
            )
        }

        let system_program_id = system_program::id();
        let alice_program_id = Pubkey::new_rand();
        let mallory_program_id = Pubkey::new_rand();

        assert_eq!(
            change_owner(
                &system_program_id,
                &system_program_id,
                &alice_program_id,
                true
            ),
            Ok(()),
            "system program should be able to change the account owner"
        );
        assert_eq!(
            change_owner(
                &system_program_id,
                &system_program_id,
                &alice_program_id,
                false
            ),
            Err(InstructionError::ModifiedProgramId),
            "system program should not be able to change the account owner of a read-only account"
        );
        assert_eq!(
            change_owner(
                &system_program_id,
                &mallory_program_id,
                &alice_program_id,
                true
            ),
            Err(InstructionError::ModifiedProgramId),
            "system program should not be able to change the account owner of a non-system account"
        );

        assert_eq!(
            change_owner(
                &mallory_program_id,
                &mallory_program_id,
                &alice_program_id,
                true
            ),
            Ok(()),
            "mallory should be able to change the account owner, if she leaves clear data"
        );

        assert_eq!(
            verify_account_changes(
                &mallory_program_id,
                &PreInstructionAccount::new(
                    &Account::new_data(0, &[42], &mallory_program_id).unwrap(),
                    true,
                    need_account_data_checked(&mallory_program_id, &mallory_program_id, true),
                ),
                &Account::new_data(0, &[0], &alice_program_id,).unwrap(),
            ),
            Ok(()),
            "mallory should be able to change the account owner, if she leaves clear data"
        );
        assert_eq!(
            verify_account_changes(
                &mallory_program_id,
                &PreInstructionAccount::new(
                    &Account::new_data(0, &[42], &mallory_program_id).unwrap(),
                    true,
                    need_account_data_checked(&mallory_program_id, &mallory_program_id, true),
                ),
                &Account::new_data(0, &[42], &alice_program_id,).unwrap(),
            ),
            Err(InstructionError::ModifiedProgramId),
            "mallory should not be able to inject data into the alice program"
        );
    }

    #[test]
    fn test_verify_account_changes_executable() {
        let owner = Pubkey::new_rand();
        let change_executable = |program_id: &Pubkey,
                                 is_writable: bool,
                                 pre_executable: bool,
                                 post_executable: bool|
         -> Result<(), InstructionError> {
            let pre = PreInstructionAccount::new(
                &Account {
                    owner,
                    executable: pre_executable,
                    ..Account::default()
                },
                is_writable,
                need_account_data_checked(&owner, &program_id, is_writable),
            );

            let post = Account {
                owner,
                executable: post_executable,
                ..Account::default()
            };
            verify_account_changes(&program_id, &pre, &post)
        };

        let mallory_program_id = Pubkey::new_rand();
        let system_program_id = system_program::id();

        assert_eq!(
            change_executable(&system_program_id, true, false, true),
            Err(InstructionError::ExecutableModified),
            "system program can't change executable if system doesn't own the account"
        );
        assert_eq!(
            change_executable(&owner, true, false, true),
            Ok(()),
            "alice program should be able to change executable"
        );
        assert_eq!(
            change_executable(&owner, false, false, true),
            Err(InstructionError::ExecutableModified),
            "system program can't modify executable of read-only accounts"
        );
        assert_eq!(
            change_executable(&owner, true, true, false),
            Err(InstructionError::ExecutableModified),
            "system program can't reverse executable"
        );
        assert_eq!(
            change_executable(&mallory_program_id, true, false, true),
            Err(InstructionError::ExecutableModified),
            "malicious Mallory should not be able to change the account executable"
        );
    }

    #[test]
    fn test_verify_account_changes_data_len() {
        assert_eq!(
            verify_account_changes(
                &system_program::id(),
                &PreInstructionAccount::new(
                    &Account::new_data(0, &[0], &system_program::id()).unwrap(),
                    true,
                    need_account_data_checked(&system_program::id(), &system_program::id(), true),
                ),
                &Account::new_data(0, &[0, 0], &system_program::id()).unwrap(),
            ),
            Ok(()),
            "system program should be able to change the data len"
        );
        let alice_program_id = Pubkey::new_rand();

        assert_eq!(
            verify_account_changes(
                &system_program::id(),
                &PreInstructionAccount::new(
                    &Account::new_data(0, &[0], &alice_program_id).unwrap(),
                    true,
                    need_account_data_checked(&alice_program_id, &system_program::id(), true),
                ),
                &Account::new_data(0, &[0, 0], &alice_program_id).unwrap(),
            ),
            Err(InstructionError::AccountDataSizeChanged),
            "system program should not be able to change the data length of accounts it does not own"
        );
    }

    #[test]
    fn test_verify_account_changes_data() {
        let alice_program_id = Pubkey::new_rand();

        let change_data =
            |program_id: &Pubkey, is_writable: bool| -> Result<(), InstructionError> {
                let pre = PreInstructionAccount::new(
                    &Account::new_data(0, &[0], &alice_program_id).unwrap(),
                    is_writable,
                    need_account_data_checked(&alice_program_id, &program_id, is_writable),
                );
                let post = Account::new_data(0, &[42], &alice_program_id).unwrap();
                verify_account_changes(&program_id, &pre, &post)
            };

        let mallory_program_id = Pubkey::new_rand();

        assert_eq!(
            change_data(&alice_program_id, true),
            Ok(()),
            "alice program should be able to change the data"
        );
        assert_eq!(
            change_data(&mallory_program_id, true),
            Err(InstructionError::ExternalAccountDataModified),
            "non-owner mallory should not be able to change the account data"
        );

        assert_eq!(
            change_data(&alice_program_id, false),
            Err(InstructionError::ReadonlyDataModified),
            "alice isn't allowed to touch a CO account"
        );
    }

    #[test]
    fn test_verify_account_changes_rent_epoch() {
        let alice_program_id = Pubkey::new_rand();
        let pre = PreInstructionAccount::new(
            &Account::new(0, 0, &alice_program_id),
            false,
            need_account_data_checked(&alice_program_id, &system_program::id(), false),
        );
        let mut post = Account::new(0, 0, &alice_program_id);

        assert_eq!(
            verify_account_changes(&system_program::id(), &pre, &post),
            Ok(()),
            "nothing changed!"
        );

        post.rent_epoch += 1;
        assert_eq!(
            verify_account_changes(&system_program::id(), &pre, &post),
            Err(InstructionError::RentEpochModified),
            "no one touches rent_epoch"
        );
    }

    #[test]
    fn test_verify_account_changes_deduct_lamports_and_reassign_account() {
        let alice_program_id = Pubkey::new_rand();
        let bob_program_id = Pubkey::new_rand();
        let pre = PreInstructionAccount::new(
            &Account::new_data(42, &[42], &alice_program_id).unwrap(),
            true,
            need_account_data_checked(&alice_program_id, &alice_program_id, true),
        );
        let post = Account::new_data(1, &[0], &bob_program_id).unwrap();

        // positive test of this capability
        assert_eq!(
            verify_account_changes(&alice_program_id, &pre, &post),
            Ok(()),
            "alice should be able to deduct lamports and give the account to bob if the data is zeroed",
        );
    }

    #[test]
    fn test_verify_account_changes_lamports() {
        let alice_program_id = Pubkey::new_rand();
        let pre = PreInstructionAccount::new(
            &Account::new(42, 0, &alice_program_id),
            false,
            need_account_data_checked(&alice_program_id, &system_program::id(), false),
        );
        let post = Account::new(0, 0, &alice_program_id);

        assert_eq!(
            verify_account_changes(&system_program::id(), &pre, &post),
            Err(InstructionError::ExternalAccountLamportSpend),
            "debit should fail, even if system program"
        );

        let pre = PreInstructionAccount::new(
            &Account::new(42, 0, &alice_program_id),
            false,
            need_account_data_checked(&alice_program_id, &alice_program_id, false),
        );

        assert_eq!(
            verify_account_changes(&alice_program_id, &pre, &post,),
            Err(InstructionError::ReadonlyLamportChange),
            "debit should fail, even if owning program"
        );

        let pre = PreInstructionAccount::new(
            &Account::new(42, 0, &alice_program_id),
            true,
            need_account_data_checked(&alice_program_id, &system_program::id(), true),
        );
        let post = Account::new(0, 0, &system_program::id());
        assert_eq!(
            verify_account_changes(&system_program::id(), &pre, &post),
            Err(InstructionError::ModifiedProgramId),
            "system program can't debit the account unless it was the pre.owner"
        );

        let pre = PreInstructionAccount::new(
            &Account::new(42, 0, &system_program::id()),
            true,
            need_account_data_checked(&system_program::id(), &system_program::id(), true),
        );
        let post = Account::new(0, 0, &alice_program_id);
        assert_eq!(
            verify_account_changes(&system_program::id(), &pre, &post),
            Ok(()),
            "system can spend (and change owner)"
        );
    }

    #[test]
    fn test_verify_account_changes_data_size_changed() {
        let alice_program_id = Pubkey::new_rand();
        let pre = PreInstructionAccount::new(
            &Account::new_data(42, &[0], &alice_program_id).unwrap(),
            true,
            need_account_data_checked(&alice_program_id, &system_program::id(), true),
        );
        let post = Account::new_data(42, &[0, 0], &alice_program_id).unwrap();
        assert_eq!(
            verify_account_changes(&system_program::id(), &pre, &post),
            Err(InstructionError::AccountDataSizeChanged),
            "system program should not be able to change another program's account data size"
        );
        let pre = PreInstructionAccount::new(
            &Account::new_data(42, &[0], &alice_program_id).unwrap(),
            true,
            need_account_data_checked(&alice_program_id, &alice_program_id, true),
        );
        assert_eq!(
            verify_account_changes(&alice_program_id, &pre, &post),
            Err(InstructionError::AccountDataSizeChanged),
            "non-system programs cannot change their data size"
        );
        let pre = PreInstructionAccount::new(
            &Account::new_data(42, &[0], &system_program::id()).unwrap(),
            true,
            need_account_data_checked(&system_program::id(), &system_program::id(), true),
        );
        assert_eq!(
            verify_account_changes(&system_program::id(), &pre, &post),
            Ok(()),
            "system program should be able to change acount data size"
        );
    }

    #[test]
    fn test_process_message_readonly_handling() {
        #[derive(Serialize, Deserialize)]
        enum MockSystemInstruction {
            Correct,
            AttemptCredit { lamports: u64 },
            AttemptDataChange { data: u8 },
        }

        fn mock_system_process_instruction(
            _program_id: &Pubkey,
            keyed_accounts: &mut [KeyedAccount],
            data: &[u8],
        ) -> Result<(), InstructionError> {
            if let Ok(instruction) = bincode::deserialize(data) {
                match instruction {
                    MockSystemInstruction::Correct => Ok(()),
                    MockSystemInstruction::AttemptCredit { lamports } => {
                        keyed_accounts[0].account.lamports -= lamports;
                        keyed_accounts[1].account.lamports += lamports;
                        Ok(())
                    }
                    // Change data in a read-only account
                    MockSystemInstruction::AttemptDataChange { data } => {
                        keyed_accounts[1].account.data = vec![data];
                        Ok(())
                    }
                }
            } else {
                Err(InstructionError::InvalidInstructionData)
            }
        }

        let mock_system_program_id = Pubkey::new(&[2u8; 32]);
        let mut message_processor = MessageProcessor::default();
        message_processor
            .add_instruction_processor(mock_system_program_id, mock_system_process_instruction);

        let mut accounts: Vec<Account> = Vec::new();
        let account = Account::new(100, 1, &mock_system_program_id);
        accounts.push(account);
        let account = Account::new(0, 1, &mock_system_program_id);
        accounts.push(account);

        let mut loaders: Vec<Vec<(Pubkey, Account)>> = Vec::new();
        let account = create_loadable_account("mock_system_program");
        loaders.push(vec![(mock_system_program_id, account)]);

        let from_pubkey = Pubkey::new_rand();
        let to_pubkey = Pubkey::new_rand();
        let account_metas = vec![
            AccountMeta::new(from_pubkey, true),
            AccountMeta::new_readonly(to_pubkey, false),
        ];
        let message = Message::new(vec![Instruction::new(
            mock_system_program_id,
            &MockSystemInstruction::Correct,
            account_metas.clone(),
        )]);

        let result = message_processor.process_message(&message, &mut loaders, &mut accounts);
        assert_eq!(result, Ok(()));
        assert_eq!(accounts[0].lamports, 100);
        assert_eq!(accounts[1].lamports, 0);

        let message = Message::new(vec![Instruction::new(
            mock_system_program_id,
            &MockSystemInstruction::AttemptCredit { lamports: 50 },
            account_metas.clone(),
        )]);

        let result = message_processor.process_message(&message, &mut loaders, &mut accounts);
        assert_eq!(
            result,
            Err(TransactionError::InstructionError(
                0,
                InstructionError::ReadonlyLamportChange
            ))
        );

        let message = Message::new(vec![Instruction::new(
            mock_system_program_id,
            &MockSystemInstruction::AttemptDataChange { data: 50 },
            account_metas,
        )]);

        let result = message_processor.process_message(&message, &mut loaders, &mut accounts);
        assert_eq!(
            result,
            Err(TransactionError::InstructionError(
                0,
                InstructionError::ReadonlyDataModified
            ))
        );
    }

    #[test]
    fn test_get_loader_instruction_data() {
        // First ensure the ix_data is unaffected if not invoking via a loader.
        let ix_data = [1];
        let mut loader_ix_data = vec![];

        let native_pubkey = Pubkey::new_rand();
        let native_loader = (native_pubkey, Account::new(0, 0, &native_pubkey));
        assert_eq!(
            get_loader_instruction_data(&[native_loader.clone()], &ix_data, &mut loader_ix_data),
            &ix_data
        );

        // Now ensure the ix_data is wrapped when there's a loader present.
        let acme_pubkey = Pubkey::new_rand();
        let acme_loader = (acme_pubkey, Account::new(0, 0, &native_pubkey));
        let expected_ix = LoaderInstruction::InvokeMain {
            data: ix_data.to_vec(),
        };
        let expected_ix_data = bincode::serialize(&expected_ix).unwrap();
        assert_eq!(
            get_loader_instruction_data(
                &[native_loader.clone(), acme_loader.clone()],
                &ix_data,
                &mut loader_ix_data
            ),
            &expected_ix_data[..]
        );

        // Note there was an allocation in the input vector.
        assert_eq!(loader_ix_data, expected_ix_data);
    }
}
