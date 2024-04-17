//! trait for abstracting underlying storage of pubkey and account pairs to be written
use {
    crate::{
        account_storage::meta::StoredAccountMeta, accounts_hash::AccountHash,
        accounts_index::ZeroLamport,
    },
    solana_sdk::{
        account::{AccountSharedData, ReadableAccount},
        clock::{Epoch, Slot},
        pubkey::Pubkey,
    },
};

/// hold a ref to an account to store. The account could be represented in memory a few different ways
#[derive(Debug, Copy, Clone)]
pub enum AccountForStorage<'a> {
    AddressAndAccount((&'a Pubkey, &'a AccountSharedData)),
    StoredAccountMeta(&'a StoredAccountMeta<'a>),
}

impl<'a> From<(&'a Pubkey, &'a AccountSharedData)> for AccountForStorage<'a> {
    fn from(source: (&'a Pubkey, &'a AccountSharedData)) -> Self {
        Self::AddressAndAccount(source)
    }
}

impl<'a> From<&'a StoredAccountMeta<'a>> for AccountForStorage<'a> {
    fn from(source: &'a StoredAccountMeta<'a>) -> Self {
        Self::StoredAccountMeta(source)
    }
}

impl<'a> ZeroLamport for AccountForStorage<'a> {
    fn is_zero_lamport(&self) -> bool {
        self.lamports() == 0
    }
}

impl<'a> AccountForStorage<'a> {
    pub fn pubkey(&self) -> &'a Pubkey {
        match self {
            AccountForStorage::AddressAndAccount((pubkey, _account)) => pubkey,
            AccountForStorage::StoredAccountMeta(account) => account.pubkey(),
        }
    }
}

impl<'a> ReadableAccount for AccountForStorage<'a> {
    fn lamports(&self) -> u64 {
        match self {
            AccountForStorage::AddressAndAccount((_pubkey, account)) => account.lamports(),
            AccountForStorage::StoredAccountMeta(account) => account.lamports(),
        }
    }
    fn data(&self) -> &[u8] {
        match self {
            AccountForStorage::AddressAndAccount((_pubkey, account)) => account.data(),
            AccountForStorage::StoredAccountMeta(account) => account.data(),
        }
    }
    fn owner(&self) -> &Pubkey {
        match self {
            AccountForStorage::AddressAndAccount((_pubkey, account)) => account.owner(),
            AccountForStorage::StoredAccountMeta(account) => account.owner(),
        }
    }
    fn executable(&self) -> bool {
        match self {
            AccountForStorage::AddressAndAccount((_pubkey, account)) => account.executable(),
            AccountForStorage::StoredAccountMeta(account) => account.executable(),
        }
    }
    fn rent_epoch(&self) -> Epoch {
        match self {
            AccountForStorage::AddressAndAccount((_pubkey, account)) => account.rent_epoch(),
            AccountForStorage::StoredAccountMeta(account) => account.rent_epoch(),
        }
    }
    fn to_account_shared_data(&self) -> AccountSharedData {
        match self {
            AccountForStorage::AddressAndAccount((_pubkey, account)) => {
                account.to_account_shared_data()
            }
            AccountForStorage::StoredAccountMeta(account) => account.to_account_shared_data(),
        }
    }
}

lazy_static! {
    static ref DEFAULT_ACCOUNT_SHARED_DATA: AccountSharedData = AccountSharedData::default();
}

