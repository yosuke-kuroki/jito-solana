use {
    crate::{
        accounts_db::AccountsDb,
        accounts_index::AccountIndex,
        bank::{Bank, BankSlotDelta, Builtins},
        bank_forks::ArchiveFormat,
        hardened_unpack::{unpack_snapshot, UnpackError, UnpackedAppendVecMap},
        serde_snapshot::{
            bank_from_stream, bank_to_stream, SerdeStyle, SnapshotStorage, SnapshotStorages,
        },
        snapshot_package::{
            AccountsPackage, AccountsPackagePre, AccountsPackageSendError, AccountsPackageSender,
        },
    },
    bincode::{config::Options, serialize_into},
    bzip2::bufread::BzDecoder,
    flate2::read::GzDecoder,
    log::*,
    rayon::ThreadPool,
    regex::Regex,
    solana_measure::measure::Measure,
    solana_sdk::{clock::Slot, genesis_config::GenesisConfig, hash::Hash, pubkey::Pubkey},
    std::{
        cmp::Ordering,
        collections::HashSet,
        fmt,
        fs::{self, File},
        io::{
            self, BufReader, BufWriter, Error as IoError, ErrorKind, Read, Seek, SeekFrom, Write,
        },
        path::{Path, PathBuf},
        process::{self, ExitStatus},
        str::FromStr,
        sync::Arc,
    },
    tar::Archive,
    thiserror::Error,
};

pub const SNAPSHOT_STATUS_CACHE_FILE_NAME: &str = "status_cache";

pub const MAX_SNAPSHOTS: usize = 8; // Save some snapshots but not too many
const MAX_SNAPSHOT_DATA_FILE_SIZE: u64 = 32 * 1024 * 1024 * 1024; // 32 GiB
const VERSION_STRING_V1_2_0: &str = "1.2.0";
const DEFAULT_SNAPSHOT_VERSION: SnapshotVersion = SnapshotVersion::V1_2_0;
const TMP_SNAPSHOT_PREFIX: &str = "tmp-snapshot-";
const TMP_SNAPSHOT_EXTENSION: &str = ".tmp"; // when downloading

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum SnapshotVersion {
    V1_2_0,
}

impl Default for SnapshotVersion {
    fn default() -> Self {
        DEFAULT_SNAPSHOT_VERSION
    }
}

impl fmt::Display for SnapshotVersion {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(From::from(*self))
    }
}

impl From<SnapshotVersion> for &'static str {
    fn from(snapshot_version: SnapshotVersion) -> &'static str {
        match snapshot_version {
            SnapshotVersion::V1_2_0 => VERSION_STRING_V1_2_0,
        }
    }
}

impl FromStr for SnapshotVersion {
    type Err = &'static str;

    fn from_str(version_string: &str) -> std::result::Result<Self, Self::Err> {
        // Remove leading 'v' or 'V' from slice
        let version_string = if version_string
            .get(..1)
            .map_or(false, |s| s.eq_ignore_ascii_case("v"))
        {
            &version_string[1..]
        } else {
            version_string
        };
        match version_string {
            VERSION_STRING_V1_2_0 => Ok(SnapshotVersion::V1_2_0),
            _ => Err("unsupported snapshot version"),
        }
    }
}

impl SnapshotVersion {
    pub fn as_str(self) -> &'static str {
        <&str as From<Self>>::from(self)
    }

    fn maybe_from_string(version_string: &str) -> Option<SnapshotVersion> {
        version_string.parse::<Self>().ok()
    }
}

#[derive(PartialEq, Eq, Debug)]
pub struct SlotSnapshotPaths {
    pub slot: Slot,
    pub snapshot_file_path: PathBuf,
}

#[derive(Error, Debug)]
pub enum SnapshotError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialize(#[from] bincode::Error),

    #[error("archive generation failure {0}")]
    ArchiveGenerationFailure(ExitStatus),

    #[error("storage path symlink is invalid")]
    StoragePathSymlinkInvalid,

    #[error("Unpack error: {0}")]
    UnpackError(#[from] UnpackError),

    #[error("accounts package send error")]
    AccountsPackageSendError(#[from] AccountsPackageSendError),
}
pub type Result<T> = std::result::Result<T, SnapshotError>;

impl PartialOrd for SlotSnapshotPaths {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.slot.cmp(&other.slot))
    }
}

impl Ord for SlotSnapshotPaths {
    fn cmp(&self, other: &Self) -> Ordering {
        self.slot.cmp(&other.slot)
    }
}

