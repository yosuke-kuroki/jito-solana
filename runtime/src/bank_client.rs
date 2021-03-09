use crate::bank::Bank;
use solana_sdk::{
    account::Account,
    client::{AsyncClient, Client, SyncClient},
    commitment_config::CommitmentConfig,
    epoch_info::EpochInfo,
    fee_calculator::{FeeCalculator, FeeRateGovernor},
    hash::Hash,
    instruction::Instruction,
    message::Message,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    signers::Signers,
    system_instruction,
    transaction::{self, Transaction},
    transport::{Result, TransportError},
};
use std::{
    io,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc, Mutex,
    },
    thread::{sleep, Builder},
    time::{Duration, Instant},
};

pub struct BankClient {
    bank: Arc<Bank>,
    transaction_sender: Mutex<Sender<Transaction>>,
}

impl Client for BankClient {
    fn tpu_addr(&self) -> String {
        "Local BankClient".to_string()
    }
}

impl AsyncClient for BankClient {
    fn async_send_transaction(&self, transaction: Transaction) -> Result<Signature> {
        let signature = transaction.signatures.get(0).cloned().unwrap_or_default();
        let transaction_sender = self.transaction_sender.lock().unwrap();
        transaction_sender.send(transaction).unwrap();
        Ok(signature)
    }

    fn async_send_message<T: Signers>(
        &self,
        keypairs: &T,
        message: Message,
        recent_blockhash: Hash,
    ) -> Result<Signature> {
        let transaction = Transaction::new(keypairs, message, recent_blockhash);
        self.async_send_transaction(transaction)
    }

    fn async_send_instruction(
        &self,
        keypair: &Keypair,
        instruction: Instruction,
        recent_blockhash: Hash,
    ) -> Result<Signature> {
        let message = Message::new(&[instruction], Some(&keypair.pubkey()));
        self.async_send_message(&[keypair], message, recent_blockhash)
    }

    /// Transfer `lamports` from `keypair` to `pubkey`
    fn async_transfer(
        &self,
        lamports: u64,
        keypair: &Keypair,
        pubkey: &Pubkey,
        recent_blockhash: Hash,
    ) -> Result<Signature> {
        let transfer_instruction =
            system_instruction::transfer(&keypair.pubkey(), pubkey, lamports);
        self.async_send_instruction(keypair, transfer_instruction, recent_blockhash)
    }
}

impl SyncClient for BankClient {
    fn send_and_confirm_message<T: Signers>(
        &self,
        keypairs: &T,
        message: Message,
    ) -> Result<Signature> {
        let blockhash = self.bank.last_blockhash();
        let transaction = Transaction::new(keypairs, message, blockhash);
        self.bank.process_transaction(&transaction)?;
        Ok(transaction.signatures.get(0).cloned().unwrap_or_default())
    }

    /// Create and process a transaction from a single instruction.
    fn send_and_confirm_instruction(
        &self,
        keypair: &Keypair,
        instruction: Instruction,
    ) -> Result<Signature> {
        let message = Message::new(&[instruction], Some(&keypair.pubkey()));
        self.send_and_confirm_message(&[keypair], message)
    }

    /// Transfer `lamports` from `keypair` to `pubkey`
    fn transfer_and_confirm(
        &self,
        lamports: u64,
        keypair: &Keypair,
        pubkey: &Pubkey,
    ) -> Result<Signature> {
        let transfer_instruction =
            system_instruction::transfer(&keypair.pubkey(), pubkey, lamports);
        self.send_and_confirm_instruction(keypair, transfer_instruction)
    }

    fn get_account_data(&self, pubkey: &Pubkey) -> Result<Option<Vec<u8>>> {
        Ok(self.bank.get_account(pubkey).map(|account| account.data))
    }

    fn get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>> {
        Ok(self.bank.get_account(pubkey).map(Account::from))
    }

    fn get_account_with_commitment(
        &self,
        pubkey: &Pubkey,
        _commitment_config: CommitmentConfig,
    ) -> Result<Option<Account>> {
        Ok(self.bank.get_account(pubkey).map(Account::from))
    }

    fn get_balance(&self, pubkey: &Pubkey) -> Result<u64> {
        Ok(self.bank.get_balance(pubkey))
    }

    fn get_balance_with_commitment(
        &self,
        pubkey: &Pubkey,
        _commitment_config: CommitmentConfig,
    ) -> Result<u64> {
        Ok(self.bank.get_balance(pubkey))
    }

    fn get_minimum_balance_for_rent_exemption(&self, data_len: usize) -> Result<u64> {
        Ok(self.bank.get_minimum_balance_for_rent_exemption(data_len))
    }

    fn get_recent_blockhash(&self) -> Result<(Hash, FeeCalculator)> {
        Ok(self.bank.last_blockhash_with_fee_calculator())
    }

    fn get_recent_blockhash_with_commitment(
        &self,
        _commitment_config: CommitmentConfig,
    ) -> Result<(Hash, FeeCalculator, u64)> {
        let (blockhash, fee_calculator) = self.bank.last_blockhash_with_fee_calculator();
        let last_valid_slot = self
            .bank
            .get_blockhash_last_valid_slot(&blockhash)
            .expect("bank blockhash queue should contain blockhash");
        Ok((blockhash, fee_calculator, last_valid_slot))
    }

