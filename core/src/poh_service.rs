//! The `poh_service` module implements a service that records the passing of
//! "ticks", a measure of time in the PoH stream
use crate::poh_recorder::PohRecorder;
use crate::service::Service;
use solana_sdk::poh_config::PohConfig;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep, Builder, JoinHandle};

pub struct PohService {
    tick_producer: JoinHandle<()>,
}

// Number of hashes to batch together.
// * If this number is too small, PoH hash rate will suffer.
// * The larger this number is from 1, the speed of recording transactions will suffer due to lock
//   contention with the PoH hashing within `tick_producer()`.
//
// See benches/poh.rs for some benchmarks that attempt to justify this magic number.
pub const NUM_HASHES_PER_BATCH: u64 = 128;

impl PohService {
    pub fn new(
        poh_recorder: Arc<Mutex<PohRecorder>>,
        poh_config: &Arc<PohConfig>,
        poh_exit: &Arc<AtomicBool>,
    ) -> Self {
        let poh_exit_ = poh_exit.clone();
        let poh_config = poh_config.clone();
        let tick_producer = Builder::new()
            .name("solana-poh-service-tick_producer".to_string())
            .spawn(move || {
                if poh_config.hashes_per_tick.is_none() {
                    Self::sleepy_tick_producer(poh_recorder, &poh_config, &poh_exit_);
                } else {
                    Self::tick_producer(poh_recorder, &poh_exit_);
                }
                poh_exit_.store(true, Ordering::Relaxed);
            })
            .unwrap();

        Self { tick_producer }
    }

    fn sleepy_tick_producer(
        poh_recorder: Arc<Mutex<PohRecorder>>,
        poh_config: &PohConfig,
        poh_exit: &AtomicBool,
    ) {
        while !poh_exit.load(Ordering::Relaxed) {
            sleep(poh_config.target_tick_duration);
            poh_recorder.lock().unwrap().tick();
        }
    }

    fn tick_producer(poh_recorder: Arc<Mutex<PohRecorder>>, poh_exit: &AtomicBool) {
        let poh = poh_recorder.lock().unwrap().poh.clone();
        loop {
            if poh.lock().unwrap().hash(NUM_HASHES_PER_BATCH) {
                // Lock PohRecorder only for the final hash...
                poh_recorder.lock().unwrap().tick();
                if poh_exit.load(Ordering::Relaxed) {
                    break;
                }
            }
        }
    }
}

impl Service for PohService {
    type JoinReturnType = ();

    fn join(self) -> thread::Result<()> {
        self.tick_producer.join()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocktree::{get_tmp_ledger_path, Blocktree};
    use crate::genesis_utils::create_genesis_block;
    use crate::leader_schedule_cache::LeaderScheduleCache;
    use crate::poh_recorder::WorkingBank;
    use crate::result::Result;
    use crate::test_tx::test_tx;
    use solana_runtime::bank::Bank;
    use solana_sdk::hash::hash;
    use solana_sdk::pubkey::Pubkey;
    use std::time::Duration;

    #[test]
    fn test_poh_service() {
        let (genesis_block, _mint_keypair) = create_genesis_block(2);
        let bank = Arc::new(Bank::new(&genesis_block));
        let prev_hash = bank.last_blockhash();
        let ledger_path = get_tmp_ledger_path!();
        {
            let blocktree =
                Blocktree::open(&ledger_path).expect("Expected to be able to open database ledger");
            let poh_config = Arc::new(PohConfig {
                hashes_per_tick: Some(2),
                target_tick_duration: Duration::from_millis(42),
            });
            let (poh_recorder, entry_receiver) = PohRecorder::new(
                bank.tick_height(),
                prev_hash,
                bank.slot(),
                Some(4),
                bank.ticks_per_slot(),
                &Pubkey::default(),
                &Arc::new(blocktree),
                &Arc::new(LeaderScheduleCache::new_from_bank(&bank)),
                &poh_config,
            );
            let poh_recorder = Arc::new(Mutex::new(poh_recorder));
            let exit = Arc::new(AtomicBool::new(false));
            let working_bank = WorkingBank {
                bank: bank.clone(),
                min_tick_height: bank.tick_height(),
                max_tick_height: std::u64::MAX,
            };

            let entry_producer: JoinHandle<Result<()>> = {
                let poh_recorder = poh_recorder.clone();
                let exit = exit.clone();

                Builder::new()
                    .name("solana-poh-service-entry_producer".to_string())
                    .spawn(move || {
                        loop {
                            // send some data
                            let h1 = hash(b"hello world!");
                            let tx = test_tx();
                            let _ = poh_recorder
                                .lock()
                                .unwrap()
                                .record(bank.slot(), h1, vec![tx]);

                            if exit.load(Ordering::Relaxed) {
                                break Ok(());
                            }
                        }
                    })
                    .unwrap()
            };

            let poh_service = PohService::new(poh_recorder.clone(), &poh_config, &exit);
            poh_recorder.lock().unwrap().set_working_bank(working_bank);

            // get some events
            let mut hashes = 0;
            let mut need_tick = true;
            let mut need_entry = true;
            let mut need_partial = true;

            while need_tick || need_entry || need_partial {
                for entry in entry_receiver.recv().unwrap().1 {
                    let entry = &entry.0;
                    if entry.is_tick() {
                        assert!(
                            entry.num_hashes <= poh_config.hashes_per_tick.unwrap(),
                            format!(
                                "{} <= {}",
                                entry.num_hashes,
                                poh_config.hashes_per_tick.unwrap()
                            )
                        );

                        if entry.num_hashes == poh_config.hashes_per_tick.unwrap() {
                            need_tick = false;
                        } else {
                            need_partial = false;
                        }

                        hashes += entry.num_hashes;

                        assert_eq!(hashes, poh_config.hashes_per_tick.unwrap());

                        hashes = 0;
                    } else {
                        assert!(entry.num_hashes >= 1);
                        need_entry = false;
                        hashes += entry.num_hashes;
                    }
                }
            }
            exit.store(true, Ordering::Relaxed);
            let _ = poh_service.join().unwrap();
            let _ = entry_producer.join().unwrap();
        }
        Blocktree::destroy(&ledger_path).unwrap();
    }
}
