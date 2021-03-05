use crate::{
    instruction_recorder::InstructionRecorder, log_collector::LogCollector,
    native_loader::NativeLoader, rent_collector::RentCollector,
};
use log::*;
use serde::{Deserialize, Serialize};
use solana_sdk::{
    account::Account,
    account_utils::StateMut,
    bpf_loader_upgradeable::{self, UpgradeableLoaderState},
    feature_set::{instructions_sysvar_enabled, track_writable_deescalation, FeatureSet},
    ic_msg,
    instruction::{CompiledInstruction, Instruction, InstructionError},
    keyed_account::{create_keyed_readonly_accounts, KeyedAccount},
    message::Message,
    native_loader,
    process_instruction::{
        BpfComputeBudget, ComputeMeter, Executor, InvokeContext, Logger,
        ProcessInstructionWithContext,
    },
    pubkey::Pubkey,
    rent::Rent,
    system_program,
    transaction::TransactionError,
};
use std::{
    cell::{Ref, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
};

pub struct Executors {
    pub executors: HashMap<Pubkey, Arc<dyn Executor>>,
    pub is_dirty: bool,
}
impl Default for Executors {
    fn default() -> Self {
        Self {
            executors: HashMap::default(),
            is_dirty: false,
        }
    }
}
impl Executors {
    pub fn insert(&mut self, key: Pubkey, executor: Arc<dyn Executor>) {
        let _ = self.executors.insert(key, executor);
        self.is_dirty = true;
    }
    pub fn get(&self, key: &Pubkey) -> Option<Arc<dyn Executor>> {
        self.executors.get(key).cloned()
    }
}

#[derive(Default, Debug)]
pub struct ExecuteDetailsTimings {
    pub serialize_us: u64,
    pub create_vm_us: u64,
    pub execute_us: u64,
    pub deserialize_us: u64,
    pub changed_account_count: u64,
    pub total_account_count: u64,
    pub total_data_size: usize,
    pub data_size_changed: usize,
}

impl ExecuteDetailsTimings {
    pub fn accumulate(&mut self, other: &ExecuteDetailsTimings) {
        self.serialize_us += other.serialize_us;
        self.create_vm_us += other.create_vm_us;
        self.execute_us += other.execute_us;
        self.deserialize_us += other.deserialize_us;
        self.changed_account_count += other.changed_account_count;
        self.total_account_count += other.total_account_count;
        self.total_data_size += other.total_data_size;
        self.data_size_changed += other.data_size_changed;
    }
}

// The relevant state of an account before an Instruction executes, used
// to verify account integrity after the Instruction completes
#[derive(Clone, Debug, Default)]
pub struct PreAccount {
    key: Pubkey,
    is_writable: bool,
    account: RefCell<Account>,
    changed: bool,
}
impl PreAccount {
    pub fn new(key: &Pubkey, account: &Account, is_writable: bool) -> Self {
        Self {
            key: *key,
            is_writable,
            account: RefCell::new(account.clone()),
            changed: false,
        }
    }

    pub fn verify(
        &self,
        program_id: &Pubkey,
        is_writable: Option<bool>,
        rent: &Rent,
        post: &Account,
        timings: &mut ExecuteDetailsTimings,
    ) -> Result<(), InstructionError> {
        let pre = self.account.borrow();

        let is_writable = if let Some(is_writable) = is_writable {
            is_writable
        } else {
            self.is_writable
        };

        // Only the owner of the account may change owner and
        //   only if the account is writable and
        //   only if the account is not executable and
        //   only if the data is zero-initialized or empty
        let owner_changed = pre.owner != post.owner;
        if owner_changed
            && (!is_writable // line coverage used to get branch coverage
                || pre.executable
                || *program_id != pre.owner
            || !Self::is_zeroed(&post.data))
        {
            return Err(InstructionError::ModifiedProgramId);
        }

        // An account not assigned to the program cannot have its balance decrease.
        if *program_id != pre.owner // line coverage used to get branch coverage
         && pre.lamports > post.lamports
        {
            return Err(InstructionError::ExternalAccountLamportSpend);
        }

        // The balance of read-only and executable accounts may not change
        let lamports_changed = pre.lamports != post.lamports;
        if lamports_changed {
            if !is_writable {
                return Err(InstructionError::ReadonlyLamportChange);
            }
            if pre.executable {
                return Err(InstructionError::ExecutableLamportChange);
            }
        }

        // Only the system program can change the size of the data
        //  and only if the system program owns the account
        let data_len_changed = pre.data.len() != post.data.len();
        if data_len_changed
            && (!system_program::check_id(program_id) // line coverage used to get branch coverage
                || !system_program::check_id(&pre.owner))
        {
            return Err(InstructionError::AccountDataSizeChanged);
        }

        // Only the owner may change account data
        //   and if the account is writable
        //   and if the account is not executable
        if !(*program_id == pre.owner
            && is_writable  // line coverage used to get branch coverage
            && !pre.executable)
            && pre.data != post.data
        {
            if pre.executable {
                return Err(InstructionError::ExecutableDataModified);
            } else if is_writable {
                return Err(InstructionError::ExternalAccountDataModified);
            } else {
                return Err(InstructionError::ReadonlyDataModified);
            }
        }

        // executable is one-way (false->true) and only the account owner may set it.
        let executable_changed = pre.executable != post.executable;
        if executable_changed {
            if !rent.is_exempt(post.lamports, post.data.len()) {
                return Err(InstructionError::ExecutableAccountNotRentExempt);
            }
            if !is_writable // line coverage used to get branch coverage
                || pre.executable
                || *program_id != pre.owner
            {
                return Err(InstructionError::ExecutableModified);
            }
        }

        // No one modifies rent_epoch (yet).
        let rent_epoch_changed = pre.rent_epoch != post.rent_epoch;
        if rent_epoch_changed {
            return Err(InstructionError::RentEpochModified);
        }

        timings.total_account_count += 1;
        timings.total_data_size += post.data.len();
        if owner_changed
            || lamports_changed
            || data_len_changed
            || executable_changed
            || rent_epoch_changed
            || self.changed
        {
            timings.changed_account_count += 1;
            timings.data_size_changed += post.data.len();
        }

        Ok(())
    }

    pub fn update(&mut self, account: &Account) {
        let mut pre = self.account.borrow_mut();

        pre.lamports = account.lamports;
        pre.owner = account.owner;
        pre.executable = account.executable;
        if pre.data.len() != account.data.len() {
            // Only system account can change data size, copy with alloc
            pre.data = account.data.clone();
        } else {
            // Copy without allocate
            pre.data.clone_from_slice(&account.data);
        }

        self.changed = true;
    }

    pub fn key(&self) -> Pubkey {
        self.key
    }

    pub fn lamports(&self) -> u64 {
        self.account.borrow().lamports
    }

    pub fn is_zeroed(buf: &[u8]) -> bool {
        const ZEROS_LEN: usize = 1024;
        static ZEROS: [u8; ZEROS_LEN] = [0; ZEROS_LEN];
        let mut chunks = buf.chunks_exact(ZEROS_LEN);

        chunks.all(|chunk| chunk == &ZEROS[..])
            && chunks.remainder() == &ZEROS[..chunks.remainder().len()]
    }
}

pub struct ThisComputeMeter {
    remaining: u64,
}
impl ComputeMeter for ThisComputeMeter {
    fn consume(&mut self, amount: u64) -> Result<(), InstructionError> {
        let exceeded = self.remaining < amount;
        self.remaining = self.remaining.saturating_sub(amount);
        if exceeded {
            return Err(InstructionError::ComputationalBudgetExceeded);
        }
        Ok(())
    }
    fn get_remaining(&self) -> u64 {
        self.remaining
    }
}
pub struct ThisInvokeContext<'a> {
    program_ids: Vec<Pubkey>,
    rent: Rent,
    pre_accounts: Vec<PreAccount>,
    account_deps: &'a [(Pubkey, RefCell<Account>)],
    programs: &'a [(Pubkey, ProcessInstructionWithContext)],
    logger: Rc<RefCell<dyn Logger>>,
    bpf_compute_budget: BpfComputeBudget,
    compute_meter: Rc<RefCell<dyn ComputeMeter>>,
    executors: Rc<RefCell<Executors>>,
    instruction_recorder: Option<InstructionRecorder>,
    feature_set: Arc<FeatureSet>,
    pub timings: ExecuteDetailsTimings,
}
impl<'a> ThisInvokeContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        program_id: &Pubkey,
        rent: Rent,
        pre_accounts: Vec<PreAccount>,
        account_deps: &'a [(Pubkey, RefCell<Account>)],
        programs: &'a [(Pubkey, ProcessInstructionWithContext)],
        log_collector: Option<Rc<LogCollector>>,
        bpf_compute_budget: BpfComputeBudget,
        executors: Rc<RefCell<Executors>>,
        instruction_recorder: Option<InstructionRecorder>,
        feature_set: Arc<FeatureSet>,
    ) -> Self {
        let mut program_ids = Vec::with_capacity(bpf_compute_budget.max_invoke_depth);
        program_ids.push(*program_id);
        Self {
            program_ids,
            rent,
            pre_accounts,
            account_deps,
            programs,
            logger: Rc::new(RefCell::new(ThisLogger { log_collector })),
            bpf_compute_budget,
            compute_meter: Rc::new(RefCell::new(ThisComputeMeter {
                remaining: bpf_compute_budget.max_units,
            })),
            executors,
            instruction_recorder,
            feature_set,
            timings: ExecuteDetailsTimings::default(),
        }
    }
}
impl<'a> InvokeContext for ThisInvokeContext<'a> {
    fn push(&mut self, key: &Pubkey) -> Result<(), InstructionError> {
        if self.program_ids.len() > self.bpf_compute_budget.max_invoke_depth {
            return Err(InstructionError::CallDepth);
        }
        if self.program_ids.contains(key) && self.program_ids.last() != Some(key) {
            // Reentrancy not allowed unless caller is calling itself
            return Err(InstructionError::ReentrancyNotAllowed);
        }
        self.program_ids.push(*key);
        Ok(())
    }
    fn pop(&mut self) {
        self.program_ids.pop();
    }
    fn invoke_depth(&self) -> usize {
        self.program_ids.len()
    }
    fn verify_and_update(
        &mut self,
        message: &Message,
        instruction: &CompiledInstruction,
        accounts: &[Rc<RefCell<Account>>],
        caller_privileges: Option<&[bool]>,
    ) -> Result<(), InstructionError> {
        let track_writable_deescalation =
            self.is_feature_active(&track_writable_deescalation::id());
        match self.program_ids.last() {
            Some(program_id) => MessageProcessor::verify_and_update(
                message,
                instruction,
                &mut self.pre_accounts,
                accounts,
                program_id,
                &self.rent,
                track_writable_deescalation,
                caller_privileges,
                &mut self.timings,
            ),
            None => Err(InstructionError::GenericError), // Should never happen
        }
    }
    fn get_caller(&self) -> Result<&Pubkey, InstructionError> {
        self.program_ids
            .last()
            .ok_or(InstructionError::GenericError)
    }
    fn get_programs(&self) -> &[(Pubkey, ProcessInstructionWithContext)] {
        self.programs
    }
    fn get_logger(&self) -> Rc<RefCell<dyn Logger>> {
        self.logger.clone()
    }
    fn get_bpf_compute_budget(&self) -> &BpfComputeBudget {
        &self.bpf_compute_budget
    }
    fn get_compute_meter(&self) -> Rc<RefCell<dyn ComputeMeter>> {
        self.compute_meter.clone()
    }
    fn add_executor(&self, pubkey: &Pubkey, executor: Arc<dyn Executor>) {
        self.executors.borrow_mut().insert(*pubkey, executor);
    }
    fn get_executor(&self, pubkey: &Pubkey) -> Option<Arc<dyn Executor>> {
        self.executors.borrow().get(&pubkey)
    }
    fn record_instruction(&self, instruction: &Instruction) {
        if let Some(recorder) = &self.instruction_recorder {
            recorder.record_instruction(instruction.clone());
        }
    }
    fn is_feature_active(&self, feature_id: &Pubkey) -> bool {
        self.feature_set.is_active(feature_id)
    }
    fn get_account(&self, pubkey: &Pubkey) -> Option<RefCell<Account>> {
        if let Some(account) = self.pre_accounts.iter().find_map(|pre| {
            if pre.key == *pubkey {
                Some(pre.account.clone())
            } else {
                None
            }
        }) {
            return Some(account);
        }
        self.account_deps.iter().find_map(|(key, account)| {
            if key == pubkey {
                Some(account.clone())
            } else {
                None
            }
        })
    }
    fn update_timing(
        &mut self,
        serialize_us: u64,
        create_vm_us: u64,
        execute_us: u64,
        deserialize_us: u64,
    ) {
        self.timings.serialize_us += serialize_us;
        self.timings.create_vm_us += create_vm_us;
        self.timings.execute_us += execute_us;
        self.timings.deserialize_us += deserialize_us;
    }
}
pub struct ThisLogger {
    log_collector: Option<Rc<LogCollector>>,
}
impl Logger for ThisLogger {
    fn log_enabled(&self) -> bool {
        log_enabled!(log::Level::Info) || self.log_collector.is_some()
    }
    fn log(&self, message: &str) {
        debug!("{}", message);
        if let Some(log_collector) = &self.log_collector {
            log_collector.log(message);
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct MessageProcessor {
    #[serde(skip)]
    programs: Vec<(Pubkey, ProcessInstructionWithContext)>,
    #[serde(skip)]
    native_loader: NativeLoader,
}

impl std::fmt::Debug for MessageProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        #[derive(Debug)]
        struct MessageProcessor<'a> {
            programs: Vec<String>,
            native_loader: &'a NativeLoader,
        }

        // These are just type aliases for work around of Debug-ing above pointers
        type ErasedProcessInstructionWithContext = fn(
            &'static Pubkey,
            &'static [KeyedAccount<'static>],
            &'static [u8],
            &'static mut dyn InvokeContext,
        ) -> Result<(), InstructionError>;

