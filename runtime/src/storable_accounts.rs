//! trait for abstracting underlying storage of pubkey and account pairs to be written
use {
    crate::{accounts_db::IncludeSlotInHash, append_vec::StoredAccountMeta},
    solana_sdk::{account::ReadableAccount, clock::Slot, pubkey::Pubkey},
};

/// abstract access to pubkey, account, slot, target_slot of either:
/// a. (slot, &[&Pubkey, &ReadableAccount])
/// b. (slot, &[&Pubkey, &ReadableAccount, Slot]) (we will use this later)
/// This trait avoids having to allocate redundant data when there is a duplicated slot parameter.
/// All legacy callers do not have a unique slot per account to store.
pub trait StorableAccounts<'a, T: ReadableAccount + Sync>: Sync {
    /// pubkey at 'index'
    fn pubkey(&self, index: usize) -> &Pubkey;
    /// account at 'index'
    fn account(&self, index: usize) -> &T;
    // current slot for account at 'index'
    fn slot(&self, index: usize) -> Slot;
    /// slot that all accounts are to be written to
    fn target_slot(&self) -> Slot;
    /// true if no accounts to write
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// # accounts to write
    fn len(&self) -> usize;
    /// are there accounts from multiple slots
    /// only used for an assert
    fn contains_multiple_slots(&self) -> bool;
    /// true iff hashing these accounts should include the slot
    fn include_slot_in_hash(&self) -> IncludeSlotInHash;
}

/// accounts that are moving from 'old_slot' to 'target_slot'
/// since all accounts are from the same old slot, we don't need to create a slice with per-account slot
/// but, we need slot(_) to return 'old_slot' for all accounts
/// Created a struct instead of a tuple to make the code easier to read.
pub struct StorableAccountsMovingSlots<'a, T: ReadableAccount + Sync> {
    pub accounts: &'a [(&'a Pubkey, &'a T)],
    /// accounts will be written to this slot
    pub target_slot: Slot,
    /// slot where accounts are currently stored
    pub old_slot: Slot,
    /// This is temporarily here until feature activation.
    pub include_slot_in_hash: IncludeSlotInHash,
}

impl<'a, T: ReadableAccount + Sync> StorableAccounts<'a, T> for StorableAccountsMovingSlots<'a, T> {
    fn pubkey(&self, index: usize) -> &Pubkey {
        self.accounts[index].0
    }
    fn account(&self, index: usize) -> &T {
        self.accounts[index].1
    }
    fn slot(&self, _index: usize) -> Slot {
        // per-index slot is not unique per slot, but it is different than 'target_slot'
        self.old_slot
    }
    fn target_slot(&self) -> Slot {
        self.target_slot
    }
    fn len(&self) -> usize {
        self.accounts.len()
    }
    fn contains_multiple_slots(&self) -> bool {
        false
    }
    fn include_slot_in_hash(&self) -> IncludeSlotInHash {
        self.include_slot_in_hash
    }
}

/// The last parameter exists until this feature is activated:
///  ignore slot when calculating an account hash #28420
impl<'a, T: ReadableAccount + Sync> StorableAccounts<'a, T>
    for (Slot, &'a [(&'a Pubkey, &'a T)], IncludeSlotInHash)
{
    fn pubkey(&self, index: usize) -> &Pubkey {
        self.1[index].0
    }
    fn account(&self, index: usize) -> &T {
        self.1[index].1
    }
    fn slot(&self, _index: usize) -> Slot {
        // per-index slot is not unique per slot when per-account slot is not included in the source data
        self.target_slot()
    }
    fn target_slot(&self) -> Slot {
        self.0
    }
    fn len(&self) -> usize {
        self.1.len()
    }
    fn contains_multiple_slots(&self) -> bool {
        false
    }
    fn include_slot_in_hash(&self) -> IncludeSlotInHash {
        self.2
    }
}

/// this tuple contains slot info PER account
impl<'a> StorableAccounts<'a, StoredAccountMeta<'a>>
    for (
        Slot,
        &'a [(&'a StoredAccountMeta<'a>, Slot)],
        IncludeSlotInHash,
    )
{
    fn pubkey(&self, index: usize) -> &Pubkey {
        self.1[index].0.pubkey()
    }
    fn account(&self, index: usize) -> &StoredAccountMeta<'a> {
        self.1[index].0
    }
    fn slot(&self, index: usize) -> Slot {
        // note that this could be different than 'target_slot()' PER account
        self.1[index].1
    }
    fn target_slot(&self) -> Slot {
        self.0
    }
    fn len(&self) -> usize {
        self.1.len()
    }
    fn contains_multiple_slots(&self) -> bool {
        let len = self.len();
        if len > 0 {
            let slot = self.slot(0);
            // true if any item has a different slot than the first item
            (1..len).any(|i| slot != self.slot(i))
        } else {
            false
        }
    }
    fn include_slot_in_hash(&self) -> IncludeSlotInHash {
        self.2
    }
}
#[cfg(test)]
pub mod tests {
    use {
        super::*,
        crate::{
            accounts_db::INCLUDE_SLOT_IN_HASH_TESTS,
            append_vec::{AccountMeta, StoredAccountMeta, StoredMeta},
        },
        solana_sdk::{
            account::{accounts_equal, AccountSharedData, WritableAccount},
            hash::Hash,
        },
    };

