#[cfg(test)]
use {
    super::*,
    crate::{
        accounts::{create_test_accounts, Accounts},
        accounts_db::get_temp_accounts_paths,
        bank::{Bank, StatusCacheRc},
    },
    bincode::serialize_into,
    rand::{thread_rng, Rng},
    solana_sdk::{
        account::Account,
        clock::Slot,
        genesis_config::{create_genesis_config, ClusterType},
        pubkey::Pubkey,
        signature::{Keypair, Signer},
    },
    std::io::{BufReader, Cursor},
    tempfile::TempDir,
};

#[cfg(test)]
fn copy_append_vecs<P: AsRef<Path>>(
    accounts_db: &AccountsDB,
    output_dir: P,
) -> std::io::Result<()> {
    let storage_entries = accounts_db.get_snapshot_storages(Slot::max_value());
    for storage in storage_entries.iter().flatten() {
        let storage_path = storage.get_path();
        let output_path = output_dir.as_ref().join(AppendVec::new_relative_path(
            storage.slot(),
            storage.append_vec_id(),
        ));

        std::fs::copy(storage_path, output_path)?;
    }

    Ok(())
}

#[cfg(test)]
fn check_accounts(accounts: &Accounts, pubkeys: &[Pubkey], num: usize) {
    for _ in 1..num {
        let idx = thread_rng().gen_range(0, num - 1);
        let ancestors = vec![(0, 0)].into_iter().collect();
        let account = accounts.load_slow(&ancestors, &pubkeys[idx]);
        let account1 = Some((
            Account::new((idx + 1) as u64, 0, &Account::default().owner),
            0,
        ));
        assert_eq!(account, account1);
    }
}

#[cfg(test)]
fn context_accountsdb_from_stream<'a, C, R, P>(
    stream: &mut BufReader<R>,
    account_paths: &[PathBuf],
    stream_append_vecs_path: P,
) -> Result<AccountsDB, Error>
where
    C: TypeContext<'a>,
    R: Read,
    P: AsRef<Path>,
{
    // read and deserialise the accounts database directly from the stream
    reconstruct_accountsdb_from_fields(
        C::deserialize_accounts_db_fields(stream)?,
        account_paths,
        stream_append_vecs_path,
        &ClusterType::Development,
    )
}

#[cfg(test)]
fn accountsdb_from_stream<R, P>(
    serde_style: SerdeStyle,
    stream: &mut BufReader<R>,
    account_paths: &[PathBuf],
    stream_append_vecs_path: P,
) -> Result<AccountsDB, Error>
where
    R: Read,
    P: AsRef<Path>,
{
    match serde_style {
        SerdeStyle::NEWER => context_accountsdb_from_stream::<TypeContextFuture, R, P>(
            stream,
            account_paths,
            stream_append_vecs_path,
        ),
    }
}

#[cfg(test)]
fn accountsdb_to_stream<W>(
    serde_style: SerdeStyle,
    stream: &mut W,
    accounts_db: &AccountsDB,
    slot: Slot,
    account_storage_entries: &[SnapshotStorage],
) -> Result<(), Error>
where
    W: Write,
{
    match serde_style {
        SerdeStyle::NEWER => serialize_into(
            stream,
            &SerializableAccountsDB::<TypeContextFuture> {
                accounts_db,
                slot,
                account_storage_entries,
                phantom: std::marker::PhantomData::default(),
            },
        ),
    }
}

#[cfg(test)]
fn test_accounts_serialize_style(serde_style: SerdeStyle) {
    solana_logger::setup();
    let (_accounts_dir, paths) = get_temp_accounts_paths(4).unwrap();
    let accounts = Accounts::new(paths, &ClusterType::Development);

    let mut pubkeys: Vec<Pubkey> = vec![];
    create_test_accounts(&accounts, &mut pubkeys, 100, 0);
    check_accounts(&accounts, &pubkeys, 100);
    accounts.add_root(0);

    let mut writer = Cursor::new(vec![]);
    accountsdb_to_stream(
        serde_style,
        &mut writer,
        &*accounts.accounts_db,
        0,
        &accounts.accounts_db.get_snapshot_storages(0),
    )
    .unwrap();

    let copied_accounts = TempDir::new().unwrap();

    // Simulate obtaining a copy of the AppendVecs from a tarball
    copy_append_vecs(&accounts.accounts_db, copied_accounts.path()).unwrap();

    let buf = writer.into_inner();
    let mut reader = BufReader::new(&buf[..]);
    let (_accounts_dir, daccounts_paths) = get_temp_accounts_paths(2).unwrap();
    let daccounts = Accounts::new_empty(
        accountsdb_from_stream(
            serde_style,
            &mut reader,
            &daccounts_paths,
            copied_accounts.path(),
        )
        .unwrap(),
    );
    check_accounts(&daccounts, &pubkeys, 100);
    assert_eq!(accounts.bank_hash_at(0), daccounts.bank_hash_at(0));
}

