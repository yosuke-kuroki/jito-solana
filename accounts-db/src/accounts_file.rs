use {
    crate::{
        account_info::AccountInfo,
        account_storage::meta::{StorableAccountsWithHashes, StoredAccountInfo, StoredAccountMeta},
        accounts_db::AccountsFileId,
        accounts_hash::AccountHash,
        append_vec::{AppendVec, AppendVecError, IndexInfo},
        storable_accounts::StorableAccounts,
        tiered_storage::{
            error::TieredStorageError, hot::HOT_FORMAT, index::IndexOffset, TieredStorage,
        },
    },
    solana_sdk::{
        account::{AccountSharedData, ReadableAccount},
        clock::Slot,
        pubkey::Pubkey,
    },
    std::{borrow::Borrow, io::Read, mem, path::PathBuf},
    thiserror::Error,
};

// Data placement should be aligned at the next boundary. Without alignment accessing the memory may
// crash on some architectures.
pub const ALIGN_BOUNDARY_OFFSET: usize = mem::size_of::<u64>();
#[macro_export]
macro_rules! u64_align {
    ($addr: expr) => {
        ($addr + (ALIGN_BOUNDARY_OFFSET - 1)) & !(ALIGN_BOUNDARY_OFFSET - 1)
    };
}

#[derive(Error, Debug)]
/// An enum for AccountsFile related errors.
pub enum AccountsFileError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("AppendVecError: {0}")]
    AppendVecError(#[from] AppendVecError),

    #[error("TieredStorageError: {0}")]
    TieredStorageError(#[from] TieredStorageError),
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum MatchAccountOwnerError {
    #[error("The account owner does not match with the provided list")]
    NoMatch,
    #[error("Unable to load the account")]
    UnableToLoad,
}

pub type Result<T> = std::result::Result<T, AccountsFileError>;

#[derive(Debug)]
/// An enum for accessing an accounts file which can be implemented
/// under different formats.
pub enum AccountsFile {
    AppendVec(AppendVec),
    TieredStorage(TieredStorage),
}

impl AccountsFile {
    /// Create an AccountsFile instance from the specified path.
    ///
    /// The second element of the returned tuple is the number of accounts in the
    /// accounts file.
    pub fn new_from_file(path: impl Into<PathBuf>, current_len: usize) -> Result<(Self, usize)> {
        let (av, num_accounts) = AppendVec::new_from_file(path, current_len)?;
        Ok((Self::AppendVec(av), num_accounts))
    }

    pub fn flush(&self) -> Result<()> {
        match self {
            Self::AppendVec(av) => av.flush(),
            Self::TieredStorage(_) => Ok(()),
        }
    }

    pub fn reset(&self) {
        match self {
            Self::AppendVec(av) => av.reset(),
            Self::TieredStorage(_) => {}
        }
    }

    pub fn remaining_bytes(&self) -> u64 {
        match self {
            Self::AppendVec(av) => av.remaining_bytes(),
            Self::TieredStorage(ts) => ts.capacity().saturating_sub(ts.len() as u64),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::AppendVec(av) => av.len(),
            Self::TieredStorage(ts) => ts.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::AppendVec(av) => av.is_empty(),
            Self::TieredStorage(ts) => ts.is_empty(),
        }
    }

    pub fn capacity(&self) -> u64 {
        match self {
            Self::AppendVec(av) => av.capacity(),
            Self::TieredStorage(ts) => ts.capacity(),
        }
    }

    pub fn file_name(slot: Slot, id: AccountsFileId) -> String {
        format!("{slot}.{id}")
    }

    /// Return (account metadata, next_index) pair for the account at the
    /// specified `offset` if any.  Otherwise return None.   Also return the
    /// index of the next entry.
    pub fn get_account(&self, offset: usize) -> Option<(StoredAccountMeta<'_>, usize)> {
        match self {
            Self::AppendVec(av) => av.get_account(offset),
            // Note: The conversion here is needed as the AccountsDB currently
            // assumes all offsets are multiple of 8 while TieredStorage uses
            // IndexOffset that is equivalent to AccountInfo::reduced_offset.
            Self::TieredStorage(ts) => ts
                .reader()?
                .get_account(IndexOffset(AccountInfo::get_reduced_offset(offset)))
                .ok()?
                .map(|(metas, index_offset)| {
                    (metas, AccountInfo::reduced_offset_to_offset(index_offset.0))
                }),
        }
    }

    /// return an `AccountSharedData` for an account at `offset`, if any.  Otherwise return None.
    pub(crate) fn get_stored_account(&self, offset: usize) -> Option<AccountSharedData> {
        match self {
            Self::AppendVec(av) => av.get_stored_account(offset),
            Self::TieredStorage(_) => unimplemented!(),
        }
    }

    pub fn account_matches_owners(
        &self,
        offset: usize,
        owners: &[Pubkey],
    ) -> std::result::Result<usize, MatchAccountOwnerError> {
        match self {
            Self::AppendVec(av) => av.account_matches_owners(offset, owners),
            // Note: The conversion here is needed as the AccountsDB currently
            // assumes all offsets are multiple of 8 while TieredStorage uses
            // IndexOffset that is equivalent to AccountInfo::reduced_offset.
            Self::TieredStorage(ts) => {
                let Some(reader) = ts.reader() else {
                    return Err(MatchAccountOwnerError::UnableToLoad);
                };
                reader.account_matches_owners(
                    IndexOffset(AccountInfo::get_reduced_offset(offset)),
                    owners,
                )
            }
        }
    }

    /// Return the path of the underlying account file.
    pub fn get_path(&self) -> PathBuf {
        match self {
            Self::AppendVec(av) => av.get_path(),
            Self::TieredStorage(ts) => ts.path().to_path_buf(),
        }
    }

    /// Return iterator for account metadata
    pub fn account_iter(&self) -> AccountsFileIter {
        AccountsFileIter::new(self)
    }

    /// for each offset in `sorted_offsets`, return the account size
    pub(crate) fn get_account_sizes(&self, sorted_offsets: &[usize]) -> Vec<usize> {
        match self {
            Self::AppendVec(av) => av.get_account_sizes(sorted_offsets),
            Self::TieredStorage(ts) => ts
                .reader()
                .and_then(|reader| reader.get_account_sizes(sorted_offsets).ok())
                .unwrap_or_default(),
        }
    }

    /// iterate over all entries to put in index
    pub(crate) fn scan_index(&self, callback: impl FnMut(IndexInfo)) {
        match self {
            Self::AppendVec(av) => av.scan_index(callback),
            Self::TieredStorage(ts) => {
                if let Some(reader) = ts.reader() {
                    _ = reader.scan_index(callback);
                }
            }
        }
    }

    /// iterate over all pubkeys
    pub(crate) fn scan_pubkeys(&self, callback: impl FnMut(&Pubkey)) {
        match self {
            Self::AppendVec(av) => av.scan_pubkeys(callback),
            Self::TieredStorage(ts) => {
                if let Some(reader) = ts.reader() {
                    _ = reader.scan_pubkeys(callback);
                }
            }
        }
    }

    /// Return a vector of account metadata for each account, starting from `offset`.
    pub fn accounts(&self, offset: usize) -> Vec<StoredAccountMeta> {
        match self {
            Self::AppendVec(av) => av.accounts(offset),
            // Note: The conversion here is needed as the AccountsDB currently
            // assumes all offsets are multiple of 8 while TieredStorage uses
            // IndexOffset that is equivalent to AccountInfo::reduced_offset.
            Self::TieredStorage(ts) => ts
                .reader()
                .and_then(|reader| {
                    reader
                        .accounts(IndexOffset(AccountInfo::get_reduced_offset(offset)))
                        .ok()
                })
                .unwrap_or_default(),
        }
    }

    /// Copy each account metadata, account and hash to the internal buffer.
    /// If there is no room to write the first entry, None is returned.
    /// Otherwise, returns the starting offset of each account metadata.
    /// Plus, the final return value is the offset where the next entry would be appended.
    /// So, return.len() is 1 + (number of accounts written)
    /// After each account is appended, the internal `current_len` is updated
    /// and will be available to other threads.
    pub fn append_accounts<
        'a,
        'b,
        T: ReadableAccount + Sync,
        U: StorableAccounts<'a, T>,
        V: Borrow<AccountHash>,
    >(
        &self,
        accounts: &StorableAccountsWithHashes<'a, 'b, T, U, V>,
        skip: usize,
    ) -> Option<Vec<StoredAccountInfo>> {
        match self {
            Self::AppendVec(av) => av.append_accounts(accounts, skip),
            // Note: The conversion here is needed as the AccountsDB currently
            // assumes all offsets are multiple of 8 while TieredStorage uses
            // IndexOffset that is equivalent to AccountInfo::reduced_offset.
            Self::TieredStorage(ts) => ts
                .write_accounts(accounts, skip, &HOT_FORMAT)
                .map(|mut infos| {
                    infos.iter_mut().for_each(|info| {
                        info.offset = AccountInfo::reduced_offset_to_offset(info.offset as u32);
                    });
                    infos
                })
                .ok(),
        }
    }

    /// Returns a Read implementation suitable for use when archiving accounts files
    pub fn data_for_archive(&self) -> impl Read + '_ {
        match self {
            Self::AppendVec(av) => av.data_for_archive(),
            Self::TieredStorage(ts) => ts
                .reader()
                .expect("must be a reader when archiving")
                .data_for_archive(),
        }
    }
}

pub struct AccountsFileIter<'a> {
    file_entry: &'a AccountsFile,
    offset: usize,
}

