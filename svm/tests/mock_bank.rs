#![allow(unused)]
#[allow(deprecated)]
use solana_sdk::sysvar::recent_blockhashes::{Entry as BlockhashesEntry, RecentBlockhashes};
use {
    solana_bpf_loader_program::syscalls::{
        SyscallAbort, SyscallGetClockSysvar, SyscallGetRentSysvar, SyscallInvokeSignedRust,
        SyscallLog, SyscallMemcpy, SyscallMemset, SyscallSetReturnData,
    },
    solana_compute_budget::compute_budget::ComputeBudget,
    solana_feature_set::FeatureSet,
    solana_program_runtime::{
        invoke_context::InvokeContext,
        loaded_programs::{BlockRelation, ForkGraph, ProgramCacheEntry},
        solana_rbpf::{
            program::{BuiltinFunction, BuiltinProgram, FunctionRegistry},
            vm::Config,
        },
    },
    solana_sdk::{
        account::{AccountSharedData, ReadableAccount, WritableAccount},
        bpf_loader_upgradeable::{self, UpgradeableLoaderState},
        clock::{Clock, UnixTimestamp},
        compute_budget, native_loader,
        pubkey::Pubkey,
        rent::Rent,
        slot_hashes::Slot,
        sysvar::SysvarId,
    },
    solana_svm::{
        transaction_processing_callback::{AccountState, TransactionProcessingCallback},
        transaction_processor::TransactionBatchProcessor,
    },
    solana_type_overrides::sync::{Arc, RwLock},
    std::{
        cmp::Ordering,
        collections::HashMap,
        env,
        fs::{self, File},
        io::Read,
    },
};

pub const EXECUTION_SLOT: u64 = 5; // The execution slot must be greater than the deployment slot
pub const EXECUTION_EPOCH: u64 = 2; // The execution epoch must be greater than the deployment epoch
pub const WALLCLOCK_TIME: i64 = 1704067200; // Arbitrarily Jan 1, 2024

pub struct MockForkGraph {}

impl ForkGraph for MockForkGraph {
    fn relationship(&self, a: Slot, b: Slot) -> BlockRelation {
        match a.cmp(&b) {
            Ordering::Less => BlockRelation::Ancestor,
            Ordering::Equal => BlockRelation::Equal,
            Ordering::Greater => BlockRelation::Descendant,
        }
    }
}

#[derive(Default, Clone)]
pub struct MockBankCallback {
    pub feature_set: Arc<FeatureSet>,
    pub account_shared_data: Arc<RwLock<HashMap<Pubkey, AccountSharedData>>>,
    #[allow(clippy::type_complexity)]
    pub inspected_accounts:
        Arc<RwLock<HashMap<Pubkey, Vec<(Option<AccountSharedData>, /* is_writable */ bool)>>>>,
}

impl TransactionProcessingCallback for MockBankCallback {
    fn account_matches_owners(&self, account: &Pubkey, owners: &[Pubkey]) -> Option<usize> {
        if let Some(data) = self.account_shared_data.read().unwrap().get(account) {
            if data.lamports() == 0 {
                None
            } else {
                owners.iter().position(|entry| data.owner() == entry)
            }
        } else {
            None
        }
    }

    fn get_account_shared_data(&self, pubkey: &Pubkey) -> Option<AccountSharedData> {
        self.account_shared_data
            .read()
            .unwrap()
            .get(pubkey)
            .cloned()
    }

    fn add_builtin_account(&self, name: &str, program_id: &Pubkey) {
        let account_data = native_loader::create_loadable_account_with_fields(name, (5000, 0));

        self.account_shared_data
            .write()
            .unwrap()
            .insert(*program_id, account_data);
    }

    fn inspect_account(&self, address: &Pubkey, account_state: AccountState, is_writable: bool) {
        let account = match account_state {
            AccountState::Dead => None,
            AccountState::Alive(account) => Some(account.clone()),
        };
        self.inspected_accounts
            .write()
            .unwrap()
            .entry(*address)
            .or_default()
            .push((account, is_writable));
    }
}

impl MockBankCallback {
    #[allow(unused)]
    pub fn override_feature_set(&mut self, new_set: FeatureSet) {
        self.feature_set = Arc::new(new_set)
    }

