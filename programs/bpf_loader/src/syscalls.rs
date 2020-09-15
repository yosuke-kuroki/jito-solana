use crate::{alloc, BPFError};
use alloc::Alloc;
use solana_rbpf::{
    ebpf::{ELF_INSN_DUMP_OFFSET, MM_HEAP_START},
    error::EbpfError,
    memory_region::{translate_addr, MemoryRegion},
    vm::{EbpfVm, SyscallObject},
};
use solana_runtime::message_processor::MessageProcessor;
use solana_sdk::{
    account::Account,
    account::KeyedAccount,
    account_info::AccountInfo,
    bpf_loader, bpf_loader_deprecated,
    entrypoint::{MAX_PERMITTED_DATA_INCREASE, SUCCESS},
    entrypoint_native::{ComputeMeter, InvokeContext, Logger},
    instruction::{AccountMeta, Instruction, InstructionError},
    message::Message,
    program_error::ProgramError,
    pubkey::{Pubkey, PubkeyError},
};
use std::{
    alloc::Layout,
    cell::{RefCell, RefMut},
    convert::TryFrom,
    mem::{align_of, size_of},
    rc::Rc,
    slice::from_raw_parts_mut,
    str::{from_utf8, Utf8Error},
};
use thiserror::Error as ThisError;

/// Error definitions
#[derive(Debug, ThisError, PartialEq)]
pub enum SyscallError {
    #[error("{0}: {1:?}")]
    InvalidString(Utf8Error, Vec<u8>),
    #[error("BPF program called abort()!")]
    Abort,
    #[error("BPF program Panicked in {0} at {1}:{2}")]
    Panic(String, u64, u64),
    #[error("cannot borrow invoke context")]
    InvokeContextBorrowFailed,
    #[error("malformed signer seed: {0}: {1:?}")]
    MalformedSignerSeed(Utf8Error, Vec<u8>),
    #[error("Could not create program address with signer seeds: {0}")]
    BadSeeds(PubkeyError),
    #[error("Program id is not supported by cross-program invocations")]
    ProgramNotSupported,
    #[error("{0}")]
    InstructionError(InstructionError),
    #[error("Cross-program invocation with unauthorized signer or writable account")]
    PrivilegeEscalation,
    #[error("Unaligned pointer")]
    UnalignedPointer,
}
impl From<SyscallError> for EbpfError<BPFError> {
    fn from(error: SyscallError) -> Self {
        EbpfError::UserError(error.into())
    }
}

trait SyscallConsume {
    fn consume(&mut self, amount: u64) -> Result<(), EbpfError<BPFError>>;
}
impl SyscallConsume for Rc<RefCell<dyn ComputeMeter>> {
    fn consume(&mut self, amount: u64) -> Result<(), EbpfError<BPFError>> {
        self.try_borrow_mut()
            .map_err(|_| SyscallError::InvokeContextBorrowFailed)?
            .consume(amount)
            .map_err(SyscallError::InstructionError)?;
        Ok(())
    }
}

/// Program heap allocators are intended to allocate/free from a given
/// chunk of memory.  The specific allocator implementation is
/// selectable at build-time.
/// Only one allocator is currently supported

/// Simple bump allocator, never frees
use crate::allocator_bump::BPFAllocator;

/// Default program heap size, allocators
/// are expected to enforce this
const DEFAULT_HEAP_SIZE: usize = 32 * 1024;

pub fn register_syscalls<'a>(
    loader_id: &'a Pubkey,
    vm: &mut EbpfVm<'a, BPFError>,
    callers_keyed_accounts: &'a [KeyedAccount<'a>],
    invoke_context: &'a mut dyn InvokeContext,
) -> Result<MemoryRegion, EbpfError<BPFError>> {
    let compute_budget = invoke_context.get_compute_budget();
    // Syscall functions common across languages

    vm.register_syscall_ex("abort", syscall_abort)?;
    vm.register_syscall_with_context_ex("sol_panic_", Box::new(SyscallPanic { loader_id }))?;
    vm.register_syscall_with_context_ex(
        "sol_log_",
        Box::new(SyscallLog {
            cost: compute_budget.log_units,
            compute_meter: invoke_context.get_compute_meter(),
            logger: invoke_context.get_logger(),
            loader_id,
        }),
    )?;
    vm.register_syscall_with_context_ex(
        "sol_log_64_",
        Box::new(SyscallLogU64 {
            cost: compute_budget.log_64_units,
            compute_meter: invoke_context.get_compute_meter(),
            logger: invoke_context.get_logger(),
        }),
    )?;
    if invoke_context.is_cross_program_supported() {
        vm.register_syscall_with_context_ex(
            "sol_create_program_address",
            Box::new(SyscallCreateProgramAddress {
                cost: compute_budget.create_program_address_units,
                compute_meter: invoke_context.get_compute_meter(),
                loader_id,
            }),
        )?;

        // Cross-program invocation syscalls

        let invoke_context = Rc::new(RefCell::new(invoke_context));
        vm.register_syscall_with_context_ex(
            "sol_invoke_signed_c",
            Box::new(SyscallProcessSolInstructionC {
                callers_keyed_accounts,
                invoke_context: invoke_context.clone(),
                loader_id,
            }),
        )?;
        vm.register_syscall_with_context_ex(
            "sol_invoke_signed_rust",
            Box::new(SyscallProcessInstructionRust {
                callers_keyed_accounts,
                invoke_context: invoke_context.clone(),
                loader_id,
            }),
        )?;
    }

    // Memory allocator
    let heap = vec![0_u8; DEFAULT_HEAP_SIZE];
    let heap_region = MemoryRegion::new_from_slice(&heap, MM_HEAP_START);
    vm.register_syscall_with_context_ex(
        "sol_alloc_free_",
        Box::new(SyscallSolAllocFree {
            aligned: *loader_id != bpf_loader_deprecated::id(),
            allocator: BPFAllocator::new(heap, MM_HEAP_START),
        }),
    )?;

    Ok(heap_region)
}

