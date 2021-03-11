use solana_sdk::{
    account::AccountSharedData,
    instruction::{CompiledInstruction, Instruction, InstructionError},
    keyed_account::KeyedAccount,
    message::Message,
    pubkey::Pubkey,
};
use std::{cell::RefCell, fmt::Debug, rc::Rc, sync::Arc};

// Prototype of a native loader entry point
///
/// program_id: Program ID of the currently executing program
/// keyed_accounts: Accounts passed as part of the instruction
/// instruction_data: Instruction data
/// invoke_context: Invocation context
pub type LoaderEntrypoint = unsafe extern "C" fn(
    program_id: &Pubkey,
    keyed_accounts: &[KeyedAccount],
    instruction_data: &[u8],
    invoke_context: &dyn InvokeContext,
) -> Result<(), InstructionError>;

pub type ProcessInstructionWithContext =
    fn(&Pubkey, &[KeyedAccount], &[u8], &mut dyn InvokeContext) -> Result<(), InstructionError>;

/// Invocation context passed to loaders
pub trait InvokeContext {
    /// Push a program ID on to the invocation stack
    fn push(&mut self, key: &Pubkey) -> Result<(), InstructionError>;
    /// Pop a program ID off of the invocation stack
    fn pop(&mut self);
    /// Current depth of the invocation stake
    fn invoke_depth(&self) -> usize;
    /// Verify and update PreAccount state based on program execution
    fn verify_and_update(
        &mut self,
        message: &Message,
        instruction: &CompiledInstruction,
        accounts: &[Rc<RefCell<AccountSharedData>>],
        caller_pivileges: Option<&[bool]>,
    ) -> Result<(), InstructionError>;
    /// Get the program ID of the currently executing program
    fn get_caller(&self) -> Result<&Pubkey, InstructionError>;
    /// Get a list of built-in programs
    fn get_programs(&self) -> &[(Pubkey, ProcessInstructionWithContext)];
    /// Get this invocation's logger
    fn get_logger(&self) -> Rc<RefCell<dyn Logger>>;
    /// Get this invocation's compute budget
    fn get_bpf_compute_budget(&self) -> &BpfComputeBudget;
    /// Get this invocation's compute meter
    fn get_compute_meter(&self) -> Rc<RefCell<dyn ComputeMeter>>;
    /// Loaders may need to do work in order to execute a program.  Cache
    /// the work that can be re-used across executions
    fn add_executor(&self, pubkey: &Pubkey, executor: Arc<dyn Executor>);
    /// Get the completed loader work that can be re-used across executions
    fn get_executor(&self, pubkey: &Pubkey) -> Option<Arc<dyn Executor>>;
    /// Record invoked instruction
    fn record_instruction(&self, instruction: &Instruction);
    /// Get the bank's active feature set
    fn is_feature_active(&self, feature_id: &Pubkey) -> bool;
    /// Get an account from a pre-account
    fn get_account(&self, pubkey: &Pubkey) -> Option<Rc<RefCell<AccountSharedData>>>;
    /// Update timing
    fn update_timing(
        &mut self,
        serialize_us: u64,
        create_vm_us: u64,
        execute_us: u64,
        deserialize_us: u64,
    );
}

/// Convenience macro to log a message with an `Rc<RefCell<dyn Logger>>`
#[macro_export]
macro_rules! ic_logger_msg {
    ($logger:expr, $message:expr) => {
        if let Ok(logger) = $logger.try_borrow_mut() {
            if logger.log_enabled() {
                logger.log($message);
            }
        }
    };
    ($logger:expr, $fmt:expr, $($arg:tt)*) => {
        if let Ok(logger) = $logger.try_borrow_mut() {
            if logger.log_enabled() {
                logger.log(&format!($fmt, $($arg)*));
            }
        }
    };
}