    pub fn configure_sysvars(&self) {
        // We must fill in the sysvar cache entries

        // clock contents are important because we use them for a sysvar loading test
        let clock = Clock {
            slot: EXECUTION_SLOT,
            epoch_start_timestamp: WALLCLOCK_TIME.saturating_sub(10) as UnixTimestamp,
            epoch: EXECUTION_EPOCH,
            leader_schedule_epoch: EXECUTION_EPOCH,
            unix_timestamp: WALLCLOCK_TIME as UnixTimestamp,
        };

        let mut account_data = AccountSharedData::default();
        account_data.set_data(bincode::serialize(&clock).unwrap());
        self.account_shared_data
            .write()
            .unwrap()
            .insert(Clock::id(), account_data);

        // default rent is fine
        let rent = Rent::default();

        let mut account_data = AccountSharedData::default();
        account_data.set_data(bincode::serialize(&rent).unwrap());
        self.account_shared_data
            .write()
            .unwrap()
            .insert(Rent::id(), account_data);

        // SystemInstruction::AdvanceNonceAccount asserts RecentBlockhashes is non-empty
        // but then just gets the blockhash from InvokeContext. so the sysvar doesnt need real entries
        #[allow(deprecated)]
        let recent_blockhashes = vec![BlockhashesEntry::default()];

        let mut account_data = AccountSharedData::default();
        account_data.set_data(bincode::serialize(&recent_blockhashes).unwrap());
        #[allow(deprecated)]
        self.account_shared_data
            .write()
            .unwrap()
            .insert(RecentBlockhashes::id(), account_data);
    }
}

fn load_program(name: String) -> Vec<u8> {
    // Loading the program file
    let mut dir = env::current_dir().unwrap();
    dir.push("tests");
    dir.push("example-programs");
    dir.push(name.as_str());
    let name = name.replace('-', "_");
    dir.push(name + "_program.so");
    let mut file = File::open(dir.clone()).expect("file not found");
    let metadata = fs::metadata(dir).expect("Unable to read metadata");
    let mut buffer = vec![0; metadata.len() as usize];
    file.read_exact(&mut buffer).expect("Buffer overflow");
    buffer
}

pub fn program_address(program_name: &str) -> Pubkey {
    Pubkey::create_with_seed(&Pubkey::default(), program_name, &Pubkey::default()).unwrap()
}

pub fn program_data_size(program_name: &str) -> usize {
    load_program(program_name.to_string()).len()
}

pub fn deploy_program(name: String, deployment_slot: Slot, mock_bank: &MockBankCallback) -> Pubkey {
    deploy_program_with_upgrade_authority(name, deployment_slot, mock_bank, None)
}

pub fn deploy_program_with_upgrade_authority(
    name: String,
    deployment_slot: Slot,
    mock_bank: &MockBankCallback,
    upgrade_authority_address: Option<Pubkey>,
) -> Pubkey {
    let rent = Rent::default();
    let program_account = program_address(&name);
    let program_data_account = bpf_loader_upgradeable::get_program_data_address(&program_account);

    let state = UpgradeableLoaderState::Program {
        programdata_address: program_data_account,
    };

    // The program account must have funds and hold the executable binary
    let mut account_data = AccountSharedData::default();
    let buffer = bincode::serialize(&state).unwrap();
    account_data.set_lamports(rent.minimum_balance(buffer.len()));
    account_data.set_owner(bpf_loader_upgradeable::id());
    account_data.set_executable(true);
    account_data.set_data(buffer);
    mock_bank
        .account_shared_data
        .write()
        .unwrap()
        .insert(program_account, account_data);

    let mut account_data = AccountSharedData::default();
    let state = UpgradeableLoaderState::ProgramData {
        slot: deployment_slot,
        upgrade_authority_address: None,
    };
    let mut header = bincode::serialize(&state).unwrap();
    let mut complement = vec![
        0;
        std::cmp::max(
            0,
            UpgradeableLoaderState::size_of_programdata_metadata().saturating_sub(header.len())
        )
    ];
    let mut buffer = load_program(name);
    header.append(&mut complement);
    header.append(&mut buffer);
    account_data.set_lamports(rent.minimum_balance(header.len()));
    account_data.set_owner(bpf_loader_upgradeable::id());
    account_data.set_data(header);
    mock_bank
        .account_shared_data
        .write()
        .unwrap()
        .insert(program_data_account, account_data);

    program_account
}

