//! The `poh_recorder` module provides an object for synchronizing with Proof of History.
//! It synchronizes PoH, bank's register_tick and the ledger
//!
use crate::bank::Bank;
use crate::entry::Entry;
use crate::poh::Poh;
use crate::result::{Error, Result};
use solana_sdk::hash::Hash;
use solana_sdk::transaction::Transaction;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PohRecorderError {
    InvalidCallingObject,
    MaxHeightReached,
}

#[derive(Clone)]
pub struct PohRecorder {
    poh: Arc<Mutex<Poh>>,
    bank: Arc<Bank>,
    sender: Sender<Vec<Entry>>,
    max_tick_height: Option<u64>,
}

impl PohRecorder {
    pub fn hash(&self) -> Result<()> {
        // TODO: amortize the cost of this lock by doing the loop in here for
        // some min amount of hashes
        let mut poh = self.poh.lock().unwrap();

        self.check_tick_height(&poh)?;

        poh.hash();

        Ok(())
    }

    pub fn tick(&mut self) -> Result<()> {
        // Register and send the entry out while holding the lock if the max PoH height
        // hasn't been reached.
        // This guarantees PoH order and Entry production and banks LastId queue is the same
        let mut poh = self.poh.lock().unwrap();

        self.check_tick_height(&poh)?;

        self.register_and_send_tick(&mut *poh)
    }

    pub fn record(&self, mixin: Hash, txs: Vec<Transaction>) -> Result<()> {
        // Register and send the entry out while holding the lock.
        // This guarantees PoH order and Entry production and banks LastId queue is the same.
        let mut poh = self.poh.lock().unwrap();

        self.check_tick_height(&poh)?;

        self.record_and_send_txs(&mut *poh, mixin, txs)
    }

    /// A recorder to synchronize PoH with the following data structures
    /// * bank - the LastId's queue is updated on `tick` and `record` events
    /// * sender - the Entry channel that outputs to the ledger
    pub fn new(
        bank: Arc<Bank>,
        sender: Sender<Vec<Entry>>,
        last_entry_id: Hash,
        max_tick_height: Option<u64>,
    ) -> Self {
        let poh = Arc::new(Mutex::new(Poh::new(last_entry_id, bank.tick_height())));
        PohRecorder {
            poh,
            bank,
            sender,
            max_tick_height,
        }
    }

    fn check_tick_height(&self, poh: &Poh) -> Result<()> {
        match self.max_tick_height {
            Some(max_tick_height) if poh.tick_height >= max_tick_height => {
                Err(Error::PohRecorderError(PohRecorderError::MaxHeightReached))
            }
            _ => Ok(()),
        }
    }

    fn record_and_send_txs(&self, poh: &mut Poh, mixin: Hash, txs: Vec<Transaction>) -> Result<()> {
        let entry = poh.record(mixin);
        assert!(!txs.is_empty(), "Entries without transactions are used to track real-time passing in the ledger and can only be generated with PohRecorder::tick function");
        let entry = Entry {
            tick_height: entry.tick_height,
            num_hashes: entry.num_hashes,
            id: entry.id,
            transactions: txs,
        };
        self.sender.send(vec![entry])?;
        Ok(())
    }

    fn register_and_send_tick(&self, poh: &mut Poh) -> Result<()> {
        let tick = poh.tick();
        let tick = Entry {
            tick_height: tick.tick_height,
            num_hashes: tick.num_hashes,
            id: tick.id,
            transactions: vec![],
        };
        self.bank.register_tick(&tick.id);
        self.sender.send(vec![tick])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mint::Mint;
    use crate::test_tx::test_tx;
    use solana_sdk::hash::hash;
    use std::sync::mpsc::channel;
    use std::sync::Arc;

    #[test]
    fn test_poh() {
        let mint = Mint::new(1);
        let bank = Arc::new(Bank::new(&mint));
        let prev_id = bank.last_id();
        let (entry_sender, entry_receiver) = channel();
        let mut poh_recorder = PohRecorder::new(bank, entry_sender, prev_id, Some(3));

        //send some data
        let h1 = hash(b"hello world!");
        let tx = test_tx();
        assert!(poh_recorder.record(h1, vec![tx.clone()]).is_ok());
        //get some events
        let e = entry_receiver.recv().unwrap();
        assert_eq!(e[0].tick_height, 1);

        assert!(poh_recorder.tick().is_ok());
        let e = entry_receiver.recv().unwrap();
        assert_eq!(e[0].tick_height, 2);

        assert!(poh_recorder.tick().is_ok());
        let e = entry_receiver.recv().unwrap();
        assert_eq!(e[0].tick_height, 3);

        // max tick height reached
        assert!(poh_recorder.tick().is_err());
        assert!(poh_recorder.record(h1, vec![tx]).is_err());

        //make sure it handles channel close correctly
        drop(entry_receiver);
        assert!(poh_recorder.tick().is_err());
    }
}