/// Convenience macro to log a message with an `InvokeContext`
#[macro_export]
macro_rules! ic_msg {
    ($invoke_context:expr, $message:expr) => {
        $crate::ic_logger_msg!($invoke_context.get_logger(), $message)
    };
    ($invoke_context:expr, $fmt:expr, $($arg:tt)*) => {
        $crate::ic_logger_msg!($invoke_context.get_logger(), $fmt, $($arg)*)
    };
}

#[derive(Clone, Copy, Debug, AbiExample)]
pub struct BpfComputeBudget {
    /// Number of compute units that an instruction is allowed.  Compute units
    /// are consumed by program execution, resources they use, etc...
    pub max_units: u64,
    /// Number of compute units consumed by a log call
    pub log_units: u64,
    /// Number of compute units consumed by a log_u64 call
    pub log_64_units: u64,
    /// Number of compute units consumed by a create_program_address call
    pub create_program_address_units: u64,
    /// Number of compute units consumed by an invoke call (not including the cost incurred by
    /// the called program)
    pub invoke_units: u64,
    /// Maximum cross-program invocation depth allowed including the original caller
    pub max_invoke_depth: usize,
    /// Base number of compute units consumed to call SHA256
    pub sha256_base_cost: u64,
    /// Incremental number of units consumed by SHA256 (based on bytes)
    pub sha256_byte_cost: u64,
    /// Maximum BPF to BPF call depth
    pub max_call_depth: usize,
    /// Size of a stack frame in bytes, must match the size specified in the LLVM BPF backend
    pub stack_frame_size: usize,
    /// Number of compute units consumed by logging a `Pubkey`
    pub log_pubkey_units: u64,
    /// Maximum cross-program invocation instruction size
    pub max_cpi_instruction_size: usize,
}
impl Default for BpfComputeBudget {
    fn default() -> Self {
        Self::new()
    }
}
impl BpfComputeBudget {
    pub fn new() -> Self {
        BpfComputeBudget {
            max_units: 200_000,
            log_units: 100,
            log_64_units: 100,
            create_program_address_units: 1500,
            invoke_units: 1000,
            max_invoke_depth: 4,
            sha256_base_cost: 85,
            sha256_byte_cost: 1,
            max_call_depth: 64,
            stack_frame_size: 4_096,
            log_pubkey_units: 100,
            max_cpi_instruction_size: 1280, // IPv6 Min MTU size
        }
    }
}

/// Compute meter
pub trait ComputeMeter {
    /// Consume compute units
    fn consume(&mut self, amount: u64) -> Result<(), InstructionError>;
    /// Get the number of remaining compute units
    fn get_remaining(&self) -> u64;
}

/// Log messages
pub trait Logger {
    fn log_enabled(&self) -> bool;

    /// Log a message.
    ///
    /// Unless explicitly stated, log messages are not considered stable and may change in the
    /// future as necessary
    fn log(&self, message: &str);
}

///
/// Stable program log messages
///
/// The format of these log messages should not be modified to avoid breaking downstream consumers
/// of program logging
///
pub mod stable_log {
    use super::*;

    /// Log a program invoke.
    ///
    /// The general form is:
    ///     "Program <address> invoke [<depth>]"
    pub fn program_invoke(
        logger: &Rc<RefCell<dyn Logger>>,
        program_id: &Pubkey,
        invoke_depth: usize,
    ) {
        ic_logger_msg!(logger, "Program {} invoke [{}]", program_id, invoke_depth);
    }

    /// Log a message from the program itself.
    ///
    /// The general form is:
    ///     "Program log: <program-generated output>"
    /// That is, any program-generated output is guaranteed to be prefixed by "Program log: "
    pub fn program_log(logger: &Rc<RefCell<dyn Logger>>, message: &str) {
        ic_logger_msg!(logger, "Program log: {}", message);
    }

    /// Log successful program execution.
    ///
    /// The general form is:
    ///     "Program <address> success"
    pub fn program_success(logger: &Rc<RefCell<dyn Logger>>, program_id: &Pubkey) {
        ic_logger_msg!(logger, "Program {} success", program_id);
    }