        // rustc doesn't compile due to bug without this work around
        // https://github.com/rust-lang/rust/issues/50280
        // https://users.rust-lang.org/t/display-function-pointer/17073/2
        let processor = MessageProcessor {
            programs: self
                .programs
                .iter()
                .map(|(pubkey, instruction)| {
                    let erased_instruction: ErasedProcessInstructionWithContext = *instruction;
                    format!("{}: {:p}", pubkey, erased_instruction)
                })
                .collect::<Vec<_>>(),
            native_loader: &self.native_loader,
        };

        write!(f, "{:?}", processor)
    }
}

impl Default for MessageProcessor {
    fn default() -> Self {
        Self {
            programs: vec![],
            native_loader: NativeLoader::default(),
        }
    }
}
impl Clone for MessageProcessor {
    fn clone(&self) -> Self {
        MessageProcessor {
            programs: self.programs.clone(),
            native_loader: NativeLoader::default(),
        }
    }
}

#[cfg(RUSTC_WITH_SPECIALIZATION)]
impl ::solana_frozen_abi::abi_example::AbiExample for MessageProcessor {
    fn example() -> Self {
        // MessageProcessor's fields are #[serde(skip)]-ed and not Serialize
        // so, just rely on Default anyway.
        MessageProcessor::default()
    }
}

impl MessageProcessor {
    /// Add a static entrypoint to intercept instructions before the dynamic loader.
    pub fn add_program(
        &mut self,
        program_id: Pubkey,
        process_instruction: ProcessInstructionWithContext,
    ) {
        match self.programs.iter_mut().find(|(key, _)| program_id == *key) {
            Some((_, processor)) => *processor = process_instruction,
            None => self.programs.push((program_id, process_instruction)),
        }
    }

    pub fn add_loader(
        &mut self,
        program_id: Pubkey,
        process_instruction: ProcessInstructionWithContext,
    ) {
        self.add_program(program_id, process_instruction);
    }

