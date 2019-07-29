//! The `logger` module provides a setup function for `env_logger`. Its only function,
//! `setup()` may be called multiple times.

use env_logger;
use std::sync::Once;

static INIT: Once = Once::new();

pub fn setup_with_filter(filter: &str) {
    INIT.call_once(|| {
        env_logger::Builder::from_env(env_logger::Env::new().default_filter_or(filter))
            .default_format_timestamp_nanos(true)
            .init();
    });
}

pub fn setup() {
    setup_with_filter("error");
}
