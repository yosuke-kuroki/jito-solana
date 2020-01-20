//! The `pubsub` module implements a threaded subscription service on client RPC request

use core::hash::Hash;
use jsonrpc_core::futures::Future;
use jsonrpc_pubsub::{typed::Sink, SubscriptionId};
use serde::Serialize;
use solana_client::rpc_response::{RpcAccount, RpcKeyedAccount};
use solana_ledger::bank_forks::BankForks;
use solana_runtime::bank::Bank;
use solana_sdk::{
    account::Account, clock::Slot, pubkey::Pubkey, signature::Signature, transaction,
};
use solana_vote_program::vote_state::MAX_LOCKOUT_HISTORY;
use std::ops::DerefMut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SendError, Sender};
use std::thread::{Builder, JoinHandle};
use std::time::Duration;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

const RECEIVE_DELAY_MILLIS: u64 = 100;

pub type Confirmations = usize;

#[derive(Serialize, Clone, Copy, Debug)]
pub struct SlotInfo {
    pub slot: Slot,
    pub parent: Slot,
    pub root: Slot,
}

enum NotificationEntry {
    Slot(SlotInfo),
    Bank((Slot, Arc<RwLock<BankForks>>)),
}

impl std::fmt::Debug for NotificationEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            NotificationEntry::Slot(slot_info) => write!(f, "Slot({:?})", slot_info),
            NotificationEntry::Bank((current_slot, _)) => {
                write!(f, "Bank({{current_slot: {:?}}})", current_slot)
            }
        }
    }
}

type NotificationSend = Arc<Mutex<NotificationEntry>>;

type RpcAccountSubscriptions =
    RwLock<HashMap<Pubkey, HashMap<SubscriptionId, (Sink<RpcAccount>, Confirmations)>>>;
type RpcProgramSubscriptions =
    RwLock<HashMap<Pubkey, HashMap<SubscriptionId, (Sink<RpcKeyedAccount>, Confirmations)>>>;
type RpcSignatureSubscriptions = RwLock<
    HashMap<Signature, HashMap<SubscriptionId, (Sink<transaction::Result<()>>, Confirmations)>>,
>;
type RpcSlotSubscriptions = RwLock<HashMap<SubscriptionId, Sink<SlotInfo>>>;

fn add_subscription<K, S>(
    subscriptions: &mut HashMap<K, HashMap<SubscriptionId, (Sink<S>, Confirmations)>>,
    hashmap_key: &K,
    confirmations: Option<Confirmations>,
    sub_id: &SubscriptionId,
    sink: &Sink<S>,
) where
    K: Eq + Hash + Clone + Copy,
    S: Clone,
{
    let confirmations = confirmations.unwrap_or(0);
    let confirmations = if confirmations > MAX_LOCKOUT_HISTORY {
        MAX_LOCKOUT_HISTORY
    } else {
        confirmations
    };
    if let Some(current_hashmap) = subscriptions.get_mut(hashmap_key) {
        current_hashmap.insert(sub_id.clone(), (sink.clone(), confirmations));
        return;
    }
    let mut hashmap = HashMap::new();
    hashmap.insert(sub_id.clone(), (sink.clone(), confirmations));
    subscriptions.insert(*hashmap_key, hashmap);
}

fn remove_subscription<K, S>(
    subscriptions: &mut HashMap<K, HashMap<SubscriptionId, (Sink<S>, Confirmations)>>,
    sub_id: &SubscriptionId,
) -> bool
where
    K: Eq + Hash + Clone + Copy,
    S: Clone,
{
    let mut found = false;
    subscriptions.retain(|_, v| {
        v.retain(|k, _| {
            if *k == *sub_id {
                found = true;
            }
            !found
        });
        !v.is_empty()
    });
    found
}

