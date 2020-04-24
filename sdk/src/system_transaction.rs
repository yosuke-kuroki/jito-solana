//! The `system_transaction` module provides functionality for creating system transactions.

use crate::{
    hash::Hash,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};

/// Create and sign new SystemInstruction::CreateAccount transaction
pub fn create_account(
    from_keypair: &Keypair,
    to_keypair: &Keypair,
    recent_blockhash: Hash,
    lamports: u64,
    space: u64,
    program_id: &Pubkey,
) -> Transaction {
    let from_pubkey = from_keypair.pubkey();
    let to_pubkey = to_keypair.pubkey();
    let create_instruction =
        system_instruction::create_account(&from_pubkey, &to_pubkey, lamports, space, program_id);
    Transaction::new_signed_instructions(
        &[from_keypair, to_keypair],
        &[create_instruction],
        recent_blockhash,
    )
}

/// Create and sign new system_instruction::Assign transaction
pub fn assign(from_keypair: &Keypair, recent_blockhash: Hash, program_id: &Pubkey) -> Transaction {
    let from_pubkey = from_keypair.pubkey();
    let assign_instruction = system_instruction::assign(&from_pubkey, program_id);
    Transaction::new_signed_instructions(&[from_keypair], &[assign_instruction], recent_blockhash)
}

/// Create and sign new system_instruction::Transfer transaction
pub fn transfer(
    from_keypair: &Keypair,
    to: &Pubkey,
    lamports: u64,
    recent_blockhash: Hash,
) -> Transaction {
    let from_pubkey = from_keypair.pubkey();
    let transfer_instruction = system_instruction::transfer(&from_pubkey, to, lamports);
    Transaction::new_signed_instructions(&[from_keypair], &[transfer_instruction], recent_blockhash)
}

/// Create and sign new nonced system_instruction::Transfer transaction
pub fn nonced_transfer(
    from_keypair: &Keypair,
    to: &Pubkey,
    lamports: u64,
    nonce_account: &Pubkey,
    nonce_authority: &Keypair,
    nonce_hash: Hash,
) -> Transaction {
    let from_pubkey = from_keypair.pubkey();
    let transfer_instruction = system_instruction::transfer(&from_pubkey, to, lamports);
    let instructions = vec![transfer_instruction];
    Transaction::new_signed_with_nonce(
        instructions,
        Some(&from_pubkey),
        &[from_keypair, nonce_authority],
        nonce_account,
        &nonce_authority.pubkey(),
        nonce_hash,
    )
}
