use chrono::prelude::*;
use serde_json::Value;
use solana_clap_utils::keypair::presigner_from_pubkey_sigs;
use solana_cli::{
    cli::{process_command, request_and_confirm_airdrop, CliCommand, CliConfig, PayCommand},
    offline::{parse_sign_only_reply_string, BlockhashQuery},
};
use solana_client::rpc_client::RpcClient;
use solana_core::validator::TestValidator;
use solana_faucet::faucet::run_local_faucet;
use solana_sdk::{
    account_utils::StateMut,
    fee_calculator::FeeCalculator,
    nonce_state::NonceState,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
};
use std::{fs::remove_dir_all, sync::mpsc::channel, thread::sleep, time::Duration};

fn check_balance(expected_balance: u64, client: &RpcClient, pubkey: &Pubkey) {
    (0..5).for_each(|tries| {
        let balance = client.retry_get_balance(pubkey, 1).unwrap().unwrap();
        if balance == expected_balance {
            return;
        }
        if tries == 4 {
            assert_eq!(balance, expected_balance);
        }
        sleep(Duration::from_millis(500));
    });
}

#[test]
fn test_cli_timestamp_tx() {
    let TestValidator {
        server,
        leader_data,
        alice,
        ledger_path,
        ..
    } = TestValidator::run();
    let bob_pubkey = Pubkey::new_rand();

    let (sender, receiver) = channel();
    run_local_faucet(alice, sender, None);
    let faucet_addr = receiver.recv().unwrap();

    let rpc_client = RpcClient::new_socket(leader_data.rpc);
    let default_signer0 = Keypair::new();
    let default_signer1 = Keypair::new();

    let mut config_payer = CliConfig::default();
    config_payer.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    config_payer.signers = vec![&default_signer0];

    let mut config_witness = CliConfig::default();
    config_witness.json_rpc_url = config_payer.json_rpc_url.clone();
    config_witness.signers = vec![&default_signer1];

    assert_ne!(
        config_payer.signers[0].pubkey(),
        config_witness.signers[0].pubkey()
    );

    request_and_confirm_airdrop(
        &rpc_client,
        &faucet_addr,
        &config_payer.signers[0].pubkey(),
        50,
    )
    .unwrap();
    check_balance(50, &rpc_client, &config_payer.signers[0].pubkey());

    request_and_confirm_airdrop(
        &rpc_client,
        &faucet_addr,
        &config_witness.signers[0].pubkey(),
        1,
    )
    .unwrap();

    // Make transaction (from config_payer to bob_pubkey) requiring timestamp from config_witness
    let date_string = "\"2018-09-19T17:30:59Z\"";
    let dt: DateTime<Utc> = serde_json::from_str(&date_string).unwrap();
    config_payer.command = CliCommand::Pay(PayCommand {
        lamports: 10,
        to: bob_pubkey,
        timestamp: Some(dt),
        timestamp_pubkey: Some(config_witness.signers[0].pubkey()),
        ..PayCommand::default()
    });
    let sig_response = process_command(&config_payer);

    let object: Value = serde_json::from_str(&sig_response.unwrap()).unwrap();
    let process_id_str = object.get("processId").unwrap().as_str().unwrap();
    let process_id_vec = bs58::decode(process_id_str)
        .into_vec()
        .expect("base58-encoded public key");
    let process_id = Pubkey::new(&process_id_vec);

    check_balance(40, &rpc_client, &config_payer.signers[0].pubkey()); // config_payer balance
    check_balance(10, &rpc_client, &process_id); // contract balance
    check_balance(0, &rpc_client, &bob_pubkey); // recipient balance

    // Sign transaction by config_witness
    config_witness.command = CliCommand::TimeElapsed(bob_pubkey, process_id, dt);
    process_command(&config_witness).unwrap();

    check_balance(40, &rpc_client, &config_payer.signers[0].pubkey()); // config_payer balance
    check_balance(0, &rpc_client, &process_id); // contract balance
    check_balance(10, &rpc_client, &bob_pubkey); // recipient balance

    server.close().unwrap();
    remove_dir_all(ledger_path).unwrap();
}

