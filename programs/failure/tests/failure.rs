use solana_runtime::bank::Bank;
use solana_runtime::bank_client::BankClient;
use solana_runtime::loader_utils::create_invoke_instruction;
use solana_sdk::client::SyncClient;
use solana_sdk::genesis_config::create_genesis_config;
use solana_sdk::instruction::InstructionError;
use solana_sdk::signature::Signer;
use solana_sdk::transaction::TransactionError;

#[test]
fn test_program_native_failure() {
    let (genesis_config, alice_keypair) = create_genesis_config(50);
    let program_id = solana_sdk::pubkey::new_rand();
    let bank = Bank::new(&genesis_config);
    bank.add_native_program("solana_failure_program", &program_id, false);

    // Call user program
    let instruction = create_invoke_instruction(alice_keypair.pubkey(), program_id, &1u8);
    let bank_client = BankClient::new(bank);
    assert_eq!(
        bank_client
            .send_and_confirm_instruction(&alice_keypair, instruction)
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(0, InstructionError::Custom(0))
    );
}