    fn get_fee_calculator_for_blockhash(&self, blockhash: &Hash) -> Result<Option<FeeCalculator>> {
        Ok(self.bank.get_fee_calculator(blockhash))
    }

    fn get_fee_rate_governor(&self) -> Result<FeeRateGovernor> {
        Ok(self.bank.get_fee_rate_governor().clone())
    }

    fn get_signature_status(
        &self,
        signature: &Signature,
    ) -> Result<Option<transaction::Result<()>>> {
        Ok(self.bank.get_signature_status(signature))
    }

    fn get_signature_status_with_commitment(
        &self,
        signature: &Signature,
        _commitment_config: CommitmentConfig,
    ) -> Result<Option<transaction::Result<()>>> {
        Ok(self.bank.get_signature_status(signature))
    }

    fn get_slot(&self) -> Result<u64> {
        Ok(self.bank.slot())
    }

    fn get_slot_with_commitment(&self, _commitment_config: CommitmentConfig) -> Result<u64> {
        Ok(self.bank.slot())
    }

    fn get_transaction_count(&self) -> Result<u64> {
        Ok(self.bank.transaction_count())
    }

    fn get_transaction_count_with_commitment(
        &self,
        _commitment_config: CommitmentConfig,
    ) -> Result<u64> {
        Ok(self.bank.transaction_count())
    }

    fn poll_for_signature_confirmation(
        &self,
        signature: &Signature,
        min_confirmed_blocks: usize,
    ) -> Result<usize> {
        // https://github.com/solana-labs/solana/issues/7199
        assert_eq!(min_confirmed_blocks, 1, "BankClient cannot observe the passage of multiple blocks, so min_confirmed_blocks must be 1");
        let now = Instant::now();
        let confirmed_blocks;
        loop {
            if self.bank.get_signature_status(signature).is_some() {
                confirmed_blocks = 1;
                break;
            }
            if now.elapsed().as_secs() > 15 {
                return Err(TransportError::IoError(io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "signature not found after {} seconds",
                        now.elapsed().as_secs()
                    ),
                )));
            }
            sleep(Duration::from_millis(250));
        }
        Ok(confirmed_blocks)
    }

    fn poll_for_signature(&self, signature: &Signature) -> Result<()> {
        let now = Instant::now();
        loop {
            let response = self.bank.get_signature_status(signature);
            if let Some(res) = response {
                if res.is_ok() {
                    break;
                }
            }
            if now.elapsed().as_secs() > 15 {
                return Err(TransportError::IoError(io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "signature not found after {} seconds",
                        now.elapsed().as_secs()
                    ),
                )));
            }
            sleep(Duration::from_millis(250));
        }
        Ok(())
    }

    fn get_new_blockhash(&self, blockhash: &Hash) -> Result<(Hash, FeeCalculator)> {
        let (last_blockhash, fee_calculator) = self.get_recent_blockhash()?;
        if last_blockhash != *blockhash {
            Ok((last_blockhash, fee_calculator))
        } else {
            Err(TransportError::IoError(io::Error::new(
                io::ErrorKind::Other,
                "Unable to get new blockhash",
            )))
        }
    }

    fn get_epoch_info(&self) -> Result<EpochInfo> {
        Ok(self.bank.get_epoch_info())
    }
}

impl BankClient {
    fn run(bank: &Bank, transaction_receiver: Receiver<Transaction>) {
        while let Ok(tx) = transaction_receiver.recv() {
            let mut transactions = vec![tx];
            while let Ok(tx) = transaction_receiver.try_recv() {
                transactions.push(tx);
            }
            let _ = bank.process_transactions(&transactions);
        }
    }

    pub fn new_shared(bank: &Arc<Bank>) -> Self {
        let (transaction_sender, transaction_receiver) = channel();
        let transaction_sender = Mutex::new(transaction_sender);
        let thread_bank = bank.clone();
        let bank = bank.clone();
        Builder::new()
            .name("solana-bank-client".to_string())
            .spawn(move || Self::run(&thread_bank, transaction_receiver))
            .unwrap();
        Self {
            bank,
            transaction_sender,
        }
    }

    pub fn new(bank: Bank) -> Self {
        Self::new_shared(&Arc::new(bank))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{genesis_config::create_genesis_config, instruction::AccountMeta};

    #[test]
    fn test_bank_client_new_with_keypairs() {
        let (genesis_config, john_doe_keypair) = create_genesis_config(10_000);
        let john_pubkey = john_doe_keypair.pubkey();
        let jane_doe_keypair = Keypair::new();
        let jane_pubkey = jane_doe_keypair.pubkey();
        let doe_keypairs = vec![&john_doe_keypair, &jane_doe_keypair];
        let bank = Bank::new(&genesis_config);
        let bank_client = BankClient::new(bank);

        // Create 2-2 Multisig Transfer instruction.
        let bob_pubkey = solana_sdk::pubkey::new_rand();
        let mut transfer_instruction = system_instruction::transfer(&john_pubkey, &bob_pubkey, 42);
        transfer_instruction
            .accounts
            .push(AccountMeta::new(jane_pubkey, true));

        let message = Message::new(&[transfer_instruction], Some(&john_pubkey));
        bank_client
            .send_and_confirm_message(&doe_keypairs, message)
            .unwrap();
        assert_eq!(bank_client.get_balance(&bob_pubkey).unwrap(), 42);
    }
}