#[test]
fn test_cli_witness_tx() {
    let TestValidator {
        server,
        leader_data,
        alice,
        ledger_path,
        ..
    } = TestValidator::run();
    let bob_pubkey = Pubkey::new_rand();

    let (sender, receiver) = channel();
    run_local_faucet(alice, sender, None);
    let faucet_addr = receiver.recv().unwrap();

    let rpc_client = RpcClient::new_socket(leader_data.rpc);
    let default_signer0 = Keypair::new();
    let default_signer1 = Keypair::new();

    let mut config_payer = CliConfig::default();
    config_payer.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    config_payer.signers = vec![&default_signer0];

    let mut config_witness = CliConfig::default();
    config_witness.json_rpc_url = config_payer.json_rpc_url.clone();
    config_witness.signers = vec![&default_signer1];

    assert_ne!(
        config_payer.signers[0].pubkey(),
        config_witness.signers[0].pubkey()
    );

    request_and_confirm_airdrop(
        &rpc_client,
        &faucet_addr,
        &config_payer.signers[0].pubkey(),
        50,
    )
    .unwrap();
    request_and_confirm_airdrop(
        &rpc_client,
        &faucet_addr,
        &config_witness.signers[0].pubkey(),
        1,
    )
    .unwrap();

    // Make transaction (from config_payer to bob_pubkey) requiring witness signature from config_witness
    config_payer.command = CliCommand::Pay(PayCommand {
        lamports: 10,
        to: bob_pubkey,
        witnesses: Some(vec![config_witness.signers[0].pubkey()]),
        ..PayCommand::default()
    });
    let sig_response = process_command(&config_payer);

    let object: Value = serde_json::from_str(&sig_response.unwrap()).unwrap();
    let process_id_str = object.get("processId").unwrap().as_str().unwrap();
    let process_id_vec = bs58::decode(process_id_str)
        .into_vec()
        .expect("base58-encoded public key");
    let process_id = Pubkey::new(&process_id_vec);

    check_balance(40, &rpc_client, &config_payer.signers[0].pubkey()); // config_payer balance
    check_balance(10, &rpc_client, &process_id); // contract balance
    check_balance(0, &rpc_client, &bob_pubkey); // recipient balance

    // Sign transaction by config_witness
    config_witness.command = CliCommand::Witness(bob_pubkey, process_id);
    process_command(&config_witness).unwrap();

    check_balance(40, &rpc_client, &config_payer.signers[0].pubkey()); // config_payer balance
    check_balance(0, &rpc_client, &process_id); // contract balance
    check_balance(10, &rpc_client, &bob_pubkey); // recipient balance

    server.close().unwrap();
    remove_dir_all(ledger_path).unwrap();
}

#[test]
fn test_cli_cancel_tx() {
    let TestValidator {
        server,
        leader_data,
        alice,
        ledger_path,
        ..
    } = TestValidator::run();
    let bob_pubkey = Pubkey::new_rand();

    let (sender, receiver) = channel();
    run_local_faucet(alice, sender, None);
    let faucet_addr = receiver.recv().unwrap();

    let rpc_client = RpcClient::new_socket(leader_data.rpc);
    let default_signer0 = Keypair::new();
    let default_signer1 = Keypair::new();

    let mut config_payer = CliConfig::default();
    config_payer.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    config_payer.signers = vec![&default_signer0];

    let mut config_witness = CliConfig::default();
    config_witness.json_rpc_url = config_payer.json_rpc_url.clone();
    config_witness.signers = vec![&default_signer1];

    assert_ne!(
        config_payer.signers[0].pubkey(),
        config_witness.signers[0].pubkey()
    );

    request_and_confirm_airdrop(
        &rpc_client,
        &faucet_addr,
        &config_payer.signers[0].pubkey(),
        50,
    )
    .unwrap();

    // Make transaction (from config_payer to bob_pubkey) requiring witness signature from config_witness
    config_payer.command = CliCommand::Pay(PayCommand {
        lamports: 10,
        to: bob_pubkey,
        witnesses: Some(vec![config_witness.signers[0].pubkey()]),
        cancelable: true,
        ..PayCommand::default()
    });
    let sig_response = process_command(&config_payer).unwrap();

    let object: Value = serde_json::from_str(&sig_response).unwrap();
    let process_id_str = object.get("processId").unwrap().as_str().unwrap();
    let process_id_vec = bs58::decode(process_id_str)
        .into_vec()
        .expect("base58-encoded public key");
    let process_id = Pubkey::new(&process_id_vec);

    check_balance(40, &rpc_client, &config_payer.signers[0].pubkey()); // config_payer balance
    check_balance(10, &rpc_client, &process_id); // contract balance
    check_balance(0, &rpc_client, &bob_pubkey); // recipient balance

    // Sign transaction by config_witness
    config_payer.command = CliCommand::Cancel(process_id);
    process_command(&config_payer).unwrap();

    check_balance(50, &rpc_client, &config_payer.signers[0].pubkey()); // config_payer balance
    check_balance(0, &rpc_client, &process_id); // contract balance
    check_balance(0, &rpc_client, &bob_pubkey); // recipient balance

    server.close().unwrap();
    remove_dir_all(ledger_path).unwrap();
}