    /// Create the KeyedAccounts that will be passed to the program
    fn create_keyed_accounts<'a>(
        message: &'a Message,
        instruction: &'a CompiledInstruction,
        executable_accounts: &'a [(Pubkey, RefCell<Account>)],
        accounts: &'a [Rc<RefCell<Account>>],
    ) -> Vec<KeyedAccount<'a>> {
        let mut keyed_accounts = create_keyed_readonly_accounts(&executable_accounts);
        let mut keyed_accounts2: Vec<_> = instruction
            .accounts
            .iter()
            .map(|&index| {
                let is_signer = message.is_signer(index as usize);
                let index = index as usize;
                let key = &message.account_keys[index];
                let account = &accounts[index];
                if message.is_writable(index) {
                    KeyedAccount::new(key, is_signer, account)
                } else {
                    KeyedAccount::new_readonly(key, is_signer, account)
                }
            })
            .collect();
        keyed_accounts.append(&mut keyed_accounts2);
        keyed_accounts
    }

    /// Process an instruction
    /// This method calls the instruction's program entrypoint method
    fn process_instruction(
        &self,
        program_id: &Pubkey,
        keyed_accounts: &[KeyedAccount],
        instruction_data: &[u8],
        invoke_context: &mut dyn InvokeContext,
    ) -> Result<(), InstructionError> {
        if let Some(root_account) = keyed_accounts.iter().next() {
            let root_id = root_account.unsigned_key();
            if native_loader::check_id(&root_account.owner()?) {
                for (id, process_instruction) in &self.programs {
                    if id == root_id {
                        // Call the builtin program
                        return process_instruction(
                            &program_id,
                            &keyed_accounts[1..],
                            instruction_data,
                            invoke_context,
                        );
                    }
                }
                // Call the program via the native loader
                return self.native_loader.process_instruction(
                    &native_loader::id(),
                    keyed_accounts,
                    instruction_data,
                    invoke_context,
                );
            } else {
                let owner_id = &root_account.owner()?;
                for (id, process_instruction) in &self.programs {
                    if id == owner_id {
                        // Call the program via a builtin loader
                        return process_instruction(
                            &program_id,
                            keyed_accounts,
                            instruction_data,
                            invoke_context,
                        );
                    }
                }
            }
        }
        Err(InstructionError::UnsupportedProgramId)
    }

    pub fn create_message(
        instruction: &Instruction,
        keyed_accounts: &[&KeyedAccount],
        signers: &[Pubkey],
        invoke_context: &Ref<&mut dyn InvokeContext>,
    ) -> Result<(Message, Pubkey, usize), InstructionError> {
        // Check for privilege escalation
        for account in instruction.accounts.iter() {
            let keyed_account = keyed_accounts
                .iter()
                .find_map(|keyed_account| {
                    if &account.pubkey == keyed_account.unsigned_key() {
                        Some(keyed_account)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    ic_msg!(
                        invoke_context,
                        "Instruction references an unknown account {}",
                        account.pubkey
                    );
                    InstructionError::MissingAccount
                })?;
            // Readonly account cannot become writable
            if account.is_writable && !keyed_account.is_writable() {
                ic_msg!(
                    invoke_context,
                    "{}'s writable privilege escalated",
                    account.pubkey
                );
                return Err(InstructionError::PrivilegeEscalation);
            }

            if account.is_signer && // If message indicates account is signed
            !( // one of the following needs to be true:
                keyed_account.signer_key().is_some() // Signed in the parent instruction
                || signers.contains(&account.pubkey) // Signed by the program
            ) {
                ic_msg!(
                    invoke_context,
                    "{}'s signer privilege escalated",
                    account.pubkey
                );
                return Err(InstructionError::PrivilegeEscalation);
            }
        }

        // validate the caller has access to the program account and that it is executable
        let program_id = instruction.program_id;
        match keyed_accounts
            .iter()
            .find(|keyed_account| &program_id == keyed_account.unsigned_key())
        {
            Some(keyed_account) => {
                if !keyed_account.executable()? {
                    ic_msg!(
                        invoke_context,
                        "Account {} is not executable",
                        keyed_account.unsigned_key()
                    );
                    return Err(InstructionError::AccountNotExecutable);
                }
            }
            None => {
                ic_msg!(invoke_context, "Unknown program {}", program_id);
                return Err(InstructionError::MissingAccount);
            }
        }

        let message = Message::new(&[instruction.clone()], None);
        let program_id_index = message.instructions[0].program_id_index as usize;

        Ok((message, program_id, program_id_index))
    }

    /// Entrypoint for a cross-program invocation from a native program
    pub fn native_invoke(
        invoke_context: &mut dyn InvokeContext,
        instruction: Instruction,
        keyed_accounts: &[&KeyedAccount],
        signers_seeds: &[&[&[u8]]],
    ) -> Result<(), InstructionError> {
        let invoke_context = RefCell::new(invoke_context);

        let (message, executables, accounts, account_refs, caller_privileges) = {
            let invoke_context = invoke_context.borrow();

            let caller_program_id = invoke_context.get_caller()?;

            // Translate and verify caller's data

            let signers = signers_seeds
                .iter()
                .map(|seeds| Pubkey::create_program_address(&seeds, caller_program_id))
                .collect::<Result<Vec<_>, solana_sdk::pubkey::PubkeyError>>()?;
            let mut caller_privileges = keyed_accounts
                .iter()
                .map(|keyed_account| keyed_account.is_writable())
                .collect::<Vec<bool>>();
            caller_privileges.insert(0, false);
            let (message, callee_program_id, _) =
                Self::create_message(&instruction, &keyed_accounts, &signers, &invoke_context)?;
            let mut accounts = vec![];
            let mut account_refs = vec![];
            'root: for account_key in message.account_keys.iter() {
                for keyed_account in keyed_accounts {
                    if account_key == keyed_account.unsigned_key() {
                        accounts.push(Rc::new(keyed_account.account.clone()));
                        account_refs.push(keyed_account);
                        continue 'root;
                    }
                }
                ic_msg!(
                    invoke_context,
                    "Instruction references an unknown account {}",
                    account_key
                );
                return Err(InstructionError::MissingAccount);
            }

            // Process instruction

            invoke_context.record_instruction(&instruction);

            let program_account =
                invoke_context
                    .get_account(&callee_program_id)
                    .ok_or_else(|| {
                        ic_msg!(invoke_context, "Unknown program {}", callee_program_id);
                        InstructionError::MissingAccount
                    })?;
            if !program_account.borrow().executable {
                ic_msg!(
                    invoke_context,
                    "Account {} is not executable",
                    callee_program_id
                );
                return Err(InstructionError::AccountNotExecutable);
            }
            let programdata_executable =
                if program_account.borrow().owner == bpf_loader_upgradeable::id() {
                    if let UpgradeableLoaderState::Program {
                        programdata_address,
                    } = program_account.borrow().state()?
                    {
                        if let Some(account) = invoke_context.get_account(&programdata_address) {
                            Some((programdata_address, account))
                        } else {
                            ic_msg!(
                                invoke_context,
                                "Unknown upgradeable programdata account {}",
                                programdata_address,
                            );
                            return Err(InstructionError::MissingAccount);
                        }
                    } else {
                        ic_msg!(
                            invoke_context,
                            "Upgradeable program account state not valid {}",
                            callee_program_id,
                        );
                        return Err(InstructionError::MissingAccount);
                    }
                } else {
                    None
                };
            let mut executables = vec![(callee_program_id, program_account)];
            if let Some(programdata) = programdata_executable {
                executables.push(programdata);
            }
            (
                message,
                executables,
                accounts,
                account_refs,
                caller_privileges,
            )
        };

        #[allow(clippy::deref_addrof)]
        MessageProcessor::process_cross_program_instruction(
            &message,
            &executables,
            &accounts,
            &caller_privileges,
            *(&mut *(invoke_context.borrow_mut())),
        )?;

        // Copy results back to caller

        {
            let invoke_context = invoke_context.borrow();
            for (i, (account, account_ref)) in accounts.iter().zip(account_refs).enumerate() {
                let account = account.borrow();
                if message.is_writable(i) && !account.executable {
                    account_ref.try_account_ref_mut()?.lamports = account.lamports;
                    account_ref.try_account_ref_mut()?.owner = account.owner;
                    if account_ref.data_len()? != account.data.len() && account_ref.data_len()? != 0
                    {
                        // Only support for `CreateAccount` at this time.
                        // Need a way to limit total realloc size across multiple CPI calls
                        ic_msg!(
                            invoke_context,
                            "Inner instructions do not support realloc, only SystemProgram::CreateAccount",
                        );
                        return Err(InstructionError::InvalidRealloc);
                    }
                    account_ref.try_account_ref_mut()?.data = account.data.clone();
                }
            }
        }

        Ok(())
    }

    /// Process a cross-program instruction
    /// This method calls the instruction's program entrypoint function
    pub fn process_cross_program_instruction(
        message: &Message,
        executable_accounts: &[(Pubkey, RefCell<Account>)],
        accounts: &[Rc<RefCell<Account>>],
        caller_privileges: &[bool],
        invoke_context: &mut dyn InvokeContext,
    ) -> Result<(), InstructionError> {
        if let Some(instruction) = message.instructions.get(0) {
            let program_id = instruction.program_id(&message.account_keys);

            // Verify the calling program hasn't misbehaved
            invoke_context.verify_and_update(
                message,
                instruction,
                accounts,
                Some(caller_privileges),
            )?;

            // Construct keyed accounts
            let keyed_accounts =
                Self::create_keyed_accounts(message, instruction, executable_accounts, accounts);

            // Invoke callee
            invoke_context.push(program_id)?;

            let mut message_processor = MessageProcessor::default();
            for (program_id, process_instruction) in invoke_context.get_programs().iter() {
                message_processor.add_program(*program_id, *process_instruction);
            }

            let mut result = message_processor.process_instruction(
                program_id,
                &keyed_accounts,
                &instruction.data,
                invoke_context,
            );
            if result.is_ok() {
                // Verify the called program has not misbehaved
                result = invoke_context.verify_and_update(message, instruction, accounts, None);
            }
            invoke_context.pop();

            result
        } else {
            // This function is always called with a valid instruction, if that changes return an error
            Err(InstructionError::GenericError)
        }
    }

    /// Record the initial state of the accounts so that they can be compared
    /// after the instruction is processed
    pub fn create_pre_accounts(
        message: &Message,
        instruction: &CompiledInstruction,
        accounts: &[Rc<RefCell<Account>>],
    ) -> Vec<PreAccount> {
        let mut pre_accounts = Vec::with_capacity(instruction.accounts.len());
        {
            let mut work = |_unique_index: usize, account_index: usize| {
                let key = &message.account_keys[account_index];
                let is_writable = message.is_writable(account_index);
                let account = accounts[account_index].borrow();
                pre_accounts.push(PreAccount::new(key, &account, is_writable));
                Ok(())
            };
            let _ = instruction.visit_each_account(&mut work);
        }
        pre_accounts
    }

    /// Verify there are no outstanding borrows
    pub fn verify_account_references(
        accounts: &[(Pubkey, RefCell<Account>)],
    ) -> Result<(), InstructionError> {
        for (_, account) in accounts.iter() {
            account
                .try_borrow_mut()
                .map_err(|_| InstructionError::AccountBorrowOutstanding)?;
        }
        Ok(())
    }

    /// Verify the results of an instruction
    pub fn verify(
        message: &Message,
        instruction: &CompiledInstruction,
        pre_accounts: &[PreAccount],
        executable_accounts: &[(Pubkey, RefCell<Account>)],
        accounts: &[Rc<RefCell<Account>>],
        rent: &Rent,
        timings: &mut ExecuteDetailsTimings,
    ) -> Result<(), InstructionError> {
        // Verify all executable accounts have zero outstanding refs
        Self::verify_account_references(executable_accounts)?;

        // Verify the per-account instruction results
        let (mut pre_sum, mut post_sum) = (0_u128, 0_u128);
        {
            let program_id = instruction.program_id(&message.account_keys);
            let mut work = |unique_index: usize, account_index: usize| {
                // Verify account has no outstanding references and take one
                let account = accounts[account_index]
                    .try_borrow_mut()
                    .map_err(|_| InstructionError::AccountBorrowOutstanding)?;
                pre_accounts[unique_index].verify(
                    &program_id,
                    Some(message.is_writable(account_index)),
                    rent,
                    &account,
                    timings,
                )?;
                pre_sum += u128::from(pre_accounts[unique_index].lamports());
                post_sum += u128::from(account.lamports);
                Ok(())
            };
            instruction.visit_each_account(&mut work)?;
        }

        // Verify that the total sum of all the lamports did not change
        if pre_sum != post_sum {
            return Err(InstructionError::UnbalancedInstruction);
        }
        Ok(())
    }

    /// Verify the results of a cross-program instruction
    fn verify_and_update(
        message: &Message,
        instruction: &CompiledInstruction,
        pre_accounts: &mut [PreAccount],
        accounts: &[Rc<RefCell<Account>>],
        program_id: &Pubkey,
        rent: &Rent,
        track_writable_deescalation: bool,
        caller_privileges: Option<&[bool]>,
        timings: &mut ExecuteDetailsTimings,
    ) -> Result<(), InstructionError> {
        // Verify the per-account instruction results
        let (mut pre_sum, mut post_sum) = (0_u128, 0_u128);
        let mut work = |_unique_index: usize, account_index: usize| {
            if account_index < message.account_keys.len() && account_index < accounts.len() {
                let key = &message.account_keys[account_index];
                let account = &accounts[account_index];
                let is_writable = if track_writable_deescalation {
                    Some(if let Some(caller_privileges) = caller_privileges {
                        caller_privileges[account_index]
                    } else {
                        message.is_writable(account_index)
                    })
                } else {
                    None
                };
                // Find the matching PreAccount
                for pre_account in pre_accounts.iter_mut() {
                    if *key == pre_account.key() {
                        // Verify account has no outstanding references and take one
                        let account = account
                            .try_borrow_mut()
                            .map_err(|_| InstructionError::AccountBorrowOutstanding)?;

                        pre_account.verify(&program_id, is_writable, &rent, &account, timings)?;
                        pre_sum += u128::from(pre_account.lamports());
                        post_sum += u128::from(account.lamports);

                        pre_account.update(&account);

                        return Ok(());
                    }
                }
            }
            Err(InstructionError::MissingAccount)
        };
        instruction.visit_each_account(&mut work)?;
        work(0, instruction.program_id_index as usize)?;

        // Verify that the total sum of all the lamports did not change
        if pre_sum != post_sum {
            return Err(InstructionError::UnbalancedInstruction);
        }
        Ok(())
    }

    /// Execute an instruction
    /// This method calls the instruction's program entrypoint method and verifies that the result of
    /// the call does not violate the bank's accounting rules.
    /// The accounts are committed back to the bank only if this function returns Ok(_).
    #[allow(clippy::too_many_arguments)]
    fn execute_instruction(
        &self,
        message: &Message,
        instruction: &CompiledInstruction,
        executable_accounts: &[(Pubkey, RefCell<Account>)],
        accounts: &[Rc<RefCell<Account>>],
        account_deps: &[(Pubkey, RefCell<Account>)],
        rent_collector: &RentCollector,
        log_collector: Option<Rc<LogCollector>>,
        executors: Rc<RefCell<Executors>>,
        instruction_recorder: Option<InstructionRecorder>,
        instruction_index: usize,
        feature_set: Arc<FeatureSet>,
        bpf_compute_budget: BpfComputeBudget,
        timings: &mut ExecuteDetailsTimings,
    ) -> Result<(), InstructionError> {
        // Fixup the special instructions key if present
        // before the account pre-values are taken care of
        if feature_set.is_active(&instructions_sysvar_enabled::id()) {
            for (i, key) in message.account_keys.iter().enumerate() {
                if solana_sdk::sysvar::instructions::check_id(key) {
                    let mut mut_account_ref = accounts[i].borrow_mut();
                    solana_sdk::sysvar::instructions::store_current_index(
                        &mut mut_account_ref.data,
                        instruction_index as u16,
                    );
                    break;
                }
            }
        }

        let pre_accounts = Self::create_pre_accounts(message, instruction, accounts);
        let program_id = instruction.program_id(&message.account_keys);
        let mut invoke_context = ThisInvokeContext::new(
            program_id,
            rent_collector.rent,
            pre_accounts,
            account_deps,
            &self.programs,
            log_collector,
            bpf_compute_budget,
            executors,
            instruction_recorder,
            feature_set,
        );
        let keyed_accounts =
            Self::create_keyed_accounts(message, instruction, executable_accounts, accounts);
        self.process_instruction(
            program_id,
            &keyed_accounts,
            &instruction.data,
            &mut invoke_context,
        )?;
        Self::verify(
            message,
            instruction,
            &invoke_context.pre_accounts,
            executable_accounts,
            accounts,
            &rent_collector.rent,
            timings,
        )?;

        timings.accumulate(&invoke_context.timings);

        Ok(())
    }

    /// Process a message.
    /// This method calls each instruction in the message over the set of loaded Accounts
    /// The accounts are committed back to the bank only if every instruction succeeds
    #[allow(clippy::too_many_arguments)]
    pub fn process_message(
        &self,
        message: &Message,
        loaders: &[Vec<(Pubkey, RefCell<Account>)>],
        accounts: &[Rc<RefCell<Account>>],
        account_deps: &[(Pubkey, RefCell<Account>)],
        rent_collector: &RentCollector,
        log_collector: Option<Rc<LogCollector>>,
        executors: Rc<RefCell<Executors>>,
        instruction_recorders: Option<&[InstructionRecorder]>,
        feature_set: Arc<FeatureSet>,
        bpf_compute_budget: BpfComputeBudget,
        timings: &mut ExecuteDetailsTimings,
    ) -> Result<(), TransactionError> {
        for (instruction_index, instruction) in message.instructions.iter().enumerate() {
            let instruction_recorder = instruction_recorders
                .as_ref()
                .map(|recorders| recorders[instruction_index].clone());
            self.execute_instruction(
                message,
                instruction,
                &loaders[instruction_index],
                accounts,
                account_deps,
                rent_collector,
                log_collector.clone(),
                executors.clone(),
                instruction_recorder,
                instruction_index,
                feature_set.clone(),
                bpf_compute_budget,
                timings,
            )
            .map_err(|err| TransactionError::InstructionError(instruction_index as u8, err))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        instruction::{AccountMeta, Instruction, InstructionError},
        message::Message,
        native_loader::create_loadable_account,
    };

    #[test]
    fn test_invoke_context() {
        const MAX_DEPTH: usize = 10;
        let mut program_ids = vec![];
        let mut keys = vec![];
        let mut pre_accounts = vec![];
        let mut accounts = vec![];
        for i in 0..MAX_DEPTH {
            program_ids.push(solana_sdk::pubkey::new_rand());
            keys.push(solana_sdk::pubkey::new_rand());
            accounts.push(Rc::new(RefCell::new(Account::new(
                i as u64,
                1,
                &program_ids[i],
            ))));
            pre_accounts.push(PreAccount::new(&keys[i], &accounts[i].borrow(), false))
        }
        let account = Account::new(1, 1, &solana_sdk::pubkey::Pubkey::default());
        for program_id in program_ids.iter() {
            pre_accounts.push(PreAccount::new(program_id, &account.clone(), false));
        }

        let mut invoke_context = ThisInvokeContext::new(
            &program_ids[0],
            Rent::default(),
            pre_accounts,
            &[],
            &[],
            None,
            BpfComputeBudget::default(),
            Rc::new(RefCell::new(Executors::default())),
            None,
            Arc::new(FeatureSet::all_enabled()),
        );

        // Check call depth increases and has a limit
        let mut depth_reached = 1;
        for program_id in program_ids.iter().skip(1) {
            if Err(InstructionError::CallDepth) == invoke_context.push(program_id) {
                break;
            }
            depth_reached += 1;
        }
        assert_ne!(depth_reached, 0);
        assert!(depth_reached < MAX_DEPTH);

        // Mock each invocation
        for owned_index in (1..depth_reached).rev() {
            let not_owned_index = owned_index - 1;
            let metas = vec![
                AccountMeta::new(keys[not_owned_index], false),
                AccountMeta::new(keys[owned_index], false),
            ];
            let message = Message::new(
                &[Instruction::new_with_bytes(
                    program_ids[owned_index],
                    &[0],
                    metas,
                )],
                None,
            );

            // modify account owned by the program
            accounts[owned_index].borrow_mut().data[0] = (MAX_DEPTH + owned_index) as u8;
            let mut these_accounts = accounts[not_owned_index..owned_index + 1].to_vec();
            these_accounts.push(Rc::new(RefCell::new(Account::new(
                1,
                1,
                &solana_sdk::pubkey::Pubkey::default(),
            ))));
            invoke_context
                .verify_and_update(&message, &message.instructions[0], &these_accounts, None)
                .unwrap();
            assert_eq!(
                invoke_context.pre_accounts[owned_index]
                    .account
                    .borrow()
                    .data[0],
                (MAX_DEPTH + owned_index) as u8
            );

            // modify account not owned by the program
            let data = accounts[not_owned_index].borrow_mut().data[0];
            accounts[not_owned_index].borrow_mut().data[0] = (MAX_DEPTH + not_owned_index) as u8;
            assert_eq!(
                invoke_context.verify_and_update(
                    &message,
                    &message.instructions[0],
                    &accounts[not_owned_index..owned_index + 1],
                    None
                ),
                Err(InstructionError::ExternalAccountDataModified)
            );
            assert_eq!(
                invoke_context.pre_accounts[not_owned_index]
                    .account
                    .borrow()
                    .data[0],
                data
            );
            accounts[not_owned_index].borrow_mut().data[0] = data;

            invoke_context.pop();
        }
    }

    #[test]
    fn test_is_zeroed() {
        const ZEROS_LEN: usize = 1024;
        let mut buf = [0; ZEROS_LEN];
        assert_eq!(PreAccount::is_zeroed(&buf), true);
        buf[0] = 1;
        assert_eq!(PreAccount::is_zeroed(&buf), false);

        let mut buf = [0; ZEROS_LEN - 1];
        assert_eq!(PreAccount::is_zeroed(&buf), true);
        buf[0] = 1;
        assert_eq!(PreAccount::is_zeroed(&buf), false);

        let mut buf = [0; ZEROS_LEN + 1];
        assert_eq!(PreAccount::is_zeroed(&buf), true);
        buf[0] = 1;
        assert_eq!(PreAccount::is_zeroed(&buf), false);

        let buf = vec![];
        assert_eq!(PreAccount::is_zeroed(&buf), true);
    }

    #[test]
    fn test_verify_account_references() {
        let accounts = vec![(
            solana_sdk::pubkey::new_rand(),
            RefCell::new(Account::default()),
        )];

        assert!(MessageProcessor::verify_account_references(&accounts).is_ok());

        let mut _borrowed = accounts[0].1.borrow();
        assert_eq!(
            MessageProcessor::verify_account_references(&accounts),
            Err(InstructionError::AccountBorrowOutstanding)
        );
    }

    struct Change {
        program_id: Pubkey,
        is_writable: bool,
        rent: Rent,
        pre: PreAccount,
        post: Account,
    }
    impl Change {
        pub fn new(owner: &Pubkey, program_id: &Pubkey) -> Self {
            Self {
                program_id: *program_id,
                rent: Rent::default(),
                is_writable: true,
                pre: PreAccount::new(
                    &solana_sdk::pubkey::new_rand(),
                    &Account {
                        owner: *owner,
                        lamports: std::u64::MAX,
                        data: vec![],
                        ..Account::default()
                    },
                    false,
                ),
                post: Account {
                    owner: *owner,
                    lamports: std::u64::MAX,
                    ..Account::default()
                },
            }
        }
        pub fn read_only(mut self) -> Self {
            self.is_writable = false;
            self
        }
        pub fn executable(mut self, pre: bool, post: bool) -> Self {
            self.pre.account.borrow_mut().executable = pre;
            self.post.executable = post;
            self
        }
        pub fn lamports(mut self, pre: u64, post: u64) -> Self {
            self.pre.account.borrow_mut().lamports = pre;
            self.post.lamports = post;
            self
        }
        pub fn owner(mut self, post: &Pubkey) -> Self {
            self.post.owner = *post;
            self
        }
        pub fn data(mut self, pre: Vec<u8>, post: Vec<u8>) -> Self {
            self.pre.account.borrow_mut().data = pre;
            self.post.data = post;
            self
        }
        pub fn rent_epoch(mut self, pre: u64, post: u64) -> Self {
            self.pre.account.borrow_mut().rent_epoch = pre;
            self.post.rent_epoch = post;
            self
        }
        pub fn verify(&self) -> Result<(), InstructionError> {
            self.pre.verify(
                &self.program_id,
                Some(self.is_writable),
                &self.rent,
                &self.post,
                &mut ExecuteDetailsTimings::default(),
            )
        }
    }

    #[test]
    fn test_verify_account_changes_owner() {
        let system_program_id = system_program::id();
        let alice_program_id = solana_sdk::pubkey::new_rand();
        let mallory_program_id = solana_sdk::pubkey::new_rand();

        assert_eq!(
            Change::new(&system_program_id, &system_program_id)
                .owner(&alice_program_id)
                .verify(),
            Ok(()),
            "system program should be able to change the account owner"
        );
        assert_eq!(
            Change::new(&system_program_id, &system_program_id)
                .owner(&alice_program_id)
                .read_only()
                .verify(),
            Err(InstructionError::ModifiedProgramId),
            "system program should not be able to change the account owner of a read-only account"
        );
        assert_eq!(
            Change::new(&mallory_program_id, &system_program_id)
                .owner(&alice_program_id)
                .verify(),
            Err(InstructionError::ModifiedProgramId),
            "system program should not be able to change the account owner of a non-system account"
        );
        assert_eq!(
            Change::new(&mallory_program_id, &mallory_program_id)
                .owner(&alice_program_id)
                .verify(),
            Ok(()),
            "mallory should be able to change the account owner, if she leaves clear data"
        );
        assert_eq!(
            Change::new(&mallory_program_id, &mallory_program_id)
                .owner(&alice_program_id)
                .data(vec![42], vec![0])
                .verify(),
            Ok(()),
            "mallory should be able to change the account owner, if she leaves clear data"
        );
        assert_eq!(
            Change::new(&mallory_program_id, &mallory_program_id)
                .owner(&alice_program_id)
                .executable(true, true)
                .data(vec![42], vec![0])
                .verify(),
            Err(InstructionError::ModifiedProgramId),
            "mallory should not be able to change the account owner, if the account executable"
        );
        assert_eq!(
            Change::new(&mallory_program_id, &mallory_program_id)
                .owner(&alice_program_id)
                .data(vec![42], vec![42])
                .verify(),
            Err(InstructionError::ModifiedProgramId),
            "mallory should not be able to inject data into the alice program"
        );
    }

    #[test]
    fn test_verify_account_changes_executable() {
        let owner = solana_sdk::pubkey::new_rand();
        let mallory_program_id = solana_sdk::pubkey::new_rand();
        let system_program_id = system_program::id();

        assert_eq!(
            Change::new(&owner, &system_program_id)
                .executable(false, true)
                .verify(),
            Err(InstructionError::ExecutableModified),
            "system program can't change executable if system doesn't own the account"
        );
        assert_eq!(
            Change::new(&owner, &system_program_id)
                .executable(true, true)
                .data(vec![1], vec![2])
                .verify(),
            Err(InstructionError::ExecutableDataModified),
            "system program can't change executable data if system doesn't own the account"
        );
        assert_eq!(
            Change::new(&owner, &owner).executable(false, true).verify(),
            Ok(()),
            "owner should be able to change executable"
        );
        assert_eq!(
            Change::new(&owner, &owner)
                .executable(false, true)
                .read_only()
                .verify(),
            Err(InstructionError::ExecutableModified),
            "owner can't modify executable of read-only accounts"
        );
        assert_eq!(
            Change::new(&owner, &owner).executable(true, false).verify(),
            Err(InstructionError::ExecutableModified),
            "owner program can't reverse executable"
        );
        assert_eq!(
            Change::new(&owner, &mallory_program_id)
                .executable(false, true)
                .verify(),
            Err(InstructionError::ExecutableModified),
            "malicious Mallory should not be able to change the account executable"
        );
        assert_eq!(
            Change::new(&owner, &owner)
                .executable(false, true)
                .data(vec![1], vec![2])
                .verify(),
            Ok(()),
            "account data can change in the same instruction that sets the bit"
        );
        assert_eq!(
            Change::new(&owner, &owner)
                .executable(true, true)
                .data(vec![1], vec![2])
                .verify(),
            Err(InstructionError::ExecutableDataModified),
            "owner should not be able to change an account's data once its marked executable"
        );
        assert_eq!(
            Change::new(&owner, &owner)
                .executable(true, true)
                .lamports(1, 2)
                .verify(),
            Err(InstructionError::ExecutableLamportChange),
            "owner should not be able to add lamports once marked executable"
        );
        assert_eq!(
            Change::new(&owner, &owner)
                .executable(true, true)
                .lamports(1, 2)
                .verify(),
            Err(InstructionError::ExecutableLamportChange),
            "owner should not be able to add lamports once marked executable"
        );
        assert_eq!(
            Change::new(&owner, &owner)
                .executable(true, true)
                .lamports(2, 1)
                .verify(),
            Err(InstructionError::ExecutableLamportChange),
            "owner should not be able to subtract lamports once marked executable"
        );
        let data = vec![1; 100];
        let min_lamports = Rent::default().minimum_balance(data.len());
        assert_eq!(
            Change::new(&owner, &owner)
                .executable(false, true)
                .lamports(0, min_lamports)
                .data(data.clone(), data.clone())
                .verify(),
            Ok(()),
        );
        assert_eq!(
            Change::new(&owner, &owner)
                .executable(false, true)
                .lamports(0, min_lamports - 1)
                .data(data.clone(), data)
                .verify(),
            Err(InstructionError::ExecutableAccountNotRentExempt),
            "owner should not be able to change an account's data once its marked executable"
        );
    }

    #[test]
    fn test_verify_account_changes_data_len() {
        let alice_program_id = solana_sdk::pubkey::new_rand();

        assert_eq!(
            Change::new(&system_program::id(), &system_program::id())
                .data(vec![0], vec![0, 0])
                .verify(),
            Ok(()),
            "system program should be able to change the data len"
        );
        assert_eq!(
            Change::new(&alice_program_id, &system_program::id())
            .data(vec![0], vec![0,0])
            .verify(),
        Err(InstructionError::AccountDataSizeChanged),
        "system program should not be able to change the data length of accounts it does not own"
        );
    }

    #[test]
    fn test_verify_account_changes_data() {
        let alice_program_id = solana_sdk::pubkey::new_rand();
        let mallory_program_id = solana_sdk::pubkey::new_rand();

        assert_eq!(
            Change::new(&alice_program_id, &alice_program_id)
                .data(vec![0], vec![42])
                .verify(),
            Ok(()),
            "alice program should be able to change the data"
        );
        assert_eq!(
            Change::new(&mallory_program_id, &alice_program_id)
                .data(vec![0], vec![42])
                .verify(),
            Err(InstructionError::ExternalAccountDataModified),
            "non-owner mallory should not be able to change the account data"
        );
        assert_eq!(
            Change::new(&alice_program_id, &alice_program_id)
                .data(vec![0], vec![42])
                .read_only()
                .verify(),
            Err(InstructionError::ReadonlyDataModified),
            "alice isn't allowed to touch a CO account"
        );
    }

    #[test]
    fn test_verify_account_changes_rent_epoch() {
        let alice_program_id = solana_sdk::pubkey::new_rand();

        assert_eq!(
            Change::new(&alice_program_id, &system_program::id()).verify(),
            Ok(()),
            "nothing changed!"
        );
        assert_eq!(
            Change::new(&alice_program_id, &system_program::id())
                .rent_epoch(0, 1)
                .verify(),
            Err(InstructionError::RentEpochModified),
            "no one touches rent_epoch"
        );
    }

    #[test]
    fn test_verify_account_changes_deduct_lamports_and_reassign_account() {
        let alice_program_id = solana_sdk::pubkey::new_rand();
        let bob_program_id = solana_sdk::pubkey::new_rand();

        // positive test of this capability
        assert_eq!(
            Change::new(&alice_program_id, &alice_program_id)
            .owner(&bob_program_id)
            .lamports(42, 1)
            .data(vec![42], vec![0])
            .verify(),
        Ok(()),
        "alice should be able to deduct lamports and give the account to bob if the data is zeroed",
    );
    }

    #[test]
    fn test_verify_account_changes_lamports() {
        let alice_program_id = solana_sdk::pubkey::new_rand();

        assert_eq!(
            Change::new(&alice_program_id, &system_program::id())
                .lamports(42, 0)
                .read_only()
                .verify(),
            Err(InstructionError::ExternalAccountLamportSpend),
            "debit should fail, even if system program"
        );
        assert_eq!(
            Change::new(&alice_program_id, &alice_program_id)
                .lamports(42, 0)
                .read_only()
                .verify(),
            Err(InstructionError::ReadonlyLamportChange),
            "debit should fail, even if owning program"
        );
        assert_eq!(
            Change::new(&alice_program_id, &system_program::id())
                .lamports(42, 0)
                .owner(&system_program::id())
                .verify(),
            Err(InstructionError::ModifiedProgramId),
            "system program can't debit the account unless it was the pre.owner"
        );
        assert_eq!(
            Change::new(&system_program::id(), &system_program::id())
                .lamports(42, 0)
                .owner(&alice_program_id)
                .verify(),
            Ok(()),
            "system can spend (and change owner)"
        );
    }

    #[test]
    fn test_verify_account_changes_data_size_changed() {
        let alice_program_id = solana_sdk::pubkey::new_rand();

        assert_eq!(
            Change::new(&alice_program_id, &system_program::id())
                .data(vec![0], vec![0, 0])
                .verify(),
            Err(InstructionError::AccountDataSizeChanged),
            "system program should not be able to change another program's account data size"
        );
        assert_eq!(
            Change::new(&alice_program_id, &alice_program_id)
                .data(vec![0], vec![0, 0])
                .verify(),
            Err(InstructionError::AccountDataSizeChanged),
            "non-system programs cannot change their data size"
        );
        assert_eq!(
            Change::new(&system_program::id(), &system_program::id())
                .data(vec![0], vec![0, 0])
                .verify(),
            Ok(()),
            "system program should be able to change account data size"
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
            keyed_accounts: &[KeyedAccount],
            data: &[u8],
            _invoke_context: &mut dyn InvokeContext,
        ) -> Result<(), InstructionError> {
            if let Ok(instruction) = bincode::deserialize(data) {
                match instruction {
                    MockSystemInstruction::Correct => Ok(()),
                    MockSystemInstruction::AttemptCredit { lamports } => {
                        keyed_accounts[0].account.borrow_mut().lamports -= lamports;
                        keyed_accounts[1].account.borrow_mut().lamports += lamports;
                        Ok(())
                    }
                    // Change data in a read-only account
                    MockSystemInstruction::AttemptDataChange { data } => {
                        keyed_accounts[1].account.borrow_mut().data = vec![data];
                        Ok(())
                    }
                }
            } else {
                Err(InstructionError::InvalidInstructionData)
            }
        }

        let mock_system_program_id = Pubkey::new(&[2u8; 32]);
        let rent_collector = RentCollector::default();
        let mut message_processor = MessageProcessor::default();
        message_processor.add_program(mock_system_program_id, mock_system_process_instruction);

        let mut accounts: Vec<Rc<RefCell<Account>>> = Vec::new();
        let account = Account::new_ref(100, 1, &mock_system_program_id);
        accounts.push(account);
        let account = Account::new_ref(0, 1, &mock_system_program_id);
        accounts.push(account);

        let mut loaders: Vec<Vec<(Pubkey, RefCell<Account>)>> = Vec::new();
        let account = RefCell::new(create_loadable_account("mock_system_program", 1));
        loaders.push(vec![(mock_system_program_id, account)]);

        let executors = Rc::new(RefCell::new(Executors::default()));

        let from_pubkey = solana_sdk::pubkey::new_rand();
        let to_pubkey = solana_sdk::pubkey::new_rand();
        let account_metas = vec![
            AccountMeta::new(from_pubkey, true),
            AccountMeta::new_readonly(to_pubkey, false),
        ];
        let message = Message::new(
            &[Instruction::new_with_bincode(
                mock_system_program_id,
                &MockSystemInstruction::Correct,
                account_metas.clone(),
            )],
            Some(&from_pubkey),
        );

        let result = message_processor.process_message(
            &message,
            &loaders,
            &accounts,
            &[],
            &rent_collector,
            None,
            executors.clone(),
            None,
            Arc::new(FeatureSet::all_enabled()),
            BpfComputeBudget::new(),
            &mut ExecuteDetailsTimings::default(),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(accounts[0].borrow().lamports, 100);
        assert_eq!(accounts[1].borrow().lamports, 0);

        let message = Message::new(
            &[Instruction::new_with_bincode(
                mock_system_program_id,
                &MockSystemInstruction::AttemptCredit { lamports: 50 },
                account_metas.clone(),
            )],
            Some(&from_pubkey),
        );

        let result = message_processor.process_message(
            &message,
            &loaders,
            &accounts,
            &[],
            &rent_collector,
            None,
            executors.clone(),
            None,
            Arc::new(FeatureSet::all_enabled()),
            BpfComputeBudget::new(),
            &mut ExecuteDetailsTimings::default(),
        );
        assert_eq!(
            result,
            Err(TransactionError::InstructionError(
                0,
                InstructionError::ReadonlyLamportChange
            ))
        );

        let message = Message::new(
            &[Instruction::new_with_bincode(
                mock_system_program_id,
                &MockSystemInstruction::AttemptDataChange { data: 50 },
                account_metas,
            )],
            Some(&from_pubkey),
        );

        let result = message_processor.process_message(
            &message,
            &loaders,
            &accounts,
            &[],
            &rent_collector,
            None,
            executors,
            None,
            Arc::new(FeatureSet::all_enabled()),
            BpfComputeBudget::new(),
            &mut ExecuteDetailsTimings::default(),
        );
        assert_eq!(
            result,
            Err(TransactionError::InstructionError(
                0,
                InstructionError::ReadonlyDataModified
            ))
        );
    }

    #[test]
    fn test_process_message_duplicate_accounts() {
        #[derive(Serialize, Deserialize)]
        enum MockSystemInstruction {
            BorrowFail,
            MultiBorrowMut,
            DoWork { lamports: u64, data: u8 },
        }

        fn mock_system_process_instruction(
            _program_id: &Pubkey,
            keyed_accounts: &[KeyedAccount],
            data: &[u8],
            _invoke_context: &mut dyn InvokeContext,
        ) -> Result<(), InstructionError> {
            if let Ok(instruction) = bincode::deserialize(data) {
                match instruction {
                    MockSystemInstruction::BorrowFail => {
                        let from_account = keyed_accounts[0].try_account_ref_mut()?;
                        let dup_account = keyed_accounts[2].try_account_ref_mut()?;
                        if from_account.lamports != dup_account.lamports {
                            return Err(InstructionError::InvalidArgument);
                        }
                        Ok(())
                    }
                    MockSystemInstruction::MultiBorrowMut => {
                        let from_lamports = {
                            let from_account = keyed_accounts[0].try_account_ref_mut()?;
                            from_account.lamports
                        };
                        let dup_lamports = {
                            let dup_account = keyed_accounts[2].try_account_ref_mut()?;
                            dup_account.lamports
                        };
                        if from_lamports != dup_lamports {
                            return Err(InstructionError::InvalidArgument);
                        }
                        Ok(())
                    }
                    MockSystemInstruction::DoWork { lamports, data } => {
                        {
                            let mut to_account = keyed_accounts[1].try_account_ref_mut()?;
                            let mut dup_account = keyed_accounts[2].try_account_ref_mut()?;
                            dup_account.lamports -= lamports;
                            to_account.lamports += lamports;
                            dup_account.data = vec![data];
                        }
                        keyed_accounts[0].try_account_ref_mut()?.lamports -= lamports;
                        keyed_accounts[1].try_account_ref_mut()?.lamports += lamports;
                        Ok(())
                    }
                }
            } else {
                Err(InstructionError::InvalidInstructionData)
            }
        }

        let mock_program_id = Pubkey::new(&[2u8; 32]);
        let rent_collector = RentCollector::default();
        let mut message_processor = MessageProcessor::default();
        message_processor.add_program(mock_program_id, mock_system_process_instruction);

        let mut accounts: Vec<Rc<RefCell<Account>>> = Vec::new();
        let account = Account::new_ref(100, 1, &mock_program_id);
        accounts.push(account);
        let account = Account::new_ref(0, 1, &mock_program_id);
        accounts.push(account);

        let mut loaders: Vec<Vec<(Pubkey, RefCell<Account>)>> = Vec::new();
        let account = RefCell::new(create_loadable_account("mock_system_program", 1));
        loaders.push(vec![(mock_program_id, account)]);

        let executors = Rc::new(RefCell::new(Executors::default()));

        let from_pubkey = solana_sdk::pubkey::new_rand();
        let to_pubkey = solana_sdk::pubkey::new_rand();
        let dup_pubkey = from_pubkey;
        let account_metas = vec![
            AccountMeta::new(from_pubkey, true),
            AccountMeta::new(to_pubkey, false),
            AccountMeta::new(dup_pubkey, false),
        ];

        // Try to borrow mut the same account
        let message = Message::new(
            &[Instruction::new_with_bincode(
                mock_program_id,
                &MockSystemInstruction::BorrowFail,
                account_metas.clone(),
            )],
            Some(&from_pubkey),
        );
        let result = message_processor.process_message(
            &message,
            &loaders,
            &accounts,
            &[],
            &rent_collector,
            None,
            executors.clone(),
            None,
            Arc::new(FeatureSet::all_enabled()),
            BpfComputeBudget::new(),
            &mut ExecuteDetailsTimings::default(),
        );
        assert_eq!(
            result,
            Err(TransactionError::InstructionError(
                0,
                InstructionError::AccountBorrowFailed
            ))
        );

        // Try to borrow mut the same account in a safe way
        let message = Message::new(
            &[Instruction::new_with_bincode(
                mock_program_id,
                &MockSystemInstruction::MultiBorrowMut,
                account_metas.clone(),
            )],
            Some(&from_pubkey),
        );
        let result = message_processor.process_message(
            &message,
            &loaders,
            &accounts,
            &[],
            &rent_collector,
            None,
            executors.clone(),
            None,
            Arc::new(FeatureSet::all_enabled()),
            BpfComputeBudget::new(),
            &mut ExecuteDetailsTimings::default(),
        );
        assert_eq!(result, Ok(()));

        // Do work on the same account but at different location in keyed_accounts[]
        let message = Message::new(
            &[Instruction::new_with_bincode(
                mock_program_id,
                &MockSystemInstruction::DoWork {
                    lamports: 10,
                    data: 42,
                },
                account_metas,
            )],
            Some(&from_pubkey),
        );
        let result = message_processor.process_message(
            &message,
            &loaders,
            &accounts,
            &[],
            &rent_collector,
            None,
            executors,
            None,
            Arc::new(FeatureSet::all_enabled()),
            BpfComputeBudget::new(),
            &mut ExecuteDetailsTimings::default(),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(accounts[0].borrow().lamports, 80);
        assert_eq!(accounts[1].borrow().lamports, 20);
        assert_eq!(accounts[0].borrow().data, vec![42]);
    }

    #[test]
    fn test_process_cross_program() {
        #[derive(Debug, Serialize, Deserialize)]
        enum MockInstruction {
            NoopSuccess,
            NoopFail,
            ModifyOwned,
            ModifyNotOwned,
        }

        fn mock_process_instruction(
            program_id: &Pubkey,
            keyed_accounts: &[KeyedAccount],
            data: &[u8],
            _invoke_context: &mut dyn InvokeContext,
        ) -> Result<(), InstructionError> {
            assert_eq!(*program_id, keyed_accounts[0].owner()?);
            assert_ne!(
                keyed_accounts[1].owner()?,
                *keyed_accounts[0].unsigned_key()
            );

            if let Ok(instruction) = bincode::deserialize(data) {
                match instruction {
                    MockInstruction::NoopSuccess => (),
                    MockInstruction::NoopFail => return Err(InstructionError::GenericError),
                    MockInstruction::ModifyOwned => {
                        keyed_accounts[0].try_account_ref_mut()?.data[0] = 1
                    }
                    MockInstruction::ModifyNotOwned => {
                        keyed_accounts[1].try_account_ref_mut()?.data[0] = 1
                    }
                }
            } else {
                return Err(InstructionError::InvalidInstructionData);
            }
            Ok(())
        }

        let caller_program_id = solana_sdk::pubkey::new_rand();
        let callee_program_id = solana_sdk::pubkey::new_rand();

        let mut program_account = Account::new(1, 0, &native_loader::id());
        program_account.executable = true;
        let executable_preaccount = PreAccount::new(&callee_program_id, &program_account, true);
        let executable_accounts = vec![(callee_program_id, RefCell::new(program_account.clone()))];

        let owned_key = solana_sdk::pubkey::new_rand();
        let owned_account = Account::new(42, 1, &callee_program_id);
        let owned_preaccount = PreAccount::new(&owned_key, &owned_account, true);

        let not_owned_key = solana_sdk::pubkey::new_rand();
        let not_owned_account = Account::new(84, 1, &solana_sdk::pubkey::new_rand());
        let not_owned_preaccount = PreAccount::new(&not_owned_key, &not_owned_account, true);

        #[allow(unused_mut)]
        let mut accounts = vec![
            Rc::new(RefCell::new(owned_account)),
            Rc::new(RefCell::new(not_owned_account)),
            Rc::new(RefCell::new(program_account)),
        ];
        let programs: Vec<(_, ProcessInstructionWithContext)> =
            vec![(callee_program_id, mock_process_instruction)];
        let mut invoke_context = ThisInvokeContext::new(
            &caller_program_id,
            Rent::default(),
            vec![
                owned_preaccount,
                not_owned_preaccount,
                executable_preaccount,
            ],
            &[],
            programs.as_slice(),
            None,
            BpfComputeBudget::default(),
            Rc::new(RefCell::new(Executors::default())),
            None,
            Arc::new(FeatureSet::all_enabled()),
        );
        let metas = vec![
            AccountMeta::new(owned_key, false),
            AccountMeta::new(not_owned_key, false),
        ];

        // not owned account modified by the caller (before the invoke)
        accounts[0].borrow_mut().data[0] = 1;
        let instruction = Instruction::new_with_bincode(
            callee_program_id,
            &MockInstruction::NoopSuccess,
            metas.clone(),
        );
        let message = Message::new(&[instruction], None);
        let caller_privileges = message
            .account_keys
            .iter()
            .enumerate()
            .map(|(i, _)| message.is_writable(i))
            .collect::<Vec<bool>>();
        assert_eq!(
            MessageProcessor::process_cross_program_instruction(
                &message,
                &executable_accounts,
                &accounts,
                &caller_privileges,
                &mut invoke_context,
            ),
            Err(InstructionError::ExternalAccountDataModified)
        );
        accounts[0].borrow_mut().data[0] = 0;

        let cases = vec![
            (MockInstruction::NoopSuccess, Ok(())),
            (
                MockInstruction::NoopFail,
                Err(InstructionError::GenericError),
            ),
            (MockInstruction::ModifyOwned, Ok(())),
            (
                MockInstruction::ModifyNotOwned,
                Err(InstructionError::ExternalAccountDataModified),
            ),
        ];

        for case in cases {
            let instruction =
                Instruction::new_with_bincode(callee_program_id, &case.0, metas.clone());
            let message = Message::new(&[instruction], None);
            let caller_privileges = message
                .account_keys
                .iter()
                .enumerate()
                .map(|(i, _)| message.is_writable(i))
                .collect::<Vec<bool>>();
            assert_eq!(
                MessageProcessor::process_cross_program_instruction(
                    &message,
                    &executable_accounts,
                    &accounts,
                    &caller_privileges,
                    &mut invoke_context,
                ),
                case.1
            );
        }
    }

    #[test]
    fn test_debug() {
        let mut message_processor = MessageProcessor::default();
        #[allow(clippy::unnecessary_wraps)]
        fn mock_process_instruction(
            _program_id: &Pubkey,
            _keyed_accounts: &[KeyedAccount],
            _data: &[u8],
            _invoke_context: &mut dyn InvokeContext,
        ) -> Result<(), InstructionError> {
            Ok(())
        }
        #[allow(clippy::unnecessary_wraps)]
        fn mock_ix_processor(
            _pubkey: &Pubkey,
            _ka: &[KeyedAccount],
            _data: &[u8],
            _context: &mut dyn InvokeContext,
        ) -> Result<(), InstructionError> {
            Ok(())
        }
        let program_id = solana_sdk::pubkey::new_rand();
        message_processor.add_program(program_id, mock_process_instruction);
        message_processor.add_loader(program_id, mock_ix_processor);

        assert!(!format!("{:?}", message_processor).is_empty());
    }
}
