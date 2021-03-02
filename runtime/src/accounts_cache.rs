use dashmap::DashMap;
use solana_sdk::{account::Account, clock::Slot, hash::Hash, pubkey::Pubkey};
use std::{
    collections::BTreeSet,
    ops::Deref,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, RwLock,
    },
};

pub type SlotCache = Arc<SlotCacheInner>;

#[derive(Default, Debug)]
pub struct SlotCacheInner {
    cache: DashMap<Pubkey, CachedAccount>,
    same_account_writes: AtomicU64,
    same_account_writes_size: AtomicU64,
    unique_account_writes_size: AtomicU64,
    is_frozen: AtomicBool,
}

impl SlotCacheInner {
    pub fn report_slot_store_metrics(&self) {
        datapoint_info!(
            "slot_repeated_writes",
            (
                "same_account_writes",
                self.same_account_writes.load(Ordering::Relaxed),
                i64
            ),
            (
                "same_account_writes_size",
                self.same_account_writes_size.load(Ordering::Relaxed),
                i64
            ),
            (
                "unique_account_writes_size",
                self.unique_account_writes_size.load(Ordering::Relaxed),
                i64
            )
        );
    }

    pub fn insert(&self, pubkey: &Pubkey, account: Account, hash: Hash) {
        if self.cache.contains_key(pubkey) {
            self.same_account_writes.fetch_add(1, Ordering::Relaxed);
            self.same_account_writes_size
                .fetch_add(account.data.len() as u64, Ordering::Relaxed);
        } else {
            self.unique_account_writes_size
                .fetch_add(account.data.len() as u64, Ordering::Relaxed);
        }
        self.cache.insert(*pubkey, CachedAccount { account, hash });
    }

    pub fn get_cloned(&self, pubkey: &Pubkey) -> Option<CachedAccount> {
        self.cache
            .get(pubkey)
            // 1) Maybe can eventually use a Cow to avoid a clone on every read
            // 2) Popping is only safe if it's guaranteed that only
            //    replay/banking threads are reading from the AccountsDb
            .map(|account_ref| account_ref.value().clone())
    }

    pub fn mark_slot_frozen(&self) {
        self.is_frozen.store(true, Ordering::SeqCst);
    }

    pub fn is_frozen(&self) -> bool {
        self.is_frozen.load(Ordering::SeqCst)
    }

    pub fn total_bytes(&self) -> u64 {
        self.unique_account_writes_size.load(Ordering::Relaxed)
            + self.same_account_writes_size.load(Ordering::Relaxed)
    }
}

impl Deref for SlotCacheInner {
    type Target = DashMap<Pubkey, CachedAccount>;
    fn deref(&self) -> &Self::Target {
        &self.cache
    }
}

#[derive(Debug, Clone)]
pub struct CachedAccount {
    pub account: Account,
    pub hash: Hash,
}

#[derive(Debug, Default)]
pub struct AccountsCache {
    cache: DashMap<Slot, SlotCache>,
    // Queue of potentially unflushed roots. Random eviction + cache too large
    // could have triggered a flush of this slot already
    maybe_unflushed_roots: RwLock<BTreeSet<Slot>>,
    max_flushed_root: AtomicU64,
}

impl AccountsCache {
    pub fn report_size(&self) {
        let total_unique_writes_size: u64 = self
            .cache
            .iter()
            .map(|item| {
                let slot_cache = item.value();
                slot_cache
                    .unique_account_writes_size
                    .load(Ordering::Relaxed)
            })
            .sum();
        datapoint_info!(
            "accounts_cache_size",
            (
                "num_roots",
                self.maybe_unflushed_roots.read().unwrap().len(),
                i64
            ),
            ("num_slots", self.cache.len(), i64),
            ("total_unique_writes_size", total_unique_writes_size, i64),
        );
    }

    pub fn store(&self, slot: Slot, pubkey: &Pubkey, account: Account, hash: Hash) {
        let slot_cache = self.slot_cache(slot).unwrap_or_else(||
            // DashMap entry.or_insert() returns a RefMut, essentially a write lock,
            // which is dropped after this block ends, minimizing time held by the lock.
            // However, we still want to persist the reference to the `SlotStores` behind
            // the lock, hence we clone it out, (`SlotStores` is an Arc so is cheap to clone).
            self
                .cache
                .entry(slot)
                .or_insert(Arc::new(SlotCacheInner::default()))
                .clone());

        slot_cache.insert(pubkey, account, hash);
    }