/// abstract access to pubkey, account, slot, target_slot of either:
/// a. (slot, &[&Pubkey, &ReadableAccount])
/// b. (slot, &[&Pubkey, &ReadableAccount, Slot]) (we will use this later)
/// This trait avoids having to allocate redundant data when there is a duplicated slot parameter.
/// All legacy callers do not have a unique slot per account to store.
pub trait StorableAccounts<'a>: Sync {
    /// account at 'index'
    fn account<Ret>(
        &self,
        index: usize,
        callback: impl for<'local> FnMut(AccountForStorage<'local>) -> Ret,
    ) -> Ret;
    /// None if account is zero lamports
    fn account_default_if_zero_lamport<Ret>(
        &self,
        index: usize,
        mut callback: impl for<'local> FnMut(AccountForStorage<'local>) -> Ret,
    ) -> Ret {
        self.account(index, |account| {
            callback(if account.lamports() != 0 {
                account
            } else {
                // preserve the pubkey, but use a default value for the account
                AccountForStorage::AddressAndAccount((
                    account.pubkey(),
                    &DEFAULT_ACCOUNT_SHARED_DATA,
                ))
            })
        })
    }
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
    fn contains_multiple_slots(&self) -> bool {
        false
    }

    /// true iff the impl can provide hash
    /// Otherwise, hash has to be provided separately to store functions.
    fn has_hash(&self) -> bool {
        false
    }

    /// return hash for account at 'index'
    /// Should only be called if 'has_hash' = true
    fn hash(&self, _index: usize) -> &AccountHash {
        // this should never be called if has_hash returns false
        unimplemented!();
    }
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
}

impl<'a, T: ReadableAccount + Sync> StorableAccounts<'a> for StorableAccountsMovingSlots<'a, T>
where
    AccountForStorage<'a>: From<(&'a Pubkey, &'a T)>,
{
    fn account<Ret>(
        &self,
        index: usize,
        mut callback: impl for<'local> FnMut(AccountForStorage<'local>) -> Ret,
    ) -> Ret {
        callback((self.accounts[index].0, self.accounts[index].1).into())
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
}

impl<'a: 'b, 'b, T: ReadableAccount + Sync + 'a> StorableAccounts<'a>
    for (Slot, &'b [(&'a Pubkey, &'a T)])
where
    AccountForStorage<'a>: From<(&'a Pubkey, &'a T)>,
{
    fn account<Ret>(
        &self,
        index: usize,
        mut callback: impl for<'local> FnMut(AccountForStorage<'local>) -> Ret,
    ) -> Ret {
        callback((self.1[index].0, self.1[index].1).into())
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
}
impl<'a, T: ReadableAccount + Sync> StorableAccounts<'a> for (Slot, &'a [&'a (Pubkey, T)])
where
    AccountForStorage<'a>: From<(&'a Pubkey, &'a T)>,
{
    fn account<Ret>(
        &self,
        index: usize,
        mut callback: impl for<'local> FnMut(AccountForStorage<'local>) -> Ret,
    ) -> Ret {
        callback((&self.1[index].0, &self.1[index].1).into())
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
}

impl<'a> StorableAccounts<'a> for (Slot, &'a [&'a StoredAccountMeta<'a>]) {
    fn account<Ret>(
        &self,
        index: usize,
        mut callback: impl for<'local> FnMut(AccountForStorage<'local>) -> Ret,
    ) -> Ret {
        callback(self.1[index].into())
    }
    fn slot(&self, _index: usize) -> Slot {
        // per-index slot is not unique per slot when per-account slot is not included in the source data
        self.0
    }
    fn target_slot(&self) -> Slot {
        self.0
    }
    fn len(&self) -> usize {
        self.1.len()
    }
    fn has_hash(&self) -> bool {
        true
    }
    fn hash(&self, index: usize) -> &AccountHash {
        self.1[index].hash()
    }
}

/// holds slices of accounts being moved FROM a common source slot to 'target_slot'
pub struct StorableAccountsBySlot<'a> {
    target_slot: Slot,
    /// each element is (source slot, accounts moving FROM source slot)
    slots_and_accounts: &'a [(Slot, &'a [&'a StoredAccountMeta<'a>])],

    /// This is calculated based off slots_and_accounts.
    /// cumulative offset of all account slices prior to this one
    /// starting_offsets[0] is the starting offset of slots_and_accounts[1]
    /// The starting offset of slots_and_accounts[0] is always 0
    starting_offsets: Vec<usize>,
    /// true if there is more than 1 slot represented in slots_and_accounts
    contains_multiple_slots: bool,
    /// total len of all accounts, across all slots_and_accounts
    len: usize,
}

