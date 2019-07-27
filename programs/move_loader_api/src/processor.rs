use crate::account_state::{pubkey_to_address, LibraAccountState};
use crate::data_store::DataStore;
use crate::id;
use bytecode_verifier::{VerifiedModule, VerifiedScript};
use log::*;
use serde_derive::{Deserialize, Serialize};
use solana_sdk::{
    account::KeyedAccount, instruction::InstructionError, loader_instruction::LoaderInstruction,
    pubkey::Pubkey,
};
use types::{
    account_address::AccountAddress,
    transaction::{TransactionArgument, TransactionOutput},
};
use vm::{
    access::ModuleAccess,
    file_format::{CompiledModule, CompiledScript},
    gas_schedule::{MAXIMUM_NUMBER_OF_GAS_UNITS, MAX_PRICE_PER_GAS_UNIT},
    transaction_metadata::TransactionMetadata,
};
use vm_cache_map::Arena;
use vm_runtime::{
    code_cache::{
        module_adapter::ModuleFetcherImpl,
        module_cache::{BlockModuleCache, ModuleCache, VMModuleCache},
    },
    txn_executor::TransactionExecutor,
    value::Local,
};

pub fn process_instruction(
    _program_id: &Pubkey,
    keyed_accounts: &mut [KeyedAccount],
    data: &[u8],
) -> Result<(), InstructionError> {
    solana_logger::setup();

    let command = bincode::deserialize::<LoaderInstruction>(data).map_err(|err| {
        info!("Invalid instruction: {:?} {:?}", data, err);
        InstructionError::InvalidInstructionData
    })?;

    trace!("{:?}", command);

    match command {
        LoaderInstruction::Write { offset, bytes } => {
            MoveProcessor::do_write(keyed_accounts, offset, bytes)
        }
        LoaderInstruction::Finalize => MoveProcessor::do_finalize(keyed_accounts),
        LoaderInstruction::InvokeMain { data } => {
            MoveProcessor::do_invoke_main(keyed_accounts, data)
        }
    }
}

pub const PROGRAM_INDEX: usize = 0;
pub const GENESIS_INDEX: usize = 1;

// TODO: Not quite right yet
/// Invoke information passed via the Invoke Instruction
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct InvokeInfo {
    /// Sender of the "transaction", the "sender" who is calling this program
    pub sender_address: AccountAddress,
    /// Name of the function to call
    pub function_name: String,
    /// Arguments to pass to the program being invoked
    pub args: Vec<TransactionArgument>,
}

pub struct MoveProcessor {}

impl MoveProcessor {
    #[allow(clippy::needless_pass_by_value)]
    fn map_vm_runtime_error(err: vm::errors::VMRuntimeError) -> InstructionError {
        debug!("Execution failed: {:?}", err);
        match err.err {
            vm::errors::VMErrorKind::OutOfGasError => InstructionError::InsufficientFunds,
            _ => InstructionError::GenericError,
        }
    }
    fn map_vm_invariant_violation_error(err: vm::errors::VMInvariantViolation) -> InstructionError {
        debug!("Error: Execution failed: {:?}", err);
        InstructionError::GenericError
    }
    fn map_vm_binary_error(err: vm::errors::BinaryError) -> InstructionError {
        debug!("Error: Script deserialize failed: {:?}", err);
        InstructionError::InvalidInstructionData
    }
    #[allow(clippy::needless_pass_by_value)]
    fn map_data_error(err: std::boxed::Box<bincode::ErrorKind>) -> InstructionError {
        debug!("Error: Account data: {:?}", err);
        InstructionError::InvalidAccountData
    }
    fn map_vm_verification_error(
        err: (CompiledModule, Vec<vm::errors::VerificationError>),
    ) -> InstructionError {
        debug!("Error: Script verification failed: {:?}", err.1);
        InstructionError::InvalidInstructionData
    }
    fn map_failure_error(err: failure::Error) -> InstructionError {
        debug!("Error: Script verification failed: {:?}", err);
        InstructionError::InvalidInstructionData
    }
    #[allow(clippy::needless_pass_by_value)]
    fn missing_account() -> InstructionError {
        debug!("Error: Missing account");
        InstructionError::InvalidAccountData
    }

