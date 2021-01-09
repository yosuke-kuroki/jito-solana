use crate::bank_forks::ArchiveFormat;
use crate::snapshot_utils::SnapshotVersion;
use crate::{accounts_db::SnapshotStorages, bank::BankSlotDelta};
use solana_sdk::clock::Slot;
use solana_sdk::hash::Hash;
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, SendError, Sender},
};
use tempfile::TempDir;

pub type AccountsPackageSender = Sender<AccountsPackage>;
pub type AccountsPackageReceiver = Receiver<AccountsPackage>;
pub type AccountsPackageSendError = SendError<AccountsPackage>;

#[derive(Debug)]
pub struct AccountsPackage {
    pub slot: Slot,
    pub block_height: Slot,
    pub slot_deltas: Vec<BankSlotDelta>,
    pub snapshot_links: TempDir,
    pub storages: SnapshotStorages,
    pub tar_output_file: PathBuf,
    pub hash: Hash,
    pub archive_format: ArchiveFormat,
    pub snapshot_version: SnapshotVersion,
}

impl AccountsPackage {
    pub fn new(
        slot: Slot,
        block_height: u64,
        slot_deltas: Vec<BankSlotDelta>,
        snapshot_links: TempDir,
        storages: SnapshotStorages,
        tar_output_file: PathBuf,
        hash: Hash,
        archive_format: ArchiveFormat,
        snapshot_version: SnapshotVersion,
    ) -> Self {
        Self {
            slot,
            block_height,
            slot_deltas,
            snapshot_links,
            storages,
            tar_output_file,
            hash,
            archive_format,
            snapshot_version,
        }
    }
}
