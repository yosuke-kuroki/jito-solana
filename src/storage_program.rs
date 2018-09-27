//! storage program
//!  Receive mining proofs from miners, validate the answers
//!  and give reward for good proofs.

use bincode::deserialize;
use solana_program_interface::account::Account;
use solana_program_interface::pubkey::Pubkey;
use transaction::Transaction;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum StorageProgram {
    SubmitMiningProof { sha_state: [u8; 32] },
}

pub enum StorageError {
    InvalidUserData,
}

pub const STORAGE_PROGRAM_ID: [u8; 32] = [1u8; 32];

impl StorageProgram {
    pub fn check_id(program_id: &Pubkey) -> bool {
        program_id.as_ref() == STORAGE_PROGRAM_ID
    }

    pub fn id() -> Pubkey {
        Pubkey::new(&STORAGE_PROGRAM_ID)
    }

    pub fn get_balance(account: &Account) -> i64 {
        account.tokens
    }

    pub fn process_transaction(
        tx: &Transaction,
        _accounts: &mut [Account],
    ) -> Result<(), StorageError> {
        if let Ok(syscall) = deserialize(&tx.userdata) {
            match syscall {
                StorageProgram::SubmitMiningProof { sha_state } => {
                    info!("Mining proof submitted with state {}", sha_state[0]);
                    return Ok(());
                }
            }
        } else {
            return Err(StorageError::InvalidUserData);
        }
    }
}

#[cfg(test)]
mod test {}