#[macro_export]
macro_rules! translate {
    ($vm_addr:expr, $len:expr, $regions:expr, $loader_id: expr) => {
        translate_addr::<BPFError>(
            $vm_addr as u64,
            $len as usize,
            file!(),
            line!() as usize - ELF_INSN_DUMP_OFFSET + 1,
            $regions,
        )
    };
}

#[macro_export]
macro_rules! translate_type_mut {
    ($t:ty, $vm_addr:expr, $regions:expr, $loader_id: expr) => {{
        if $loader_id != &bpf_loader_deprecated::id()
            && ($vm_addr as u64 as *mut $t).align_offset(align_of::<$t>()) != 0
        {
            Err(SyscallError::UnalignedPointer.into())
        } else {
            unsafe {
                match translate_addr::<BPFError>(
                    $vm_addr as u64,
                    size_of::<$t>(),
                    file!(),
                    line!() as usize - ELF_INSN_DUMP_OFFSET + 1,
                    $regions,
                ) {
                    Ok(value) => Ok(&mut *(value as *mut $t)),
                    Err(e) => Err(e),
                }
            }
        }
    }};
}
#[macro_export]
macro_rules! translate_type {
    ($t:ty, $vm_addr:expr, $regions:expr, $loader_id: expr) => {
        match translate_type_mut!($t, $vm_addr, $regions, $loader_id) {
            Ok(value) => Ok(&*value),
            Err(e) => Err(e),
        }
    };
}

#[macro_export]
macro_rules! translate_slice_mut {
    ($t:ty, $vm_addr:expr, $len: expr, $regions:expr, $loader_id: expr) => {{
        if $loader_id != &bpf_loader_deprecated::id()
            && ($vm_addr as u64 as *mut $t).align_offset(align_of::<$t>()) != 0
        {
            Err(SyscallError::UnalignedPointer.into())
        } else {
            match translate_addr::<BPFError>(
                $vm_addr as u64,
                $len as usize * size_of::<$t>(),
                file!(),
                line!() as usize - ELF_INSN_DUMP_OFFSET + 1,
                $regions,
            ) {
                Ok(value) => Ok(unsafe { from_raw_parts_mut(value as *mut $t, $len as usize) }),
                Err(e) => Err(e),
            }
        }
    }};
}
#[macro_export]
macro_rules! translate_slice {
    ($t:ty, $vm_addr:expr, $len: expr, $regions:expr, $loader_id: expr) => {
        match translate_slice_mut!($t, $vm_addr, $len, $regions, $loader_id) {
            Ok(value) => Ok(&*value),
            Err(e) => Err(e),
        }
    };
}

/// Take a virtual pointer to a string (points to BPF VM memory space), translate it
/// pass it to a user-defined work function
fn translate_string_and_do(
    addr: u64,
    len: u64,
    regions: &[MemoryRegion],
    loader_id: &Pubkey,
    work: &mut dyn FnMut(&str) -> Result<u64, EbpfError<BPFError>>,
) -> Result<u64, EbpfError<BPFError>> {
    let buf = translate_slice!(u8, addr, len, regions, loader_id)?;
    let i = match buf.iter().position(|byte| *byte == 0) {
        Some(i) => i,
        None => len as usize,
    };
    match from_utf8(&buf[..i]) {
        Ok(message) => work(message),
        Err(err) => Err(SyscallError::InvalidString(err, buf[..i].to_vec()).into()),
    }
}

/// Abort syscall functions, called when the BPF program calls `abort()`
/// LLVM will insert calls to `abort()` if it detects an untenable situation,
/// `abort()` is not intended to be called explicitly by the program.
/// Causes the BPF program to be halted immediately
pub fn syscall_abort(
    _arg1: u64,
    _arg2: u64,
    _arg3: u64,
    _arg4: u64,
    _arg5: u64,
    _ro_regions: &[MemoryRegion],
    _rw_regions: &[MemoryRegion],
) -> Result<u64, EbpfError<BPFError>> {
    Err(SyscallError::Abort.into())
}

/// Panic syscall function, called when the BPF program calls 'sol_panic_()`
/// Causes the BPF program to be halted immediately
/// Log a user's info message
pub struct SyscallPanic<'a> {
    loader_id: &'a Pubkey,
}
impl<'a> SyscallObject<BPFError> for SyscallPanic<'a> {
    fn call(
        &mut self,
        file: u64,
        len: u64,
        line: u64,
        column: u64,
        _arg5: u64,
        ro_regions: &[MemoryRegion],
        _rw_regions: &[MemoryRegion],
    ) -> Result<u64, EbpfError<BPFError>> {
        translate_string_and_do(
            file,
            len,
            ro_regions,
            &self.loader_id,
            &mut |string: &str| Err(SyscallError::Panic(string.to_string(), line, column).into()),
        )
    }
}

/// Log a user's info message
pub struct SyscallLog<'a> {
    cost: u64,
    compute_meter: Rc<RefCell<dyn ComputeMeter>>,
    logger: Rc<RefCell<dyn Logger>>,
    loader_id: &'a Pubkey,
}
impl<'a> SyscallObject<BPFError> for SyscallLog<'a> {
    fn call(
        &mut self,
        addr: u64,
        len: u64,
        _arg3: u64,
        _arg4: u64,
        _arg5: u64,
        ro_regions: &[MemoryRegion],
        _rw_regions: &[MemoryRegion],
    ) -> Result<u64, EbpfError<BPFError>> {
        self.compute_meter.consume(self.cost)?;
        let mut logger = self
            .logger
            .try_borrow_mut()
            .map_err(|_| SyscallError::InvokeContextBorrowFailed)?;
        if logger.log_enabled() {
            translate_string_and_do(
                addr,
                len,
                ro_regions,
                &self.loader_id,
                &mut |string: &str| {
                    logger.log(&format!("Program log: {}", string));
                    Ok(0)
                },
            )?;
        }
        Ok(0)
    }
}