#[test]
fn test_offline_pay_tx() {
    let TestValidator {
        server,
        leader_data,
        alice,
        ledger_path,
        ..
    } = TestValidator::run();
    let bob_pubkey = Pubkey::new_rand();

    let (sender, receiver) = channel();
    run_local_faucet(alice, sender, None);
    let faucet_addr = receiver.recv().unwrap();

    let rpc_client = RpcClient::new_socket(leader_data.rpc);
    let default_signer = Keypair::new();
    let default_offline_signer = Keypair::new();

    let mut config_offline = CliConfig::default();
    config_offline.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    config_offline.signers = vec![&default_offline_signer];
    let mut config_online = CliConfig::default();
    config_online.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    config_online.signers = vec![&default_signer];
    assert_ne!(
        config_offline.signers[0].pubkey(),
        config_online.signers[0].pubkey()
    );

    request_and_confirm_airdrop(
        &rpc_client,
        &faucet_addr,
        &config_offline.signers[0].pubkey(),
        50,
    )
    .unwrap();

    request_and_confirm_airdrop(
        &rpc_client,
        &faucet_addr,
        &config_online.signers[0].pubkey(),
        50,
    )
    .unwrap();
    check_balance(50, &rpc_client, &config_offline.signers[0].pubkey());
    check_balance(50, &rpc_client, &config_online.signers[0].pubkey());

    let (blockhash, _) = rpc_client.get_recent_blockhash().unwrap();
    config_offline.command = CliCommand::Pay(PayCommand {
        lamports: 10,
        to: bob_pubkey,
        blockhash_query: BlockhashQuery::None(blockhash, FeeCalculator::default()),
        sign_only: true,
        ..PayCommand::default()
    });
    let sig_response = process_command(&config_offline).unwrap();

    check_balance(50, &rpc_client, &config_offline.signers[0].pubkey());
    check_balance(50, &rpc_client, &config_online.signers[0].pubkey());
    check_balance(0, &rpc_client, &bob_pubkey);

    let (blockhash, signers) = parse_sign_only_reply_string(&sig_response);
    let offline_presigner =
        presigner_from_pubkey_sigs(&config_offline.signers[0].pubkey(), &signers).unwrap();
    let online_pubkey = config_online.signers[0].pubkey();
    config_online.signers = vec![&offline_presigner];
    config_online.command = CliCommand::Pay(PayCommand {
        lamports: 10,
        to: bob_pubkey,
        blockhash_query: BlockhashQuery::FeeCalculator(blockhash),
        ..PayCommand::default()
    });
    process_command(&config_online).unwrap();

    check_balance(40, &rpc_client, &config_offline.signers[0].pubkey());
    check_balance(50, &rpc_client, &online_pubkey);
    check_balance(10, &rpc_client, &bob_pubkey);

    server.close().unwrap();
    remove_dir_all(ledger_path).unwrap();
}

#[test]
fn test_nonced_pay_tx() {
    solana_logger::setup();

    let TestValidator {
        server,
        leader_data,
        alice,
        ledger_path,
        ..
    } = TestValidator::run();
    let (sender, receiver) = channel();
    run_local_faucet(alice, sender, None);
    let faucet_addr = receiver.recv().unwrap();

    let rpc_client = RpcClient::new_socket(leader_data.rpc);
    let default_signer = Keypair::new();

    let mut config = CliConfig::default();
    config.json_rpc_url = format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    config.signers = vec![&default_signer];

    let minimum_nonce_balance = rpc_client
        .get_minimum_balance_for_rent_exemption(NonceState::size())
        .unwrap();

    request_and_confirm_airdrop(
        &rpc_client,
        &faucet_addr,
        &config.signers[0].pubkey(),
        50 + minimum_nonce_balance,
    )
    .unwrap();
    check_balance(
        50 + minimum_nonce_balance,
        &rpc_client,
        &config.signers[0].pubkey(),
    );

    // Create nonce account
    let nonce_account = Keypair::new();
    config.command = CliCommand::CreateNonceAccount {
        nonce_account: 1,
        seed: None,
        nonce_authority: Some(config.signers[0].pubkey()),
        lamports: minimum_nonce_balance,
    };
    config.signers.push(&nonce_account);
    process_command(&config).unwrap();

    check_balance(50, &rpc_client, &config.signers[0].pubkey());
    check_balance(minimum_nonce_balance, &rpc_client, &nonce_account.pubkey());

    // Fetch nonce hash
    let account = rpc_client.get_account(&nonce_account.pubkey()).unwrap();
    let nonce_state: NonceState = account.state().unwrap();
    let nonce_hash = match nonce_state {
        NonceState::Initialized(_meta, hash) => hash,
        _ => panic!("Nonce is not initialized"),
    };

    let bob_pubkey = Pubkey::new_rand();
    config.signers = vec![&default_signer];
    config.command = CliCommand::Pay(PayCommand {
        lamports: 10,
        to: bob_pubkey,
        blockhash_query: BlockhashQuery::FeeCalculator(nonce_hash),
        nonce_account: Some(nonce_account.pubkey()),
        ..PayCommand::default()
    });
    process_command(&config).expect("failed to process pay command");

    check_balance(40, &rpc_client, &config.signers[0].pubkey());
    check_balance(10, &rpc_client, &bob_pubkey);

    // Verify that nonce has been used
    let account = rpc_client.get_account(&nonce_account.pubkey()).unwrap();
    let nonce_state: NonceState = account.state().unwrap();
    match nonce_state {
        NonceState::Initialized(_meta, hash) => assert_ne!(hash, nonce_hash),
        _ => assert!(false, "Nonce is not initialized"),
    }

    server.close().unwrap();
    remove_dir_all(ledger_path).unwrap();
}