impl<'a> StorableAccountsBySlot<'a> {
    /// each element of slots_and_accounts is (source slot, accounts moving FROM source slot)
    pub fn new(
        target_slot: Slot,
        slots_and_accounts: &'a [(Slot, &'a [&'a StoredAccountMeta<'a>])],
    ) -> Self {
        let mut cumulative_len = 0usize;
        let mut starting_offsets = Vec::with_capacity(slots_and_accounts.len());
        let first_slot = slots_and_accounts
            .first()
            .map(|(slot, _)| *slot)
            .unwrap_or_default();
        let mut contains_multiple_slots = false;
        for (slot, accounts) in slots_and_accounts {
            cumulative_len = cumulative_len.saturating_add(accounts.len());
            starting_offsets.push(cumulative_len);
            contains_multiple_slots |= &first_slot != slot;
        }
        Self {
            target_slot,
            slots_and_accounts,
            starting_offsets,
            contains_multiple_slots,
            len: cumulative_len,
        }
    }
    /// given an overall index for all accounts in self:
    /// return (slots_and_accounts index, index within those accounts)
    fn find_internal_index(&self, index: usize) -> (usize, usize) {
        // search offsets for the accounts slice that contains 'index'.
        // This could be a binary search.
        for (offset_index, next_offset) in self.starting_offsets.iter().enumerate() {
            if next_offset > &index {
                // offset of prior entry
                let prior_offset = if offset_index > 0 {
                    self.starting_offsets[offset_index.saturating_sub(1)]
                } else {
                    0
                };
                return (offset_index, index - prior_offset);
            }
        }
        panic!("failed");
    }
}

impl<'a> StorableAccounts<'a> for StorableAccountsBySlot<'a> {
    fn account<Ret>(
        &self,
        index: usize,
        mut callback: impl for<'local> FnMut(AccountForStorage<'local>) -> Ret,
    ) -> Ret {
        let indexes = self.find_internal_index(index);
        callback(self.slots_and_accounts[indexes.0].1[indexes.1].into())
    }
    fn slot(&self, index: usize) -> Slot {
        let indexes = self.find_internal_index(index);
        self.slots_and_accounts[indexes.0].0
    }
    fn target_slot(&self) -> Slot {
        self.target_slot
    }
    fn len(&self) -> usize {
        self.len
    }
    fn contains_multiple_slots(&self) -> bool {
        self.contains_multiple_slots
    }
    fn has_hash(&self) -> bool {
        true
    }
    fn hash(&self, index: usize) -> &AccountHash {
        let indexes = self.find_internal_index(index);
        self.slots_and_accounts[indexes.0].1[indexes.1].hash()
    }
}

/// this tuple contains a single different source slot that applies to all accounts
/// accounts are StoredAccountMeta
impl<'a> StorableAccounts<'a> for (Slot, &'a [&'a StoredAccountMeta<'a>], Slot) {
    fn account<Ret>(
        &self,
        index: usize,
        mut callback: impl for<'local> FnMut(AccountForStorage<'local>) -> Ret,
    ) -> Ret {
        callback(self.1[index].into())
    }
    fn slot(&self, _index: usize) -> Slot {
        // same other slot for all accounts
        self.2
    }
    fn target_slot(&self) -> Slot {
        self.0
    }
    fn len(&self) -> usize {
        self.1.len()
    }
    fn has_hash(&self) -> bool {
        true
    }
    fn hash(&self, index: usize) -> &AccountHash {
        self.1[index].hash()
    }
}