fn check_confirmations_and_notify<K, S, F, N, X>(
    subscriptions: &HashMap<K, HashMap<SubscriptionId, (Sink<S>, Confirmations)>>,
    hashmap_key: &K,
    current_slot: Slot,
    bank_forks: &Arc<RwLock<BankForks>>,
    bank_method: F,
    notify: N,
) where
    K: Eq + Hash + Clone + Copy,
    S: Clone + Serialize,
    F: Fn(&Bank, &K) -> X,
    N: Fn(X, &Sink<S>, u64),
    X: Clone + Serialize,
{
    let current_ancestors = bank_forks
        .read()
        .unwrap()
        .get(current_slot)
        .unwrap()
        .ancestors
        .clone();
    if let Some(hashmap) = subscriptions.get(hashmap_key) {
        for (_bank_sub_id, (sink, confirmations)) in hashmap.iter() {
            let desired_slot: Vec<u64> = current_ancestors
                .iter()
                .filter(|(_, &v)| v == *confirmations)
                .map(|(k, _)| k)
                .cloned()
                .collect();
            let root: Vec<u64> = current_ancestors
                .iter()
                .filter(|(_, &v)| v == 32)
                .map(|(k, _)| k)
                .cloned()
                .collect();
            let root = if root.len() == 1 { root[0] } else { 0 };
            if desired_slot.len() == 1 {
                let desired_bank = bank_forks
                    .read()
                    .unwrap()
                    .get(desired_slot[0])
                    .unwrap()
                    .clone();
                let result = bank_method(&desired_bank, hashmap_key);
                notify(result, &sink, root);
            }
        }
    }
}

fn notify_account(result: Option<(Account, Slot)>, sink: &Sink<RpcAccount>, root: Slot) {
    if let Some((account, fork)) = result {
        if fork >= root {
            sink.notify(Ok(RpcAccount::encode(account))).wait().unwrap();
        }
    }
}

fn notify_signature<S>(result: Option<S>, sink: &Sink<S>, _root: Slot)
where
    S: Clone + Serialize,
{
    if let Some(result) = result {
        sink.notify(Ok(result)).wait().unwrap();
    }
}

fn notify_program(accounts: Vec<(Pubkey, Account)>, sink: &Sink<RpcKeyedAccount>, _root: Slot) {
    for (pubkey, account) in accounts.iter() {
        sink.notify(Ok(RpcKeyedAccount {
            pubkey: pubkey.to_string(),
            account: RpcAccount::encode(account.clone()),
        }))
        .wait()
        .unwrap();
    }
}

pub struct RpcSubscriptions {
    account_subscriptions: Arc<RpcAccountSubscriptions>,
    program_subscriptions: Arc<RpcProgramSubscriptions>,
    signature_subscriptions: Arc<RpcSignatureSubscriptions>,
    slot_subscriptions: Arc<RpcSlotSubscriptions>,
    notification_sender: Arc<Mutex<Sender<Arc<Mutex<NotificationEntry>>>>>,
    t_cleanup: Option<JoinHandle<()>>,
    exit: Arc<AtomicBool>,
}

impl Default for RpcSubscriptions {
    fn default() -> Self {
        Self::new(&Arc::new(AtomicBool::new(false)))
    }
}

impl Drop for RpcSubscriptions {
    fn drop(&mut self) {
        self.shutdown().unwrap_or_else(|err| {
            warn!("RPC Notification - shutdown error: {:?}", err);
        });
    }
}

impl RpcSubscriptions {
    pub fn new(exit: &Arc<AtomicBool>) -> Self {
        let (notification_sender, notification_receiver): (
            Sender<NotificationSend>,
            Receiver<NotificationSend>,
        ) = std::sync::mpsc::channel();

        let account_subscriptions = Arc::new(RpcAccountSubscriptions::default());
        let program_subscriptions = Arc::new(RpcProgramSubscriptions::default());
        let signature_subscriptions = Arc::new(RpcSignatureSubscriptions::default());
        let slot_subscriptions = Arc::new(RpcSlotSubscriptions::default());
        let notification_sender = Arc::new(Mutex::new(notification_sender));

        let exit_clone = exit.clone();
        let account_subscriptions_clone = account_subscriptions.clone();
        let program_subscriptions_clone = program_subscriptions.clone();
        let signature_subscriptions_clone = signature_subscriptions.clone();
        let slot_subscriptions_clone = slot_subscriptions.clone();

        let t_cleanup = Builder::new()
            .name("solana-rpc-notifications".to_string())
            .spawn(move || {
                Self::process_notifications(
                    exit_clone,
                    notification_receiver,
                    account_subscriptions_clone,
                    program_subscriptions_clone,
                    signature_subscriptions_clone,
                    slot_subscriptions_clone,
                );
            })
            .unwrap();

        Self {
            account_subscriptions,
            program_subscriptions,
            signature_subscriptions,
            slot_subscriptions,
            notification_sender,
            t_cleanup: Some(t_cleanup),
            exit: exit.clone(),
        }
    }

