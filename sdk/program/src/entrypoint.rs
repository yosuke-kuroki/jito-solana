//! @brief Solana Rust-based BPF program entry point supported by the latest
//! BPFLoader.  For more information see './bpf_loader.rs'

extern crate alloc;
use crate::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey};
use alloc::vec::Vec;
use std::{
    alloc::Layout,
    cell::RefCell,
    mem::{align_of, size_of},
    ptr::null_mut,
    rc::Rc,
    // Hide Result from bindgen gets confused about generics in non-generic type declarations
    result::Result as ResultGeneric,
    slice::{from_raw_parts, from_raw_parts_mut},
};

pub type ProgramResult = ResultGeneric<(), ProgramError>;

/// User implemented function to process an instruction
///
/// program_id: Program ID of the currently executing program accounts: Accounts
/// passed as part of the instruction instruction_data: Instruction data
pub type ProcessInstruction =
    fn(program_id: &Pubkey, accounts: &[AccountInfo], instruction_data: &[u8]) -> ProgramResult;

/// Programs indicate success with a return value of 0
pub const SUCCESS: u64 = 0;

/// Start address of the memory region used for program heap.
pub const HEAP_START_ADDRESS: usize = 0x300000000;
/// Length of the heap memory region used for program heap.
pub const HEAP_LENGTH: usize = 32 * 1024;

/// Declare the entry point of the program and use the default local heap
/// implementation
///
/// Deserialize the program input arguments and call the user defined
/// `process_instruction` function. Users must call this macro otherwise an
/// entry point for their program will not be created.
///
/// If the program defines the feature `custom-heap` then the default heap
/// implementation will not be included and the program is free to implement
/// their own `#[global_allocator]`
#[macro_export]
macro_rules! entrypoint {
    ($process_instruction:ident) => {
        #[cfg(all(not(feature = "custom-heap"), not(test)))]
        #[global_allocator]
        static A: $crate::entrypoint::BumpAllocator = $crate::entrypoint::BumpAllocator {
            start: $crate::entrypoint::HEAP_START_ADDRESS,
            len: $crate::entrypoint::HEAP_LENGTH,
        };

        /// # Safety
        #[no_mangle]
        pub unsafe extern "C" fn entrypoint(input: *mut u8) -> u64 {
            let (program_id, accounts, instruction_data) =
                unsafe { $crate::entrypoint::deserialize(input) };
            match $process_instruction(&program_id, &accounts, &instruction_data) {
                Ok(()) => $crate::entrypoint::SUCCESS,
                Err(error) => error.into(),
            }
        }
    };
}

/// The bump allocator used as the default rust heap when running programs.
pub struct BumpAllocator {
    pub start: usize,
    pub len: usize,
}
unsafe impl std::alloc::GlobalAlloc for BumpAllocator {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pos_ptr = self.start as *mut usize;

        let mut pos = *pos_ptr;
        if pos == 0 {
            // First time, set starting position
            pos = self.start + self.len;
        }
        pos = pos.saturating_sub(layout.size());
        pos &= !(layout.align().wrapping_sub(1));
        if pos < self.start + size_of::<*mut u8>() {
            return null_mut();
        }
        *pos_ptr = pos;
        pos as *mut u8
    }
    #[inline]
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {
        // I'm a bump allocator, I don't free
    }
}

/// Maximum number of bytes a program may add to an account during a single realloc
pub const MAX_PERMITTED_DATA_INCREASE: usize = 1_024 * 10;

