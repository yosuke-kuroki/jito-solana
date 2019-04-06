//! The `rpc` module implements the Solana RPC interface.

use crate::bank_forks::BankForks;
use crate::cluster_info::ClusterInfo;
use crate::packet::PACKET_DATA_SIZE;
use crate::storage_stage::StorageState;
use bincode::{deserialize, serialize};
use bs58;
use jsonrpc_core::{Error, Metadata, Result};
use jsonrpc_derive::rpc;
use solana_drone::drone::request_airdrop_transaction;
use solana_runtime::bank::Bank;
use solana_sdk::account::Account;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::{self, Transaction};
use std::mem;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::sleep;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct JsonRpcConfig {
    pub enable_fullnode_exit: bool, // Enable the 'fullnodeExit' command
    pub drone_addr: Option<SocketAddr>,
}

impl Default for JsonRpcConfig {
    fn default() -> Self {
        Self {
            enable_fullnode_exit: false,
            drone_addr: None,
        }
    }
}

#[derive(Clone)]
pub struct JsonRpcRequestProcessor {
    bank_forks: Arc<RwLock<BankForks>>,
    storage_state: StorageState,
    config: JsonRpcConfig,
    fullnode_exit: Arc<AtomicBool>,
}

impl JsonRpcRequestProcessor {
    fn bank(&self) -> Arc<Bank> {
        self.bank_forks.read().unwrap().working_bank()
    }

    pub fn new(
        storage_state: StorageState,
        config: JsonRpcConfig,
        bank_forks: Arc<RwLock<BankForks>>,
        fullnode_exit: &Arc<AtomicBool>,
    ) -> Self {
        JsonRpcRequestProcessor {
            bank_forks,
            storage_state,
            config,
            fullnode_exit: fullnode_exit.clone(),
        }
    }

    pub fn get_account_info(&self, pubkey: &Pubkey) -> Result<Account> {
        self.bank()
            .get_account(&pubkey)
            .ok_or_else(Error::invalid_request)
    }

    pub fn get_balance(&self, pubkey: &Pubkey) -> u64 {
        self.bank().get_balance(&pubkey)
    }

    fn get_recent_blockhash(&self) -> String {
        let id = self.bank().last_blockhash();
        bs58::encode(id).into_string()
    }

    pub fn get_signature_status(&self, signature: Signature) -> Option<transaction::Result<()>> {
        self.get_signature_confirmation_status(signature)
            .map(|x| x.1)
    }

    pub fn get_signature_confirmations(&self, signature: Signature) -> Option<usize> {
        self.get_signature_confirmation_status(signature)
            .map(|x| x.0)
    }

    pub fn get_signature_confirmation_status(
        &self,
        signature: Signature,
    ) -> Option<(usize, transaction::Result<()>)> {
        self.bank().get_signature_confirmation_status(&signature)
    }

    fn get_transaction_count(&self) -> Result<u64> {
        Ok(self.bank().transaction_count() as u64)
    }

    fn get_storage_blockhash(&self) -> Result<String> {
        let hash = self.storage_state.get_storage_blockhash();
        Ok(bs58::encode(hash).into_string())
    }

    fn get_storage_entry_height(&self) -> Result<u64> {
        let entry_height = self.storage_state.get_entry_height();
        Ok(entry_height)
    }

    fn get_storage_pubkeys_for_entry_height(&self, entry_height: u64) -> Result<Vec<Pubkey>> {
        Ok(self
            .storage_state
            .get_pubkeys_for_entry_height(entry_height))
    }

    pub fn fullnode_exit(&self) -> Result<bool> {
        if self.config.enable_fullnode_exit {
            warn!("fullnode_exit request...");
            self.fullnode_exit.store(true, Ordering::Relaxed);
            Ok(true)
        } else {
            debug!("fullnode_exit ignored");
            Ok(false)
        }
    }
}

fn get_tpu_addr(cluster_info: &Arc<RwLock<ClusterInfo>>) -> Result<SocketAddr> {
    let contact_info = cluster_info.read().unwrap().my_data();
    Ok(contact_info.tpu)
}

