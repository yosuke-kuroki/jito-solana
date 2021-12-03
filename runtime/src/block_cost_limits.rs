//! defines block cost related limits
//!
use {
    lazy_static::lazy_static,
    solana_sdk::{
        feature, incinerator, native_loader, pubkey::Pubkey, secp256k1_program, system_program,
    },
    std::collections::HashMap,
};

/// Static configurations:
///
/// Number of microseconds replaying a block should take, 400 millisecond block times
/// is curerntly publicly communicated on solana.com
pub const MAX_BLOCK_REPLAY_TIME_US: u64 = 400_000;
/// number of concurrent processes,
pub const MAX_CONCURRENCY: u64 = 10;

/// Cluster data, method of collecting at https://github.com/solana-labs/solana/issues/19627
/// Dashboard: https://metrics.solana.com:8889/sources/0/dashboards/10?refresh=Paused&lower=now%28%29%20-%2012h
///
/// cluster avergaed compute unit to microsec conversion rate
pub const COMPUTE_UNIT_TO_US_RATIO: u64 = 40;
/// Number of compute units for one signature verification.
pub const SIGNATURE_COST: u64 = COMPUTE_UNIT_TO_US_RATIO * 130;
/// Number of compute units for one write lock
pub const WRITE_LOCK_UNITS: u64 = COMPUTE_UNIT_TO_US_RATIO * 10;
/// Number of data bytes per compute units
pub const DATA_BYTES_UNITS: u64 = 220 /*bytes per us*/ / COMPUTE_UNIT_TO_US_RATIO;
// Number of compute units for each built-in programs
lazy_static! {
    /// Number of compute units for each built-in programs
    pub static ref BUILT_IN_INSTRUCTION_COSTS: HashMap<Pubkey, u64> = [
        (feature::id(), COMPUTE_UNIT_TO_US_RATIO * 2),
        (incinerator::id(), COMPUTE_UNIT_TO_US_RATIO * 2),
        (native_loader::id(), COMPUTE_UNIT_TO_US_RATIO * 2),
        (solana_sdk::stake::config::id(), COMPUTE_UNIT_TO_US_RATIO * 2),
        (solana_sdk::stake::program::id(), COMPUTE_UNIT_TO_US_RATIO * 25),
        (solana_config_program::id(), COMPUTE_UNIT_TO_US_RATIO * 15),
        (solana_vote_program::id(), COMPUTE_UNIT_TO_US_RATIO * 85),
        (secp256k1_program::id(), COMPUTE_UNIT_TO_US_RATIO * 4),
        (system_program::id(), COMPUTE_UNIT_TO_US_RATIO * 10),
    ]
    .iter()
    .cloned()
    .collect();
}

/// Statically computed data:
///
/// Number of compute units that a block is allowed. A block's compute units are
/// accumualted by Transactions added to it; A transaction's compute units are
/// calculated by cost_model, based on transaction's signarures, write locks,
/// data size and built-in and BPF instructinos.
pub const MAX_BLOCK_UNITS: u64 =
    MAX_BLOCK_REPLAY_TIME_US * COMPUTE_UNIT_TO_US_RATIO * MAX_CONCURRENCY;
/// Number of compute units that a writable account in a block is allowed. The
/// limit is to prevent too many transactions write to same account, threrefore
/// reduce block's paralellism.
pub const MAX_WRITABLE_ACCOUNT_UNITS: u64 = MAX_BLOCK_REPLAY_TIME_US * COMPUTE_UNIT_TO_US_RATIO;
