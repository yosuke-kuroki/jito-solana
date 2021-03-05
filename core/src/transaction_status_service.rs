use crossbeam_channel::{Receiver, RecvTimeoutError};
use itertools::izip;
use solana_ledger::{blockstore::Blockstore, blockstore_processor::TransactionStatusBatch};
use solana_runtime::{
    bank::{Bank, InnerInstructionsList, NonceRollbackInfo, TransactionLogMessages},
    transaction_utils::OrderedIterator,
};
use solana_transaction_status::{InnerInstructions, TransactionStatusMeta};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, Builder, JoinHandle},
    time::Duration,
};

pub struct TransactionStatusService {
    thread_hdl: JoinHandle<()>,
}

impl TransactionStatusService {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        write_transaction_status_receiver: Receiver<TransactionStatusBatch>,
        blockstore: Arc<Blockstore>,
        exit: &Arc<AtomicBool>,
    ) -> Self {
        let exit = exit.clone();
        let thread_hdl = Builder::new()
            .name("solana-transaction-status-writer".to_string())
            .spawn(move || loop {
                if exit.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(RecvTimeoutError::Disconnected) = Self::write_transaction_status_batch(
                    &write_transaction_status_receiver,
                    &blockstore,
                ) {
                    break;
                }
            })
            .unwrap();
        Self { thread_hdl }
    }

    fn write_transaction_status_batch(
        write_transaction_status_receiver: &Receiver<TransactionStatusBatch>,
        blockstore: &Arc<Blockstore>,
    ) -> Result<(), RecvTimeoutError> {
        let TransactionStatusBatch {
            bank,
            transactions,
            iteration_order,
            statuses,
            balances,
            token_balances,
            inner_instructions,
            transaction_logs,
        } = write_transaction_status_receiver.recv_timeout(Duration::from_secs(1))?;

        let slot = bank.slot();
        let inner_instructions_iter: Box<dyn Iterator<Item = Option<InnerInstructionsList>>> =
            if let Some(inner_instructions) = inner_instructions {
                Box::new(inner_instructions.into_iter())
            } else {
                Box::new(std::iter::repeat_with(|| None))
            };
        let transaction_logs_iter: Box<dyn Iterator<Item = TransactionLogMessages>> =
            if let Some(transaction_logs) = transaction_logs {
                Box::new(transaction_logs.into_iter())
            } else {
                Box::new(std::iter::repeat_with(Vec::new))
            };
        for (
            (_, transaction),
            (status, nonce_rollback),
            pre_balances,
            post_balances,
            pre_token_balances,
            post_token_balances,
            inner_instructions,
            log_messages,
        ) in izip!(
            OrderedIterator::new(&transactions, iteration_order.as_deref()),
            statuses,
            balances.pre_balances,
            balances.post_balances,
            token_balances.pre_token_balances,
            token_balances.post_token_balances,
            inner_instructions_iter,
            transaction_logs_iter
        ) {
            if Bank::can_commit(&status) && !transaction.signatures.is_empty() {
                let fee_calculator = nonce_rollback
                    .map(|nonce_rollback| nonce_rollback.fee_calculator())
                    .unwrap_or_else(|| {
                        bank.get_fee_calculator(&transaction.message().recent_blockhash)
                    })
                    .expect("FeeCalculator must exist");
                let fee = fee_calculator.calculate_fee(transaction.message());
                let (writable_keys, readonly_keys) =
                    transaction.message.get_account_keys_by_lock_type();

                let inner_instructions = inner_instructions.map(|inner_instructions| {
                    inner_instructions
                        .into_iter()
                        .enumerate()
                        .map(|(index, instructions)| InnerInstructions {
                            index: index as u8,
                            instructions,
                        })
                        .filter(|i| !i.instructions.is_empty())
                        .collect()
                });

                let log_messages = Some(log_messages);
                let pre_token_balances = Some(pre_token_balances);
                let post_token_balances = Some(post_token_balances);

                blockstore
                    .write_transaction_status(
                        slot,
                        transaction.signatures[0],
                        writable_keys,
                        readonly_keys,
                        TransactionStatusMeta {
                            status,
                            fee,
                            pre_balances,
                            post_balances,
                            inner_instructions,
                            log_messages,
                            pre_token_balances,
                            post_token_balances,
                        },
                    )
                    .expect("Expect database write to succeed");
            }
        }
        Ok(())
    }

    pub fn join(self) -> thread::Result<()> {
        self.thread_hdl.join()
    }
}