fn verify_pubkey(input: String) -> Result<Pubkey> {
    let pubkey_vec = bs58::decode(input).into_vec().map_err(|err| {
        info!("verify_pubkey: invalid input: {:?}", err);
        Error::invalid_request()
    })?;
    if pubkey_vec.len() != mem::size_of::<Pubkey>() {
        info!(
            "verify_pubkey: invalid pubkey_vec length: {}",
            pubkey_vec.len()
        );
        Err(Error::invalid_request())
    } else {
        Ok(Pubkey::new(&pubkey_vec))
    }
}

fn verify_signature(input: &str) -> Result<Signature> {
    let signature_vec = bs58::decode(input).into_vec().map_err(|err| {
        info!("verify_signature: invalid input: {}: {:?}", input, err);
        Error::invalid_request()
    })?;
    if signature_vec.len() != mem::size_of::<Signature>() {
        info!(
            "verify_signature: invalid signature_vec length: {}",
            signature_vec.len()
        );
        Err(Error::invalid_request())
    } else {
        Ok(Signature::new(&signature_vec))
    }
}

#[derive(Clone)]
pub struct Meta {
    pub request_processor: Arc<RwLock<JsonRpcRequestProcessor>>,
    pub cluster_info: Arc<RwLock<ClusterInfo>>,
}
impl Metadata for Meta {}

#[rpc(server)]
pub trait RpcSol {
    type Metadata;

    #[rpc(meta, name = "confirmTransaction")]
    fn confirm_transaction(&self, _: Self::Metadata, _: String) -> Result<bool>;

    #[rpc(meta, name = "getAccountInfo")]
    fn get_account_info(&self, _: Self::Metadata, _: String) -> Result<Account>;

    #[rpc(meta, name = "getBalance")]
    fn get_balance(&self, _: Self::Metadata, _: String) -> Result<u64>;

    #[rpc(meta, name = "getRecentBlockhash")]
    fn get_recent_blockhash(&self, _: Self::Metadata) -> Result<String>;

    #[rpc(meta, name = "getSignatureStatus")]
    fn get_signature_status(
        &self,
        _: Self::Metadata,
        _: String,
    ) -> Result<Option<transaction::Result<()>>>;

    #[rpc(meta, name = "getTransactionCount")]
    fn get_transaction_count(&self, _: Self::Metadata) -> Result<u64>;

    #[rpc(meta, name = "requestAirdrop")]
    fn request_airdrop(&self, _: Self::Metadata, _: String, _: u64) -> Result<String>;

    #[rpc(meta, name = "sendTransaction")]
    fn send_transaction(&self, _: Self::Metadata, _: Vec<u8>) -> Result<String>;

    #[rpc(meta, name = "getStorageBlockhash")]
    fn get_storage_blockhash(&self, _: Self::Metadata) -> Result<String>;

    #[rpc(meta, name = "getStorageEntryHeight")]
    fn get_storage_entry_height(&self, _: Self::Metadata) -> Result<u64>;

    #[rpc(meta, name = "getStoragePubkeysForEntryHeight")]
    fn get_storage_pubkeys_for_entry_height(
        &self,
        _: Self::Metadata,
        _: u64,
    ) -> Result<Vec<Pubkey>>;

    #[rpc(meta, name = "fullnodeExit")]
    fn fullnode_exit(&self, _: Self::Metadata) -> Result<bool>;

    #[rpc(meta, name = "getNumBlocksSinceSignatureConfirmation")]
    fn get_num_blocks_since_signature_confirmation(
        &self,
        _: Self::Metadata,
        _: String,
    ) -> Result<Option<usize>>;

    #[rpc(meta, name = "getSignatureConfirmation")]
    fn get_signature_confirmation(
        &self,
        _: Self::Metadata,
        _: String,
    ) -> Result<Option<(usize, transaction::Result<()>)>>;
}

pub struct RpcSolImpl;
impl RpcSol for RpcSolImpl {
    type Metadata = Meta;

    fn confirm_transaction(&self, meta: Self::Metadata, id: String) -> Result<bool> {
        debug!("confirm_transaction rpc request received: {:?}", id);
        self.get_signature_status(meta, id).map(|status_option| {
            if status_option.is_none() {
                return false;
            }
            status_option.unwrap().is_ok()
        })
    }

    fn get_account_info(&self, meta: Self::Metadata, id: String) -> Result<Account> {
        debug!("get_account_info rpc request received: {:?}", id);
        let pubkey = verify_pubkey(id)?;
        meta.request_processor
            .read()
            .unwrap()
            .get_account_info(&pubkey)
    }