    fn check_account(
        pubkey: &Pubkey,
        current_slot: Slot,
        bank_forks: &Arc<RwLock<BankForks>>,
        account_subscriptions: Arc<RpcAccountSubscriptions>,
    ) {
        let subscriptions = account_subscriptions.read().unwrap();
        check_confirmations_and_notify(
            &subscriptions,
            pubkey,
            current_slot,
            bank_forks,
            Bank::get_account_modified_since_parent,
            notify_account,
        );
    }

    fn check_program(
        program_id: &Pubkey,
        current_slot: Slot,
        bank_forks: &Arc<RwLock<BankForks>>,
        program_subscriptions: Arc<RpcProgramSubscriptions>,
    ) {
        let subscriptions = program_subscriptions.read().unwrap();
        check_confirmations_and_notify(
            &subscriptions,
            program_id,
            current_slot,
            bank_forks,
            Bank::get_program_accounts_modified_since_parent,
            notify_program,
        );
    }

    fn check_signature(
        signature: &Signature,
        current_slot: Slot,
        bank_forks: &Arc<RwLock<BankForks>>,
        signature_subscriptions: Arc<RpcSignatureSubscriptions>,
    ) {
        let mut subscriptions = signature_subscriptions.write().unwrap();
        check_confirmations_and_notify(
            &subscriptions,
            signature,
            current_slot,
            bank_forks,
            Bank::get_signature_status,
            notify_signature,
        );
        subscriptions.remove(&signature);
    }

    pub fn add_account_subscription(
        &self,
        pubkey: &Pubkey,
        confirmations: Option<Confirmations>,
        sub_id: &SubscriptionId,
        sink: &Sink<RpcAccount>,
    ) {
        let mut subscriptions = self.account_subscriptions.write().unwrap();
        add_subscription(&mut subscriptions, pubkey, confirmations, sub_id, sink);
    }

    pub fn remove_account_subscription(&self, id: &SubscriptionId) -> bool {
        let mut subscriptions = self.account_subscriptions.write().unwrap();
        remove_subscription(&mut subscriptions, id)
    }

    pub fn add_program_subscription(
        &self,
        program_id: &Pubkey,
        confirmations: Option<Confirmations>,
        sub_id: &SubscriptionId,
        sink: &Sink<RpcKeyedAccount>,
    ) {
        let mut subscriptions = self.program_subscriptions.write().unwrap();
        add_subscription(&mut subscriptions, program_id, confirmations, sub_id, sink);
    }

    pub fn remove_program_subscription(&self, id: &SubscriptionId) -> bool {
        let mut subscriptions = self.program_subscriptions.write().unwrap();
        remove_subscription(&mut subscriptions, id)
    }

    pub fn add_signature_subscription(
        &self,
        signature: &Signature,
        confirmations: Option<Confirmations>,
        sub_id: &SubscriptionId,
        sink: &Sink<transaction::Result<()>>,
    ) {
        let mut subscriptions = self.signature_subscriptions.write().unwrap();
        add_subscription(&mut subscriptions, signature, confirmations, sub_id, sink);
    }

    pub fn remove_signature_subscription(&self, id: &SubscriptionId) -> bool {
        let mut subscriptions = self.signature_subscriptions.write().unwrap();
        remove_subscription(&mut subscriptions, id)
    }

    /// Notify subscribers of changes to any accounts or new signatures since
    /// the bank's last checkpoint.
    pub fn notify_subscribers(&self, current_slot: Slot, bank_forks: &Arc<RwLock<BankForks>>) {
        self.enqueue_notification(NotificationEntry::Bank((current_slot, bank_forks.clone())));
    }