    fn arguments_to_locals(args: Vec<TransactionArgument>) -> Vec<Local> {
        let mut locals = vec![];
        for arg in args.into_iter() {
            locals.push(match arg {
                TransactionArgument::U64(i) => Local::u64(i),
                TransactionArgument::Address(a) => Local::address(a),
                TransactionArgument::ByteArray(b) => Local::bytearray(b),
                TransactionArgument::String(s) => Local::string(s),
            });
        }
        locals
    }

    fn serialize_verified_program(
        script: &VerifiedScript,
        modules: &[VerifiedModule],
    ) -> Result<(Vec<u8>), InstructionError> {
        let mut script_bytes = vec![];
        script
            .as_inner()
            .serialize(&mut script_bytes)
            .map_err(Self::map_failure_error)?;
        let mut modules_bytes = vec![];
        for module in modules.iter() {
            let mut buf = vec![];
            module
                .as_inner()
                .serialize(&mut buf)
                .map_err(Self::map_failure_error)?;
            modules_bytes.push(buf);
        }
        bincode::serialize(&LibraAccountState::VerifiedProgram {
            script_bytes,
            modules_bytes,
        })
        .map_err(Self::map_data_error)
    }

    fn deserialize_compiled_program(
        data: &[u8],
    ) -> Result<(CompiledScript, Vec<CompiledModule>), InstructionError> {
        let (script_bytes, modules_bytes) =
            match bincode::deserialize(data).map_err(Self::map_data_error)? {
                LibraAccountState::CompiledProgram {
                    script_bytes,
                    modules_bytes,
                } => (script_bytes, modules_bytes),
                _ => {
                    debug!("Error: Program account does not contain a program");
                    return Err(InstructionError::InvalidArgument);
                }
            };

        let script =
            CompiledScript::deserialize(&script_bytes).map_err(Self::map_vm_binary_error)?;
        let modules = modules_bytes
            .iter()
            .map(|bytes| CompiledModule::deserialize(&bytes))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Self::map_vm_binary_error)?;

