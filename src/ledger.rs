//! The `ledger` module provides functions for parallel verification of the
//! Proof of History ledger.

use bincode::{self, deserialize, serialize_into};
use entry::{next_entry, Entry};
use hash::Hash;
use packet;
use packet::{SharedBlob, BLOB_DATA_SIZE, BLOB_SIZE};
use rayon::prelude::*;
use std::cmp::min;
use std::collections::VecDeque;
use std::io::Cursor;
use std::mem::size_of;
use transaction::Transaction;

pub trait Block {
    /// Verifies the hashes and counts of a slice of transactions are all consistent.
    fn verify(&self, start_hash: &Hash) -> bool;
    fn to_blobs(&self, blob_recycler: &packet::BlobRecycler, q: &mut VecDeque<SharedBlob>);
}

impl Block for [Entry] {
    fn verify(&self, start_hash: &Hash) -> bool {
        let genesis = [Entry::new_tick(0, start_hash)];
        let entry_pairs = genesis.par_iter().chain(self).zip(self);
        entry_pairs.all(|(x0, x1)| x1.verify(&x0.id))
    }

    fn to_blobs(&self, blob_recycler: &packet::BlobRecycler, q: &mut VecDeque<SharedBlob>) {
        let mut start = 0;
        let mut end = 0;
        while start < self.len() {
            let mut entries: Vec<Vec<Entry>> = Vec::new();
            let mut total = 0;
            for i in &self[start..] {
                total += size_of::<Transaction>() * i.transactions.len();
                total += size_of::<Entry>();
                if total >= BLOB_DATA_SIZE {
                    break;
                }
                end += 1;
            }
            // See if we need to split the transactions
            if end <= start {
                let mut transaction_start = 0;
                let num_transactions_per_blob = BLOB_DATA_SIZE / size_of::<Transaction>();
                let total_entry_chunks = (self[end].transactions.len() + num_transactions_per_blob
                    - 1) / num_transactions_per_blob;
                trace!(
                    "splitting transactions end: {} total_chunks: {}",
                    end,
                    total_entry_chunks
                );
                for _ in 0..total_entry_chunks {
                    let transaction_end = min(
                        transaction_start + num_transactions_per_blob,
                        self[end].transactions.len(),
                    );
                    let mut entry = Entry {
                        num_hashes: self[end].num_hashes,
                        id: self[end].id,
                        transactions: self[end].transactions[transaction_start..transaction_end]
                            .to_vec(),
                    };
                    entries.push(vec![entry]);
                    transaction_start = transaction_end;
                }
                end += 1;
            } else {
                entries.push(self[start..end].to_vec());
            }

            for entry in entries {
                let b = blob_recycler.allocate();
                let pos = {
                    let mut bd = b.write().unwrap();
                    let mut out = Cursor::new(bd.data_mut());
                    serialize_into(&mut out, &entry).expect("failed to serialize output");
                    out.position() as usize
                };
                assert!(pos < BLOB_SIZE);
                b.write().unwrap().set_size(pos);
                q.push_back(b);
            }
            start = end;
        }
    }
}

/// Create a vector of Entries of length `transaction_batches.len()` from `start_hash` hash, `num_hashes`, and `transaction_batches`.
pub fn next_entries(
    start_hash: &Hash,
    num_hashes: u64,
    transaction_batches: Vec<Vec<Transaction>>,
) -> Vec<Entry> {
    let mut id = *start_hash;
    let mut entries = vec![];
    for transactions in transaction_batches {
        let entry = next_entry(&id, num_hashes, transactions);
        id = entry.id;
        entries.push(entry);
    }
    entries
}

pub fn reconstruct_entries_from_blobs(blobs: &VecDeque<SharedBlob>) -> bincode::Result<Vec<Entry>> {
    let mut entries_to_apply: Vec<Entry> = Vec::new();
    let mut last_id = Hash::default();
    for msgs in blobs {
        let blob = msgs.read().unwrap();
        let entries: Vec<Entry> = deserialize(&blob.data()[..blob.meta.size])?;
        for entry in entries {
            if entry.id == last_id {
                if let Some(last_entry) = entries_to_apply.last_mut() {
                    last_entry.transactions.extend(entry.transactions);
                }
            } else {
                last_id = entry.id;
                entries_to_apply.push(entry);
            }
        }
    }
    Ok(entries_to_apply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hash::hash;
    use packet::BlobRecycler;
    use signature::{KeyPair, KeyPairUtil};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use transaction::Transaction;

    #[test]
    fn test_verify_slice() {
        let zero = Hash::default();
        let one = hash(&zero);
        assert!(vec![][..].verify(&zero)); // base case
        assert!(vec![Entry::new_tick(0, &zero)][..].verify(&zero)); // singleton case 1
        assert!(!vec![Entry::new_tick(0, &zero)][..].verify(&one)); // singleton case 2, bad
        assert!(next_entries(&zero, 0, vec![vec![]; 2])[..].verify(&zero)); // inductive step

        let mut bad_ticks = next_entries(&zero, 0, vec![vec![]; 2]);
        bad_ticks[1].id = one;
        assert!(!bad_ticks.verify(&zero)); // inductive step, bad
    }

    #[test]
    fn test_entry_to_blobs() {
        let zero = Hash::default();
        let one = hash(&zero);
        let keypair = KeyPair::new();
        let tx0 = Transaction::new(&keypair, keypair.pubkey(), 1, one);
        let transactions = vec![tx0; 10000];
        let e0 = Entry::new(&zero, 0, transactions);

        let entries = vec![e0];
        let blob_recycler = BlobRecycler::default();
        let mut blob_q = VecDeque::new();
        entries.to_blobs(&blob_recycler, &mut blob_q);

        assert_eq!(reconstruct_entries_from_blobs(&blob_q).unwrap(), entries);
    }

    #[test]
    fn test_bad_blobs_attack() {
        let blob_recycler = BlobRecycler::default();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 8000);
        let blobs_q = packet::to_blobs(vec![(0, addr)], &blob_recycler).unwrap(); // <-- attack!
        assert!(reconstruct_entries_from_blobs(&blobs_q).is_err());
    }

    #[test]
    fn test_next_entries() {
        let mut id = Hash::default();
        let next_id = hash(&id);
        let keypair = KeyPair::new();
        let tx0 = Transaction::new(&keypair, keypair.pubkey(), 1, next_id);
        let transactions = vec![tx0; 5];
        let transaction_batches = vec![transactions.clone(); 5];
        let entries0 = next_entries(&id, 1, transaction_batches);

        assert_eq!(entries0.len(), 5);

        let mut entries1 = vec![];
        for _ in 0..5 {
            let entry = next_entry(&id, 1, transactions.clone());
            id = entry.id;
            entries1.push(entry);
        }
        assert_eq!(entries0, entries1);
    }
}

#[cfg(all(feature = "unstable", test))]
mod bench {
    extern crate test;
    use self::test::Bencher;
    use ledger::*;

    #[bench]
    fn bench_next_entries(bencher: &mut Bencher) {
        let start_hash = Hash::default();
        let entries = next_entries(&start_hash, 10_000, vec![vec![]; 8]);
        bencher.iter(|| {
            assert!(entries.verify(&start_hash));
        });
    }
}
