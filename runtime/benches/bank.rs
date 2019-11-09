#![feature(test)]

extern crate test;

use log::*;
use solana_runtime::bank::*;
use solana_runtime::bank_client::BankClient;
use solana_runtime::loader_utils::create_invoke_instruction;
use solana_sdk::account::KeyedAccount;
use solana_sdk::client::AsyncClient;
use solana_sdk::client::SyncClient;
use solana_sdk::clock::MAX_RECENT_BLOCKHASHES;
use solana_sdk::genesis_config::create_genesis_config;
use solana_sdk::instruction::InstructionError;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, KeypairUtil};
use solana_sdk::transaction::Transaction;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;
use test::Bencher;

const BUILTIN_PROGRAM_ID: [u8; 32] = [
    098, 117, 105, 108, 116, 105, 110, 095, 112, 114, 111, 103, 114, 097, 109, 095, 105, 100, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

const NOOP_PROGRAM_ID: [u8; 32] = [
    098, 117, 105, 108, 116, 105, 110, 095, 112, 114, 111, 103, 114, 097, 109, 095, 105, 100, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
];

fn process_instruction(
    _program_id: &Pubkey,
    _keyed_accounts: &mut [KeyedAccount],
    _data: &[u8],
) -> Result<(), InstructionError> {
    Ok(())
}

pub fn create_builtin_transactions(
    bank_client: &BankClient,
    mint_keypair: &Keypair,
) -> Vec<Transaction> {
    let program_id = Pubkey::new(&BUILTIN_PROGRAM_ID);

    (0..4096)
        .into_iter()
        .map(|_| {
            // Seed the signer account
            let rando0 = Keypair::new();
            bank_client
                .transfer(10_000, &mint_keypair, &rando0.pubkey())
                .expect(&format!("{}:{}", line!(), file!()));

            let instruction = create_invoke_instruction(rando0.pubkey(), program_id, &1u8);
            let (blockhash, _fee_calculator) = bank_client.get_recent_blockhash().unwrap();
            Transaction::new_signed_instructions(&[&rando0], vec![instruction], blockhash)
        })
        .collect()
}

pub fn create_native_loader_transactions(
    bank_client: &BankClient,
    mint_keypair: &Keypair,
) -> Vec<Transaction> {
    let program_id = Pubkey::new(&NOOP_PROGRAM_ID);

    (0..4096)
        .into_iter()
        .map(|_| {
            // Seed the signer account©41
            let rando0 = Keypair::new();
            bank_client
                .transfer(10_000, &mint_keypair, &rando0.pubkey())
                .expect(&format!("{}:{}", line!(), file!()));

            let instruction = create_invoke_instruction(rando0.pubkey(), program_id, &1u8);
            let (blockhash, _fee_calculator) = bank_client.get_recent_blockhash().unwrap();
            Transaction::new_signed_instructions(&[&rando0], vec![instruction], blockhash)
        })
        .collect()
}

fn sync_bencher(bank: &Arc<Bank>, _bank_client: &BankClient, transactions: &Vec<Transaction>) {
    let results = bank.process_transactions(&transactions);
    assert!(results.iter().all(Result::is_ok));
}

fn async_bencher(bank: &Arc<Bank>, bank_client: &BankClient, transactions: &Vec<Transaction>) {
    for transaction in transactions.clone() {
        bank_client.async_send_transaction(transaction).unwrap();
    }
    for _ in 0..1_000_000_000_u64 {
        if bank
            .get_signature_status(&transactions.last().unwrap().signatures.get(0).unwrap())
            .is_some()
        {
            break;
        }
        sleep(Duration::from_nanos(1));
    }
    if !bank
        .get_signature_status(&transactions.last().unwrap().signatures.get(0).unwrap())
        .unwrap()
        .is_ok()
    {
        error!(
            "transaction failed: {:?}",
            bank.get_signature_status(&transactions.last().unwrap().signatures.get(0).unwrap())
                .unwrap()
        );
        assert!(false);
    }
}

fn do_bench_transactions(
    bencher: &mut Bencher,
    bench_work: &dyn Fn(&Arc<Bank>, &BankClient, &Vec<Transaction>),
    create_transactions: &dyn Fn(&BankClient, &Keypair) -> Vec<Transaction>,
) {
    solana_logger::setup();
    let ns_per_s = 1_000_000_000;
    let (mut genesis_config, mint_keypair) = create_genesis_config(100_000_000);
    genesis_config.ticks_per_slot = 100;
    let mut bank = Bank::new(&genesis_config);
    bank.add_instruction_processor(Pubkey::new(&BUILTIN_PROGRAM_ID), process_instruction);
    bank.register_native_instruction_processor(
        "solana_noop_program",
        &Pubkey::new(&NOOP_PROGRAM_ID),
    );
    let bank = Arc::new(bank);
    let bank_client = BankClient::new_shared(&bank);
    let transactions = create_transactions(&bank_client, &mint_keypair);

    // Do once to fund accounts, load modules, etc...
    let results = bank.process_transactions(&transactions);
    assert!(results.iter().all(Result::is_ok));

    bencher.iter(|| {
        // Since bencher runs this multiple times, we need to clear the signatures.
        bank.clear_signatures();
        bench_work(&bank, &bank_client, &transactions);
    });

    let summary = bencher.bench(|_bencher| {}).unwrap();
    info!("  {:?} transactions", transactions.len());
    info!("  {:?} ns/iter median", summary.median as u64);
    assert!(0f64 != summary.median);
    let tps = transactions.len() as u64 * (ns_per_s / summary.median as u64);
    info!("  {:?} TPS", tps);
}

#[bench]
#[ignore]
fn bench_bank_sync_process_builtin_transactions(bencher: &mut Bencher) {
    do_bench_transactions(bencher, &sync_bencher, &create_builtin_transactions);
}

#[bench]
#[ignore]
fn bench_bank_sync_process_native_loader_transactions(bencher: &mut Bencher) {
    do_bench_transactions(bencher, &sync_bencher, &create_native_loader_transactions);
}

#[bench]
#[ignore]
fn bench_bank_async_process_builtin_transactions(bencher: &mut Bencher) {
    do_bench_transactions(bencher, &async_bencher, &create_builtin_transactions);
}

#[bench]
#[ignore]
fn bench_bank_async_process_native_loader_transactions(bencher: &mut Bencher) {
    do_bench_transactions(bencher, &async_bencher, &create_native_loader_transactions);
}

#[bench]
#[ignore]
fn bench_bank_update_recent_blockhashes(bencher: &mut Bencher) {
    let (genesis_config, _mint_keypair) = create_genesis_config(100);
    let mut bank = Arc::new(Bank::new(&genesis_config));
    goto_end_of_slot(Arc::get_mut(&mut bank).unwrap());
    let genesis_hash = bank.last_blockhash();
    // Prime blockhash_queue
    for i in 0..(MAX_RECENT_BLOCKHASHES + 1) {
        bank = Arc::new(Bank::new_from_parent(
            &bank,
            &Pubkey::default(),
            (i + 1) as u64,
        ));
        goto_end_of_slot(Arc::get_mut(&mut bank).unwrap());
    }
    // Verify blockhash_queue is full (genesis hash has been kicked out)
    assert!(!bank.check_hash_age(&genesis_hash, MAX_RECENT_BLOCKHASHES));
    bencher.iter(|| {
        bank.update_recent_blockhashes();
    });
}