#[cfg(test)]
pub mod tests {
    use {
        super::*,
        crate::{
            account_storage::meta::{AccountMeta, StoredAccountMeta, StoredMeta},
            append_vec::AppendVecStoredAccountMeta,
        },
        solana_sdk::{
            account::{accounts_equal, AccountSharedData, WritableAccount},
            hash::Hash,
        },
    };

    fn compare<'a>(a: &impl StorableAccounts<'a>, b: &impl StorableAccounts<'a>) {
        assert_eq!(a.target_slot(), b.target_slot());
        assert_eq!(a.len(), b.len());
        assert_eq!(a.is_empty(), b.is_empty());
        (0..a.len()).for_each(|i| {
            b.account(i, |account| {
                a.account(i, |account_a| {
                    assert_eq!(account_a.pubkey(), account.pubkey());
                    assert!(accounts_equal(&account_a, &account));
                });
            });
        })
    }

    #[test]
    fn test_contains_multiple_slots() {
        let pk = Pubkey::from([1; 32]);
        let slot = 0;
        let lamports = 1;
        let owner = Pubkey::default();
        let executable = false;
        let rent_epoch = 0;
        let meta = StoredMeta {
            write_version_obsolete: 5,
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
        let hash = AccountHash(Hash::new_unique());
        let stored_account = StoredAccountMeta::AppendVec(AppendVecStoredAccountMeta {
            meta: &meta,
            account_meta: &account_meta,
            data: &data,
            offset,
            stored_size,
            hash: &hash,
        });

        let test3 = (slot, &vec![&stored_account, &stored_account][..], slot);
        assert!(!test3.contains_multiple_slots());
    }

    #[test]
    fn test_storable_accounts() {
        let max_slots = 3_u64;
        for target_slot in 0..max_slots {
            for entries in 0..2 {
                for starting_slot in 0..max_slots {
                    let data = Vec::default();
                    let hash = AccountHash(Hash::new_unique());
                    let mut raw = Vec::new();
                    let mut raw2 = Vec::new();
                    let mut raw4 = Vec::new();
                    for entry in 0..entries {
                        let pk = Pubkey::from([entry; 32]);
                        let account = AccountSharedData::create(
                            (entry as u64) * starting_slot,
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
                                write_version_obsolete: 0, // just something
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
                        let raw = &raw[entry as usize];
                        raw2.push(StoredAccountMeta::AppendVec(AppendVecStoredAccountMeta {
                            meta: &raw.3,
                            account_meta: &raw.4,
                            data: &data,
                            offset,
                            stored_size,
                            hash: &hash,
                        }));
                        raw4.push((raw.0, raw.1.clone()));
                    }

                    let mut two = Vec::new();
                    let mut three = Vec::new();
                    let mut four_pubkey_and_account_value = Vec::new();
                    raw.iter()
                        .zip(raw2.iter().zip(raw4.iter()))
                        .for_each(|(raw, (raw2, raw4))| {
                            two.push((&raw.0, &raw.1)); // 2 item tuple
                            three.push(raw2);
                            four_pubkey_and_account_value.push(raw4);
                        });
                    let test2 = (target_slot, &two[..]);
                    let test4 = (target_slot, &four_pubkey_and_account_value[..]);

                    let source_slot = starting_slot % max_slots;
                    let test3 = (target_slot, &three[..], source_slot);
                    let old_slot = starting_slot;
                    let test_moving_slots = StorableAccountsMovingSlots {
                        accounts: &two[..],
                        target_slot,
                        old_slot,
                    };
                    let for_slice = [(old_slot, &three[..])];
                    let test_moving_slots2 = StorableAccountsBySlot::new(target_slot, &for_slice);
                    compare(&test2, &test3);
                    compare(&test2, &test4);
                    compare(&test2, &test_moving_slots);
                    compare(&test2, &test_moving_slots2);
                    for (i, raw) in raw.iter().enumerate() {
                        test3.account(i, |account| {
                            assert_eq!(raw.0, *account.pubkey());
                            assert!(accounts_equal(&raw.1, &account));
                        });
                        assert_eq!(raw.2, test3.slot(i));
                        assert_eq!(target_slot, test4.slot(i));
                        assert_eq!(target_slot, test2.slot(i));
                        assert_eq!(old_slot, test_moving_slots.slot(i));
                        assert_eq!(old_slot, test_moving_slots2.slot(i));
                    }
                    assert_eq!(target_slot, test3.target_slot());
                    assert_eq!(target_slot, test4.target_slot());
                    assert_eq!(target_slot, test_moving_slots2.target_slot());
                    assert!(!test2.contains_multiple_slots());
                    assert!(!test4.contains_multiple_slots());
                    assert!(!test_moving_slots.contains_multiple_slots());
                    assert_eq!(test3.contains_multiple_slots(), entries > 1);
                }
            }
        }
    }

    #[test]
    fn test_storable_accounts_by_slot() {
        solana_logger::setup();
        // slots 0..4
        // each one containing a subset of the overall # of entries (0..4)
        for entries in 0..6 {
            let data = Vec::default();
            let hashes = (0..entries)
                .map(|_| AccountHash(Hash::new_unique()))
                .collect::<Vec<_>>();
            let mut raw = Vec::new();
            let mut raw2 = Vec::new();
            for entry in 0..entries {
                let pk = Pubkey::from([entry; 32]);
                let account = AccountSharedData::create(
                    entry as u64,
                    Vec::default(),
                    Pubkey::default(),
                    false,
                    0,
                );
                raw.push((
                    pk,
                    account.clone(),
                    StoredMeta {
                        write_version_obsolete: 500 + (entry * 3) as u64, // just something
                        pubkey: pk,
                        data_len: (entry * 2) as u64, // just something
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
                raw2.push(StoredAccountMeta::AppendVec(AppendVecStoredAccountMeta {
                    meta: &raw[entry as usize].2,
                    account_meta: &raw[entry as usize].3,
                    data: &data,
                    offset,
                    stored_size,
                    hash: &hashes[entry as usize],
                }));
            }
            let raw2_refs = raw2.iter().collect::<Vec<_>>();

            // enumerate through permutations of # entries (ie. accounts) in each slot. Each one is 0..=entries.
            for entries0 in 0..=entries {
                let remaining1 = entries.saturating_sub(entries0);
                for entries1 in 0..=remaining1 {
                    let remaining2 = entries.saturating_sub(entries0 + entries1);
                    for entries2 in 0..=remaining2 {
                        let remaining3 = entries.saturating_sub(entries0 + entries1 + entries2);
                        let entries_by_level = [entries0, entries1, entries2, remaining3];
                        let mut overall_index = 0;
                        let mut expected_slots = Vec::default();
                        let slots_and_accounts = entries_by_level
                            .iter()
                            .enumerate()
                            .filter_map(|(slot, count)| {
                                let slot = slot as Slot;
                                let count = *count as usize;
                                (overall_index < raw2.len()).then(|| {
                                    let range = overall_index..(overall_index + count);
                                    let result = &raw2_refs[range.clone()];
                                    range.for_each(|_| expected_slots.push(slot));
                                    overall_index += count;
                                    (slot, result)
                                })
                            })
                            .collect::<Vec<_>>();
                        let storable = StorableAccountsBySlot::new(99, &slots_and_accounts[..]);
                        assert!(storable.has_hash());
                        assert_eq!(99, storable.target_slot());
                        assert_eq!(entries0 != entries, storable.contains_multiple_slots());
                        (0..entries).for_each(|index| {
                            let index = index as usize;
                            storable.account(index, |account| {
                                assert!(accounts_equal(&account, &raw2[index]));
                                assert_eq!(account.pubkey(), raw2[index].pubkey());
                            });
                            assert_eq!(storable.hash(index), raw2[index].hash());
                            assert_eq!(storable.slot(index), expected_slots[index]);
                        })
                    }
                }
            }
        }
    }
}
