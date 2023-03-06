//! Helper types and functions for handling and dealing with snapshot hashes.
use {
    crate::{accounts_hash::AccountsHashEnum, epoch_accounts_hash::EpochAccountsHash},
    solana_sdk::{
        clock::Slot,
        hash::{Hash, Hasher},
    },
};

/// At startup, when loading from snapshots, the starting snapshot hashes need to be passed to
/// SnapshotPackagerService, which is in charge of pushing the hashes to CRDS.  This struct wraps
/// up those values make it easier to pass from bank_forks_utils, through validator, to
/// SnapshotPackagerService.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartingSnapshotHashes {
    pub full: FullSnapshotHash,
    pub incremental: Option<IncrementalSnapshotHash>,
}

/// Used by SnapshotPackagerService and SnapshotGossipManager, this struct adds type safety to
/// ensure a full snapshot hash is pushed to the right CRDS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullSnapshotHash {
    pub hash: (Slot, SnapshotHash),
}

/// Used by SnapshotPackagerService and SnapshotGossipManager, this struct adds type safety to
/// ensure an incremental snapshot hash is pushed to the right CRDS.  `base` is the (full) snapshot
/// this incremental snapshot (`hash`) is based on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncrementalSnapshotHash {
    pub base: (Slot, SnapshotHash),
    pub hash: (Slot, SnapshotHash),
}

/// FullSnapshotHashes is used by SnapshotPackagerService to collect the snapshot hashes from full
/// snapshots and then push those hashes to CRDS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullSnapshotHashes {
    pub hashes: Vec<(Slot, SnapshotHash)>,
}

/// IncrementalSnapshotHashes is used by SnapshotPackagerService to collect the snapshot hashes
/// from incremental snapshots and then push those hashes to CRDS.  `base` is the (full) snapshot
/// all the incremental snapshots (`hashes`) are based on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalSnapshotHashes {
    pub base: (Slot, SnapshotHash),
    pub hashes: Vec<(Slot, SnapshotHash)>,
}

/// The hash used for snapshot archives
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct SnapshotHash(pub Hash);

impl SnapshotHash {
    /// Make a snapshot hash from an accounts hash and epoch accounts hash
    #[must_use]
    pub fn new(
        accounts_hash: &AccountsHashEnum,
        epoch_accounts_hash: Option<&EpochAccountsHash>,
    ) -> Self {
        let snapshot_hash = match epoch_accounts_hash {
            None => *accounts_hash.as_hash(),
            Some(epoch_accounts_hash) => {
                let mut hasher = Hasher::default();
                hasher.hash(accounts_hash.as_hash().as_ref());
                hasher.hash(epoch_accounts_hash.as_ref().as_ref());
                hasher.result()
            }
        };

        Self(snapshot_hash)
    }
}