        Ok((script, modules))
    }

    fn deserialize_verified_program(
        data: &[u8],
    ) -> Result<(VerifiedScript, Vec<VerifiedModule>), InstructionError> {
        let (script_bytes, modules_bytes) =
            match bincode::deserialize(data).map_err(Self::map_data_error)? {
                LibraAccountState::VerifiedProgram {
                    script_bytes,
                    modules_bytes,
                } => (script_bytes, modules_bytes),
                _ => {
                    debug!("Error: Program account does not contain a program");
                    return Err(InstructionError::InvalidArgument);
                }
            };

        let script =
            VerifiedScript::deserialize(&script_bytes).map_err(Self::map_vm_binary_error)?;
        let modules = modules_bytes
            .iter()
            .map(|bytes| VerifiedModule::deserialize(&bytes))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Self::map_vm_binary_error)?;

        Ok((script, modules))
    }

    fn execute(
        invoke_info: InvokeInfo,
        script: VerifiedScript,
        modules: Vec<VerifiedModule>,
        data_store: &DataStore,
    ) -> Result<TransactionOutput, InstructionError> {
        let allocator = Arena::new();
        let code_cache = VMModuleCache::new(&allocator);
        let module_cache = BlockModuleCache::new(&code_cache, ModuleFetcherImpl::new(data_store));
        let mut modules_to_publish = vec![];

        let main_module = script.into_module();
        let module_id = main_module.self_id();
        module_cache.cache_module(main_module);
        for verified_module in modules {
            let mut raw_bytes = vec![];
            verified_module
                .as_inner()
                .serialize(&mut raw_bytes)
                .expect("Unable to serialize module"); // TODO remove expect
            modules_to_publish.push((verified_module.self_id(), raw_bytes));
            module_cache.cache_module(verified_module);
        }

        let mut txn_metadata = TransactionMetadata::default();
        txn_metadata.sender = invoke_info.sender_address;
        // Caps execution to the Libra prescribed 10 milliseconds
        txn_metadata.max_gas_amount = *MAXIMUM_NUMBER_OF_GAS_UNITS;
        txn_metadata.gas_unit_price = *MAX_PRICE_PER_GAS_UNIT;

        let mut vm = TransactionExecutor::new(&module_cache, data_store, txn_metadata);
        vm.execute_function(
            &module_id,
            &invoke_info.function_name,
            Self::arguments_to_locals(invoke_info.args),
        )
        .map_err(Self::map_vm_invariant_violation_error)?
        .map_err(Self::map_vm_runtime_error)?;

        Ok(vm
            .make_write_set(modules_to_publish, Ok(Ok(())))
            .map_err(Self::map_vm_runtime_error)?)
    }

    fn keyed_accounts_to_data_store(
        keyed_accounts: &[KeyedAccount],
    ) -> Result<DataStore, InstructionError> {
        let mut data_store = DataStore::default();
        for keyed_account in keyed_accounts {
            match bincode::deserialize(&keyed_account.account.data).map_err(Self::map_data_error)? {
                LibraAccountState::Genesis(write_set) | LibraAccountState::User(write_set) => {
                    data_store.apply_write_set(&write_set)
                }
                _ => (), // ignore unallocated accounts
            }
        }
        Ok(data_store)
    }

    pub fn do_write(
        keyed_accounts: &mut [KeyedAccount],
        offset: u32,
        bytes: Vec<u8>,
    ) -> Result<(), InstructionError> {
        if keyed_accounts[PROGRAM_INDEX].signer_key().is_none() {
            debug!("Error: key[0] did not sign the transaction");
            return Err(InstructionError::GenericError);
        }
        let offset = offset as usize;
        let len = bytes.len();
        trace!("Write: offset={} length={}", offset, len);
        if keyed_accounts[PROGRAM_INDEX].account.data.len() < offset + len {
            debug!(
                "Error: Write overflow: {} < {}",
                keyed_accounts[PROGRAM_INDEX].account.data.len(),
                offset + len
            );
            return Err(InstructionError::GenericError);
        }
        keyed_accounts[PROGRAM_INDEX].account.data[offset..offset + len].copy_from_slice(&bytes);
        Ok(())
    }

    pub fn do_finalize(keyed_accounts: &mut [KeyedAccount]) -> Result<(), InstructionError> {
        if keyed_accounts[PROGRAM_INDEX].signer_key().is_none() {
            debug!("Error: key[0] did not sign the transaction");
            return Err(InstructionError::GenericError);
        }

        let (compiled_script, compiled_modules) =
            Self::deserialize_compiled_program(&keyed_accounts[PROGRAM_INDEX].account.data)?;

        let verified_script = VerifiedScript::new(compiled_script).unwrap();
        let verified_modules = compiled_modules
            .into_iter()
            .map(VerifiedModule::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(Self::map_vm_verification_error)?;

        keyed_accounts[PROGRAM_INDEX].account.data =
            Self::serialize_verified_program(&verified_script, &verified_modules)?;
        keyed_accounts[PROGRAM_INDEX].account.executable = true;

        info!(
            "Finalize: {:?}",
            keyed_accounts[PROGRAM_INDEX]
                .signer_key()
                .unwrap_or(&Pubkey::default())
        );
        Ok(())
    }

    pub fn do_invoke_main(
        keyed_accounts: &mut [KeyedAccount],
        data: Vec<u8>,
    ) -> Result<(), InstructionError> {
        if keyed_accounts.len() < 2 {
            debug!("Error: Requires at least a program and a genesis accounts");
            return Err(InstructionError::InvalidArgument);
        }
        if keyed_accounts[PROGRAM_INDEX].account.owner != id() {
            debug!("Error: Move program account not owned by Move loader");
            return Err(InstructionError::InvalidArgument);
        }
        if !keyed_accounts[PROGRAM_INDEX].account.executable {
            debug!("Error: Move program account not executable");
            return Err(InstructionError::InvalidArgument);
        }

        let invoke_info: InvokeInfo = bincode::deserialize(&data).map_err(Self::map_data_error)?;
        let mut data_store = Self::keyed_accounts_to_data_store(&keyed_accounts[GENESIS_INDEX..])?;
        let (verified_script, verified_modules) =
            Self::deserialize_verified_program(&keyed_accounts[PROGRAM_INDEX].account.data)?;

        let output = Self::execute(invoke_info, verified_script, verified_modules, &data_store)?;
        for event in output.events() {
            trace!("Event: {:?}", event);
        }

        data_store.apply_write_set(&output.write_set());

        // Break data store into a list of address keyed WriteSets
        let mut write_sets = data_store
            .into_write_sets()
            .map_err(|_| InstructionError::GenericError)?;

        // Genesis account holds both mint and stdlib under address 0x0
        let write_set = write_sets
            .remove(&AccountAddress::default())
            .ok_or_else(Self::missing_account)?;
        keyed_accounts[GENESIS_INDEX].account.data.clear();
        let writer = std::io::BufWriter::new(&mut keyed_accounts[GENESIS_INDEX].account.data);
        bincode::serialize_into(writer, &LibraAccountState::Genesis(write_set))
            .map_err(Self::map_data_error)?;

        // Now do the rest of the accounts
        for keyed_account in keyed_accounts[GENESIS_INDEX + 1..].iter_mut() {
            let write_set = write_sets
                .remove(&pubkey_to_address(keyed_account.unsigned_key()))
                .ok_or_else(Self::missing_account)?;
            keyed_account.account.data.clear();
            let writer = std::io::BufWriter::new(&mut keyed_account.account.data);
            bincode::serialize_into(writer, &LibraAccountState::User(write_set))
                .map_err(Self::map_data_error)?;
        }
        if !write_sets.is_empty() {
            debug!("Error: Missing keyed accounts");
            return Err(InstructionError::GenericError);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use language_e2e_tests::account::AccountResource;
    use solana_sdk::account::Account;

    #[test]
    fn test_finalize() {
        solana_logger::setup();

        let code = "main() { return; }";
        let sender_address = AccountAddress::default();
        let mut program = LibraAccount::create_program(&sender_address, code, vec![]);
        let mut keyed_accounts = vec![KeyedAccount::new(&program.key, true, &mut program.account)];
        MoveProcessor::do_finalize(&mut keyed_accounts).unwrap();
        let (_, _) = MoveProcessor::deserialize_verified_program(&program.account.data).unwrap();
    }

    #[test]
    fn test_invoke_main() {
        solana_logger::setup();

        let code = "main() { return; }";
        let sender_address = AccountAddress::default();
        let mut program = LibraAccount::create_program(&sender_address, code, vec![]);
        let mut genesis = LibraAccount::create_genesis();

        let mut keyed_accounts = vec![
            KeyedAccount::new(&program.key, true, &mut program.account),
            KeyedAccount::new(&genesis.key, false, &mut genesis.account),
        ];
        MoveProcessor::do_finalize(&mut keyed_accounts).unwrap();
        let invoke_info = InvokeInfo {
            sender_address,
            function_name: "main".to_string(),
            args: vec![],
        };
        MoveProcessor::do_invoke_main(
            &mut keyed_accounts,
            bincode::serialize(&invoke_info).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn test_invoke_endless_loop() {
        solana_logger::setup();

        let code = "
            main() {
                loop {}
                return;
            }
        ";
        let sender_address = AccountAddress::default();
        let mut program = LibraAccount::create_program(&sender_address, code, vec![]);
        let mut genesis = LibraAccount::create_genesis();

        let mut keyed_accounts = vec![
            KeyedAccount::new(&program.key, true, &mut program.account),
            KeyedAccount::new(&genesis.key, false, &mut genesis.account),
        ];
        MoveProcessor::do_finalize(&mut keyed_accounts).unwrap();

        let invoke_info = InvokeInfo {
            sender_address,
            function_name: "main".to_string(),
            args: vec![],
        };
        assert_eq!(
            MoveProcessor::do_invoke_main(
                &mut keyed_accounts,
                bincode::serialize(&invoke_info).unwrap(),
            ),
            Err(InstructionError::InsufficientFunds)
        );
    }

    #[test]
    fn test_invoke_mint_to_address() {
        solana_logger::setup();

        let amount = 42;
        let accounts = mint_coins(amount).unwrap();

        let mut data_store = DataStore::default();
        match bincode::deserialize(&accounts[GENESIS_INDEX + 1].account.data).unwrap() {
            LibraAccountState::User(write_set) => data_store.apply_write_set(&write_set),
            _ => panic!("Invalid account state"),
        }
        let payee_resource = data_store
            .read_account_resource(&accounts[GENESIS_INDEX + 1].address)
            .unwrap();

        assert_eq!(amount, AccountResource::read_balance(&payee_resource));
        assert_eq!(0, AccountResource::read_sequence_number(&payee_resource));
    }

    #[test]
    fn test_invoke_pay_from_sender() {
        solana_logger::setup();
        let amount_to_mint = 42;
        let mut accounts = mint_coins(amount_to_mint).unwrap();

        let code = "
            import 0x0.LibraAccount;
            import 0x0.LibraCoin;
            main(payee: address, amount: u64) {
                LibraAccount.pay_from_sender(move(payee), move(amount));
                return;
            }
        ";
        let mut program =
            LibraAccount::create_program(&accounts[GENESIS_INDEX + 1].address, code, vec![]);
        let mut payee = LibraAccount::create_unallocated();

        let (genesis, sender) = accounts.split_at_mut(GENESIS_INDEX + 1);
        let genesis = &mut genesis[1];
        let sender = &mut sender[0];
        let mut keyed_accounts = vec![
            KeyedAccount::new(&program.key, true, &mut program.account),
            KeyedAccount::new(&genesis.key, false, &mut genesis.account),
            KeyedAccount::new(&sender.key, false, &mut sender.account),
            KeyedAccount::new(&payee.key, false, &mut payee.account),
        ];
        MoveProcessor::do_finalize(&mut keyed_accounts).unwrap();

        let amount = 2;
        let invoke_info = InvokeInfo {
            sender_address: sender.address.clone(),
            function_name: "main".to_string(),
            args: vec![
                TransactionArgument::Address(payee.address.clone()),
                TransactionArgument::U64(amount),
            ],
        };

        MoveProcessor::do_invoke_main(
            &mut keyed_accounts,
            bincode::serialize(&invoke_info).unwrap(),
        )
        .unwrap();

        let data_store = MoveProcessor::keyed_accounts_to_data_store(&keyed_accounts[1..]).unwrap();
        let sender_resource = data_store.read_account_resource(&sender.address).unwrap();
        let payee_resource = data_store.read_account_resource(&payee.address).unwrap();

        assert_eq!(
            amount_to_mint - amount,
            AccountResource::read_balance(&sender_resource)
        );
        assert_eq!(0, AccountResource::read_sequence_number(&sender_resource));
        assert_eq!(amount, AccountResource::read_balance(&payee_resource));
        assert_eq!(0, AccountResource::read_sequence_number(&payee_resource));
    }

    #[test]
    fn test_invoke_local_module() {
        solana_logger::setup();

        let code = "
            modules:

            module M {
                public universal_truth(): u64 {
                    return 42;
                }
            }

            script:

            import Transaction.M;
            main() {
                let x: u64;
                x = M.universal_truth();
                return;
            }
        ";
        let mut genesis = LibraAccount::create_genesis();
        let mut payee = LibraAccount::create_unallocated();
        let mut program = LibraAccount::create_program(&payee.address, code, vec![]);

        let mut keyed_accounts = vec![
            KeyedAccount::new(&program.key, true, &mut program.account),
            KeyedAccount::new(&genesis.key, false, &mut genesis.account),
            KeyedAccount::new(&payee.key, false, &mut payee.account),
        ];
        MoveProcessor::do_finalize(&mut keyed_accounts).unwrap();
        let invoke_info = InvokeInfo {
            sender_address: payee.address,
            function_name: "main".to_string(),
            args: vec![],
        };
        MoveProcessor::do_invoke_main(
            &mut keyed_accounts,
            bincode::serialize(&invoke_info).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn test_invoke_published_module() {
        solana_logger::setup();

        // First publish the module

        let code = "
            module M {
                public universal_truth(): u64 {
                    return 42;
                }
            }
        ";
        let mut module = LibraAccount::create_unallocated();
        let mut program = LibraAccount::create_program(&module.address, code, vec![]);
        let mut genesis = LibraAccount::create_genesis();

        let mut keyed_accounts = vec![
            KeyedAccount::new(&program.key, true, &mut program.account),
            KeyedAccount::new(&genesis.key, false, &mut genesis.account),
            KeyedAccount::new(&module.key, false, &mut module.account),
        ];
        MoveProcessor::do_finalize(&mut keyed_accounts).unwrap();
        let invoke_info = InvokeInfo {
            sender_address: module.address,
            function_name: "main".to_string(),
            args: vec![],
        };
        MoveProcessor::do_invoke_main(
            &mut keyed_accounts,
            bincode::serialize(&invoke_info).unwrap(),
        )
        .unwrap();

        // Next invoke the published module

        let code = format!(
            "
            import 0x{}.M;
            main() {{
                let x: u64;
                x = M.universal_truth();
                return;
            }}
            ",
            module.address
        );
        let mut program =
            LibraAccount::create_program(&module.address, &code, vec![&module.account.data]);

        let mut keyed_accounts = vec![
            KeyedAccount::new(&program.key, true, &mut program.account),
            KeyedAccount::new(&genesis.key, false, &mut genesis.account),
            KeyedAccount::new(&module.key, false, &mut module.account),
        ];
        MoveProcessor::do_finalize(&mut keyed_accounts).unwrap();
        let invoke_info = InvokeInfo {
            sender_address: program.address,
            function_name: "main".to_string(),
            args: vec![],
        };
        MoveProcessor::do_invoke_main(
            &mut keyed_accounts,
            bincode::serialize(&invoke_info).unwrap(),
        )
        .unwrap();
    }

    // Helpers

    fn mint_coins(amount: u64) -> Result<Vec<LibraAccount>, InstructionError> {
        let code = "
            import 0x0.LibraAccount;
            import 0x0.LibraCoin;
            main(payee: address, amount: u64) {
                LibraAccount.mint_to_address(move(payee), move(amount));
                return;
            }
        ";
        let mut genesis = LibraAccount::create_genesis();
        let mut program = LibraAccount::create_program(&genesis.address, code, vec![]);
        let mut payee = LibraAccount::create_unallocated();

        let mut keyed_accounts = vec![
            KeyedAccount::new(&program.key, true, &mut program.account),
            KeyedAccount::new(&genesis.key, false, &mut genesis.account),
            KeyedAccount::new(&payee.key, false, &mut payee.account),
        ];
        MoveProcessor::do_finalize(&mut keyed_accounts).unwrap();
        let invoke_info = InvokeInfo {
            sender_address: genesis.address.clone(),
            function_name: "main".to_string(),
            args: vec![
                TransactionArgument::Address(pubkey_to_address(&payee.key)),
                TransactionArgument::U64(amount),
            ],
        };

        MoveProcessor::do_invoke_main(
            &mut keyed_accounts,
            bincode::serialize(&invoke_info).unwrap(),
        )
        .unwrap();

        Ok(vec![
            LibraAccount::new(program.key, program.account),
            LibraAccount::new(genesis.key, genesis.account),
            LibraAccount::new(payee.key, payee.account),
        ])
    }

    struct LibraAccount {
        pub key: Pubkey,
        pub address: AccountAddress,
        pub account: Account,
    }
    impl LibraAccount {
        pub fn new(key: Pubkey, account: Account) -> Self {
            let address = pubkey_to_address(&key);
            Self {
                key,
                address,
                account,
            }
        }

        pub fn create_unallocated() -> Self {
            let key = Pubkey::new_rand();
            let account = Account {
                lamports: 1,
                data: bincode::serialize(&LibraAccountState::create_unallocated()).unwrap(),
                owner: id(),
                executable: false,
            };
            Self::new(key, account)
        }

        pub fn create_genesis() -> Self {
            let account = Account {
                lamports: 1,
                data: vec![],
                owner: id(),
                executable: false,
            };
            let mut genesis = Self::new(Pubkey::default(), account);
            genesis.account.data =
                bincode::serialize(&LibraAccountState::create_genesis(1_000_000_000)).unwrap();
            genesis
        }

        pub fn create_program(
            sender_address: &AccountAddress,
            code: &str,
            deps: Vec<&Vec<u8>>,
        ) -> Self {
            let mut program = Self::create_unallocated();
            program.account.data = bincode::serialize(&LibraAccountState::create_program(
                sender_address,
                code,
                deps,
            ))
            .unwrap();
            program.account.executable = true;
            program
        }
    }
}