pub fn package_snapshot<P: AsRef<Path>, Q: AsRef<Path>>(
    bank: &Bank,
    snapshot_files: &SlotSnapshotPaths,
    snapshot_path: Q,
    status_cache_slot_deltas: Vec<BankSlotDelta>,
    snapshot_package_output_path: P,
    snapshot_storages: SnapshotStorages,
    archive_format: ArchiveFormat,
    snapshot_version: SnapshotVersion,
    hash_for_testing: Option<Hash>,
) -> Result<AccountsPackagePre> {
    // Hard link all the snapshots we need for this package
    let snapshot_tmpdir = tempfile::Builder::new()
        .prefix(&format!("{}{}-", TMP_SNAPSHOT_PREFIX, bank.slot()))
        .tempdir_in(snapshot_path)?;

    // Create a snapshot package
    info!(
        "Snapshot for bank: {} has {} account storage entries",
        bank.slot(),
        snapshot_storages.len()
    );

    // Hard link the snapshot into a tmpdir, to ensure its not removed prior to packaging.
    {
        let snapshot_hardlink_dir = snapshot_tmpdir
            .as_ref()
            .join(snapshot_files.slot.to_string());
        fs::create_dir_all(&snapshot_hardlink_dir)?;
        fs::hard_link(
            &snapshot_files.snapshot_file_path,
            &snapshot_hardlink_dir.join(snapshot_files.slot.to_string()),
        )?;
    }

    let package = AccountsPackagePre::new(
        bank.slot(),
        bank.block_height(),
        status_cache_slot_deltas,
        snapshot_tmpdir,
        snapshot_storages,
        bank.get_accounts_hash(),
        archive_format,
        snapshot_version,
        snapshot_package_output_path.as_ref().to_path_buf(),
        bank.capitalization(),
        hash_for_testing,
    );

    Ok(package)
}

fn get_archive_ext(archive_format: ArchiveFormat) -> &'static str {
    match archive_format {
        ArchiveFormat::TarBzip2 => ".tar.bz2",
        ArchiveFormat::TarGzip => ".tar.gz",
        ArchiveFormat::TarZstd => ".tar.zst",
        ArchiveFormat::Tar => ".tar",
    }
}

// If the validator is halted in the middle of `archive_snapshot_package` the temporary staging directory
// won't be cleaned up.  Call this function to clean them up
pub fn remove_tmp_snapshot_archives(snapshot_path: &Path) {
    if let Ok(entries) = fs::read_dir(&snapshot_path) {
        for entry in entries.filter_map(|entry| entry.ok()) {
            let filename = entry
                .file_name()
                .into_string()
                .unwrap_or_else(|_| String::new());
            if filename.starts_with(TMP_SNAPSHOT_PREFIX)
                | filename.ends_with(TMP_SNAPSHOT_EXTENSION)
            {
                if entry.path().is_file() {
                    fs::remove_file(entry.path())
                } else {
                    fs::remove_dir_all(entry.path())
                }
                .unwrap_or_else(|err| {
                    warn!("Failed to remove {}: {}", entry.path().display(), err)
                });
            }
        }
    }
}

