use serde::Serialize;
use solana_sdk::pubkey::Pubkey;

mod config_instruction;
pub mod config_processor;

pub use config_instruction::ConfigInstruction;

const CONFIG_PROGRAM_ID: [u8; 32] = [
    133, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0,
];

pub fn check_id(program_id: &Pubkey) -> bool {
    program_id.as_ref() == CONFIG_PROGRAM_ID
}

pub fn id() -> Pubkey {
    Pubkey::new(&CONFIG_PROGRAM_ID)
}

pub trait ConfigState: Serialize {
    /// Maximum space that the serialized representation will require
    fn max_space() -> u64;
}