pub fn register_builtins(
    mock_bank: &MockBankCallback,
    batch_processor: &TransactionBatchProcessor<MockForkGraph>,
) {
    const DEPLOYMENT_SLOT: u64 = 0;
    // We must register the bpf loader account as a loadable account, otherwise programs
    // won't execute.
    let bpf_loader_name = "solana_bpf_loader_upgradeable_program";
    batch_processor.add_builtin(
        mock_bank,
        bpf_loader_upgradeable::id(),
        bpf_loader_name,
        ProgramCacheEntry::new_builtin(
            DEPLOYMENT_SLOT,
            bpf_loader_name.len(),
            solana_bpf_loader_program::Entrypoint::vm,
        ),
    );

    // In order to perform a transference of native tokens using the system instruction,
    // the system program builtin must be registered.
    let system_program_name = "system_program";
    batch_processor.add_builtin(
        mock_bank,
        solana_system_program::id(),
        system_program_name,
        ProgramCacheEntry::new_builtin(
            DEPLOYMENT_SLOT,
            system_program_name.len(),
            solana_system_program::system_processor::Entrypoint::vm,
        ),
    );

    // For testing realloc, we need the compute budget program
    let compute_budget_program_name = "compute_budget_program";
    batch_processor.add_builtin(
        mock_bank,
        compute_budget::id(),
        compute_budget_program_name,
        ProgramCacheEntry::new_builtin(
            DEPLOYMENT_SLOT,
            compute_budget_program_name.len(),
            solana_compute_budget_program::Entrypoint::vm,
        ),
    );
}

pub fn create_custom_loader<'a>() -> BuiltinProgram<InvokeContext<'a>> {
    let compute_budget = ComputeBudget::default();
    let vm_config = Config {
        max_call_depth: compute_budget.max_call_depth,
        stack_frame_size: compute_budget.stack_frame_size,
        enable_address_translation: true,
        enable_stack_frame_gaps: true,
        instruction_meter_checkpoint_distance: 10000,
        enable_instruction_meter: true,
        enable_instruction_tracing: true,
        enable_symbol_and_section_labels: true,
        reject_broken_elfs: true,
        noop_instruction_rate: 256,
        sanitize_user_provided_values: true,
        external_internal_function_hash_collision: false,
        reject_callx_r10: true,
        enable_sbpf_v1: true,
        enable_sbpf_v2: false,
        optimize_rodata: false,
        aligned_memory_mapping: true,
    };

    // These functions are system calls the compile contract calls during execution, so they
    // need to be registered.
    let mut function_registry = FunctionRegistry::<BuiltinFunction<InvokeContext>>::default();
    function_registry
        .register_function_hashed(*b"abort", SyscallAbort::vm)
        .expect("Registration failed");
    function_registry
        .register_function_hashed(*b"sol_log_", SyscallLog::vm)
        .expect("Registration failed");
    function_registry
        .register_function_hashed(*b"sol_memcpy_", SyscallMemcpy::vm)
        .expect("Registration failed");
    function_registry
        .register_function_hashed(*b"sol_memset_", SyscallMemset::vm)
        .expect("Registration failed");

    function_registry
        .register_function_hashed(*b"sol_invoke_signed_rust", SyscallInvokeSignedRust::vm)
        .expect("Registration failed");

    function_registry
        .register_function_hashed(*b"sol_set_return_data", SyscallSetReturnData::vm)
        .expect("Registration failed");

    function_registry
        .register_function_hashed(*b"sol_get_clock_sysvar", SyscallGetClockSysvar::vm)
        .expect("Registration failed");

    function_registry
        .register_function_hashed(*b"sol_get_rent_sysvar", SyscallGetRentSysvar::vm)
        .expect("Registration failed");

    BuiltinProgram::new_loader(vm_config, function_registry)
}