pub fn archive_snapshot_package(snapshot_package: &AccountsPackage) -> Result<()> {
    info!(
        "Generating snapshot archive for slot {}",
        snapshot_package.slot
    );

    serialize_status_cache(
        snapshot_package.slot,
        &snapshot_package.slot_deltas,
        &snapshot_package
            .snapshot_links
            .path()
            .join(SNAPSHOT_STATUS_CACHE_FILE_NAME),
    )?;

    let mut timer = Measure::start("snapshot_package-package_snapshots");
    let tar_dir = snapshot_package
        .tar_output_file
        .parent()
        .expect("Tar output path is invalid");

    fs::create_dir_all(tar_dir)?;

    // Create the staging directories
    let staging_dir = tempfile::Builder::new()
        .prefix(&format!(
            "{}{}-",
            TMP_SNAPSHOT_PREFIX, snapshot_package.slot
        ))
        .tempdir_in(tar_dir)?;

    let staging_accounts_dir = staging_dir.path().join("accounts");
    let staging_snapshots_dir = staging_dir.path().join("snapshots");
    let staging_version_file = staging_dir.path().join("version");
    fs::create_dir_all(&staging_accounts_dir)?;

    // Add the snapshots to the staging directory
    symlink::symlink_dir(
        snapshot_package.snapshot_links.path(),
        &staging_snapshots_dir,
    )?;

    // Add the AppendVecs into the compressible list
    for storage in snapshot_package.storages.iter().flatten() {
        storage.flush()?;
        let storage_path = storage.get_path();
        let output_path = staging_accounts_dir.join(crate::append_vec::AppendVec::file_name(
            storage.slot(),
            storage.append_vec_id(),
        ));

        // `storage_path` - The file path where the AppendVec itself is located
        // `output_path` - The file path where the AppendVec will be placed in the staging directory.
        let storage_path =
            fs::canonicalize(storage_path).expect("Could not get absolute path for accounts");
        symlink::symlink_file(storage_path, &output_path)?;
        if !output_path.is_file() {
            return Err(SnapshotError::StoragePathSymlinkInvalid);
        }
    }

    // Write version file
    {
        let mut f = fs::File::create(staging_version_file)?;
        f.write_all(snapshot_package.snapshot_version.as_str().as_bytes())?;
    }

    let file_ext = get_archive_ext(snapshot_package.archive_format);

    // Tar the staging directory into the archive at `archive_path`
    //
    // system `tar` program is used for -S (sparse file support)
    let archive_path = tar_dir.join(format!(
        "{}{}{}",
        TMP_SNAPSHOT_PREFIX, snapshot_package.slot, file_ext
    ));

    let mut tar = process::Command::new("tar")
        .args(&[
            "chS",
            "-C",
            staging_dir.path().to_str().unwrap(),
            "accounts",
            "snapshots",
            "version",
        ])
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::inherit())
        .spawn()?;

    match &mut tar.stdout {
        None => {
            return Err(SnapshotError::Io(IoError::new(
                ErrorKind::Other,
                "tar stdout unavailable".to_string(),
            )));
        }
        Some(tar_output) => {
            let mut archive_file = fs::File::create(&archive_path)?;

            match snapshot_package.archive_format {
                ArchiveFormat::TarBzip2 => {
                    let mut encoder =
                        bzip2::write::BzEncoder::new(archive_file, bzip2::Compression::Best);
                    io::copy(tar_output, &mut encoder)?;
                    let _ = encoder.finish()?;
                }
                ArchiveFormat::TarGzip => {
                    let mut encoder =
                        flate2::write::GzEncoder::new(archive_file, flate2::Compression::default());
                    io::copy(tar_output, &mut encoder)?;
                    let _ = encoder.finish()?;
                }
                ArchiveFormat::Tar => {
                    io::copy(tar_output, &mut archive_file)?;
                }
                ArchiveFormat::TarZstd => {
                    let mut encoder = zstd::stream::Encoder::new(archive_file, 0)?;
                    io::copy(tar_output, &mut encoder)?;
                    let _ = encoder.finish()?;
                }
            };
        }
    }

    let tar_exit_status = tar.wait()?;
    if !tar_exit_status.success() {
        warn!("tar command failed with exit code: {}", tar_exit_status);
        return Err(SnapshotError::ArchiveGenerationFailure(tar_exit_status));
    }

    // Atomically move the archive into position for other validators to find
    let metadata = fs::metadata(&archive_path)?;
    fs::rename(&archive_path, &snapshot_package.tar_output_file)?;

    purge_old_snapshot_archives(snapshot_package.tar_output_file.parent().unwrap());

    timer.stop();
    info!(
        "Successfully created {:?}. slot: {}, elapsed ms: {}, size={}",
        snapshot_package.tar_output_file,
        snapshot_package.slot,
        timer.as_ms(),
        metadata.len()
    );
    datapoint_info!(
        "snapshot-package",
        ("slot", snapshot_package.slot, i64),
        ("duration_ms", timer.as_ms(), i64),
        ("size", metadata.len(), i64)
    );
    Ok(())
}

pub fn get_snapshot_paths<P: AsRef<Path>>(snapshot_path: P) -> Vec<SlotSnapshotPaths>
where
    P: fmt::Debug,
{
    match fs::read_dir(&snapshot_path) {
        Ok(paths) => {
            let mut names = paths
                .filter_map(|entry| {
                    entry.ok().and_then(|e| {
                        e.path()
                            .file_name()
                            .and_then(|n| n.to_str().map(|s| s.parse::<u64>().ok()))
                            .unwrap_or(None)
                    })
                })
                .map(|slot| {
                    let snapshot_path = snapshot_path.as_ref().join(slot.to_string());
                    SlotSnapshotPaths {
                        slot,
                        snapshot_file_path: snapshot_path.join(get_snapshot_file_name(slot)),
                    }
                })
                .collect::<Vec<SlotSnapshotPaths>>();

            names.sort();
            names
        }
        Err(err) => {
            info!(
                "Unable to read snapshot directory {:?}: {}",
                snapshot_path, err
            );
            vec![]
        }
    }
}

pub fn serialize_snapshot_data_file<F>(data_file_path: &Path, serializer: F) -> Result<u64>
where
    F: FnOnce(&mut BufWriter<File>) -> Result<()>,
{
    serialize_snapshot_data_file_capped::<F>(
        data_file_path,
        MAX_SNAPSHOT_DATA_FILE_SIZE,
        serializer,
    )
}

pub fn deserialize_snapshot_data_file<F, T>(data_file_path: &Path, deserializer: F) -> Result<T>
where
    F: FnOnce(&mut BufReader<File>) -> Result<T>,
{
    deserialize_snapshot_data_file_capped::<F, T>(
        data_file_path,
        MAX_SNAPSHOT_DATA_FILE_SIZE,
        deserializer,
    )
}

