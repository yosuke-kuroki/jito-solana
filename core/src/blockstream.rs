//! The `blockstream` module provides a method for streaming entries out via a
//! local unix socket, to provide client services such as a block explorer with
//! real-time access to entries.

use crate::entry::Entry;
use crate::result::Result;
use chrono::{SecondsFormat, Utc};
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use std::cell::RefCell;
use std::io::prelude::*;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;

pub trait EntryWriter: std::fmt::Debug {
    fn write(&self, payload: String) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct EntryVec {
    values: RefCell<Vec<String>>,
}

impl EntryWriter for EntryVec {
    fn write(&self, payload: String) -> Result<()> {
        self.values.borrow_mut().push(payload);
        Ok(())
    }
}

impl EntryVec {
    pub fn new() -> Self {
        EntryVec {
            values: RefCell::new(Vec::new()),
        }
    }

    pub fn entries(&self) -> Vec<String> {
        self.values.borrow().clone()
    }
}

#[derive(Debug)]
pub struct EntrySocket {
    socket: String,
}

const MESSAGE_TERMINATOR: &str = "\n";

impl EntryWriter for EntrySocket {
    fn write(&self, payload: String) -> Result<()> {
        let mut socket = UnixStream::connect(Path::new(&self.socket))?;
        socket.write_all(payload.as_bytes())?;
        socket.write_all(MESSAGE_TERMINATOR.as_bytes())?;
        socket.shutdown(Shutdown::Write)?;
        Ok(())
    }
}

pub trait BlockstreamEvents {
    fn emit_entry_event(
        &self,
        slot: u64,
        tick_height: u64,
        leader_id: &Pubkey,
        entries: &Entry,
    ) -> Result<()>;
    fn emit_block_event(
        &self,
        slot: u64,
        tick_height: u64,
        leader_id: &Pubkey,
        blockhash: Hash,
    ) -> Result<()>;
}

#[derive(Debug)]
pub struct Blockstream<T: EntryWriter> {
    pub output: T,
}

impl<T> BlockstreamEvents for Blockstream<T>
where
    T: EntryWriter,
{
    fn emit_entry_event(
        &self,
        slot: u64,
        tick_height: u64,
        leader_id: &Pubkey,
        entry: &Entry,
    ) -> Result<()> {
        let json_entry = serde_json::to_string(&entry)?;
        let payload = format!(
            r#"{{"dt":"{}","t":"entry","s":{},"h":{},"l":"{:?}","entry":{}}}"#,
            Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
            slot,
            tick_height,
            leader_id,
            json_entry,
        );
        self.output.write(payload)?;
        Ok(())
    }

    fn emit_block_event(
        &self,
        slot: u64,
        tick_height: u64,
        leader_id: &Pubkey,
        blockhash: Hash,
    ) -> Result<()> {
        let payload = format!(
            r#"{{"dt":"{}","t":"block","s":{},"h":{},"l":"{:?}","hash":"{:?}"}}"#,
            Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
            slot,
            tick_height,
            leader_id,
            blockhash,
        );
        self.output.write(payload)?;
        Ok(())
    }
}

pub type SocketBlockstream = Blockstream<EntrySocket>;

impl SocketBlockstream {
    pub fn new(socket: String) -> Self {
        Blockstream {
            output: EntrySocket { socket },
        }
    }
}

pub type MockBlockstream = Blockstream<EntryVec>;

impl MockBlockstream {
    pub fn new(_: String) -> Self {
        Blockstream {
            output: EntryVec::new(),
        }
    }

    pub fn entries(&self) -> Vec<String> {
        self.output.entries()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::entry::Entry;
    use chrono::{DateTime, FixedOffset};
    use serde_json::Value;
    use solana_sdk::hash::Hash;
    use solana_sdk::signature::{Keypair, KeypairUtil};
    use std::collections::HashSet;

    #[test]
    fn test_blockstream() -> () {
        let blockstream = MockBlockstream::new("test_stream".to_string());
        let ticks_per_slot = 5;

        let mut blockhash = Hash::default();
        let mut entries = Vec::new();
        let mut expected_entries = Vec::new();

        let tick_height_initial = 0;
        let tick_height_final = tick_height_initial + ticks_per_slot + 2;
        let mut curr_slot = 0;
        let leader_id = Keypair::new().pubkey();

        for tick_height in tick_height_initial..=tick_height_final {
            if tick_height == 5 {
                blockstream
                    .emit_block_event(curr_slot, tick_height - 1, &leader_id, blockhash)
                    .unwrap();
                curr_slot += 1;
            }
            let entry = Entry::new(&mut blockhash, 1, vec![]); // just ticks
            blockhash = entry.hash;
            blockstream
                .emit_entry_event(curr_slot, tick_height, &leader_id, &entry)
                .unwrap();
            expected_entries.push(entry.clone());
            entries.push(entry);
        }

        assert_eq!(
            blockstream.entries().len() as u64,
            // one entry per tick (0..=N+2) is +3, plus one block
            ticks_per_slot + 3 + 1
        );

        let mut j = 0;
        let mut matched_entries = 0;
        let mut matched_slots = HashSet::new();
        let mut matched_blocks = HashSet::new();

        for item in blockstream.entries() {
            let json: Value = serde_json::from_str(&item).unwrap();
            let dt_str = json["dt"].as_str().unwrap();

            // Ensure `ts` field parses as valid DateTime
            let _dt: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(dt_str).unwrap();

            let item_type = json["t"].as_str().unwrap();
            match item_type {
                "block" => {
                    let hash = json["hash"].to_string();
                    matched_blocks.insert(hash);
                }

                "entry" => {
                    let slot = json["s"].as_u64().unwrap();
                    matched_slots.insert(slot);
                    let entry_obj = json["entry"].clone();
                    let entry: Entry = serde_json::from_value(entry_obj).unwrap();

                    assert_eq!(entry, expected_entries[j]);
                    matched_entries += 1;
                    j += 1;
                }

                _ => {
                    assert!(false, "unknown item type {}", item);
                }
            }
        }

        assert_eq!(matched_entries, expected_entries.len());
        assert_eq!(matched_slots.len(), 2);
        assert_eq!(matched_blocks.len(), 1);
    }
}
