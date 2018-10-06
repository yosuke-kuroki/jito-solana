use influx_db_client as influxdb;
use metrics;
use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use timing;

const DEFAULT_METRICS_RATE: usize = 100;

pub struct Counter {
    pub name: &'static str,
    /// total accumulated value
    pub counts: AtomicUsize,
    pub times: AtomicUsize,
    /// last accumulated value logged
    pub lastlog: AtomicUsize,
    pub lograte: AtomicUsize,
}

macro_rules! create_counter {
    ($name:expr, $lograte:expr) => {
        Counter {
            name: $name,
            counts: AtomicUsize::new(0),
            times: AtomicUsize::new(0),
            lastlog: AtomicUsize::new(0),
            lograte: AtomicUsize::new($lograte),
        }
    };
}

macro_rules! inc_counter {
    ($name:expr, $count:expr) => {
        unsafe { $name.inc($count) };
    };
}

macro_rules! inc_new_counter_info {
    ($name:expr, $count:expr) => {{
        inc_new_counter!($name, $count, Level::Info, 0);
    }};
    ($name:expr, $count:expr, $lograte:expr) => {{
        inc_new_counter!($name, $count, Level::Info, $lograte);
    }};
}

macro_rules! inc_new_counter {
    ($name:expr, $count:expr, $level:expr, $lograte:expr) => {{
        if log_enabled!($level) {
            static mut INC_NEW_COUNTER: Counter = create_counter!($name, $lograte);
            inc_counter!(INC_NEW_COUNTER, $count);
        }
    }};
}

impl Counter {
    fn default_log_rate() -> usize {
        let v = env::var("SOLANA_DEFAULT_METRICS_RATE")
            .map(|x| x.parse().unwrap_or(DEFAULT_METRICS_RATE))
            .unwrap_or(DEFAULT_METRICS_RATE);
        if v == 0 {
            DEFAULT_METRICS_RATE
        } else {
            v
        }
    }
    pub fn inc(&mut self, events: usize) {
        let counts = self.counts.fetch_add(events, Ordering::Relaxed);
        let times = self.times.fetch_add(1, Ordering::Relaxed);
        let mut lograte = self.lograte.load(Ordering::Relaxed);
        if lograte == 0 {
            lograte = Counter::default_log_rate();
            self.lograte.store(lograte, Ordering::Relaxed);
        }
        if times % lograte == 0 && times > 0 {
            info!(
                "COUNTER:{{\"name\": \"{}\", \"counts\": {}, \"samples\": {},  \"now\": {}, \"events\": {}}}",
                self.name,
                counts + events,
                times,
                timing::timestamp(),
                events,
            );

            let lastlog = self.lastlog.load(Ordering::Relaxed);
            let prev = self
                .lastlog
                .compare_and_swap(lastlog, counts, Ordering::Relaxed);
            if prev == lastlog {
                metrics::submit(
                    influxdb::Point::new(&format!("counter-{}", self.name))
                        .add_field(
                            "count",
                            influxdb::Value::Integer(counts as i64 - lastlog as i64),
                        ).to_owned(),
                );
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use counter::{Counter, DEFAULT_METRICS_RATE};
    use log::Level;
    use std::env;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Once, RwLock, ONCE_INIT};

    fn get_env_lock() -> &'static RwLock<()> {
        static mut ENV_LOCK: Option<RwLock<()>> = None;
        static INIT_HOOK: Once = ONCE_INIT;

        unsafe {
            INIT_HOOK.call_once(|| {
                ENV_LOCK = Some(RwLock::new(()));
            });
            &ENV_LOCK.as_ref().unwrap()
        }
    }

    #[test]
    fn test_counter() {
        let _readlock = get_env_lock().read();
        static mut COUNTER: Counter = create_counter!("test", 100);
        let count = 1;
        inc_counter!(COUNTER, count);
        unsafe {
            assert_eq!(COUNTER.counts.load(Ordering::Relaxed), 1);
            assert_eq!(COUNTER.times.load(Ordering::Relaxed), 1);
            assert_eq!(COUNTER.lograte.load(Ordering::Relaxed), 100);
            assert_eq!(COUNTER.lastlog.load(Ordering::Relaxed), 0);
            assert_eq!(COUNTER.name, "test");
        }
        for _ in 0..199 {
            inc_counter!(COUNTER, 2);
        }
        unsafe {
            assert_eq!(COUNTER.lastlog.load(Ordering::Relaxed), 199);
        }
        inc_counter!(COUNTER, 2);
        unsafe {
            assert_eq!(COUNTER.lastlog.load(Ordering::Relaxed), 399);
        }
    }
    #[test]
    fn test_inc_new_counter() {
        let _readlock = get_env_lock().read();
        //make sure that macros are syntactically correct
        //the variable is internal to the macro scope so there is no way to introspect it
        inc_new_counter_info!("counter-1", 1);
        inc_new_counter_info!("counter-2", 1, 2);
    }
    #[test]
    fn test_lograte() {
        let _readlock = get_env_lock().read();
        assert_eq!(
            Counter::default_log_rate(),
            DEFAULT_METRICS_RATE,
            "default_log_rate() is {}, expected {}, SOLANA_DEFAULT_METRICS_RATE environment variable set?",
            Counter::default_log_rate(),
            DEFAULT_METRICS_RATE,
        );
        static mut COUNTER: Counter = create_counter!("test_lograte", 0);
        inc_counter!(COUNTER, 2);
        unsafe {
            assert_eq!(
                COUNTER.lograte.load(Ordering::Relaxed),
                DEFAULT_METRICS_RATE
            );
        }
    }

    #[test]
    fn test_lograte_env() {
        assert_ne!(DEFAULT_METRICS_RATE, 0);
        let _writelock = get_env_lock().write();
        static mut COUNTER: Counter = create_counter!("test_lograte_env", 0);
        env::set_var("SOLANA_DEFAULT_METRICS_RATE", "50");
        inc_counter!(COUNTER, 2);
        unsafe {
            assert_eq!(COUNTER.lograte.load(Ordering::Relaxed), 50);
        }

        static mut COUNTER2: Counter = create_counter!("test_lograte_env", 0);
        env::set_var("SOLANA_DEFAULT_METRICS_RATE", "0");
        inc_counter!(COUNTER2, 2);
        unsafe {
            assert_eq!(
                COUNTER2.lograte.load(Ordering::Relaxed),
                DEFAULT_METRICS_RATE
            );
        }
    }
}