    fn get_balance(&self, meta: Self::Metadata, id: String) -> Result<u64> {
        debug!("get_balance rpc request received: {:?}", id);
        let pubkey = verify_pubkey(id)?;
        Ok(meta.request_processor.read().unwrap().get_balance(&pubkey))
    }

    fn get_recent_blockhash(&self, meta: Self::Metadata) -> Result<String> {
        debug!("get_recent_blockhash rpc request received");
        Ok(meta
            .request_processor
            .read()
            .unwrap()
            .get_recent_blockhash())
    }

    fn get_signature_status(
        &self,
        meta: Self::Metadata,
        id: String,
    ) -> Result<Option<transaction::Result<()>>> {
        self.get_signature_confirmation(meta, id)
            .map(|res| res.map(|x| x.1))
    }

    fn get_num_blocks_since_signature_confirmation(
        &self,
        meta: Self::Metadata,
        id: String,
    ) -> Result<Option<usize>> {
        self.get_signature_confirmation(meta, id)
            .map(|res| res.map(|x| x.0))
    }

    fn get_signature_confirmation(
        &self,
        meta: Self::Metadata,
        id: String,
    ) -> Result<Option<(usize, transaction::Result<()>)>> {
        debug!("get_signature_confirmation rpc request received: {:?}", id);
        let signature = verify_signature(&id)?;
        Ok(meta
            .request_processor
            .read()
            .unwrap()
            .get_signature_confirmation_status(signature))
    }

    fn get_transaction_count(&self, meta: Self::Metadata) -> Result<u64> {
        debug!("get_transaction_count rpc request received");
        meta.request_processor
            .read()
            .unwrap()
            .get_transaction_count()
    }

    fn request_airdrop(&self, meta: Self::Metadata, id: String, lamports: u64) -> Result<String> {
        trace!("request_airdrop id={} lamports={}", id, lamports);

        let drone_addr = meta
            .request_processor
            .read()
            .unwrap()
            .config
            .drone_addr
            .ok_or_else(Error::invalid_request)?;
        let pubkey = verify_pubkey(id)?;

        let blockhash = meta
            .request_processor
            .read()
            .unwrap()
            .bank()
            .last_blockhash();
        let transaction = request_airdrop_transaction(&drone_addr, &pubkey, lamports, blockhash)
            .map_err(|err| {
                info!("request_airdrop_transaction failed: {:?}", err);
                Error::internal_error()
            })?;;

        let data = serialize(&transaction).map_err(|err| {
            info!("request_airdrop: serialize error: {:?}", err);
            Error::internal_error()
        })?;

        let transactions_socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let transactions_addr = get_tpu_addr(&meta.cluster_info)?;
        transactions_socket
            .send_to(&data, transactions_addr)
            .map_err(|err| {
                info!("request_airdrop: send_to error: {:?}", err);
                Error::internal_error()
            })?;

        let signature = transaction.signatures[0];
        let now = Instant::now();
        let mut signature_status;
        loop {
            signature_status = meta
                .request_processor
                .read()
                .unwrap()
                .get_signature_status(signature);

            if signature_status == Some(Ok(())) {
                info!("airdrop signature ok");
                return Ok(bs58::encode(signature).into_string());
            } else if now.elapsed().as_secs() > 5 {
                info!("airdrop signature timeout");
                return Err(Error::internal_error());
            }
            sleep(Duration::from_millis(100));
        }
    }

    fn send_transaction(&self, meta: Self::Metadata, data: Vec<u8>) -> Result<String> {
        let tx: Transaction = deserialize(&data).map_err(|err| {
            info!("send_transaction: deserialize error: {:?}", err);
            Error::invalid_request()
        })?;
        if data.len() >= PACKET_DATA_SIZE {
            info!(
                "send_transaction: transaction too large: {} bytes (max: {} bytes)",
                data.len(),
                PACKET_DATA_SIZE
            );
            return Err(Error::invalid_request());
        }
        let transactions_socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let transactions_addr = get_tpu_addr(&meta.cluster_info)?;
        trace!("send_transaction: leader is {:?}", &transactions_addr);
        transactions_socket
            .send_to(&data, transactions_addr)
            .map_err(|err| {
                info!("send_transaction: send_to error: {:?}", err);
                Error::internal_error()
            })?;
        let signature = bs58::encode(tx.signatures[0]).into_string();
        trace!(
            "send_transaction: sent {} bytes, signature={}",
            data.len(),
            signature
        );
        Ok(signature)
    }

