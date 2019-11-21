use crate::result::{Error, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use solana_client::rpc_request::RpcTransactionStatus;
use solana_ledger::{blocktree::Blocktree, blocktree_processor::TransactionStatusBatch};
use solana_runtime::bank::Bank;
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
        blocktree: Arc<Blocktree>,
        exit: &Arc<AtomicBool>,
    ) -> Self {
        let exit = exit.clone();
        let thread_hdl = Builder::new()
            .name("solana-transaction-status-writer".to_string())
            .spawn(move || loop {
                if exit.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(e) = Self::write_transaction_status_batch(
                    &write_transaction_status_receiver,
                    &blocktree,
                ) {
                    match e {
                        Error::CrossbeamRecvTimeoutError(RecvTimeoutError::Disconnected) => break,
                        Error::CrossbeamRecvTimeoutError(RecvTimeoutError::Timeout) => (),
                        _ => info!("Error from write_transaction_status_batch: {:?}", e),
                    }
                }
            })
            .unwrap();
        Self { thread_hdl }
    }

    fn write_transaction_status_batch(
        write_transaction_status_receiver: &Receiver<TransactionStatusBatch>,
        blocktree: &Arc<Blocktree>,
    ) -> Result<()> {
        let TransactionStatusBatch {
            bank,
            transactions,
            statuses,
        } = write_transaction_status_receiver.recv_timeout(Duration::from_secs(1))?;

        let slot = bank.slot();
        for (transaction, status) in transactions.iter().zip(statuses) {
            if Bank::can_commit(&status) && !transaction.signatures.is_empty() {
                let fee_calculator = bank
                    .get_fee_calculator(&transaction.message().recent_blockhash)
                    .expect("FeeCalculator must exist");
                let fee = fee_calculator.calculate_fee(transaction.message());
                blocktree
                    .write_transaction_status(
                        (slot, transaction.signatures[0]),
                        &RpcTransactionStatus { status, fee },
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