/// Log 5 64-bit values
pub struct SyscallLogU64 {
    cost: u64,
    compute_meter: Rc<RefCell<dyn ComputeMeter>>,
    logger: Rc<RefCell<dyn Logger>>,
}
impl SyscallObject<BPFError> for SyscallLogU64 {
    fn call(
        &mut self,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
        arg5: u64,
        _ro_regions: &[MemoryRegion],
        _rw_regions: &[MemoryRegion],
    ) -> Result<u64, EbpfError<BPFError>> {
        self.compute_meter.consume(self.cost)?;
        let mut logger = self
            .logger
            .try_borrow_mut()
            .map_err(|_| SyscallError::InvokeContextBorrowFailed)?;
        if logger.log_enabled() {
            logger.log(&format!(
                "Program log: {:#x}, {:#x}, {:#x}, {:#x}, {:#x}",
                arg1, arg2, arg3, arg4, arg5
            ));
        }
        Ok(0)
    }
}

/// Dynamic memory allocation syscall called when the BPF program calls
/// `sol_alloc_free_()`.  The allocator is expected to allocate/free
/// from/to a given chunk of memory and enforce size restrictions.  The
/// memory chunk is given to the allocator during allocator creation and
/// information about that memory (start address and size) is passed
/// to the VM to use for enforcement.
pub struct SyscallSolAllocFree {
    aligned: bool,
    allocator: BPFAllocator,
}
impl SyscallObject<BPFError> for SyscallSolAllocFree {
    fn call(
        &mut self,
        size: u64,
        free_addr: u64,
        _arg3: u64,
        _arg4: u64,
        _arg5: u64,
        _ro_regions: &[MemoryRegion],
        _rw_regions: &[MemoryRegion],
    ) -> Result<u64, EbpfError<BPFError>> {
        let align = if self.aligned {
            align_of::<u128>()
        } else {
            align_of::<u8>()
        };
        let layout = match Layout::from_size_align(size as usize, align) {
            Ok(layout) => layout,
            Err(_) => return Ok(0),
        };
        if free_addr == 0 {
            match self.allocator.alloc(layout) {
                Ok(addr) => Ok(addr as u64),
                Err(_) => Ok(0),
            }
        } else {
            self.allocator.dealloc(free_addr, layout);
            Ok(0)
        }
    }
}

/// Create a program address
struct SyscallCreateProgramAddress<'a> {
    cost: u64,
    compute_meter: Rc<RefCell<dyn ComputeMeter>>,
    loader_id: &'a Pubkey,
}
impl<'a> SyscallObject<BPFError> for SyscallCreateProgramAddress<'a> {
    fn call(
        &mut self,
        seeds_addr: u64,
        seeds_len: u64,
        program_id_addr: u64,
        address_addr: u64,
        _arg5: u64,
        ro_regions: &[MemoryRegion],
        rw_regions: &[MemoryRegion],
    ) -> Result<u64, EbpfError<BPFError>> {
        self.compute_meter.consume(self.cost)?;
        // TODO need ref?
        let untranslated_seeds =
            translate_slice!(&[&u8], seeds_addr, seeds_len, ro_regions, self.loader_id)?;
        let seeds = untranslated_seeds
            .iter()
            .map(|untranslated_seed| {
                translate_slice!(
                    u8,
                    untranslated_seed.as_ptr(),
                    untranslated_seed.len(),
                    ro_regions,
                    self.loader_id
                )
            })
            .collect::<Result<Vec<_>, EbpfError<BPFError>>>()?;
        let program_id = translate_type!(Pubkey, program_id_addr, ro_regions, self.loader_id)?;

        let new_address = match Pubkey::create_program_address(&seeds, program_id)
            .map_err(SyscallError::BadSeeds)
        {
            Ok(address) => address,
            Err(_) => return Ok(1),
        };
        let address = translate_slice_mut!(u8, address_addr, 32, rw_regions, self.loader_id)?;
        address.copy_from_slice(new_address.as_ref());
        Ok(0)
    }
}

// Cross-program invocation syscalls

struct AccountReferences<'a> {
    lamports: &'a mut u64,
    owner: &'a mut Pubkey,
    data: &'a mut [u8],
    ref_to_len_in_vm: &'a mut u64,
    serialized_len_ptr: &'a mut u64,
}
type TranslatedAccounts<'a> = (Vec<Rc<RefCell<Account>>>, Vec<AccountReferences<'a>>);

/// Implemented by language specific data structure translators
trait SyscallProcessInstruction<'a> {
    fn get_context_mut(&self) -> Result<RefMut<&'a mut dyn InvokeContext>, EbpfError<BPFError>>;
    fn get_callers_keyed_accounts(&self) -> &'a [KeyedAccount<'a>];
    fn translate_instruction(
        &self,
        addr: u64,
        ro_regions: &[MemoryRegion],
    ) -> Result<Instruction, EbpfError<BPFError>>;
    fn translate_accounts(
        &self,
        message: &Message,
        account_infos_addr: u64,
        account_infos_len: usize,
        ro_regions: &[MemoryRegion],
        rw_regions: &[MemoryRegion],
    ) -> Result<TranslatedAccounts<'a>, EbpfError<BPFError>>;
    fn translate_signers(
        &self,
        program_id: &Pubkey,
        signers_seeds_addr: u64,
        signers_seeds_len: usize,
        ro_regions: &[MemoryRegion],
    ) -> Result<Vec<Pubkey>, EbpfError<BPFError>>;
}

