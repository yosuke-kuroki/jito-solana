use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use solana_ledger::blockstore::Blockstore;
use solana_measure::measure::Measure;
use solana_runtime::bank::Bank;
use solana_sdk::{feature_set, timing::slot_duration_from_slots_per_year};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, Builder, JoinHandle},
    time::Duration,
};

pub type CacheBlockTimeReceiver = Receiver<Arc<Bank>>;
pub type CacheBlockTimeSender = Sender<Arc<Bank>>;

pub struct CacheBlockTimeService {
    thread_hdl: JoinHandle<()>,
}

const CACHE_BLOCK_TIME_WARNING_MS: u64 = 150;

impl CacheBlockTimeService {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        cache_block_time_receiver: CacheBlockTimeReceiver,
        blockstore: Arc<Blockstore>,
        exit: &Arc<AtomicBool>,
    ) -> Self {
        let exit = exit.clone();
        let thread_hdl = Builder::new()
            .name("solana-cache-block-time".to_string())
            .spawn(move || loop {
                if exit.load(Ordering::Relaxed) {
                    break;
                }
                let recv_result = cache_block_time_receiver.recv_timeout(Duration::from_secs(1));
                match recv_result {
                    Err(RecvTimeoutError::Disconnected) => {
                        break;
                    }
                    Ok(bank) => {
                        let mut cache_block_time_timer = Measure::start("cache_block_time_timer");
                        Self::cache_block_time(bank, &blockstore);
                        cache_block_time_timer.stop();
                        if cache_block_time_timer.as_ms() > CACHE_BLOCK_TIME_WARNING_MS {
                            warn!(
                                "cache_block_time operation took: {}ms",
                                cache_block_time_timer.as_ms()
                            );
                        }
                    }
                    _ => {}
                }
            })
            .unwrap();
        Self { thread_hdl }
    }

    fn cache_block_time(bank: Arc<Bank>, blockstore: &Arc<Blockstore>) {
        if bank
            .feature_set
            .is_active(&feature_set::timestamp_correction::id())
        {
            if let Err(e) = blockstore.cache_block_time(bank.slot(), bank.clock().unix_timestamp) {
                error!("cache_block_time failed: slot {:?} {:?}", bank.slot(), e);
            }
        } else {
            let slot_duration = slot_duration_from_slots_per_year(bank.slots_per_year());
            let epoch = bank.epoch_schedule().get_epoch(bank.slot());
            let stakes = HashMap::new();
            let stakes = bank.epoch_vote_accounts(epoch).unwrap_or(&stakes);

            if let Err(e) =
                blockstore.cache_block_time_from_slot_entries(bank.slot(), slot_duration, stakes)
            {
                error!("cache_block_time failed: slot {:?} {:?}", bank.slot(), e);
            }
        }
    }

    pub fn join(self) -> thread::Result<()> {
        self.thread_hdl.join()
    }
}
