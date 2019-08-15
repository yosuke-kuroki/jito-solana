pub mod config;
pub mod rewards_pools;
pub mod stake_instruction;
pub mod stake_state;

const STAKE_PROGRAM_ID: [u8; 32] = [
    6, 161, 216, 23, 145, 55, 84, 42, 152, 52, 55, 189, 254, 42, 122, 178, 85, 127, 83, 92, 138,
    120, 114, 43, 104, 164, 157, 192, 0, 0, 0, 0,
];

solana_sdk::solana_name_id!(
    STAKE_PROGRAM_ID,
    "Stake11111111111111111111111111111111111111"
);

use solana_sdk::genesis_block::Builder;

pub fn genesis(mut builder: Builder) -> Builder {
    for (pubkey, account) in crate::rewards_pools::genesis().iter() {
        builder = builder.rewards_pool(*pubkey, account.clone());
    }
    builder.accounts(&[crate::config::genesis()])
}
