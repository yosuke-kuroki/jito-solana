#![feature(test)]
extern crate bincode;
extern crate rand;
extern crate rayon;
extern crate solana;
extern crate solana_sdk;
extern crate test;

use rand::{thread_rng, Rng};
use rayon::prelude::*;
use solana::bank::{Bank, MAX_ENTRY_IDS};
use solana::banking_stage::{BankingStage, NUM_THREADS};
use solana::entry::Entry;
use solana::hash::hash;
use solana::mint::Mint;
use solana::packet::to_packets_chunked;
use solana::signature::{KeypairUtil, Signature};
use solana::system_transaction::SystemTransaction;
use solana::transaction::Transaction;
use solana_sdk::pubkey::Pubkey;
use std::iter;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;
use test::Bencher;

fn check_txs(receiver: &Receiver<Vec<Entry>>, ref_tx_count: usize) {
    let mut total = 0;
    loop {
        let entries = receiver.recv_timeout(Duration::new(1, 0));
        if let Ok(entries) = entries {
            for entry in &entries {
                total += entry.transactions.len();
            }
        } else {
            break;
        }
        if total >= ref_tx_count {
            break;
        }
    }
    assert_eq!(total, ref_tx_count);
}

#[bench]
fn bench_banking_stage_multi_accounts(bencher: &mut Bencher) {
    let txes = 1000 * NUM_THREADS;
    let mint_total = 1_000_000_000_000;
    let mint = Mint::new(mint_total);

    let (verified_sender, verified_receiver) = channel();
    let bank = Arc::new(Bank::new(&mint));
    let dummy = Transaction::system_move(
        &mint.keypair(),
        mint.keypair().pubkey(),
        1,
        mint.last_id(),
        0,
    );
    let transactions: Vec<_> = (0..txes)
        .into_par_iter()
        .map(|_| {
            let mut new = dummy.clone();
            let from: Vec<u8> = (0..64).map(|_| thread_rng().gen()).collect();
            let to: Vec<u8> = (0..64).map(|_| thread_rng().gen()).collect();
            let sig: Vec<u8> = (0..64).map(|_| thread_rng().gen()).collect();
            new.account_keys[0] = Pubkey::new(&from[0..32]);
            new.account_keys[1] = Pubkey::new(&to[0..32]);
            new.signature = Signature::new(&sig[0..64]);
            new
        }).collect();
    // fund all the accounts
    transactions.iter().for_each(|tx| {
        let fund = Transaction::system_move(
            &mint.keypair(),
            tx.account_keys[0],
            mint_total / txes as i64,
            mint.last_id(),
            0,
        );
        assert!(bank.process_transaction(&fund).is_ok());
    });
    //sanity check, make sure all the transactions can execute sequentially
    transactions.iter().for_each(|tx| {
        let res = bank.process_transaction(&tx);
        assert!(res.is_ok(), "sanity test transactions");
    });
    bank.clear_signatures();
    //sanity check, make sure all the transactions can execute in parallel
    let res = bank.process_transactions(&transactions);
    for r in res {
        assert!(r.is_ok(), "sanity parallel execution");
    }
    bank.clear_signatures();
    let verified: Vec<_> = to_packets_chunked(&transactions.clone(), 192)
        .into_iter()
        .map(|x| {
            let len = x.read().unwrap().packets.len();
            (x, iter::repeat(1).take(len).collect())
        }).collect();
    let (_stage, signal_receiver) = BankingStage::new(
        &bank,
        verified_receiver,
        Default::default(),
        &mint.last_id(),
        0,
        None,
    );

    let mut id = mint.last_id();
    for _ in 0..MAX_ENTRY_IDS {
        id = hash(&id.as_ref());
        bank.register_entry_id(&id);
    }

    bencher.iter(move || {
        // make sure the tx last id is still registered
        if bank.count_valid_ids(&[mint.last_id()]).len() == 0 {
            bank.register_entry_id(&mint.last_id());
        }
        for v in verified.chunks(verified.len() / NUM_THREADS) {
            verified_sender.send(v.to_vec()).unwrap();
        }
        check_txs(&signal_receiver, txes);
        bank.clear_signatures();
    });
}

