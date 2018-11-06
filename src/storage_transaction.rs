use hash::Hash;
use signature::{Keypair, KeypairUtil};
use storage_program::StorageProgram;
use transaction::Transaction;

pub trait StorageTransaction {
    fn storage_new_mining_proof(from_keypair: &Keypair, sha_state: Hash, last_id: Hash) -> Self;
}

impl StorageTransaction for Transaction {
    fn storage_new_mining_proof(from_keypair: &Keypair, sha_state: Hash, last_id: Hash) -> Self {
        let program = StorageProgram::SubmitMiningProof { sha_state };
        Transaction::new(
            from_keypair,
            &[from_keypair.pubkey()],
            StorageProgram::id(),
            &program,
            last_id,
            0,
        )
    }
}