#[cfg(test)]
fn test_bank_serialize_style(serde_style: SerdeStyle) {
    solana_logger::setup();
    let (genesis_config, _) = create_genesis_config(500);
    let bank0 = Arc::new(Bank::new(&genesis_config));
    let bank1 = Bank::new_from_parent(&bank0, &Pubkey::default(), 1);
    bank0.squash();

    // Create an account on a non-root fork
    let key1 = Keypair::new();
    bank1.deposit(&key1.pubkey(), 5);

    let bank2 = Bank::new_from_parent(&bank0, &Pubkey::default(), 2);

    // Test new account
    let key2 = Keypair::new();
    bank2.deposit(&key2.pubkey(), 10);
    assert_eq!(bank2.get_balance(&key2.pubkey()), 10);

    let key3 = Keypair::new();
    bank2.deposit(&key3.pubkey(), 0);

    bank2.squash();

    let snapshot_storages = bank2.get_snapshot_storages();
    let mut buf = vec![];
    let mut writer = Cursor::new(&mut buf);
    crate::serde_snapshot::bank_to_stream(
        serde_style,
        &mut std::io::BufWriter::new(&mut writer),
        &bank2,
        &snapshot_storages,
    )
    .unwrap();

    let rdr = Cursor::new(&buf[..]);
    let mut reader = std::io::BufReader::new(&buf[rdr.position() as usize..]);

    // Create a new set of directories for this bank's accounts
    let (_accounts_dir, dbank_paths) = get_temp_accounts_paths(4).unwrap();
    let ref_sc = StatusCacheRc::default();
    ref_sc.status_cache.write().unwrap().add_root(2);
    // Create a directory to simulate AppendVecs unpackaged from a snapshot tar
    let copied_accounts = TempDir::new().unwrap();
    copy_append_vecs(&bank2.rc.accounts.accounts_db, copied_accounts.path()).unwrap();
    let mut dbank = crate::serde_snapshot::bank_from_stream(
        serde_style,
        &mut reader,
        copied_accounts.path(),
        &dbank_paths,
        &genesis_config,
        &[],
        None,
        None,
    )
    .unwrap();
    dbank.src = ref_sc;
    assert_eq!(dbank.get_balance(&key1.pubkey()), 0);
    assert_eq!(dbank.get_balance(&key2.pubkey()), 10);
    assert_eq!(dbank.get_balance(&key3.pubkey()), 0);
    assert!(bank2 == dbank);
}

#[cfg(test)]
pub(crate) fn reconstruct_accounts_db_via_serialization(
    accounts: &AccountsDB,
    slot: Slot,
) -> AccountsDB {
    let mut writer = Cursor::new(vec![]);
    let snapshot_storages = accounts.get_snapshot_storages(slot);
    accountsdb_to_stream(
        SerdeStyle::NEWER,
        &mut writer,
        &accounts,
        slot,
        &snapshot_storages,
    )
    .unwrap();

    let buf = writer.into_inner();
    let mut reader = BufReader::new(&buf[..]);
    let copied_accounts = TempDir::new().unwrap();
    // Simulate obtaining a copy of the AppendVecs from a tarball
    copy_append_vecs(&accounts, copied_accounts.path()).unwrap();
    accountsdb_from_stream(SerdeStyle::NEWER, &mut reader, &[], copied_accounts.path()).unwrap()
}

#[test]
fn test_accounts_serialize_newer() {
    test_accounts_serialize_style(SerdeStyle::NEWER)
}

#[test]
fn test_bank_serialize_newer() {
    test_bank_serialize_style(SerdeStyle::NEWER)
}

#[cfg(all(test, RUSTC_WITH_SPECIALIZATION))]
mod test_bank_serialize {
    use super::*;

    // These some what long test harness is required to freeze the ABI of
    // Bank's serialization due to versioned nature
    #[frozen_abi(digest = "Giao4XJq9QgW78sqmT3nRMvENt4BgHXdzphCDGFPbXqW")]
    #[derive(Serialize, AbiExample)]
    pub struct BankAbiTestWrapperFuture {
        #[serde(serialize_with = "wrapper_future")]
        bank: Bank,
    }

    pub fn wrapper_future<S>(bank: &Bank, s: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let snapshot_storages = bank.rc.accounts.accounts_db.get_snapshot_storages(0);
        // ensure there is a single snapshot storage example for ABI digesting
        assert_eq!(snapshot_storages.len(), 1);

        (SerializableBankAndStorage::<future::Context> {
            bank,
            snapshot_storages: &snapshot_storages,
            phantom: std::marker::PhantomData::default(),
        })
        .serialize(s)
    }
}