/// Cross-program invocation called from Rust
pub struct SyscallProcessInstructionRust<'a> {
    callers_keyed_accounts: &'a [KeyedAccount<'a>],
    invoke_context: Rc<RefCell<&'a mut dyn InvokeContext>>,
    loader_id: &'a Pubkey,
}
impl<'a> SyscallProcessInstruction<'a> for SyscallProcessInstructionRust<'a> {
    fn get_context_mut(&self) -> Result<RefMut<&'a mut dyn InvokeContext>, EbpfError<BPFError>> {
        self.invoke_context
            .try_borrow_mut()
            .map_err(|_| SyscallError::InvokeContextBorrowFailed.into())
    }
    fn get_callers_keyed_accounts(&self) -> &'a [KeyedAccount<'a>] {
        self.callers_keyed_accounts
    }
    fn translate_instruction(
        &self,
        addr: u64,
        ro_regions: &[MemoryRegion],
    ) -> Result<Instruction, EbpfError<BPFError>> {
        let ix = translate_type!(Instruction, addr, ro_regions, self.loader_id)?;
        let accounts = translate_slice!(
            AccountMeta,
            ix.accounts.as_ptr(),
            ix.accounts.len(),
            ro_regions,
            self.loader_id
        )?
        .to_vec();
        let data = translate_slice!(
            u8,
            ix.data.as_ptr(),
            ix.data.len(),
            ro_regions,
            self.loader_id
        )?
        .to_vec();
        Ok(Instruction {
            program_id: ix.program_id,
            accounts,
            data,
        })
    }

    fn translate_accounts(
        &self,
        message: &Message,
        account_infos_addr: u64,
        account_infos_len: usize,
        ro_regions: &[MemoryRegion],
        rw_regions: &[MemoryRegion],
    ) -> Result<TranslatedAccounts<'a>, EbpfError<BPFError>> {
        let account_infos = if account_infos_len > 0 {
            translate_slice!(
                AccountInfo,
                account_infos_addr,
                account_infos_len,
                ro_regions,
                self.loader_id
            )?
        } else {
            &[]
        };

        let mut accounts = Vec::with_capacity(message.account_keys.len());
        let mut refs = Vec::with_capacity(message.account_keys.len());
        'root: for account_key in message.account_keys.iter() {
            for account_info in account_infos.iter() {
                let key = translate_type!(
                    Pubkey,
                    account_info.key as *const _,
                    ro_regions,
                    self.loader_id
                )?;
                if account_key == key {
                    let lamports = {
                        // Double translate lamports out of RefCell
                        let ptr = translate_type!(
                            u64,
                            account_info.lamports.as_ptr(),
                            ro_regions,
                            self.loader_id
                        )?;
                        translate_type_mut!(u64, *ptr, rw_regions, self.loader_id)?
                    };
                    let owner = translate_type_mut!(
                        Pubkey,
                        account_info.owner as *const _,
                        ro_regions,
                        self.loader_id
                    )?;
                    let (data, ref_to_len_in_vm, serialized_len_ptr) = {
                        // Double translate data out of RefCell
                        let data = *translate_type!(
                            &[u8],
                            account_info.data.as_ptr(),
                            ro_regions,
                            self.loader_id
                        )?;
                        let translated =
                            translate!(account_info.data.as_ptr(), 8, ro_regions, self.loader_id)?
                                as *mut u64;
                        let ref_to_len_in_vm = unsafe { &mut *translated.offset(1) };
                        let ref_of_len_in_input_buffer = unsafe { data.as_ptr().offset(-8) };
                        let serialized_len_ptr = translate_type_mut!(
                            u64,
                            ref_of_len_in_input_buffer,
                            rw_regions,
                            self.loader_id
                        )?;
                        (
                            translate_slice_mut!(
                                u8,
                                data.as_ptr(),
                                data.len(),
                                rw_regions,
                                self.loader_id
                            )?,
                            ref_to_len_in_vm,
                            serialized_len_ptr,
                        )
                    };

                    accounts.push(Rc::new(RefCell::new(Account {
                        lamports: *lamports,
                        data: data.to_vec(),
                        executable: account_info.executable,
                        owner: *owner,
                        rent_epoch: account_info.rent_epoch,
                    })));
                    refs.push(AccountReferences {
                        lamports,
                        owner,
                        data,
                        ref_to_len_in_vm,
                        serialized_len_ptr,
                    });
                    continue 'root;
                }
            }
            return Err(SyscallError::InstructionError(InstructionError::MissingAccount).into());
        }

        Ok((accounts, refs))
    }

    fn translate_signers(
        &self,
        program_id: &Pubkey,
        signers_seeds_addr: u64,
        signers_seeds_len: usize,
        ro_regions: &[MemoryRegion],
    ) -> Result<Vec<Pubkey>, EbpfError<BPFError>> {
        let mut signers = Vec::new();
        if signers_seeds_len > 0 {
            let signers_seeds = translate_slice!(
                &[&str],
                signers_seeds_addr,
                signers_seeds_len,
                ro_regions,
                self.loader_id
            )?;
            for signer_seeds in signers_seeds.iter() {
                let untranslated_seeds = translate_slice!(
                    &str,
                    signer_seeds.as_ptr(),
                    signer_seeds.len(),
                    ro_regions,
                    self.loader_id
                )?;
                let seeds = untranslated_seeds
                    .iter()
                    .map(|untranslated_seed| {
                        translate_slice!(
                            u8,
                            untranslated_seed.as_ptr(),
                            untranslated_seed.len(),
                            ro_regions,
                            self.loader_id
                        )
                    })
                    .collect::<Result<Vec<_>, EbpfError<BPFError>>>()?;
                let signer = Pubkey::create_program_address(&seeds, program_id)
                    .map_err(SyscallError::BadSeeds)?;
                signers.push(signer);
            }
            Ok(signers)
        } else {
            Ok(vec![])
        }
    }
}
impl<'a> SyscallObject<BPFError> for SyscallProcessInstructionRust<'a> {
    fn call(
        &mut self,
        instruction_addr: u64,
        account_infos_addr: u64,
        account_infos_len: u64,
        signers_seeds_addr: u64,
        signers_seeds_len: u64,
        ro_regions: &[MemoryRegion],
        rw_regions: &[MemoryRegion],
    ) -> Result<u64, EbpfError<BPFError>> {
        call(
            self,
            instruction_addr,
            account_infos_addr,
            account_infos_len,
            signers_seeds_addr,
            signers_seeds_len,
            ro_regions,
            rw_regions,
        )
    }
}