#[bench]
fn bench_banking_stage_multi_programs(bencher: &mut Bencher) {
    let progs = 5;
    let txes = 1000 * NUM_THREADS;
    let mint_total = 1_000_000_000_000;
    let mint = Mint::new(mint_total);

    let (verified_sender, verified_receiver) = channel();
    let bank = Arc::new(Bank::new(&mint));
    let dummy = Transaction::system_move(
        &mint.keypair(),
        mint.keypair().pubkey(),
        1,
        mint.last_id(),
        0,
    );
    let transactions: Vec<_> = (0..txes)
        .into_par_iter()
        .map(|_| {
            let mut new = dummy.clone();
            let from: Vec<u8> = (0..32).map(|_| thread_rng().gen()).collect();
            let sig: Vec<u8> = (0..64).map(|_| thread_rng().gen()).collect();
            let to: Vec<u8> = (0..32).map(|_| thread_rng().gen()).collect();
            new.account_keys[0] = Pubkey::new(&from[0..32]);
            new.account_keys[1] = Pubkey::new(&to[0..32]);
            let prog = new.instructions[0].clone();
            for i in 1..progs {
                //generate programs that spend to random keys
                let to: Vec<u8> = (0..32).map(|_| thread_rng().gen()).collect();
                let to_key = Pubkey::new(&to[0..32]);
                new.account_keys.push(to_key);
                assert_eq!(new.account_keys.len(), i + 2);
                new.instructions.push(prog.clone());
                assert_eq!(new.instructions.len(), i + 1);
                new.instructions[i].accounts[1] = 1 + i as u8;
                assert_eq!(new.key(i, 1), Some(&to_key));
                assert_eq!(
                    new.account_keys[new.instructions[i].accounts[1] as usize],
                    to_key
                );
            }
            assert_eq!(new.instructions.len(), progs);
            new.signature = Signature::new(&sig[0..64]);
            new
        }).collect();
    transactions.iter().for_each(|tx| {
        let fund = Transaction::system_move(
            &mint.keypair(),
            tx.account_keys[0],
            mint_total / txes as i64,
            mint.last_id(),
            0,
        );
        assert!(bank.process_transaction(&fund).is_ok());
    });
    //sanity check, make sure all the transactions can execute sequentially
    transactions.iter().for_each(|tx| {
        let res = bank.process_transaction(&tx);
        assert!(res.is_ok(), "sanity test transactions");
    });
    bank.clear_signatures();
    //sanity check, make sure all the transactions can execute in parallel
    let res = bank.process_transactions(&transactions);
    for r in res {
        assert!(r.is_ok(), "sanity parallel execution");
    }
    bank.clear_signatures();
    let verified: Vec<_> = to_packets_chunked(&transactions.clone(), 96)
        .into_iter()
        .map(|x| {
            let len = x.read().unwrap().packets.len();
            (x, iter::repeat(1).take(len).collect())
        }).collect();
    let (_stage, signal_receiver) = BankingStage::new(
        &bank,
        verified_receiver,
        Default::default(),
        &mint.last_id(),
        0,
        None,
    );

    let mut id = mint.last_id();
    for _ in 0..MAX_ENTRY_IDS {
        id = hash(&id.as_ref());
        bank.register_entry_id(&id);
    }

    bencher.iter(move || {
        // make sure the transactions are still valid
        if bank.count_valid_ids(&[mint.last_id()]).len() == 0 {
            bank.register_entry_id(&mint.last_id());
        }
        for v in verified.chunks(verified.len() / NUM_THREADS) {
            verified_sender.send(v.to_vec()).unwrap();
        }
        check_txs(&signal_receiver, txes);
        bank.clear_signatures();
    });
}