    pub fn add_slot_subscription(&self, sub_id: &SubscriptionId, sink: &Sink<SlotInfo>) {
        let mut subscriptions = self.slot_subscriptions.write().unwrap();
        subscriptions.insert(sub_id.clone(), sink.clone());
    }

    pub fn remove_slot_subscription(&self, id: &SubscriptionId) -> bool {
        let mut subscriptions = self.slot_subscriptions.write().unwrap();
        subscriptions.remove(id).is_some()
    }

    pub fn notify_slot(&self, slot: Slot, parent: Slot, root: Slot) {
        self.enqueue_notification(NotificationEntry::Slot(SlotInfo { slot, parent, root }));
    }

    fn enqueue_notification(&self, notification_entry: NotificationEntry) {
        match self
            .notification_sender
            .lock()
            .unwrap()
            .send(Arc::new(Mutex::new(notification_entry)))
        {
            Ok(()) => (),
            Err(SendError(notification)) => {
                warn!(
                    "Dropped RPC Notification - receiver disconnected : {:?}",
                    notification
                );
            }
        }
    }

    fn process_notifications(
        exit: Arc<AtomicBool>,
        notification_receiver: Receiver<Arc<Mutex<NotificationEntry>>>,
        account_subscriptions: Arc<RpcAccountSubscriptions>,
        program_subscriptions: Arc<RpcProgramSubscriptions>,
        signature_subscriptions: Arc<RpcSignatureSubscriptions>,
        slot_subscriptions: Arc<RpcSlotSubscriptions>,
    ) {
        loop {
            if exit.load(Ordering::Relaxed) {
                break;
            }
            match notification_receiver.recv_timeout(Duration::from_millis(RECEIVE_DELAY_MILLIS)) {
                Ok(notification_entry) => {
                    let mut notification_entry = notification_entry.lock().unwrap();
                    match notification_entry.deref_mut() {
                        NotificationEntry::Slot(slot_info) => {
                            let subscriptions = slot_subscriptions.read().unwrap();
                            for (_, sink) in subscriptions.iter() {
                                sink.notify(Ok(*slot_info)).wait().unwrap();
                            }
                        }
                        NotificationEntry::Bank((current_slot, bank_forks)) => {
                            let pubkeys: Vec<_> = {
                                let subs = account_subscriptions.read().unwrap();
                                subs.keys().cloned().collect()
                            };
                            for pubkey in &pubkeys {
                                Self::check_account(
                                    pubkey,
                                    *current_slot,
                                    &bank_forks,
                                    account_subscriptions.clone(),
                                );
                            }

                            let programs: Vec<_> = {
                                let subs = program_subscriptions.read().unwrap();
                                subs.keys().cloned().collect()
                            };
                            for program_id in &programs {
                                Self::check_program(
                                    program_id,
                                    *current_slot,
                                    &bank_forks,
                                    program_subscriptions.clone(),
                                );
                            }

                            let signatures: Vec<_> = {
                                let subs = signature_subscriptions.read().unwrap();
                                subs.keys().cloned().collect()
                            };
                            for signature in &signatures {
                                Self::check_signature(
                                    signature,
                                    *current_slot,
                                    &bank_forks,
                                    signature_subscriptions.clone(),
                                );
                            }
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    // not a problem - try reading again
                }
                Err(RecvTimeoutError::Disconnected) => {
                    warn!("RPC Notification thread - sender disconnected");
                    break;
                }
            }
        }
    }

    fn shutdown(&mut self) -> std::thread::Result<()> {
        if self.t_cleanup.is_some() {
            info!("RPC Notification thread - shutting down");
            self.exit.store(true, Ordering::Relaxed);
            let x = self.t_cleanup.take().unwrap().join();
            info!("RPC Notification thread - shut down.");
            x
        } else {
            warn!("RPC Notification thread - already shut down.");
            Ok(())
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::genesis_utils::{create_genesis_config, GenesisConfigInfo};
    use jsonrpc_core::futures;
    use jsonrpc_pubsub::typed::Subscriber;
    use solana_budget_program;
    use solana_sdk::{
        signature::{Keypair, KeypairUtil},
        system_transaction,
    };
    use tokio::prelude::{Async, Stream};

    pub(crate) fn robust_poll<T>(
        mut receiver: futures::sync::mpsc::Receiver<T>,
    ) -> Result<T, RecvTimeoutError> {
        const INITIAL_DELAY_MS: u64 = RECEIVE_DELAY_MILLIS * 2;

        std::thread::sleep(Duration::from_millis(INITIAL_DELAY_MS));
        for _i in 0..5 {
            let found = receiver.poll();
            if let Ok(Async::Ready(Some(result))) = found {
                return Ok(result);
            }
            std::thread::sleep(Duration::from_millis(RECEIVE_DELAY_MILLIS));
        }
        Err(RecvTimeoutError::Timeout)
    }

    pub(crate) fn robust_poll_or_panic<T>(receiver: futures::sync::mpsc::Receiver<T>) -> T {
        robust_poll(receiver).unwrap_or_else(|err| panic!("expected response! {}", err))
    }

    #[test]
    fn test_check_account_subscribe() {
        let GenesisConfigInfo {
            genesis_config,
            mint_keypair,
            ..
        } = create_genesis_config(100);
        let bank = Bank::new(&genesis_config);
        let blockhash = bank.last_blockhash();
        let bank_forks = Arc::new(RwLock::new(BankForks::new(0, bank)));
        let alice = Keypair::new();
        let tx = system_transaction::create_account(
            &mint_keypair,
            &alice,
            blockhash,
            1,
            16,
            &solana_budget_program::id(),
        );
        bank_forks
            .write()
            .unwrap()
            .get(0)
            .unwrap()
            .process_transaction(&tx)
            .unwrap();

        let (subscriber, _id_receiver, transport_receiver) =
            Subscriber::new_test("accountNotification");
        let sub_id = SubscriptionId::Number(0 as u64);
        let sink = subscriber.assign_id(sub_id.clone()).unwrap();
        let exit = Arc::new(AtomicBool::new(false));
        let subscriptions = RpcSubscriptions::new(&exit);
        subscriptions.add_account_subscription(&alice.pubkey(), None, &sub_id, &sink);

        assert!(subscriptions
            .account_subscriptions
            .read()
            .unwrap()
            .contains_key(&alice.pubkey()));

        subscriptions.notify_subscribers(0, &bank_forks);
        let response = robust_poll_or_panic(transport_receiver);
        let expected = format!(
            r#"{{"jsonrpc":"2.0","method":"accountNotification","params":{{"result":{{"data":"1111111111111111","executable":false,"lamports":1,"owner":"Budget1111111111111111111111111111111111111","rentEpoch":1}},"subscription":0}}}}"#
        );
        assert_eq!(expected, response);

        subscriptions.remove_account_subscription(&sub_id);
        assert!(!subscriptions
            .account_subscriptions
            .read()
            .unwrap()
            .contains_key(&alice.pubkey()));
    }

    #[test]
    fn test_check_program_subscribe() {
        let GenesisConfigInfo {
            genesis_config,
            mint_keypair,
            ..
        } = create_genesis_config(100);
        let bank = Bank::new(&genesis_config);
        let blockhash = bank.last_blockhash();
        let bank_forks = Arc::new(RwLock::new(BankForks::new(0, bank)));
        let alice = Keypair::new();
        let tx = system_transaction::create_account(
            &mint_keypair,
            &alice,
            blockhash,
            1,
            16,
            &solana_budget_program::id(),
        );
        bank_forks
            .write()
            .unwrap()
            .get(0)
            .unwrap()
            .process_transaction(&tx)
            .unwrap();

        let (subscriber, _id_receiver, transport_receiver) =
            Subscriber::new_test("programNotification");
        let sub_id = SubscriptionId::Number(0 as u64);
        let sink = subscriber.assign_id(sub_id.clone()).unwrap();
        let exit = Arc::new(AtomicBool::new(false));
        let subscriptions = RpcSubscriptions::new(&exit);
        subscriptions.add_program_subscription(&solana_budget_program::id(), None, &sub_id, &sink);

        assert!(subscriptions
            .program_subscriptions
            .read()
            .unwrap()
            .contains_key(&solana_budget_program::id()));

        subscriptions.notify_subscribers(0, &bank_forks);
        let response = robust_poll_or_panic(transport_receiver);
        let expected = format!(
            r#"{{"jsonrpc":"2.0","method":"programNotification","params":{{"result":{{"account":{{"data":"1111111111111111","executable":false,"lamports":1,"owner":"Budget1111111111111111111111111111111111111","rentEpoch":1}},"pubkey":"{:?}"}},"subscription":0}}}}"#,
            alice.pubkey()
        );
        assert_eq!(expected, response);

        subscriptions.remove_program_subscription(&sub_id);
        assert!(!subscriptions
            .program_subscriptions
            .read()
            .unwrap()
            .contains_key(&solana_budget_program::id()));
    }
    #[test]
    fn test_check_signature_subscribe() {
        let GenesisConfigInfo {
            genesis_config,
            mint_keypair,
            ..
        } = create_genesis_config(100);
        let bank = Bank::new(&genesis_config);
        let blockhash = bank.last_blockhash();
        let bank_forks = Arc::new(RwLock::new(BankForks::new(0, bank)));
        let alice = Keypair::new();
        let tx = system_transaction::transfer(&mint_keypair, &alice.pubkey(), 20, blockhash);
        let signature = tx.signatures[0];
        bank_forks
            .write()
            .unwrap()
            .get(0)
            .unwrap()
            .process_transaction(&tx)
            .unwrap();

        let (subscriber, _id_receiver, transport_receiver) =
            Subscriber::new_test("signatureNotification");
        let sub_id = SubscriptionId::Number(0 as u64);
        let sink = subscriber.assign_id(sub_id.clone()).unwrap();
        let exit = Arc::new(AtomicBool::new(false));
        let subscriptions = RpcSubscriptions::new(&exit);
        subscriptions.add_signature_subscription(&signature, None, &sub_id, &sink);

        assert!(subscriptions
            .signature_subscriptions
            .read()
            .unwrap()
            .contains_key(&signature));

        subscriptions.notify_subscribers(0, &bank_forks);
        let response = robust_poll_or_panic(transport_receiver);
        let expected_res: Option<transaction::Result<()>> = Some(Ok(()));
        let expected_res_str =
            serde_json::to_string(&serde_json::to_value(expected_res).unwrap()).unwrap();
        let expected = format!(
            r#"{{"jsonrpc":"2.0","method":"signatureNotification","params":{{"result":{},"subscription":0}}}}"#,
            expected_res_str
        );
        assert_eq!(expected, response);

        subscriptions.remove_signature_subscription(&sub_id);
        assert!(!subscriptions
            .signature_subscriptions
            .read()
            .unwrap()
            .contains_key(&signature));
    }
    #[test]
    fn test_check_slot_subscribe() {
        let (subscriber, _id_receiver, transport_receiver) =
            Subscriber::new_test("slotNotification");
        let sub_id = SubscriptionId::Number(0 as u64);
        let sink = subscriber.assign_id(sub_id.clone()).unwrap();
        let exit = Arc::new(AtomicBool::new(false));
        let subscriptions = RpcSubscriptions::new(&exit);
        subscriptions.add_slot_subscription(&sub_id, &sink);

        assert!(subscriptions
            .slot_subscriptions
            .read()
            .unwrap()
            .contains_key(&sub_id));

        subscriptions.notify_slot(0, 0, 0);
        let response = robust_poll_or_panic(transport_receiver);
        let expected_res = SlotInfo {
            parent: 0,
            slot: 0,
            root: 0,
        };
        let expected_res_str =
            serde_json::to_string(&serde_json::to_value(expected_res).unwrap()).unwrap();
        let expected = format!(
            r#"{{"jsonrpc":"2.0","method":"slotNotification","params":{{"result":{},"subscription":0}}}}"#,
            expected_res_str
        );
        assert_eq!(expected, response);

        subscriptions.remove_slot_subscription(&sub_id);
        assert!(!subscriptions
            .slot_subscriptions
            .read()
            .unwrap()
            .contains_key(&sub_id));
    }
}