/// Rust representation of C's SolInstruction
#[derive(Debug)]
struct SolInstruction {
    program_id_addr: u64,
    accounts_addr: u64,
    accounts_len: usize,
    data_addr: u64,
    data_len: usize,
}

/// Rust representation of C's SolAccountMeta
#[derive(Debug)]
struct SolAccountMeta {
    pubkey_addr: u64,
    is_writable: bool,
    is_signer: bool,
}

/// Rust representation of C's SolAccountInfo
#[derive(Debug)]
struct SolAccountInfo {
    key_addr: u64,
    lamports_addr: u64,
    data_len: u64,
    data_addr: u64,
    owner_addr: u64,
    rent_epoch: u64,
    is_signer: bool,
    is_writable: bool,
    executable: bool,
}

/// Rust representation of C's SolSignerSeed
#[derive(Debug)]
struct SolSignerSeedC {
    addr: u64,
    len: u64,
}

/// Rust representation of C's SolSignerSeeds
#[derive(Debug)]
struct SolSignerSeedsC {
    addr: u64,
    len: u64,
}

/// Cross-program invocation called from C
pub struct SyscallProcessSolInstructionC<'a> {
    callers_keyed_accounts: &'a [KeyedAccount<'a>],
    invoke_context: Rc<RefCell<&'a mut dyn InvokeContext>>,
    loader_id: &'a Pubkey,
}
impl<'a> SyscallProcessInstruction<'a> for SyscallProcessSolInstructionC<'a> {
    fn get_context_mut(&self) -> Result<RefMut<&'a mut dyn InvokeContext>, EbpfError<BPFError>> {
        self.invoke_context
            .try_borrow_mut()
            .map_err(|_| SyscallError::InvokeContextBorrowFailed.into())
    }
    fn get_callers_keyed_accounts(&self) -> &'a [KeyedAccount<'a>] {
        self.callers_keyed_accounts
    }
    fn translate_instruction(
        &self,
        addr: u64,
        ro_regions: &[MemoryRegion],
    ) -> Result<Instruction, EbpfError<BPFError>> {
        let ix_c = translate_type!(SolInstruction, addr, ro_regions, self.loader_id)?;
        let program_id = translate_type!(Pubkey, ix_c.program_id_addr, ro_regions, self.loader_id)?;
        let meta_cs = translate_slice!(
            SolAccountMeta,
            ix_c.accounts_addr,
            ix_c.accounts_len,
            ro_regions,
            self.loader_id
        )?;
        let data = translate_slice!(
            u8,
            ix_c.data_addr,
            ix_c.data_len,
            ro_regions,
            self.loader_id
        )?
        .to_vec();
        let accounts = meta_cs
            .iter()
            .map(|meta_c| {
                let pubkey =
                    translate_type!(Pubkey, meta_c.pubkey_addr, ro_regions, self.loader_id)?;
                Ok(AccountMeta {
                    pubkey: *pubkey,
                    is_signer: meta_c.is_signer,
                    is_writable: meta_c.is_writable,
                })
            })
            .collect::<Result<Vec<AccountMeta>, EbpfError<BPFError>>>()?;

        Ok(Instruction {
            program_id: *program_id,
            accounts,
            data,
        })
    }

    fn translate_accounts(
        &self,
        message: &Message,
        account_infos_addr: u64,
        account_infos_len: usize,
        ro_regions: &[MemoryRegion],
        rw_regions: &[MemoryRegion],
    ) -> Result<TranslatedAccounts<'a>, EbpfError<BPFError>> {
        let account_infos = translate_slice!(
            SolAccountInfo,
            account_infos_addr,
            account_infos_len,
            ro_regions,
            self.loader_id
        )?;
        let mut accounts = Vec::with_capacity(message.account_keys.len());
        let mut refs = Vec::with_capacity(message.account_keys.len());
        'root: for account_key in message.account_keys.iter() {
            for account_info in account_infos.iter() {
                let key =
                    translate_type!(Pubkey, account_info.key_addr, ro_regions, self.loader_id)?;
                if account_key == key {
                    let lamports = translate_type_mut!(
                        u64,
                        account_info.lamports_addr,
                        rw_regions,
                        self.loader_id
                    )?;
                    let owner = translate_type_mut!(
                        Pubkey,
                        account_info.owner_addr,
                        ro_regions,
                        self.loader_id
                    )?;
                    let data = translate_slice_mut!(
                        u8,
                        account_info.data_addr,
                        account_info.data_len,
                        rw_regions,
                        self.loader_id
                    )?;
                    let ref_to_len_in_vm =
                        unsafe { &mut *(&account_info.data_len as *const u64 as u64 as *mut u64) };
                    let ref_of_len_in_input_buffer =
                        unsafe { (account_info.data_addr as *mut u8).offset(-8) };
                    let serialized_len_ptr = translate_type_mut!(
                        u64,
                        ref_of_len_in_input_buffer,
                        rw_regions,
                        self.loader_id
                    )?;

                    accounts.push(Rc::new(RefCell::new(Account {
                        lamports: *lamports,
                        data: data.to_vec(),
                        executable: account_info.executable,
                        owner: *owner,
                        rent_epoch: account_info.rent_epoch,
                    })));
                    refs.push(AccountReferences {
                        lamports,
                        owner,
                        data,
                        ref_to_len_in_vm,
                        serialized_len_ptr,
                    });
                    continue 'root;
                }
            }
            return Err(SyscallError::InstructionError(InstructionError::MissingAccount).into());
        }

        Ok((accounts, refs))
    }

    fn translate_signers(
        &self,
        program_id: &Pubkey,
        signers_seeds_addr: u64,
        signers_seeds_len: usize,
        ro_regions: &[MemoryRegion],
    ) -> Result<Vec<Pubkey>, EbpfError<BPFError>> {
        if signers_seeds_len > 0 {
            let signers_seeds = translate_slice!(
                SolSignerSeedC,
                signers_seeds_addr,
                signers_seeds_len,
                ro_regions,
                self.loader_id
            )?;
            Ok(signers_seeds
                .iter()
                .map(|signer_seeds| {
                    let seeds = translate_slice!(
                        SolSignerSeedC,
                        signer_seeds.addr,
                        signer_seeds.len,
                        ro_regions,
                        self.loader_id
                    )?;
                    let seeds_bytes = seeds
                        .iter()
                        .map(|seed| {
                            translate_slice!(u8, seed.addr, seed.len, ro_regions, self.loader_id)
                        })
                        .collect::<Result<Vec<_>, EbpfError<BPFError>>>()?;
                    Pubkey::create_program_address(&seeds_bytes, program_id)
                        .map_err(|err| SyscallError::BadSeeds(err).into())
                })
                .collect::<Result<Vec<_>, EbpfError<BPFError>>>()?)
        } else {
            Ok(vec![])
        }
    }
}
impl<'a> SyscallObject<BPFError> for SyscallProcessSolInstructionC<'a> {
    fn call(
        &mut self,
        instruction_addr: u64,
        account_infos_addr: u64,
        account_infos_len: u64,
        signers_seeds_addr: u64,
        signers_seeds_len: u64,
        ro_regions: &[MemoryRegion],
        rw_regions: &[MemoryRegion],
    ) -> Result<u64, EbpfError<BPFError>> {
        call(
            self,
            instruction_addr,
            account_infos_addr,
            account_infos_len,
            signers_seeds_addr,
            signers_seeds_len,
            ro_regions,
            rw_regions,
        )
    }
}

