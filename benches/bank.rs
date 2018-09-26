#![feature(test)]
extern crate bincode;
extern crate rayon;
extern crate solana;
extern crate test;

use bincode::serialize;
use rayon::prelude::*;
use solana::bank::*;
use solana::hash::hash;
use solana::mint::Mint;
use solana::signature::{Keypair, KeypairUtil};
use solana::transaction::Transaction;
use test::Bencher;

#[bench]
fn bench_process_transaction(bencher: &mut Bencher) {
    let mint = Mint::new(100_000_000);
    let bank = Bank::new(&mint);

    // Create transactions between unrelated parties.
    let transactions: Vec<_> = (0..4096)
        .into_par_iter()
        .map(|i| {
            // Seed the 'from' account.
            let rando0 = Keypair::new();
            let tx = Transaction::new(&mint.keypair(), rando0.pubkey(), 10_000, mint.last_id());
            assert!(bank.process_transaction(&tx).is_ok());

            // Seed the 'to' account and a cell for its signature.
            let last_id = hash(&serialize(&i).unwrap()); // Unique hash
            bank.register_entry_id(&last_id);

            let rando1 = Keypair::new();
            let tx = Transaction::new(&rando0, rando1.pubkey(), 1, last_id);
            assert!(bank.process_transaction(&tx).is_ok());

            // Finally, return the transaction to the benchmark.
            tx
        }).collect();

    bencher.iter(|| {
        // Since benchmarker runs this multiple times, we need to clear the signatures.
        bank.clear_signatures();
        let results = bank.process_transactions(&transactions);
        assert!(results.iter().all(Result::is_ok));
    })
}
