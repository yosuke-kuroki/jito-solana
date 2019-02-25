//! The `genesis_block` module is a library for generating the chain's genesis block.

use crate::hash::{hash, Hash};
use crate::pubkey::Pubkey;
use crate::signature::{Keypair, KeypairUtil};
use crate::timing::{DEFAULT_SLOTS_PER_EPOCH, DEFAULT_TICKS_PER_SLOT};
use std::fs::File;
use std::io::Write;
use std::path::Path;

// The default (and minimal) amount of tokens given to the bootstrap leader:
// * 1 token for the bootstrap leader ID account
// * 1 token for the bootstrap leader vote account
pub const BOOTSTRAP_LEADER_TOKENS: u64 = 2;

#[derive(Serialize, Deserialize, Debug)]
pub struct GenesisBlock {
    pub bootstrap_leader_id: Pubkey,
    pub bootstrap_leader_tokens: u64,
    pub bootstrap_leader_vote_account_id: Pubkey,
    pub mint_id: Pubkey,
    pub tokens: u64,
    pub ticks_per_slot: u64,
    pub slots_per_epoch: u64,
    pub stakers_slot_offset: u64,
}

impl GenesisBlock {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(tokens: u64) -> (Self, Keypair) {
        let tokens = tokens
            .checked_add(BOOTSTRAP_LEADER_TOKENS)
            .unwrap_or(tokens);
        Self::new_with_leader(tokens, Keypair::new().pubkey(), BOOTSTRAP_LEADER_TOKENS)
    }

    pub fn new_with_leader(
        tokens: u64,
        bootstrap_leader_id: Pubkey,
        bootstrap_leader_tokens: u64,
    ) -> (Self, Keypair) {
        let mint_keypair = Keypair::new();
        let bootstrap_leader_vote_account_keypair = Keypair::new();
        (
            Self {
                bootstrap_leader_id,
                bootstrap_leader_tokens,
                bootstrap_leader_vote_account_id: bootstrap_leader_vote_account_keypair.pubkey(),
                mint_id: mint_keypair.pubkey(),
                tokens,
                ticks_per_slot: DEFAULT_TICKS_PER_SLOT,
                slots_per_epoch: DEFAULT_SLOTS_PER_EPOCH,
                stakers_slot_offset: DEFAULT_SLOTS_PER_EPOCH,
            },
            mint_keypair,
        )
    }

    pub fn last_id(&self) -> Hash {
        let serialized = serde_json::to_string(self).unwrap();
        hash(&serialized.into_bytes())
    }

    pub fn load(ledger_path: &str) -> Result<Self, std::io::Error> {
        let file = File::open(&Path::new(ledger_path).join("genesis.json"))?;
        let genesis_block = serde_json::from_reader(file)?;
        Ok(genesis_block)
    }

    pub fn write(&self, ledger_path: &str) -> Result<(), std::io::Error> {
        let serialized = serde_json::to_string(self)?;
        let mut file = File::create(&Path::new(ledger_path).join("genesis.json"))?;
        file.write_all(&serialized.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_block_new() {
        let (genesis_block, mint) = GenesisBlock::new(10_000);
        assert_eq!(genesis_block.tokens, 10_000 + BOOTSTRAP_LEADER_TOKENS);
        assert_eq!(genesis_block.mint_id, mint.pubkey());
        assert!(genesis_block.bootstrap_leader_id != Pubkey::default());
        assert!(genesis_block.bootstrap_leader_vote_account_id != Pubkey::default());
        assert_eq!(
            genesis_block.bootstrap_leader_tokens,
            BOOTSTRAP_LEADER_TOKENS
        );
    }

    #[test]
    fn test_genesis_block_new_with_leader() {
        let leader_keypair = Keypair::new();
        let (genesis_block, mint) =
            GenesisBlock::new_with_leader(20_000, leader_keypair.pubkey(), 123);

        assert_eq!(genesis_block.tokens, 20_000);
        assert_eq!(genesis_block.mint_id, mint.pubkey());
        assert_eq!(genesis_block.bootstrap_leader_id, leader_keypair.pubkey());
        assert_eq!(genesis_block.bootstrap_leader_tokens, 123);
    }
}