    pub fn load(&self, slot: Slot, pubkey: &Pubkey) -> Option<CachedAccount> {
        self.slot_cache(slot)
            .and_then(|slot_cache| slot_cache.get_cloned(pubkey))
    }

    pub fn remove_slot(&self, slot: Slot) -> Option<SlotCache> {
        self.cache.remove(&slot).map(|(_, slot_cache)| slot_cache)
    }

    pub fn slot_cache(&self, slot: Slot) -> Option<SlotCache> {
        self.cache.get(&slot).map(|result| result.value().clone())
    }

    pub fn add_root(&self, root: Slot) {
        let max_flushed_root = self.fetch_max_flush_root();
        if root > max_flushed_root || (root == max_flushed_root && root == 0) {
            self.maybe_unflushed_roots.write().unwrap().insert(root);
        }
    }

    pub fn clear_roots(&self, max_root: Option<Slot>) -> BTreeSet<Slot> {
        let mut w_maybe_unflushed_roots = self.maybe_unflushed_roots.write().unwrap();
        if let Some(max_root) = max_root {
            // `greater_than_max_root` contains all slots >= `max_root + 1`, or alternatively,
            // all slots > `max_root`. Meanwhile, `w_maybe_unflushed_roots` is left with all slots
            // <= `max_root`.
            let greater_than_max_root = w_maybe_unflushed_roots.split_off(&(max_root + 1));
            // After the replace, `w_maybe_unflushed_roots` contains slots > `max_root`, and
            // we return all slots <= `max_root`
            std::mem::replace(&mut w_maybe_unflushed_roots, greater_than_max_root)
        } else {
            std::mem::replace(&mut *w_maybe_unflushed_roots, BTreeSet::new())
        }
    }

    // Removes slots less than or equal to `max_root`. Only safe to pass in a rooted slot,
    // otherwise the slot removed could still be undergoing replay!
    pub fn remove_slots_le(&self, max_root: Slot) -> Vec<(Slot, SlotCache)> {
        let mut removed_slots = vec![];
        self.cache.retain(|slot, slot_cache| {
            let should_remove = *slot <= max_root;
            if should_remove {
                removed_slots.push((*slot, slot_cache.clone()))
            }
            !should_remove
        });
        removed_slots
    }

    pub fn find_older_frozen_slots(&self, num_to_retain: usize) -> Vec<Slot> {
        if self.cache.len() > num_to_retain {
            let mut slots: Vec<_> = self
                .cache
                .iter()
                .filter_map(|item| {
                    let (slot, slot_cache) = item.pair();
                    if slot_cache.is_frozen() {
                        Some(*slot)
                    } else {
                        None
                    }
                })
                .collect();
            slots.sort_unstable();
            slots.truncate(slots.len().saturating_sub(num_to_retain));
            slots
        } else {
            vec![]
        }
    }

    pub fn num_slots(&self) -> usize {
        self.cache.len()
    }

    pub fn fetch_max_flush_root(&self) -> Slot {
        self.max_flushed_root.load(Ordering::Relaxed)
    }

    pub fn set_max_flush_root(&self, root: Slot) {
        self.max_flushed_root.fetch_max(root, Ordering::Relaxed);
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_remove_slots_le() {
        let cache = AccountsCache::default();
        // Cache is empty, should return nothing
        assert!(cache.remove_slots_le(1).is_empty());
        let inserted_slot = 0;
        cache.store(
            inserted_slot,
            &Pubkey::new_unique(),
            Account::new(1, 0, &Pubkey::default()),
            Hash::default(),
        );
        // If the cache is told the size limit is 0, it should return the one slot
        let removed = cache.remove_slots_le(0);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].0, inserted_slot);
    }

    #[test]
    fn test_find_older_frozen_slots() {
        let cache = AccountsCache::default();
        // Cache is empty, should return nothing
        assert!(cache.find_older_frozen_slots(0).is_empty());
        let inserted_slot = 0;
        cache.store(
            inserted_slot,
            &Pubkey::new_unique(),
            Account::new(1, 0, &Pubkey::default()),
            Hash::default(),
        );

        // If the cache is told the size limit is 0, it should return nothing because there's only
        // one cached slot
        assert!(cache.find_older_frozen_slots(1).is_empty());
        // If the cache is told the size limit is 0, it should return nothing, because there's no
        // frozen slots
        assert!(cache.find_older_frozen_slots(0).is_empty());
        cache.slot_cache(inserted_slot).unwrap().mark_slot_frozen();
        // If the cache is told the size limit is 0, it should return the one frozen slot
        assert_eq!(cache.find_older_frozen_slots(0), vec![inserted_slot]);
    }
}