fn serialize_snapshot_data_file_capped<F>(
    data_file_path: &Path,
    maximum_file_size: u64,
    serializer: F,
) -> Result<u64>
where
    F: FnOnce(&mut BufWriter<File>) -> Result<()>,
{
    let data_file = File::create(data_file_path)?;
    let mut data_file_stream = BufWriter::new(data_file);
    serializer(&mut data_file_stream)?;
    data_file_stream.flush()?;

    let consumed_size = data_file_stream.seek(SeekFrom::Current(0))?;
    if consumed_size > maximum_file_size {
        let error_message = format!(
            "too large snapshot data file to serialize: {:?} has {} bytes",
            data_file_path, consumed_size
        );
        return Err(get_io_error(&error_message));
    }
    Ok(consumed_size)
}

fn deserialize_snapshot_data_file_capped<F, T>(
    data_file_path: &Path,
    maximum_file_size: u64,
    deserializer: F,
) -> Result<T>
where
    F: FnOnce(&mut BufReader<File>) -> Result<T>,
{
    let file_size = fs::metadata(&data_file_path)?.len();

    if file_size > maximum_file_size {
        let error_message = format!(
            "too large snapshot data file to deserialize: {:?} has {} bytes",
            data_file_path, file_size
        );
        return Err(get_io_error(&error_message));
    }

    let data_file = File::open(data_file_path)?;
    let mut data_file_stream = BufReader::new(data_file);

    let ret = deserializer(&mut data_file_stream)?;

    let consumed_size = data_file_stream.seek(SeekFrom::Current(0))?;

    if file_size != consumed_size {
        let error_message = format!(
            "invalid snapshot data file: {:?} has {} bytes, however consumed {} bytes to deserialize",
            data_file_path, file_size, consumed_size
        );
        return Err(get_io_error(&error_message));
    }

    Ok(ret)
}

pub fn add_snapshot<P: AsRef<Path>>(
    snapshot_path: P,
    bank: &Bank,
    snapshot_storages: &[SnapshotStorage],
    snapshot_version: SnapshotVersion,
) -> Result<SlotSnapshotPaths> {
    let slot = bank.slot();
    // snapshot_path/slot
    let slot_snapshot_dir = get_bank_snapshot_dir(snapshot_path, slot);
    fs::create_dir_all(slot_snapshot_dir.clone())?;

    // the bank snapshot is stored as snapshot_path/slot/slot
    let snapshot_bank_file_path = slot_snapshot_dir.join(get_snapshot_file_name(slot));
    info!(
        "Creating snapshot for slot {}, path: {:?}",
        slot, snapshot_bank_file_path,
    );

    let mut bank_serialize = Measure::start("bank-serialize-ms");
    let bank_snapshot_serializer = move |stream: &mut BufWriter<File>| -> Result<()> {
        let serde_style = match snapshot_version {
            SnapshotVersion::V1_2_0 => SerdeStyle::Newer,
        };
        bank_to_stream(serde_style, stream.by_ref(), bank, snapshot_storages)?;
        Ok(())
    };
    let consumed_size =
        serialize_snapshot_data_file(&snapshot_bank_file_path, bank_snapshot_serializer)?;
    bank_serialize.stop();

    // Monitor sizes because they're capped to MAX_SNAPSHOT_DATA_FILE_SIZE
    datapoint_info!(
        "snapshot-bank-file",
        ("slot", slot, i64),
        ("size", consumed_size, i64)
    );

    inc_new_counter_info!("bank-serialize-ms", bank_serialize.as_ms() as usize);

    info!(
        "{} for slot {} at {:?}",
        bank_serialize, slot, snapshot_bank_file_path,
    );

    Ok(SlotSnapshotPaths {
        slot,
        snapshot_file_path: snapshot_bank_file_path,
    })
}

fn serialize_status_cache(
    slot: Slot,
    slot_deltas: &[BankSlotDelta],
    status_cache_path: &Path,
) -> Result<()> {
    let mut status_cache_serialize = Measure::start("status_cache_serialize-ms");
    let consumed_size = serialize_snapshot_data_file(status_cache_path, |stream| {
        serialize_into(stream, slot_deltas)?;
        Ok(())
    })?;
    status_cache_serialize.stop();

    // Monitor sizes because they're capped to MAX_SNAPSHOT_DATA_FILE_SIZE
    datapoint_info!(
        "snapshot-status-cache-file",
        ("slot", slot, i64),
        ("size", consumed_size, i64)
    );

    inc_new_counter_info!(
        "serialize-status-cache-ms",
        status_cache_serialize.as_ms() as usize
    );
    Ok(())
}