    fn get_storage_blockhash(&self, meta: Self::Metadata) -> Result<String> {
        meta.request_processor
            .read()
            .unwrap()
            .get_storage_blockhash()
    }

    fn get_storage_entry_height(&self, meta: Self::Metadata) -> Result<u64> {
        meta.request_processor
            .read()
            .unwrap()
            .get_storage_entry_height()
    }

    fn get_storage_pubkeys_for_entry_height(
        &self,
        meta: Self::Metadata,
        entry_height: u64,
    ) -> Result<Vec<Pubkey>> {
        meta.request_processor
            .read()
            .unwrap()
            .get_storage_pubkeys_for_entry_height(entry_height)
    }

    fn fullnode_exit(&self, meta: Self::Metadata) -> Result<bool> {
        meta.request_processor.read().unwrap().fullnode_exit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contact_info::ContactInfo;
    use jsonrpc_core::{MetaIoHandler, Response};
    use solana_sdk::genesis_block::GenesisBlock;
    use solana_sdk::hash::{hash, Hash};
    use solana_sdk::instruction::InstructionError;
    use solana_sdk::signature::{Keypair, KeypairUtil};
    use solana_sdk::system_transaction;
    use solana_sdk::transaction::TransactionError;
    use std::thread;

    fn start_rpc_handler_with_tx(pubkey: &Pubkey) -> (MetaIoHandler<Meta>, Meta, Hash, Keypair) {
        let (bank_forks, alice) = new_bank_forks();
        let bank = bank_forks.read().unwrap().working_bank();
        let exit = Arc::new(AtomicBool::new(false));

        let blockhash = bank.last_blockhash();
        let tx = system_transaction::transfer(&alice, pubkey, 20, blockhash, 0);
        bank.process_transaction(&tx).expect("process transaction");

        let tx = system_transaction::transfer(&alice, &alice.pubkey(), 20, blockhash, 0);
        let _ = bank.process_transaction(&tx);

        let request_processor = Arc::new(RwLock::new(JsonRpcRequestProcessor::new(
            StorageState::default(),
            JsonRpcConfig::default(),
            bank_forks,
            &exit,
        )));
        let cluster_info = Arc::new(RwLock::new(ClusterInfo::new_with_invalid_keypair(
            ContactInfo::default(),
        )));
        let leader = ContactInfo::new_with_socketaddr(&socketaddr!("127.0.0.1:1234"));

        cluster_info.write().unwrap().insert_info(leader.clone());

        let mut io = MetaIoHandler::default();
        let rpc = RpcSolImpl;
        io.extend_with(rpc.to_delegate());
        let meta = Meta {
            request_processor,
            cluster_info,
        };
        (io, meta, blockhash, alice)
    }

    #[test]
    fn test_rpc_request_processor_new() {
        let bob_pubkey = Pubkey::new_rand();
        let exit = Arc::new(AtomicBool::new(false));
        let (bank_forks, alice) = new_bank_forks();
        let bank = bank_forks.read().unwrap().working_bank();
        let request_processor = JsonRpcRequestProcessor::new(
            StorageState::default(),
            JsonRpcConfig::default(),
            bank_forks,
            &exit,
        );
        thread::spawn(move || {
            let blockhash = bank.last_blockhash();
            let tx = system_transaction::transfer(&alice, &bob_pubkey, 20, blockhash, 0);
            bank.process_transaction(&tx).expect("process transaction");
        })
        .join()
        .unwrap();
        assert_eq!(request_processor.get_transaction_count().unwrap(), 1);
    }

    #[test]
    fn test_rpc_get_balance() {
        let bob_pubkey = Pubkey::new_rand();
        let (io, meta, _blockhash, _alice) = start_rpc_handler_with_tx(&bob_pubkey);

        let req = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"getBalance","params":["{}"]}}"#,
            bob_pubkey
        );
        let res = io.handle_request_sync(&req, meta);
        let expected = format!(r#"{{"jsonrpc":"2.0","result":20,"id":1}}"#);
        let expected: Response =
            serde_json::from_str(&expected).expect("expected response deserialization");
        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);
    }

    #[test]
    fn test_rpc_get_tx_count() {
        let bob_pubkey = Pubkey::new_rand();
        let (io, meta, _blockhash, _alice) = start_rpc_handler_with_tx(&bob_pubkey);

        let req = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"getTransactionCount"}}"#);
        let res = io.handle_request_sync(&req, meta);
        let expected = format!(r#"{{"jsonrpc":"2.0","result":1,"id":1}}"#);
        let expected: Response =
            serde_json::from_str(&expected).expect("expected response deserialization");
        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);
    }

    #[test]
    fn test_rpc_get_account_info() {
        let bob_pubkey = Pubkey::new_rand();
        let (io, meta, _blockhash, _alice) = start_rpc_handler_with_tx(&bob_pubkey);

        let req = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"getAccountInfo","params":["{}"]}}"#,
            bob_pubkey
        );
        let res = io.handle_request_sync(&req, meta);
        let expected = r#"{
            "jsonrpc":"2.0",
            "result":{
                "owner": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                "lamports": 20,
                "data": [],
                "executable": false
            },
            "id":1}
        "#;
        let expected: Response =
            serde_json::from_str(&expected).expect("expected response deserialization");
        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);
    }

    #[test]
    fn test_rpc_confirm_tx() {
        let bob_pubkey = Pubkey::new_rand();
        let (io, meta, blockhash, alice) = start_rpc_handler_with_tx(&bob_pubkey);
        let tx = system_transaction::transfer(&alice, &bob_pubkey, 20, blockhash, 0);

        let req = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"confirmTransaction","params":["{}"]}}"#,
            tx.signatures[0]
        );
        let res = io.handle_request_sync(&req, meta);
        let expected = format!(r#"{{"jsonrpc":"2.0","result":true,"id":1}}"#);
        let expected: Response =
            serde_json::from_str(&expected).expect("expected response deserialization");
        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);
    }

    #[test]
    fn test_rpc_get_signature_status() {
        let bob_pubkey = Pubkey::new_rand();
        let (io, meta, blockhash, alice) = start_rpc_handler_with_tx(&bob_pubkey);
        let tx = system_transaction::transfer(&alice, &bob_pubkey, 20, blockhash, 0);

        let req = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"getSignatureStatus","params":["{}"]}}"#,
            tx.signatures[0]
        );
        let res = io.handle_request_sync(&req, meta.clone());
        let expected_res: Option<transaction::Result<()>> = Some(Ok(()));
        let expected = json!({
            "jsonrpc": "2.0",
            "result": expected_res,
            "id": 1
        });
        let expected: Response =
            serde_json::from_value(expected).expect("expected response deserialization");
        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);

        // Test getSignatureStatus request on unprocessed tx
        let tx = system_transaction::transfer(&alice, &bob_pubkey, 10, blockhash, 0);
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"getSignatureStatus","params":["{}"]}}"#,
            tx.signatures[0]
        );
        let res = io.handle_request_sync(&req, meta.clone());
        let expected_res: Option<String> = None;
        let expected = json!({
            "jsonrpc": "2.0",
            "result": expected_res,
            "id": 1
        });
        let expected: Response =
            serde_json::from_value(expected).expect("expected response deserialization");
        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);

        // Test getSignatureStatus request on a TransactionError
        let tx = system_transaction::transfer(&alice, &alice.pubkey(), 20, blockhash, 0);
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"getSignatureStatus","params":["{}"]}}"#,
            tx.signatures[0]
        );
        let res = io.handle_request_sync(&req, meta);
        let expected_res: Option<transaction::Result<()>> = Some(Err(
            TransactionError::InstructionError(0, InstructionError::DuplicateAccountIndex),
        ));
        let expected = json!({
            "jsonrpc": "2.0",
            "result": expected_res,
            "id": 1
        });
        let expected: Response =
            serde_json::from_value(expected).expect("expected response deserialization");
        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);
    }

    #[test]
    fn test_rpc_get_recent_blockhash() {
        let bob_pubkey = Pubkey::new_rand();
        let (io, meta, blockhash, _alice) = start_rpc_handler_with_tx(&bob_pubkey);

        let req = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"getRecentBlockhash"}}"#);
        let res = io.handle_request_sync(&req, meta);
        let expected = format!(r#"{{"jsonrpc":"2.0","result":"{}","id":1}}"#, blockhash);
        let expected: Response =
            serde_json::from_str(&expected).expect("expected response deserialization");
        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);
    }

    #[test]
    fn test_rpc_fail_request_airdrop() {
        let bob_pubkey = Pubkey::new_rand();
        let (io, meta, _blockhash, _alice) = start_rpc_handler_with_tx(&bob_pubkey);

        // Expect internal error because no drone is available
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"requestAirdrop","params":["{}", 50]}}"#,
            bob_pubkey
        );
        let res = io.handle_request_sync(&req, meta);
        let expected =
            r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid request"},"id":1}"#;
        let expected: Response =
            serde_json::from_str(expected).expect("expected response deserialization");
        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);
    }

    #[test]
    fn test_rpc_send_bad_tx() {
        let exit = Arc::new(AtomicBool::new(false));

        let mut io = MetaIoHandler::default();
        let rpc = RpcSolImpl;
        io.extend_with(rpc.to_delegate());
        let meta = Meta {
            request_processor: {
                let request_processor = JsonRpcRequestProcessor::new(
                    StorageState::default(),
                    JsonRpcConfig::default(),
                    new_bank_forks().0,
                    &exit,
                );
                Arc::new(RwLock::new(request_processor))
            },
            cluster_info: Arc::new(RwLock::new(ClusterInfo::new_with_invalid_keypair(
                ContactInfo::default(),
            ))),
        };

        let req =
            r#"{"jsonrpc":"2.0","id":1,"method":"sendTransaction","params":[[0,0,0,0,0,0,0,0]]}"#;
        let res = io.handle_request_sync(req, meta.clone());
        let expected =
            r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid request"},"id":1}"#;
        let expected: Response =
            serde_json::from_str(expected).expect("expected response deserialization");
        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);
    }

    #[test]
    fn test_rpc_get_tpu_addr() {
        let cluster_info = Arc::new(RwLock::new(ClusterInfo::new_with_invalid_keypair(
            ContactInfo::new_with_socketaddr(&socketaddr!("127.0.0.1:1234")),
        )));
        assert_eq!(
            get_tpu_addr(&cluster_info),
            Ok(socketaddr!("127.0.0.1:1234"))
        );
    }

    #[test]
    fn test_rpc_verify_pubkey() {
        let pubkey = Pubkey::new_rand();
        assert_eq!(verify_pubkey(pubkey.to_string()).unwrap(), pubkey);
        let bad_pubkey = "a1b2c3d4";
        assert_eq!(
            verify_pubkey(bad_pubkey.to_string()),
            Err(Error::invalid_request())
        );
    }

    #[test]
    fn test_rpc_verify_signature() {
        let tx =
            system_transaction::transfer(&Keypair::new(), &Pubkey::new_rand(), 20, hash(&[0]), 0);
        assert_eq!(
            verify_signature(&tx.signatures[0].to_string()).unwrap(),
            tx.signatures[0]
        );
        let bad_signature = "a1b2c3d4";
        assert_eq!(
            verify_signature(&bad_signature.to_string()),
            Err(Error::invalid_request())
        );
    }

    fn new_bank_forks() -> (Arc<RwLock<BankForks>>, Keypair) {
        let (genesis_block, alice) = GenesisBlock::new(10_000);
        let bank = Bank::new(&genesis_block);
        (
            Arc::new(RwLock::new(BankForks::new(bank.slot(), bank))),
            alice,
        )
    }

    #[test]
    fn test_rpc_request_processor_config_default_trait_fullnode_exit_fails() {
        let exit = Arc::new(AtomicBool::new(false));
        let request_processor = JsonRpcRequestProcessor::new(
            StorageState::default(),
            JsonRpcConfig::default(),
            new_bank_forks().0,
            &exit,
        );
        assert_eq!(request_processor.fullnode_exit(), Ok(false));
        assert_eq!(exit.load(Ordering::Relaxed), false);
    }

    #[test]
    fn test_rpc_request_processor_allow_fullnode_exit_config() {
        let exit = Arc::new(AtomicBool::new(false));
        let mut config = JsonRpcConfig::default();
        config.enable_fullnode_exit = true;
        let request_processor = JsonRpcRequestProcessor::new(
            StorageState::default(),
            config,
            new_bank_forks().0,
            &exit,
        );
        assert_eq!(request_processor.fullnode_exit(), Ok(true));
        assert_eq!(exit.load(Ordering::Relaxed), true);
    }
}