    fn compare<
        'a,
        T: ReadableAccount + Sync + PartialEq + std::fmt::Debug,
        U: ReadableAccount + Sync + PartialEq + std::fmt::Debug,
    >(
        a: &impl StorableAccounts<'a, T>,
        b: &impl StorableAccounts<'a, U>,
    ) {
        assert_eq!(a.target_slot(), b.target_slot());
        assert_eq!(a.len(), b.len());
        assert_eq!(a.is_empty(), b.is_empty());
        (0..a.len()).into_iter().for_each(|i| {
            assert_eq!(a.pubkey(i), b.pubkey(i));
            assert!(accounts_equal(a.account(i), b.account(i)));
        })
    }

    #[test]
    fn test_contains_multiple_slots() {
        let pk = Pubkey::new(&[1; 32]);
        let slot = 0;
        let lamports = 1;
        let owner = Pubkey::default();
        let executable = false;
        let rent_epoch = 0;
        let meta = StoredMeta {
            write_version: 5,
            pubkey: pk,
            data_len: 7,
        };
        let account_meta = AccountMeta {
            lamports,
            owner,
            executable,
            rent_epoch,
        };
        let data = Vec::default();
        let offset = 99;
        let stored_size = 101;
        let hash = Hash::new_unique();
        let stored_account = StoredAccountMeta {
            meta: &meta,
            account_meta: &account_meta,
            data: &data,
            offset,
            stored_size,
            hash: &hash,
        };

        let test3 = (
            slot,
            &vec![(&stored_account, slot), (&stored_account, slot)][..],
            INCLUDE_SLOT_IN_HASH_TESTS,
        );
        assert!(!test3.contains_multiple_slots());
        let test3 = (
            slot,
            &vec![(&stored_account, slot), (&stored_account, slot + 1)][..],
            INCLUDE_SLOT_IN_HASH_TESTS,
        );
        assert!(test3.contains_multiple_slots());
    }

    #[test]
    fn test_storable_accounts() {
        let max_slots = 3_u64;
        for target_slot in 0..max_slots {
            for entries in 0..2 {
                for starting_slot in 0..max_slots {
                    let data = Vec::default();
                    let hash = Hash::new_unique();
                    let mut raw = Vec::new();
                    let mut raw2 = Vec::new();
                    for entry in 0..entries {
                        let pk = Pubkey::new(&[entry; 32]);
                        let account = AccountSharedData::create(
                            ((entry as u64) * starting_slot) as u64,
                            Vec::default(),
                            Pubkey::default(),
                            false,
                            0,
                        );

                        raw.push((
                            pk,
                            account.clone(),
                            starting_slot % max_slots,
                            StoredMeta {
                                write_version: 0, // just something
                                pubkey: pk,
                                data_len: u64::MAX, // just something
                            },
                            AccountMeta {
                                lamports: account.lamports(),
                                owner: *account.owner(),
                                executable: account.executable(),
                                rent_epoch: account.rent_epoch(),
                            },
                        ));
                    }
                    for entry in 0..entries {
                        let offset = 99;
                        let stored_size = 101;
                        raw2.push(StoredAccountMeta {
                            meta: &raw[entry as usize].3,
                            account_meta: &raw[entry as usize].4,
                            data: &data,
                            offset,
                            stored_size,
                            hash: &hash,
                        });
                    }

                    let mut two = Vec::new();
                    let mut three = Vec::new();
                    raw.iter().zip(raw2.iter()).for_each(|(raw, raw2)| {
                        two.push((&raw.0, &raw.1)); // 2 item tuple
                        three.push((raw2, raw.2)); // 2 item tuple, including slot
                    });
                    let test2 = (target_slot, &two[..], INCLUDE_SLOT_IN_HASH_TESTS);

                    let test3 = (target_slot, &three[..], INCLUDE_SLOT_IN_HASH_TESTS);
                    let old_slot = starting_slot;
                    let test_moving_slots = StorableAccountsMovingSlots {
                        accounts: &two[..],
                        target_slot,
                        old_slot,
                        include_slot_in_hash: INCLUDE_SLOT_IN_HASH_TESTS,
                    };
                    compare(&test2, &test3);
                    compare(&test2, &test_moving_slots);
                    for (i, raw) in raw.iter().enumerate() {
                        assert_eq!(raw.0, *test3.pubkey(i));
                        assert!(accounts_equal(&raw.1, test3.account(i)));
                        assert_eq!(raw.2, test3.slot(i));
                        assert_eq!(target_slot, test2.slot(i));
                        assert_eq!(old_slot, test_moving_slots.slot(i));
                    }
                    assert_eq!(target_slot, test3.target_slot());
                    assert!(!test2.contains_multiple_slots());
                    assert!(!test_moving_slots.contains_multiple_slots());
                    assert_eq!(test3.contains_multiple_slots(), entries > 1);
                }
            }
        }
    }
}
