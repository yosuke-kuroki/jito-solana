//! The `optimistically_confirmed_bank_tracker` module implements a threaded service to track the
//! most recent optimistically confirmed bank for use in rpc services, and triggers gossip
//! subscription notifications

use crate::rpc_subscriptions::RpcSubscriptions;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use solana_runtime::{bank::Bank, bank_forks::BankForks};
use solana_sdk::clock::Slot;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    thread::{self, Builder, JoinHandle},
    time::Duration,
};

pub struct OptimisticallyConfirmedBank {
    pub bank: Arc<Bank>,
}

impl OptimisticallyConfirmedBank {
    pub fn locked_from_bank_forks_root(bank_forks: &Arc<RwLock<BankForks>>) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(Self {
            bank: bank_forks.read().unwrap().root_bank(),
        }))
    }
}

pub enum BankNotification {
    OptimisticallyConfirmed(Slot),
    Frozen(Arc<Bank>),
    Root(Arc<Bank>),
}

impl std::fmt::Debug for BankNotification {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            BankNotification::OptimisticallyConfirmed(slot) => {
                write!(f, "OptimisticallyConfirmed({:?})", slot)
            }
            BankNotification::Frozen(bank) => write!(f, "Frozen({})", bank.slot()),
            BankNotification::Root(bank) => write!(f, "Root({})", bank.slot()),
        }
    }
}

pub type BankNotificationReceiver = Receiver<BankNotification>;
pub type BankNotificationSender = Sender<BankNotification>;

pub struct OptimisticallyConfirmedBankTracker {
    thread_hdl: JoinHandle<()>,
}

impl OptimisticallyConfirmedBankTracker {
    pub fn new(
        receiver: BankNotificationReceiver,
        exit: &Arc<AtomicBool>,
        bank_forks: Arc<RwLock<BankForks>>,
        optimistically_confirmed_bank: Arc<RwLock<OptimisticallyConfirmedBank>>,
        subscriptions: Arc<RpcSubscriptions>,
    ) -> Self {
        let exit_ = exit.clone();
        let mut pending_optimistically_confirmed_banks = HashSet::new();
        let thread_hdl = Builder::new()
            .name("solana-optimistic-bank-tracker".to_string())
            .spawn(move || loop {
                if exit_.load(Ordering::Relaxed) {
                    break;
                }

                if let Err(RecvTimeoutError::Disconnected) = Self::recv_notification(
                    &receiver,
                    &bank_forks,
                    &optimistically_confirmed_bank,
                    &subscriptions,
                    &mut pending_optimistically_confirmed_banks,
                ) {
                    break;
                }
            })
            .unwrap();
        Self { thread_hdl }
    }

    fn recv_notification(
        receiver: &Receiver<BankNotification>,
        bank_forks: &Arc<RwLock<BankForks>>,
        optimistically_confirmed_bank: &Arc<RwLock<OptimisticallyConfirmedBank>>,
        subscriptions: &Arc<RpcSubscriptions>,
        mut pending_optimistically_confirmed_banks: &mut HashSet<Slot>,
    ) -> Result<(), RecvTimeoutError> {
        let notification = receiver.recv_timeout(Duration::from_secs(1))?;
        Self::process_notification(
            notification,
            bank_forks,
            optimistically_confirmed_bank,
            subscriptions,
            &mut pending_optimistically_confirmed_banks,
        );
        Ok(())
    }