fn verify_instruction<'a>(
    syscall: &dyn SyscallProcessInstruction<'a>,
    instruction: &Instruction,
    signers: &[Pubkey],
) -> Result<(), EbpfError<BPFError>> {
    let callers_keyed_accounts = syscall.get_callers_keyed_accounts();

    // Check for privilege escalation
    for account in instruction.accounts.iter() {
        let keyed_account = callers_keyed_accounts
            .iter()
            .find_map(|keyed_account| {
                if &account.pubkey == keyed_account.unsigned_key() {
                    Some(keyed_account)
                } else {
                    None
                }
            })
            .ok_or(SyscallError::InstructionError(
                InstructionError::MissingAccount,
            ))?;
        // Readonly account cannot become writable
        if account.is_writable && !keyed_account.is_writable() {
            return Err(SyscallError::PrivilegeEscalation.into());
        }

        if account.is_signer && // If message indicates account is signed
        !( // one of the following needs to be true:
            keyed_account.signer_key().is_some() // Signed in the parent instruction
            || signers.contains(&account.pubkey) // Signed by the program
        ) {
            return Err(SyscallError::PrivilegeEscalation.into());
        }
    }

    Ok(())
}

/// Call process instruction, common to both Rust and C
fn call<'a>(
    syscall: &mut dyn SyscallProcessInstruction<'a>,
    instruction_addr: u64,
    account_infos_addr: u64,
    account_infos_len: u64,
    signers_seeds_addr: u64,
    signers_seeds_len: u64,
    ro_regions: &[MemoryRegion],
    rw_regions: &[MemoryRegion],
) -> Result<u64, EbpfError<BPFError>> {
    let mut invoke_context = syscall.get_context_mut()?;
    invoke_context
        .get_compute_meter()
        .consume(invoke_context.get_compute_budget().invoke_units)?;

    // Translate data passed from the VM

    let instruction = syscall.translate_instruction(instruction_addr, ro_regions)?;
    let caller_program_id = invoke_context
        .get_caller()
        .map_err(SyscallError::InstructionError)?;
    let signers = syscall.translate_signers(
        caller_program_id,
        signers_seeds_addr,
        signers_seeds_len as usize,
        ro_regions,
    )?;
    verify_instruction(syscall, &instruction, &signers)?;
    let message = Message::new(&[instruction], None);
    let callee_program_id_index = message.instructions[0].program_id_index as usize;
    let callee_program_id = message.account_keys[callee_program_id_index];
    let (accounts, account_refs) = syscall.translate_accounts(
        &message,
        account_infos_addr,
        account_infos_len as usize,
        ro_regions,
        rw_regions,
    )?;

    // Process instruction

    let program_account = (*accounts[callee_program_id_index]).clone();
    let executable_accounts = vec![(callee_program_id, program_account)];
    let mut message_processor = MessageProcessor::default();
    for (program_id, process_instruction) in invoke_context.get_programs().iter() {
        message_processor.add_program(*program_id, *process_instruction);
    }
    message_processor.add_loader(bpf_loader::id(), crate::process_instruction);
    message_processor.add_loader(bpf_loader_deprecated::id(), crate::process_instruction);
    message_processor.set_cross_program_support(invoke_context.is_cross_program_supported());

    #[allow(clippy::deref_addrof)]
    match message_processor.process_cross_program_instruction(
        &message,
        &executable_accounts,
        &accounts,
        *(&mut *invoke_context),
    ) {
        Ok(()) => (),
        Err(err) => match ProgramError::try_from(err) {
            Ok(err) => return Ok(err.into()),
            Err(err) => return Err(SyscallError::InstructionError(err).into()),
        },
    }

    // Copy results back into caller's AccountInfos

    for (i, (account, account_ref)) in accounts.iter().zip(account_refs).enumerate() {
        let account = account.borrow();
        if message.is_writable(i) && !account.executable {
            *account_ref.lamports = account.lamports;
            *account_ref.owner = account.owner;
            if account_ref.data.len() != account.data.len() {
                *account_ref.ref_to_len_in_vm = account.data.len() as u64;
                *account_ref.serialized_len_ptr = account.data.len() as u64;
                if !account_ref.data.is_empty() {
                    // Only support for `CreateAccount` at this time.
                    // Need a way to limit total realloc size accross multiple CPI calls
                    return Err(
                        SyscallError::InstructionError(InstructionError::InvalidRealloc).into(),
                    );
                }
                if account.data.len() > account_ref.data.len() + MAX_PERMITTED_DATA_INCREASE {
                    return Err(
                        SyscallError::InstructionError(InstructionError::InvalidRealloc).into(),
                    );
                }
            }
            account_ref
                .data
                .clone_from_slice(&account.data[0..account_ref.data.len()]);
        }
    }

    Ok(SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{MockComputeMeter, MockLogger};

    #[test]
    fn test_translate() {
        const START: u64 = 100;
        const LENGTH: u64 = 1000;
        let data = vec![0u8; LENGTH as usize];
        let addr = data.as_ptr() as u64;
        let regions = vec![MemoryRegion::new_from_slice(&data, START)];

        let cases = vec![
            (true, START, 0, addr),
            (true, START, 1, addr),
            (true, START, LENGTH, addr),
            (true, START + 1, LENGTH - 1, addr + 1),
            (false, START + 1, LENGTH, 0),
            (true, START + LENGTH - 1, 1, addr + LENGTH - 1),
            (true, START + LENGTH, 0, addr + LENGTH),
            (false, START + LENGTH, 1, 0),
            (false, START, LENGTH + 1, 0),
            (false, 0, 0, 0),
            (false, 0, 1, 0),
            (false, START - 1, 0, 0),
            (false, START - 1, 1, 0),
            (true, START + LENGTH / 2, LENGTH / 2, addr + LENGTH / 2),
        ];
        for (ok, start, length, value) in cases {
            if ok {
                assert_eq!(
                    translate!(start, length, &regions, &bpf_loader::id()).unwrap(),
                    value
                )
            } else {
                assert!(translate!(start, length, &regions, &bpf_loader::id()).is_err())
            }
        }
    }

    #[test]
    fn test_translate_type() {
        // Pubkey
        let pubkey = Pubkey::new_rand();
        let addr = &pubkey as *const _ as u64;
        let regions = vec![MemoryRegion {
            addr_host: addr,
            addr_vm: 100,
            len: std::mem::size_of::<Pubkey>() as u64,
        }];
        let translated_pubkey = translate_type!(Pubkey, 100, &regions, &bpf_loader::id()).unwrap();
        assert_eq!(pubkey, *translated_pubkey);

        // Instruction
        let instruction = Instruction::new(
            Pubkey::new_rand(),
            &"foobar",
            vec![AccountMeta::new(Pubkey::new_rand(), false)],
        );
        let addr = &instruction as *const _ as u64;
        let mut regions = vec![MemoryRegion {
            addr_host: addr,
            addr_vm: 96,
            len: std::mem::size_of::<Instruction>() as u64,
        }];
        let translated_instruction =
            translate_type!(Instruction, 96, &regions, &bpf_loader::id()).unwrap();
        assert_eq!(instruction, *translated_instruction);
        regions[0].len = 1;
        assert!(translate_type!(Instruction, 100, &regions, &bpf_loader::id()).is_err());
    }

    #[test]
    fn test_translate_slice() {
        // u8
        let mut data = vec![1u8, 2, 3, 4, 5];
        let addr = data.as_ptr() as *const _ as u64;
        let regions = vec![MemoryRegion {
            addr_host: addr,
            addr_vm: 100,
            len: data.len() as u64,
        }];
        let translated_data =
            translate_slice!(u8, 100, data.len(), &regions, &bpf_loader::id()).unwrap();
        assert_eq!(data, translated_data);
        data[0] = 10;
        assert_eq!(data, translated_data);

        // Pubkeys
        let mut data = vec![Pubkey::new_rand(); 5];
        let addr = data.as_ptr() as *const _ as u64;
        let regions = vec![MemoryRegion {
            addr_host: addr,
            addr_vm: 100,
            len: (data.len() * std::mem::size_of::<Pubkey>()) as u64,
        }];
        let translated_data =
            translate_slice!(Pubkey, 100, data.len(), &regions, &bpf_loader::id()).unwrap();
        assert_eq!(data, translated_data);
        data[0] = Pubkey::new_rand(); // Both should point to same place
        assert_eq!(data, translated_data);
    }

    #[test]
    fn test_translate_string_and_do() {
        let string = "Gaggablaghblagh!";
        let addr = string.as_ptr() as *const _ as u64;
        let regions = vec![MemoryRegion {
            addr_host: addr,
            addr_vm: 100,
            len: string.len() as u64,
        }];
        assert_eq!(
            42,
            translate_string_and_do(
                100,
                string.len() as u64,
                &regions,
                &bpf_loader::id(),
                &mut |string: &str| {
                    assert_eq!(string, "Gaggablaghblagh!");
                    Ok(42)
                }
            )
            .unwrap()
        );
    }

    #[test]
    #[should_panic(expected = "UserError(SyscallError(Abort))")]
    fn test_syscall_abort() {
        let ro_region = MemoryRegion::default();
        let rw_region = MemoryRegion::default();
        syscall_abort(0, 0, 0, 0, 0, &[ro_region], &[rw_region]).unwrap();
    }

    #[test]
    #[should_panic(expected = "UserError(SyscallError(Panic(\"Gaggablaghblagh!\", 42, 84)))")]
    fn test_syscall_sol_panic() {
        let string = "Gaggablaghblagh!";
        let addr = string.as_ptr() as *const _ as u64;
        let ro_region = MemoryRegion {
            addr_host: addr,
            addr_vm: 100,
            len: string.len() as u64,
        };
        let rw_region = MemoryRegion::default();
        let mut syscall_panic = SyscallPanic {
            loader_id: &bpf_loader::id(),
        };
        syscall_panic
            .call(
                100,
                string.len() as u64,
                42,
                84,
                0,
                &[ro_region],
                &[rw_region],
            )
            .unwrap();
    }

    #[test]
    fn test_syscall_sol_log() {
        let string = "Gaggablaghblagh!";
        let addr = string.as_ptr() as *const _ as u64;

        let compute_meter: Rc<RefCell<dyn ComputeMeter>> =
            Rc::new(RefCell::new(MockComputeMeter { remaining: 3 }));
        let log = Rc::new(RefCell::new(vec![]));
        let logger: Rc<RefCell<dyn Logger>> =
            Rc::new(RefCell::new(MockLogger { log: log.clone() }));
        let mut syscall_sol_log = SyscallLog {
            cost: 1,
            compute_meter,
            logger,
            loader_id: &bpf_loader::id(),
        };
        let ro_regions = &[MemoryRegion {
            addr_host: addr,
            addr_vm: 100,
            len: string.len() as u64,
        }];
        let rw_regions = &[MemoryRegion::default()];

        syscall_sol_log
            .call(100, string.len() as u64, 0, 0, 0, ro_regions, rw_regions)
            .unwrap();

        assert_eq!(
            Err(EbpfError::AccessViolation(
                "programs/bpf_loader/src/syscalls.rs".to_string(),
                248,
                100,
                32,
                "  regions: \n0x64-0x73".to_string()
            )),
            syscall_sol_log.call(
                100,
                string.len() as u64 * 2, // AccessViolation
                0,
                0,
                0,
                ro_regions,
                rw_regions,
            )
        );

        assert_eq!(
            Err(EbpfError::UserError(BPFError::SyscallError(
                SyscallError::InstructionError(InstructionError::ComputationalBudgetExceeded)
            ))),
            syscall_sol_log.call(
                100,
                string.len() as u64 * 2, // AccessViolation
                0,
                0,
                0,
                ro_regions,
                rw_regions,
            )
        );

        assert_eq!(log.borrow().len(), 1);
        assert_eq!(log.borrow()[0], "Program log: Gaggablaghblagh!");
    }

    #[test]
    fn test_syscall_sol_log_u64() {
        let compute_meter: Rc<RefCell<dyn ComputeMeter>> =
            Rc::new(RefCell::new(MockComputeMeter {
                remaining: std::u64::MAX,
            }));
        let log = Rc::new(RefCell::new(vec![]));
        let logger: Rc<RefCell<dyn Logger>> =
            Rc::new(RefCell::new(MockLogger { log: log.clone() }));
        let mut syscall_sol_log_u64 = SyscallLogU64 {
            cost: 0,
            compute_meter,
            logger,
        };
        let ro_regions = &[MemoryRegion::default()];
        let rw_regions = &[MemoryRegion::default()];

        syscall_sol_log_u64
            .call(1, 2, 3, 4, 5, ro_regions, rw_regions)
            .unwrap();

        assert_eq!(log.borrow().len(), 1);
        assert_eq!(log.borrow()[0], "Program log: 0x1, 0x2, 0x3, 0x4, 0x5");
    }

    #[test]
    fn test_syscall_sol_alloc_free() {
        // large alloc
        {
            let heap = vec![0_u8; 100];
            let ro_regions = &[MemoryRegion::default()];
            let rw_regions = &[MemoryRegion::new_from_slice(&heap, MM_HEAP_START)];
            let mut syscall = SyscallSolAllocFree {
                aligned: true,
                allocator: BPFAllocator::new(heap, MM_HEAP_START),
            };
            assert_ne!(
                syscall
                    .call(100, 0, 0, 0, 0, ro_regions, rw_regions)
                    .unwrap(),
                0
            );
            assert_eq!(
                syscall
                    .call(100, 0, 0, 0, 0, ro_regions, rw_regions)
                    .unwrap(),
                0
            );
            assert_eq!(
                syscall
                    .call(u64::MAX, 0, 0, 0, 0, ro_regions, rw_regions)
                    .unwrap(),
                0
            );
        }
        // many small unaligned allocs
        {
            let heap = vec![0_u8; 100];
            let ro_regions = &[MemoryRegion::default()];
            let rw_regions = &[MemoryRegion::new_from_slice(&heap, MM_HEAP_START)];
            let mut syscall = SyscallSolAllocFree {
                aligned: false,
                allocator: BPFAllocator::new(heap, MM_HEAP_START),
            };
            for _ in 0..100 {
                assert_ne!(
                    syscall.call(1, 0, 0, 0, 0, ro_regions, rw_regions).unwrap(),
                    0
                );
            }
            assert_eq!(
                syscall
                    .call(100, 0, 0, 0, 0, ro_regions, rw_regions)
                    .unwrap(),
                0
            );
        }
        // many small aligned allocs
        {
            let heap = vec![0_u8; 100];
            let ro_regions = &[MemoryRegion::default()];
            let rw_regions = &[MemoryRegion::new_from_slice(&heap, MM_HEAP_START)];
            let mut syscall = SyscallSolAllocFree {
                aligned: true,
                allocator: BPFAllocator::new(heap, MM_HEAP_START),
            };
            for _ in 0..12 {
                assert_ne!(
                    syscall.call(1, 0, 0, 0, 0, ro_regions, rw_regions).unwrap(),
                    0
                );
            }
            assert_eq!(
                syscall
                    .call(100, 0, 0, 0, 0, ro_regions, rw_regions)
                    .unwrap(),
                0
            );
        }
        // aligned allocs

        fn check_alignment<T>() {
            let heap = vec![0_u8; 100];
            let ro_regions = &[MemoryRegion::default()];
            let rw_regions = &[MemoryRegion::new_from_slice(&heap, MM_HEAP_START)];
            let mut syscall = SyscallSolAllocFree {
                aligned: true,
                allocator: BPFAllocator::new(heap, MM_HEAP_START),
            };
            let address = syscall
                .call(size_of::<u8>() as u64, 0, 0, 0, 0, ro_regions, rw_regions)
                .unwrap();
            assert_ne!(address, 0);
            assert_eq!((address as *const u8).align_offset(align_of::<u8>()), 0);
        }
        check_alignment::<u8>();
        check_alignment::<u16>();
        check_alignment::<u32>();
        check_alignment::<u64>();
        check_alignment::<u128>();
    }
}