    /// Log program execution failure
    ///
    /// The general form is:
    ///     "Program <address> failed: <program error details>"
    pub fn program_failure(
        logger: &Rc<RefCell<dyn Logger>>,
        program_id: &Pubkey,
        err: &InstructionError,
    ) {
        ic_logger_msg!(logger, "Program {} failed: {}", program_id, err);
    }
}

/// Program executor
pub trait Executor: Debug + Send + Sync {
    /// Execute the program
    fn execute(
        &self,
        loader_id: &Pubkey,
        program_id: &Pubkey,
        keyed_accounts: &[KeyedAccount],
        instruction_data: &[u8],
        invoke_context: &mut dyn InvokeContext,
        use_jit: bool,
    ) -> Result<(), InstructionError>;
}

#[derive(Debug, Default, Clone)]
pub struct MockComputeMeter {
    pub remaining: u64,
}
impl ComputeMeter for MockComputeMeter {
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

#[derive(Debug, Default, Clone)]
pub struct MockLogger {
    pub log: Rc<RefCell<Vec<String>>>,
}
impl Logger for MockLogger {
    fn log_enabled(&self) -> bool {
        true
    }
    fn log(&self, message: &str) {
        self.log.borrow_mut().push(message.to_string());
    }
}

pub struct MockInvokeContext {
    pub key: Pubkey,
    pub logger: MockLogger,
    pub bpf_compute_budget: BpfComputeBudget,
    pub compute_meter: MockComputeMeter,
    pub programs: Vec<(Pubkey, ProcessInstructionWithContext)>,
    pub invoke_depth: usize,
}
impl Default for MockInvokeContext {
    fn default() -> Self {
        MockInvokeContext {
            key: Pubkey::default(),
            logger: MockLogger::default(),
            bpf_compute_budget: BpfComputeBudget::default(),
            compute_meter: MockComputeMeter {
                remaining: std::i64::MAX as u64,
            },
            programs: vec![],
            invoke_depth: 0,
        }
    }
}
impl InvokeContext for MockInvokeContext {
    fn push(&mut self, _key: &Pubkey) -> Result<(), InstructionError> {
        self.invoke_depth = self.invoke_depth.saturating_add(1);
        Ok(())
    }
    fn pop(&mut self) {
        self.invoke_depth = self.invoke_depth.saturating_sub(1);
    }
    fn invoke_depth(&self) -> usize {
        self.invoke_depth
    }
    fn verify_and_update(
        &mut self,
        _message: &Message,
        _instruction: &CompiledInstruction,
        _accounts: &[Rc<RefCell<AccountSharedData>>],
        _caller_pivileges: Option<&[bool]>,
    ) -> Result<(), InstructionError> {
        Ok(())
    }
    fn get_caller(&self) -> Result<&Pubkey, InstructionError> {
        Ok(&self.key)
    }
    fn get_programs(&self) -> &[(Pubkey, ProcessInstructionWithContext)] {
        &self.programs
    }
    fn get_logger(&self) -> Rc<RefCell<dyn Logger>> {
        Rc::new(RefCell::new(self.logger.clone()))
    }
    fn get_bpf_compute_budget(&self) -> &BpfComputeBudget {
        &self.bpf_compute_budget
    }
    fn get_compute_meter(&self) -> Rc<RefCell<dyn ComputeMeter>> {
        Rc::new(RefCell::new(self.compute_meter.clone()))
    }
    fn add_executor(&self, _pubkey: &Pubkey, _executor: Arc<dyn Executor>) {}
    fn get_executor(&self, _pubkey: &Pubkey) -> Option<Arc<dyn Executor>> {
        None
    }
    fn record_instruction(&self, _instruction: &Instruction) {}
    fn is_feature_active(&self, _feature_id: &Pubkey) -> bool {
        true
    }
    fn get_account(&self, _pubkey: &Pubkey) -> Option<Rc<RefCell<AccountSharedData>>> {
        None
    }
    fn update_timing(
        &mut self,
        _serialize_us: u64,
        _create_vm_us: u64,
        _execute_us: u64,
        _deserialize_us: u64,
    ) {
    }
}
