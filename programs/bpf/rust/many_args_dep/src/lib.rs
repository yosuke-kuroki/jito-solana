//! @brief Solana Rust-based BPF program utility functions and types

#![no_std]

extern crate solana_sdk_bpf_utils;

use solana_sdk_bpf_utils::info;

pub fn many_args(
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    arg6: u64,
    arg7: u64,
    arg8: u64,
    arg9: u64,
) -> u64 {
    info!("Another package");
    info!(arg1, arg2, arg3, arg4, arg5);
    info!(arg6, arg7, arg8, arg9, 0);
    arg1 + arg2 + arg3 + arg4 + arg5 + arg6 + arg7 + arg8 + arg9
}

#[cfg(test)]
mod test {
    extern crate std;
    use super::*;

    #[test]
    fn pull_in_externs() {
        // Rust on Linux excludes the solana_sdk_bpf_test library unless there is a
        // direct dependency, use this test to force the pull in of the library.
        // This is not necessary on macos and unfortunate on Linux
        // Issue #4972
        extern crate solana_sdk_bpf_test;
        use solana_sdk_bpf_test::*;
        unsafe { sol_log_("X".as_ptr(), 1) };
        sol_log_64_(1, 2, 3, 4, 5);
    }

    #[test]
    fn test_many_args() {
        assert_eq!(45, many_args(1, 2, 3, 4, 5, 6, 7, 8, 9));
    }
}