/// Deserialize the input arguments
///
/// # Safety
#[allow(clippy::type_complexity)]
pub unsafe fn deserialize<'a>(input: *mut u8) -> (&'a Pubkey, Vec<AccountInfo<'a>>, &'a [u8]) {
    let mut offset: usize = 0;

    // Number of accounts present

    #[allow(clippy::cast_ptr_alignment)]
    let num_accounts = *(input.add(offset) as *const u64) as usize;
    offset += size_of::<u64>();

    // Account Infos

    let mut accounts = Vec::with_capacity(num_accounts);
    for _ in 0..num_accounts {
        let dup_info = *(input.add(offset) as *const u8);
        offset += size_of::<u8>();
        if dup_info == std::u8::MAX {
            #[allow(clippy::cast_ptr_alignment)]
            let is_signer = *(input.add(offset) as *const u8) != 0;
            offset += size_of::<u8>();

            #[allow(clippy::cast_ptr_alignment)]
            let is_writable = *(input.add(offset) as *const u8) != 0;
            offset += size_of::<u8>();

            #[allow(clippy::cast_ptr_alignment)]
            let executable = *(input.add(offset) as *const u8) != 0;
            offset += size_of::<u8>();

            offset += size_of::<u32>(); // padding to u64

            let key: &Pubkey = &*(input.add(offset) as *const Pubkey);
            offset += size_of::<Pubkey>();

            let owner: &Pubkey = &*(input.add(offset) as *const Pubkey);
            offset += size_of::<Pubkey>();

            #[allow(clippy::cast_ptr_alignment)]
            let lamports = Rc::new(RefCell::new(&mut *(input.add(offset) as *mut u64)));
            offset += size_of::<u64>();

            #[allow(clippy::cast_ptr_alignment)]
            let data_len = *(input.add(offset) as *const u64) as usize;
            offset += size_of::<u64>();

            let data = Rc::new(RefCell::new({
                from_raw_parts_mut(input.add(offset), data_len)
            }));
            offset += data_len + MAX_PERMITTED_DATA_INCREASE;
            offset += (offset as *const u8).align_offset(align_of::<u128>()); // padding

            #[allow(clippy::cast_ptr_alignment)]
            let rent_epoch = *(input.add(offset) as *const u64);
            offset += size_of::<u64>();

            accounts.push(AccountInfo {
                is_signer,
                is_writable,
                key,
                lamports,
                data,
                owner,
                executable,
                rent_epoch,
            });
        } else {
            offset += 7; // padding

            // Duplicate account, clone the original
            accounts.push(accounts[dup_info as usize].clone());
        }
    }

    // Instruction data

    #[allow(clippy::cast_ptr_alignment)]
    let instruction_data_len = *(input.add(offset) as *const u64) as usize;
    offset += size_of::<u64>();

    let instruction_data = { from_raw_parts(input.add(offset), instruction_data_len) };
    offset += instruction_data_len;

    // Program Id

    let program_id: &Pubkey = &*(input.add(offset) as *const Pubkey);

    (program_id, accounts, instruction_data)
}

#[cfg(test)]
mod test {
    use super::*;
    use std::alloc::GlobalAlloc;

    #[test]
    fn test_bump_allocator() {
        // alloc the entire
        {
            let heap = vec![0u8; 128];
            let allocator = BumpAllocator {
                start: heap.as_ptr() as *const _ as usize,
                len: heap.len(),
            };
            for i in 0..128 - size_of::<*mut u8>() {
                let ptr = unsafe {
                    allocator.alloc(Layout::from_size_align(1, size_of::<u8>()).unwrap())
                };
                assert_eq!(
                    ptr as *const _ as usize,
                    heap.as_ptr() as *const _ as usize + heap.len() - 1 - i
                );
            }
            assert_eq!(null_mut(), unsafe {
                allocator.alloc(Layout::from_size_align(1, 1).unwrap())
            });
        }
        // check alignment
        {
            let heap = vec![0u8; 128];
            let allocator = BumpAllocator {
                start: heap.as_ptr() as *const _ as usize,
                len: heap.len(),
            };
            let ptr =
                unsafe { allocator.alloc(Layout::from_size_align(1, size_of::<u8>()).unwrap()) };
            assert_eq!(0, ptr.align_offset(size_of::<u8>()));
            let ptr =
                unsafe { allocator.alloc(Layout::from_size_align(1, size_of::<u16>()).unwrap()) };
            assert_eq!(0, ptr.align_offset(size_of::<u16>()));
            let ptr =
                unsafe { allocator.alloc(Layout::from_size_align(1, size_of::<u32>()).unwrap()) };
            assert_eq!(0, ptr.align_offset(size_of::<u32>()));
            let ptr =
                unsafe { allocator.alloc(Layout::from_size_align(1, size_of::<u64>()).unwrap()) };
            assert_eq!(0, ptr.align_offset(size_of::<u64>()));
            let ptr =
                unsafe { allocator.alloc(Layout::from_size_align(1, size_of::<u128>()).unwrap()) };
            assert_eq!(0, ptr.align_offset(size_of::<u128>()));
            let ptr = unsafe { allocator.alloc(Layout::from_size_align(1, 64).unwrap()) };
            assert_eq!(0, ptr.align_offset(64));
        }
        // alloc entire block (minus the pos ptr)
        {
            let heap = vec![0u8; 128];
            let allocator = BumpAllocator {
                start: heap.as_ptr() as *const _ as usize,
                len: heap.len(),
            };
            let ptr =
                unsafe { allocator.alloc(Layout::from_size_align(120, size_of::<u8>()).unwrap()) };
            assert_ne!(ptr, null_mut());
            assert_eq!(0, ptr.align_offset(size_of::<u64>()));
        }
    }
}