    pub(crate) fn process_notification(
        notification: BankNotification,
        bank_forks: &Arc<RwLock<BankForks>>,
        optimistically_confirmed_bank: &Arc<RwLock<OptimisticallyConfirmedBank>>,
        subscriptions: &Arc<RpcSubscriptions>,
        pending_optimistically_confirmed_banks: &mut HashSet<Slot>,
    ) {
        debug!("received bank notification: {:?}", notification);
        match notification {
            BankNotification::OptimisticallyConfirmed(slot) => {
                if let Some(bank) = bank_forks
                    .read()
                    .unwrap()
                    .get(slot)
                    .filter(|b| b.is_frozen())
                {
                    let mut w_optimistically_confirmed_bank =
                        optimistically_confirmed_bank.write().unwrap();
                    if bank.slot() > w_optimistically_confirmed_bank.bank.slot() {
                        w_optimistically_confirmed_bank.bank = bank.clone();
                        subscriptions.notify_gossip_subscribers(slot);
                    }
                    drop(w_optimistically_confirmed_bank);
                } else if slot > bank_forks.read().unwrap().root_bank().slot() {
                    pending_optimistically_confirmed_banks.insert(slot);
                } else {
                    inc_new_counter_info!("dropped-already-rooted-optimistic-bank-notification", 1);
                }
            }
            BankNotification::Frozen(bank) => {
                let frozen_slot = bank.slot();
                if pending_optimistically_confirmed_banks.remove(&bank.slot()) {
                    let mut w_optimistically_confirmed_bank =
                        optimistically_confirmed_bank.write().unwrap();
                    if frozen_slot > w_optimistically_confirmed_bank.bank.slot() {
                        w_optimistically_confirmed_bank.bank = bank;
                        subscriptions.notify_gossip_subscribers(frozen_slot);
                    }
                    drop(w_optimistically_confirmed_bank);
                }
            }
            BankNotification::Root(bank) => {
                let root_slot = bank.slot();
                let mut w_optimistically_confirmed_bank =
                    optimistically_confirmed_bank.write().unwrap();
                if root_slot > w_optimistically_confirmed_bank.bank.slot() {
                    w_optimistically_confirmed_bank.bank = bank;
                }
                drop(w_optimistically_confirmed_bank);
                pending_optimistically_confirmed_banks.retain(|&s| s > root_slot);
            }
        }
    }

    pub fn close(self) -> thread::Result<()> {
        self.join()
    }

