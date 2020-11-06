#![cfg_attr(RUSTC_WITH_SPECIALIZATION, feature(specialization))]
use solana_sdk::genesis_config::GenesisConfig;

pub mod config;
pub mod legacy_stake_processor;
pub mod legacy_stake_state;
pub mod stake_instruction;
pub mod stake_state;

solana_sdk::declare_id!("Stake11111111111111111111111111111111111111");

pub fn add_genesis_accounts(genesis_config: &mut GenesisConfig) -> u64 {
    config::add_genesis_account(genesis_config)
}

#[macro_use]
extern crate solana_frozen_abi_macro;
