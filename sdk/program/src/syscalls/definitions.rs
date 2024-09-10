#[cfg(target_feature = "static-syscalls")]
pub use solana_define_syscall::sys_hash;
#[deprecated(since = "2.1.0", note = "Use `solana_msg::sol_log` instead.")]
pub use solana_msg::sol_log;
#[deprecated(
    since = "2.1.0",
    note = "Use `solana_program_memory::syscalls` instead"
)]
pub use solana_program_memory::syscalls::{sol_memcmp_, sol_memcpy_, sol_memmove_, sol_memset_};
#[deprecated(since = "2.1.0", note = "Use `solana_pubkey::syscalls` instead")]
pub use solana_pubkey::syscalls::{
    sol_create_program_address, sol_log_pubkey, sol_try_find_program_address,
};
#[deprecated(
    since = "2.1.0",
    note = "Use `solana_secp256k1_recover::sol_secp256k1_recover` instead"
)]
pub use solana_secp256k1_recover::sol_secp256k1_recover;
#[deprecated(since = "2.1.0", note = "Use solana_sha256_hasher::sol_sha256 instead")]
pub use solana_sha256_hasher::sol_sha256;
use {
    crate::{
        instruction::{AccountMeta, ProcessedSiblingInstruction},
        pubkey::Pubkey,
    },
    solana_define_syscall::define_syscall,
};
define_syscall!(fn sol_log_64_(arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64));
define_syscall!(fn sol_log_compute_units_());
define_syscall!(fn sol_keccak256(vals: *const u8, val_len: u64, hash_result: *mut u8) -> u64);
define_syscall!(fn sol_blake3(vals: *const u8, val_len: u64, hash_result: *mut u8) -> u64);
define_syscall!(fn sol_invoke_signed_c(instruction_addr: *const u8, account_infos_addr: *const u8, account_infos_len: u64, signers_seeds_addr: *const u8, signers_seeds_len: u64) -> u64);
define_syscall!(fn sol_invoke_signed_rust(instruction_addr: *const u8, account_infos_addr: *const u8, account_infos_len: u64, signers_seeds_addr: *const u8, signers_seeds_len: u64) -> u64);
define_syscall!(fn sol_set_return_data(data: *const u8, length: u64));
define_syscall!(fn sol_get_return_data(data: *mut u8, length: u64, program_id: *mut Pubkey) -> u64);
define_syscall!(fn sol_log_data(data: *const u8, data_len: u64));
define_syscall!(fn sol_get_processed_sibling_instruction(index: u64, meta: *mut ProcessedSiblingInstruction, program_id: *mut Pubkey, data: *mut u8, accounts: *mut AccountMeta) -> u64);
define_syscall!(fn sol_get_stack_height() -> u64);
define_syscall!(fn sol_curve_validate_point(curve_id: u64, point_addr: *const u8, result: *mut u8) -> u64);
define_syscall!(fn sol_curve_group_op(curve_id: u64, group_op: u64, left_input_addr: *const u8, right_input_addr: *const u8, result_point_addr: *mut u8) -> u64);
define_syscall!(fn sol_curve_multiscalar_mul(curve_id: u64, scalars_addr: *const u8, points_addr: *const u8, points_len: u64, result_point_addr: *mut u8) -> u64);
define_syscall!(fn sol_curve_pairing_map(curve_id: u64, point: *const u8, result: *mut u8) -> u64);
define_syscall!(fn sol_alt_bn128_group_op(group_op: u64, input: *const u8, input_size: u64, result: *mut u8) -> u64);
define_syscall!(fn sol_big_mod_exp(params: *const u8, result: *mut u8) -> u64);
define_syscall!(fn sol_remaining_compute_units() -> u64);
define_syscall!(fn sol_alt_bn128_compression(op: u64, input: *const u8, input_size: u64, result: *mut u8) -> u64);
define_syscall!(fn sol_get_sysvar(sysvar_id_addr: *const u8, result: *mut u8, offset: u64, length: u64) -> u64);
define_syscall!(fn sol_get_epoch_stake(vote_address: *const u8) -> u64);

// these are to be deprecated once they are superceded by sol_get_sysvar
define_syscall!(fn sol_get_clock_sysvar(addr: *mut u8) -> u64);
define_syscall!(fn sol_get_epoch_schedule_sysvar(addr: *mut u8) -> u64);
define_syscall!(fn sol_get_rent_sysvar(addr: *mut u8) -> u64);
define_syscall!(fn sol_get_last_restart_slot(addr: *mut u8) -> u64);
define_syscall!(fn sol_get_epoch_rewards_sysvar(addr: *mut u8) -> u64);

// this cannot go through sol_get_sysvar but can be removed once no longer in use
define_syscall!(fn sol_get_fees_sysvar(addr: *mut u8) -> u64);