    pub fn join(self) -> thread::Result<()> {
        self.thread_hdl.join()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_ledger::genesis_utils::{create_genesis_config, GenesisConfigInfo};
    use solana_runtime::{
        accounts_background_service::AbsRequestSender, commitment::BlockCommitmentCache,
    };
    use solana_sdk::pubkey::Pubkey;

    #[test]
    fn test_process_notification() {
        let exit = Arc::new(AtomicBool::new(false));
        let GenesisConfigInfo { genesis_config, .. } = create_genesis_config(100);
        let bank = Bank::new(&genesis_config);
        let bank_forks = Arc::new(RwLock::new(BankForks::new(bank)));
        let bank0 = bank_forks.read().unwrap().get(0).unwrap().clone();
        let bank1 = Bank::new_from_parent(&bank0, &Pubkey::default(), 1);
        bank_forks.write().unwrap().insert(bank1);
        let bank1 = bank_forks.read().unwrap().get(1).unwrap().clone();
        let bank2 = Bank::new_from_parent(&bank1, &Pubkey::default(), 2);
        bank_forks.write().unwrap().insert(bank2);
        let bank2 = bank_forks.read().unwrap().get(2).unwrap().clone();
        let bank3 = Bank::new_from_parent(&bank2, &Pubkey::default(), 3);
        bank_forks.write().unwrap().insert(bank3);

        let optimistically_confirmed_bank =
            OptimisticallyConfirmedBank::locked_from_bank_forks_root(&bank_forks);

        let block_commitment_cache = Arc::new(RwLock::new(BlockCommitmentCache::default()));
        let subscriptions = Arc::new(RpcSubscriptions::new(
            &exit,
            bank_forks.clone(),
            block_commitment_cache,
            optimistically_confirmed_bank.clone(),
        ));
        let mut pending_optimistically_confirmed_banks = HashSet::new();

        assert_eq!(optimistically_confirmed_bank.read().unwrap().bank.slot(), 0);

        OptimisticallyConfirmedBankTracker::process_notification(
            BankNotification::OptimisticallyConfirmed(2),
            &bank_forks,
            &optimistically_confirmed_bank,
            &subscriptions,
            &mut pending_optimistically_confirmed_banks,
        );
        assert_eq!(optimistically_confirmed_bank.read().unwrap().bank.slot(), 2);

        // Test max optimistically confirmed bank remains in the cache
        OptimisticallyConfirmedBankTracker::process_notification(
            BankNotification::OptimisticallyConfirmed(1),
            &bank_forks,
            &optimistically_confirmed_bank,
            &subscriptions,
            &mut pending_optimistically_confirmed_banks,
        );
        assert_eq!(optimistically_confirmed_bank.read().unwrap().bank.slot(), 2);

        // Test bank will only be cached when frozen
        OptimisticallyConfirmedBankTracker::process_notification(
            BankNotification::OptimisticallyConfirmed(3),
            &bank_forks,
            &optimistically_confirmed_bank,
            &subscriptions,
            &mut pending_optimistically_confirmed_banks,
        );
        assert_eq!(optimistically_confirmed_bank.read().unwrap().bank.slot(), 2);
        assert_eq!(pending_optimistically_confirmed_banks.len(), 1);
        assert_eq!(pending_optimistically_confirmed_banks.contains(&3), true);

        // Test bank will only be cached when frozen
        let bank3 = bank_forks.read().unwrap().get(3).unwrap().clone();
        OptimisticallyConfirmedBankTracker::process_notification(
            BankNotification::Frozen(bank3),
            &bank_forks,
            &optimistically_confirmed_bank,
            &subscriptions,
            &mut pending_optimistically_confirmed_banks,
        );
        assert_eq!(optimistically_confirmed_bank.read().unwrap().bank.slot(), 3);

        // Test higher root will be cached and clear pending_optimistically_confirmed_banks
        let bank3 = bank_forks.read().unwrap().get(3).unwrap().clone();
        let bank4 = Bank::new_from_parent(&bank3, &Pubkey::default(), 4);
        bank_forks.write().unwrap().insert(bank4);
        OptimisticallyConfirmedBankTracker::process_notification(
            BankNotification::OptimisticallyConfirmed(4),
            &bank_forks,
            &optimistically_confirmed_bank,
            &subscriptions,
            &mut pending_optimistically_confirmed_banks,
        );
        assert_eq!(optimistically_confirmed_bank.read().unwrap().bank.slot(), 3);
        assert_eq!(pending_optimistically_confirmed_banks.len(), 1);
        assert_eq!(pending_optimistically_confirmed_banks.contains(&4), true);

        let bank4 = bank_forks.read().unwrap().get(4).unwrap().clone();
        let bank5 = Bank::new_from_parent(&bank4, &Pubkey::default(), 5);
        bank_forks.write().unwrap().insert(bank5);
        let bank5 = bank_forks.read().unwrap().get(5).unwrap().clone();
        OptimisticallyConfirmedBankTracker::process_notification(
            BankNotification::Root(bank5),
            &bank_forks,
            &optimistically_confirmed_bank,
            &subscriptions,
            &mut pending_optimistically_confirmed_banks,
        );
        assert_eq!(optimistically_confirmed_bank.read().unwrap().bank.slot(), 5);
        assert_eq!(pending_optimistically_confirmed_banks.len(), 0);
        assert_eq!(pending_optimistically_confirmed_banks.contains(&4), false);

        // Banks <= root do not get added to pending list, even if not frozen
        let bank5 = bank_forks.read().unwrap().get(5).unwrap().clone();
        let bank6 = Bank::new_from_parent(&bank5, &Pubkey::default(), 6);
        bank_forks.write().unwrap().insert(bank6);
        let bank5 = bank_forks.read().unwrap().get(5).unwrap().clone();
        let bank7 = Bank::new_from_parent(&bank5, &Pubkey::default(), 7);
        bank_forks.write().unwrap().insert(bank7);
        bank_forks
            .write()
            .unwrap()
            .set_root(7, &AbsRequestSender::default(), None);
        OptimisticallyConfirmedBankTracker::process_notification(
            BankNotification::OptimisticallyConfirmed(6),
            &bank_forks,
            &optimistically_confirmed_bank,
            &subscriptions,
            &mut pending_optimistically_confirmed_banks,
        );
        assert_eq!(optimistically_confirmed_bank.read().unwrap().bank.slot(), 5);
        assert_eq!(pending_optimistically_confirmed_banks.len(), 0);
        assert_eq!(pending_optimistically_confirmed_banks.contains(&6), false);
    }
}