impl<'a> AccountsFileIter<'a> {
    pub fn new(file_entry: &'a AccountsFile) -> Self {
        Self {
            file_entry,
            offset: 0,
        }
    }
}

impl<'a> Iterator for AccountsFileIter<'a> {
    type Item = StoredAccountMeta<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some((account, next_offset)) = self.file_entry.get_account(self.offset) {
            self.offset = next_offset;
            Some(account)
        } else {
            None
        }
    }
}

/// An enum that creates AccountsFile instance with the specified format.
#[derive(Debug, Default, Copy, Clone, Eq, PartialEq)]
pub enum AccountsFileProvider {
    #[default]
    AppendVec,
    HotStorage,
}

impl AccountsFileProvider {
    pub fn new_writable(&self, path: impl Into<PathBuf>, file_size: u64) -> AccountsFile {
        match self {
            Self::AppendVec => {
                AccountsFile::AppendVec(AppendVec::new(path, true, file_size as usize))
            }
            Self::HotStorage => AccountsFile::TieredStorage(TieredStorage::new_writable(path)),
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::accounts_file::AccountsFile;
    impl AccountsFile {
        pub(crate) fn set_current_len_for_tests(&self, len: usize) {
            match self {
                Self::AppendVec(av) => av.set_current_len_for_tests(len),
                Self::TieredStorage(_) => {}
            }
        }
    }
}
