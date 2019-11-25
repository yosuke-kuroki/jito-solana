//! config for staking
//!  carries variables that the stake program cares about
use bincode::{deserialize, serialized_size};
use serde_derive::{Deserialize, Serialize};
use solana_config_program::{create_config_account, get_config_data, ConfigState};
use solana_sdk::{
    account::{Account, KeyedAccount},
    instruction::InstructionError,
    pubkey::Pubkey,
};

// stake config ID
solana_sdk::declare_id!("StakeConfig11111111111111111111111111111111");

// means that no more than RATE of current effective stake may be added or subtracted per
//  epoch
pub const DEFAULT_WARMUP_COOLDOWN_RATE: f64 = 0.25;
pub const DEFAULT_SLASH_PENALTY: u8 = ((5 * std::u8::MAX as usize) / 100) as u8;

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
pub struct Config {
    /// how much stake we can activate/deactivate per-epoch as a fraction of currently effective stake
    pub warmup_cooldown_rate: f64,
    /// percentage of stake lost when slash, expressed as a portion of std::u8::MAX
    pub slash_penalty: u8,
}

impl Config {
    pub fn from(account: &Account) -> Option<Self> {
        get_config_data(&account.data)
            .ok()
            .and_then(|data| deserialize(data).ok())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            warmup_cooldown_rate: DEFAULT_WARMUP_COOLDOWN_RATE,
            slash_penalty: DEFAULT_SLASH_PENALTY,
        }
    }
}

impl ConfigState for Config {
    fn max_space() -> u64 {
        serialized_size(&Config::default()).unwrap()
    }
}

pub fn create_genesis_account() -> (Pubkey, Account) {
    (id(), create_config_account(vec![], &Config::default(), 100))
}

pub fn create_account(lamports: u64, config: &Config) -> Account {
    create_config_account(vec![], config, lamports)
}

pub fn from_keyed_account(account: &KeyedAccount) -> Result<Config, InstructionError> {
    if !check_id(account.unsigned_key()) {
        return Err(InstructionError::InvalidArgument);
    }
    Config::from(account.account).ok_or(InstructionError::InvalidArgument)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        let mut account = create_account(1, &Config::default());
        assert_eq!(Config::from(&account), Some(Config::default()));
        assert_eq!(
            from_keyed_account(&KeyedAccount::new(&Pubkey::default(), false, &mut account)),
            Err(InstructionError::InvalidArgument)
        );
        let (pubkey, mut account) = create_genesis_account();
        assert_eq!(
            from_keyed_account(&KeyedAccount::new(&pubkey, false, &mut account)),
            Ok(Config::default())
        );
    }
}