pub fn remove_snapshot<P: AsRef<Path>>(slot: Slot, snapshot_path: P) -> Result<()> {
    let slot_snapshot_dir = get_bank_snapshot_dir(&snapshot_path, slot);
    // Remove the snapshot directory for this slot
    fs::remove_dir_all(slot_snapshot_dir)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn bank_from_archive<P: AsRef<Path>>(
    account_paths: &[PathBuf],
    frozen_account_pubkeys: &[Pubkey],
    snapshot_path: &Path,
    snapshot_tar: P,
    archive_format: ArchiveFormat,
    genesis_config: &GenesisConfig,
    debug_keys: Option<Arc<HashSet<Pubkey>>>,
    additional_builtins: Option<&Builtins>,
    account_indexes: HashSet<AccountIndex>,
    accounts_db_caching_enabled: bool,
) -> Result<Bank> {
    let unpack_dir = tempfile::Builder::new()
        .prefix(TMP_SNAPSHOT_PREFIX)
        .tempdir_in(snapshot_path)?;

    let unpacked_append_vec_map = untar_snapshot_in(
        &snapshot_tar,
        &unpack_dir.as_ref(),
        account_paths,
        archive_format,
    )?;

    let mut measure = Measure::start("bank rebuild from snapshot");
    let unpacked_snapshots_dir = unpack_dir.as_ref().join("snapshots");
    let unpacked_version_file = unpack_dir.as_ref().join("version");

    let mut snapshot_version = String::new();
    File::open(unpacked_version_file).and_then(|mut f| f.read_to_string(&mut snapshot_version))?;

    let bank = rebuild_bank_from_snapshots(
        snapshot_version.trim(),
        frozen_account_pubkeys,
        &unpacked_snapshots_dir,
        account_paths,
        unpacked_append_vec_map,
        genesis_config,
        debug_keys,
        additional_builtins,
        account_indexes,
        accounts_db_caching_enabled,
    )?;

    if !bank.verify_snapshot_bank() {
        panic!("Snapshot bank for slot {} failed to verify", bank.slot());
    }
    measure.stop();
    info!("{}", measure);

    Ok(bank)
}

pub fn get_snapshot_archive_path(
    snapshot_output_dir: PathBuf,
    snapshot_hash: &(Slot, Hash),
    archive_format: ArchiveFormat,
) -> PathBuf {
    snapshot_output_dir.join(format!(
        "snapshot-{}-{}{}",
        snapshot_hash.0,
        snapshot_hash.1,
        get_archive_ext(archive_format),
    ))
}

fn archive_format_from_str(archive_format: &str) -> Option<ArchiveFormat> {
    match archive_format {
        "tar.bz2" => Some(ArchiveFormat::TarBzip2),
        "tar.gz" => Some(ArchiveFormat::TarGzip),
        "tar.zst" => Some(ArchiveFormat::TarZstd),
        "tar" => Some(ArchiveFormat::Tar),
        _ => None,
    }
}

fn snapshot_hash_of(archive_filename: &str) -> Option<(Slot, Hash, ArchiveFormat)> {
    let snapshot_filename_regex =
        Regex::new(r"snapshot-(\d+)-([[:alnum:]]+)\.(tar|tar\.bz2|tar\.zst|tar\.gz)$").unwrap();

    if let Some(captures) = snapshot_filename_regex.captures(archive_filename) {
        let slot_str = captures.get(1).unwrap().as_str();
        let hash_str = captures.get(2).unwrap().as_str();
        let ext = captures.get(3).unwrap().as_str();

        if let (Ok(slot), Ok(hash), Some(archive_format)) = (
            slot_str.parse::<Slot>(),
            hash_str.parse::<Hash>(),
            archive_format_from_str(ext),
        ) {
            return Some((slot, hash, archive_format));
        }
    }
    None
}

pub fn get_snapshot_archives<P: AsRef<Path>>(
    snapshot_output_dir: P,
) -> Vec<(PathBuf, (Slot, Hash, ArchiveFormat))> {
    match fs::read_dir(&snapshot_output_dir) {
        Err(err) => {
            info!("Unable to read snapshot directory: {}", err);
            vec![]
        }
        Ok(files) => {
            let mut archives: Vec<_> = files
                .filter_map(|entry| {
                    if let Ok(entry) = entry {
                        let path = entry.path();
                        if path.is_file() {
                            if let Some(snapshot_hash) =
                                snapshot_hash_of(path.file_name().unwrap().to_str().unwrap())
                            {
                                return Some((path, snapshot_hash));
                            }
                        }
                    }
                    None
                })
                .collect();

            archives.sort_by(|a, b| (b.1).0.cmp(&(a.1).0)); // reverse sort by slot
            archives
        }
    }
}

pub fn get_highest_snapshot_archive_path<P: AsRef<Path>>(
    snapshot_output_dir: P,
) -> Option<(PathBuf, (Slot, Hash, ArchiveFormat))> {
    let archives = get_snapshot_archives(snapshot_output_dir);
    archives.into_iter().next()
}

pub fn purge_old_snapshot_archives<P: AsRef<Path>>(snapshot_output_dir: P) {
    let mut archives = get_snapshot_archives(snapshot_output_dir);
    // Keep the oldest snapshot so we can always play the ledger from it.
    archives.pop();
    for old_archive in archives.into_iter().skip(2) {
        fs::remove_file(old_archive.0)
            .unwrap_or_else(|err| info!("Failed to remove old snapshot: {:}", err));
    }
}

fn untar_snapshot_in<P: AsRef<Path>>(
    snapshot_tar: P,
    unpack_dir: &Path,
    account_paths: &[PathBuf],
    archive_format: ArchiveFormat,
) -> Result<UnpackedAppendVecMap> {
    let mut measure = Measure::start("snapshot untar");
    let tar_name = File::open(&snapshot_tar)?;
    let account_paths_map = match archive_format {
        ArchiveFormat::TarBzip2 => {
            let tar = BzDecoder::new(BufReader::new(tar_name));
            let mut archive = Archive::new(tar);
            unpack_snapshot(&mut archive, unpack_dir, account_paths)?
        }
        ArchiveFormat::TarGzip => {
            let tar = GzDecoder::new(BufReader::new(tar_name));
            let mut archive = Archive::new(tar);
            unpack_snapshot(&mut archive, unpack_dir, account_paths)?
        }
        ArchiveFormat::TarZstd => {
            let tar = zstd::stream::read::Decoder::new(BufReader::new(tar_name))?;
            let mut archive = Archive::new(tar);
            unpack_snapshot(&mut archive, unpack_dir, account_paths)?
        }
        ArchiveFormat::Tar => {
            let tar = BufReader::new(tar_name);
            let mut archive = Archive::new(tar);
            unpack_snapshot(&mut archive, unpack_dir, account_paths)?
        }
    };
    measure.stop();
    info!("{}", measure);
    Ok(account_paths_map)
}

#[allow(clippy::too_many_arguments)]
fn rebuild_bank_from_snapshots(
    snapshot_version: &str,
    frozen_account_pubkeys: &[Pubkey],
    unpacked_snapshots_dir: &Path,
    account_paths: &[PathBuf],
    unpacked_append_vec_map: UnpackedAppendVecMap,
    genesis_config: &GenesisConfig,
    debug_keys: Option<Arc<HashSet<Pubkey>>>,
    additional_builtins: Option<&Builtins>,
    account_indexes: HashSet<AccountIndex>,
    accounts_db_caching_enabled: bool,
) -> Result<Bank> {
    info!("snapshot version: {}", snapshot_version);

    let snapshot_version_enum =
        SnapshotVersion::maybe_from_string(snapshot_version).ok_or_else(|| {
            get_io_error(&format!(
                "unsupported snapshot version: {}",
                snapshot_version
            ))
        })?;
    let mut snapshot_paths = get_snapshot_paths(&unpacked_snapshots_dir);
    if snapshot_paths.len() > 1 {
        return Err(get_io_error("invalid snapshot format"));
    }
    let root_paths = snapshot_paths
        .pop()
        .ok_or_else(|| get_io_error("No snapshots found in snapshots directory"))?;

    info!(
        "Loading bank from {}",
        &root_paths.snapshot_file_path.display()
    );
    let bank = deserialize_snapshot_data_file(&root_paths.snapshot_file_path, |mut stream| {
        Ok(match snapshot_version_enum {
            SnapshotVersion::V1_2_0 => bank_from_stream(
                SerdeStyle::Newer,
                &mut stream,
                account_paths,
                unpacked_append_vec_map,
                genesis_config,
                frozen_account_pubkeys,
                debug_keys,
                additional_builtins,
                account_indexes,
                accounts_db_caching_enabled,
            ),
        }?)
    })?;

    let status_cache_path = unpacked_snapshots_dir.join(SNAPSHOT_STATUS_CACHE_FILE_NAME);
    let slot_deltas = deserialize_snapshot_data_file(&status_cache_path, |stream| {
        info!(
            "Rebuilding status cache from {}",
            status_cache_path.display()
        );
        let slot_deltas: Vec<BankSlotDelta> = bincode::options()
            .with_limit(MAX_SNAPSHOT_DATA_FILE_SIZE)
            .with_fixint_encoding()
            .allow_trailing_bytes()
            .deserialize_from(stream)?;
        Ok(slot_deltas)
    })?;

    bank.src.append(&slot_deltas);

    info!("Loaded bank for slot: {}", bank.slot());
    Ok(bank)
}

fn get_snapshot_file_name(slot: Slot) -> String {
    slot.to_string()
}

fn get_bank_snapshot_dir<P: AsRef<Path>>(path: P, slot: Slot) -> PathBuf {
    path.as_ref().join(slot.to_string())
}

fn get_io_error(error: &str) -> SnapshotError {
    warn!("Snapshot Error: {:?}", error);
    SnapshotError::Io(IoError::new(ErrorKind::Other, error))
}

pub fn verify_snapshot_archive<P, Q, R>(
    snapshot_archive: P,
    snapshots_to_verify: Q,
    storages_to_verify: R,
    archive_format: ArchiveFormat,
) where
    P: AsRef<Path>,
    Q: AsRef<Path>,
    R: AsRef<Path>,
{
    let temp_dir = tempfile::TempDir::new().unwrap();
    let unpack_dir = temp_dir.path();
    untar_snapshot_in(
        snapshot_archive,
        &unpack_dir,
        &[unpack_dir.to_path_buf()],
        archive_format,
    )
    .unwrap();

    // Check snapshots are the same
    let unpacked_snapshots = unpack_dir.join("snapshots");
    assert!(!dir_diff::is_different(&snapshots_to_verify, unpacked_snapshots).unwrap());

    // Check the account entries are the same
    let unpacked_accounts = unpack_dir.join("accounts");
    assert!(!dir_diff::is_different(&storages_to_verify, unpacked_accounts).unwrap());
}

pub fn purge_old_snapshots(snapshot_path: &Path) {
    // Remove outdated snapshots
    let slot_snapshot_paths = get_snapshot_paths(snapshot_path);
    let num_to_remove = slot_snapshot_paths.len().saturating_sub(MAX_SNAPSHOTS);
    for slot_files in &slot_snapshot_paths[..num_to_remove] {
        let r = remove_snapshot(slot_files.slot, snapshot_path);
        if r.is_err() {
            warn!("Couldn't remove snapshot at: {:?}", snapshot_path);
        }
    }
}

// Gather the necessary elements for a snapshot of the given `root_bank`
pub fn snapshot_bank(
    root_bank: &Bank,
    status_cache_slot_deltas: Vec<BankSlotDelta>,
    accounts_package_sender: &AccountsPackageSender,
    snapshot_path: &Path,
    snapshot_package_output_path: &Path,
    snapshot_version: SnapshotVersion,
    archive_format: &ArchiveFormat,
    hash_for_testing: Option<Hash>,
) -> Result<()> {
    let storages: Vec<_> = root_bank.get_snapshot_storages();
    let mut add_snapshot_time = Measure::start("add-snapshot-ms");
    add_snapshot(snapshot_path, &root_bank, &storages, snapshot_version)?;
    add_snapshot_time.stop();
    inc_new_counter_info!("add-snapshot-ms", add_snapshot_time.as_ms() as usize);

    // Package the relevant snapshots
    let slot_snapshot_paths = get_snapshot_paths(snapshot_path);
    let latest_slot_snapshot_paths = slot_snapshot_paths
        .last()
        .expect("no snapshots found in config snapshot_path");

    let package = package_snapshot(
        &root_bank,
        latest_slot_snapshot_paths,
        snapshot_path,
        status_cache_slot_deltas,
        snapshot_package_output_path,
        storages,
        *archive_format,
        snapshot_version,
        hash_for_testing,
    )?;

    accounts_package_sender.send(package)?;

    Ok(())
}

/// Convenience function to create a snapshot archive out of any Bank, regardless of state.  The
/// Bank will be frozen during the process.
pub fn bank_to_snapshot_archive<P: AsRef<Path>, Q: AsRef<Path>>(
    snapshot_path: P,
    bank: &Bank,
    snapshot_version: Option<SnapshotVersion>,
    snapshot_package_output_path: Q,
    archive_format: ArchiveFormat,
    thread_pool: Option<&ThreadPool>,
) -> Result<PathBuf> {
    let snapshot_version = snapshot_version.unwrap_or_default();

    assert!(bank.is_complete());
    bank.squash(); // Bank may not be a root
    bank.force_flush_accounts_cache();
    bank.clean_accounts(true);
    bank.update_accounts_hash();
    bank.rehash(); // Bank accounts may have been manually modified by the caller

    let temp_dir = tempfile::tempdir_in(snapshot_path)?;

    let storages: Vec<_> = bank.get_snapshot_storages();
    let slot_snapshot_paths = add_snapshot(&temp_dir, &bank, &storages, snapshot_version)?;
    let package = package_snapshot(
        &bank,
        &slot_snapshot_paths,
        &temp_dir,
        bank.src.slot_deltas(&bank.src.roots()),
        snapshot_package_output_path,
        storages,
        archive_format,
        snapshot_version,
        None,
    )?;

    let package = process_accounts_package_pre(package, thread_pool);

    archive_snapshot_package(&package)?;
    Ok(package.tar_output_file)
}

pub fn process_accounts_package_pre(
    accounts_package: AccountsPackagePre,
    thread_pool: Option<&ThreadPool>,
) -> AccountsPackage {
    let mut time = Measure::start("hash");

    let hash = accounts_package.hash; // temporarily remaining here
    if let Some(expected_hash) = accounts_package.hash_for_testing {
        let (hash, lamports) = AccountsDb::calculate_accounts_hash_without_index(
            &accounts_package.storages,
            thread_pool,
        );

        assert_eq!(accounts_package.expected_capitalization, lamports);

        assert_eq!(expected_hash, hash);
    };
    time.stop();

    datapoint_info!(
        "accounts_hash_verifier",
        ("calculate_hash", time.as_us(), i64),
    );

    let tar_output_file = get_snapshot_archive_path(
        accounts_package.snapshot_output_dir,
        &(accounts_package.slot, hash),
        accounts_package.archive_format,
    );

    AccountsPackage::new(
        accounts_package.slot,
        accounts_package.block_height,
        accounts_package.slot_deltas,
        accounts_package.snapshot_links,
        accounts_package.storages,
        tar_output_file,
        hash,
        accounts_package.archive_format,
        accounts_package.snapshot_version,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use bincode::{deserialize_from, serialize_into};
    use std::mem::size_of;

    #[test]
    fn test_serialize_snapshot_data_file_under_limit() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let expected_consumed_size = size_of::<u32>() as u64;
        let consumed_size = serialize_snapshot_data_file_capped(
            &temp_dir.path().join("data-file"),
            expected_consumed_size,
            |stream| {
                serialize_into(stream, &2323_u32)?;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(consumed_size, expected_consumed_size);
    }

    #[test]
    fn test_serialize_snapshot_data_file_over_limit() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let expected_consumed_size = size_of::<u32>() as u64;
        let result = serialize_snapshot_data_file_capped(
            &temp_dir.path().join("data-file"),
            expected_consumed_size - 1,
            |stream| {
                serialize_into(stream, &2323_u32)?;
                Ok(())
            },
        );
        assert_matches!(result, Err(SnapshotError::Io(ref message)) if message.to_string().starts_with("too large snapshot data file to serialize"));
    }

    #[test]
    fn test_deserialize_snapshot_data_file_under_limit() {
        let expected_data = 2323_u32;
        let expected_consumed_size = size_of::<u32>() as u64;

        let temp_dir = tempfile::TempDir::new().unwrap();
        serialize_snapshot_data_file_capped(
            &temp_dir.path().join("data-file"),
            expected_consumed_size,
            |stream| {
                serialize_into(stream, &expected_data)?;
                Ok(())
            },
        )
        .unwrap();

        let actual_data = deserialize_snapshot_data_file_capped(
            &temp_dir.path().join("data-file"),
            expected_consumed_size,
            |stream| Ok(deserialize_from::<_, u32>(stream)?),
        )
        .unwrap();
        assert_eq!(actual_data, expected_data);
    }

    #[test]
    fn test_deserialize_snapshot_data_file_over_limit() {
        let expected_data = 2323_u32;
        let expected_consumed_size = size_of::<u32>() as u64;

        let temp_dir = tempfile::TempDir::new().unwrap();
        serialize_snapshot_data_file_capped(
            &temp_dir.path().join("data-file"),
            expected_consumed_size,
            |stream| {
                serialize_into(stream, &expected_data)?;
                Ok(())
            },
        )
        .unwrap();

        let result = deserialize_snapshot_data_file_capped(
            &temp_dir.path().join("data-file"),
            expected_consumed_size - 1,
            |stream| Ok(deserialize_from::<_, u32>(stream)?),
        );
        assert_matches!(result, Err(SnapshotError::Io(ref message)) if message.to_string().starts_with("too large snapshot data file to deserialize"));
    }

    #[test]
    fn test_deserialize_snapshot_data_file_extra_data() {
        let expected_data = 2323_u32;
        let expected_consumed_size = size_of::<u32>() as u64;

        let temp_dir = tempfile::TempDir::new().unwrap();
        serialize_snapshot_data_file_capped(
            &temp_dir.path().join("data-file"),
            expected_consumed_size * 2,
            |stream| {
                serialize_into(stream.by_ref(), &expected_data)?;
                serialize_into(stream.by_ref(), &expected_data)?;
                Ok(())
            },
        )
        .unwrap();

        let result = deserialize_snapshot_data_file_capped(
            &temp_dir.path().join("data-file"),
            expected_consumed_size * 2,
            |stream| Ok(deserialize_from::<_, u32>(stream)?),
        );
        assert_matches!(result, Err(SnapshotError::Io(ref message)) if message.to_string().starts_with("invalid snapshot data file"));
    }

    #[test]
    fn test_snapshot_hash_of() {
        assert_eq!(
            snapshot_hash_of(&format!("snapshot-42-{}.tar.bz2", Hash::default())),
            Some((42, Hash::default(), ArchiveFormat::TarBzip2))
        );
        assert_eq!(
            snapshot_hash_of(&format!("snapshot-43-{}.tar.zst", Hash::default())),
            Some((43, Hash::default(), ArchiveFormat::TarZstd))
        );
        assert_eq!(
            snapshot_hash_of(&format!("snapshot-42-{}.tar", Hash::default())),
            Some((42, Hash::default(), ArchiveFormat::Tar))
        );

        assert!(snapshot_hash_of("invalid").is_none());
    }
}
