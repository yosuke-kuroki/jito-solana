//! The `result` module exposes a Result type that propagates one of many different Error types.

use crate::poh_recorder;
use crate::{cluster_info, duplicate_shred};
use solana_ledger::block_error;
use solana_ledger::blockstore;
use solana_runtime::snapshot_utils;
use solana_sdk::transaction;
use std::any::Any;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Json(serde_json::Error),
    AddrParse(std::net::AddrParseError),
    JoinError(Box<dyn Any + Send + 'static>),
    RecvError(std::sync::mpsc::RecvError),
    TryCrossbeamRecvError(crossbeam_channel::TryRecvError),
    CrossbeamRecvTimeoutError(crossbeam_channel::RecvTimeoutError),
    ReadyTimeoutError,
    RecvTimeoutError(std::sync::mpsc::RecvTimeoutError),
    CrossbeamSendError,
    TryCrossbeamSendError,
    TryRecvError(std::sync::mpsc::TryRecvError),
    Serialize(std::boxed::Box<bincode::ErrorKind>),
    TransactionError(transaction::TransactionError),
    ClusterInfoError(cluster_info::ClusterInfoError),
    SendError,
    PohRecorderError(poh_recorder::PohRecorderError),
    BlockError(block_error::BlockError),
    BlockstoreError(blockstore::BlockstoreError),
    FsExtra(fs_extra::error::Error),
    SnapshotError(snapshot_utils::SnapshotError),
    WeightedIndexError(rand::distributions::weighted::WeightedError),
    DuplicateNodeInstance,
    DuplicateShredError(duplicate_shred::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "solana error")
    }
}

impl std::error::Error for Error {}

impl std::convert::From<std::sync::mpsc::RecvError> for Error {
    fn from(e: std::sync::mpsc::RecvError) -> Error {
        Error::RecvError(e)
    }
}
impl std::convert::From<crossbeam_channel::TryRecvError> for Error {
    fn from(e: crossbeam_channel::TryRecvError) -> Error {
        Error::TryCrossbeamRecvError(e)
    }
}
impl std::convert::From<std::sync::mpsc::TryRecvError> for Error {
    fn from(e: std::sync::mpsc::TryRecvError) -> Error {
        Error::TryRecvError(e)
    }
}
impl std::convert::From<crossbeam_channel::RecvTimeoutError> for Error {
    fn from(e: crossbeam_channel::RecvTimeoutError) -> Error {
        Error::CrossbeamRecvTimeoutError(e)
    }
}
impl std::convert::From<crossbeam_channel::ReadyTimeoutError> for Error {
    fn from(_e: crossbeam_channel::ReadyTimeoutError) -> Error {
        Error::ReadyTimeoutError
    }
}
impl std::convert::From<std::sync::mpsc::RecvTimeoutError> for Error {
    fn from(e: std::sync::mpsc::RecvTimeoutError) -> Error {
        Error::RecvTimeoutError(e)
    }
}
impl std::convert::From<transaction::TransactionError> for Error {
    fn from(e: transaction::TransactionError) -> Error {
        Error::TransactionError(e)
    }
}
impl std::convert::From<cluster_info::ClusterInfoError> for Error {
    fn from(e: cluster_info::ClusterInfoError) -> Error {
        Error::ClusterInfoError(e)
    }
}
impl<T> std::convert::From<crossbeam_channel::SendError<T>> for Error {
    fn from(_e: crossbeam_channel::SendError<T>) -> Error {
        Error::CrossbeamSendError
    }
}
impl<T> std::convert::From<crossbeam_channel::TrySendError<T>> for Error {
    fn from(_e: crossbeam_channel::TrySendError<T>) -> Error {
        Error::TryCrossbeamSendError
    }
}
impl<T> std::convert::From<std::sync::mpsc::SendError<T>> for Error {
    fn from(_e: std::sync::mpsc::SendError<T>) -> Error {
        Error::SendError
    }
}
impl std::convert::From<Box<dyn Any + Send + 'static>> for Error {
    fn from(e: Box<dyn Any + Send + 'static>) -> Error {
        Error::JoinError(e)
    }
}
impl std::convert::From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}
impl std::convert::From<fs_extra::error::Error> for Error {
    fn from(e: fs_extra::error::Error) -> Error {
        Error::FsExtra(e)
    }
}
impl std::convert::From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::Json(e)
    }
}
impl std::convert::From<std::net::AddrParseError> for Error {
    fn from(e: std::net::AddrParseError) -> Error {
        Error::AddrParse(e)
    }
}
impl std::convert::From<std::boxed::Box<bincode::ErrorKind>> for Error {
    fn from(e: std::boxed::Box<bincode::ErrorKind>) -> Error {
        Error::Serialize(e)
    }
}
impl std::convert::From<poh_recorder::PohRecorderError> for Error {
    fn from(e: poh_recorder::PohRecorderError) -> Error {
        Error::PohRecorderError(e)
    }
}
impl std::convert::From<blockstore::BlockstoreError> for Error {
    fn from(e: blockstore::BlockstoreError) -> Error {
        Error::BlockstoreError(e)
    }
}
impl std::convert::From<snapshot_utils::SnapshotError> for Error {
    fn from(e: snapshot_utils::SnapshotError) -> Error {
        Error::SnapshotError(e)
    }
}
impl std::convert::From<rand::distributions::weighted::WeightedError> for Error {
    fn from(e: rand::distributions::weighted::WeightedError) -> Error {
        Error::WeightedIndexError(e)
    }
}
impl std::convert::From<duplicate_shred::Error> for Error {
    fn from(e: duplicate_shred::Error) -> Error {
        Error::DuplicateShredError(e)
    }
}

#[cfg(test)]
mod tests {
    use crate::result::Error;
    use crate::result::Result;
    use std::io;
    use std::io::Write;
    use std::net::SocketAddr;
    use std::panic;
    use std::sync::mpsc::channel;
    use std::sync::mpsc::RecvError;
    use std::sync::mpsc::RecvTimeoutError;
    use std::thread;

    fn addr_parse_error() -> Result<SocketAddr> {
        Ok("12fdfasfsafsadfs".parse()?)
    }

    fn join_error() -> Result<()> {
        panic::set_hook(Box::new(|_info| {}));
        Ok(thread::spawn(|| panic!("hi")).join()?)
    }
    fn json_error() -> Result<()> {
        Ok(serde_json::from_slice(b"=342{;;;;:}")?)
    }
    fn send_error() -> Result<()> {
        let (s, r) = channel();
        drop(r);
        s.send(())?;
        Ok(())
    }

    #[test]
    fn from_test() {
        assert_matches!(addr_parse_error(), Err(Error::AddrParse(_)));
        assert_matches!(Error::from(RecvError {}), Error::RecvError(_));
        assert_matches!(
            Error::from(RecvTimeoutError::Timeout),
            Error::RecvTimeoutError(_)
        );
        assert_matches!(send_error(), Err(Error::SendError));
        assert_matches!(join_error(), Err(Error::JoinError(_)));
        let ioe = io::Error::new(io::ErrorKind::NotFound, "hi");
        assert_matches!(Error::from(ioe), Error::Io(_));
    }
    #[test]
    fn fmt_test() {
        write!(io::sink(), "{:?}", addr_parse_error()).unwrap();
        write!(io::sink(), "{:?}", Error::from(RecvError {})).unwrap();
        write!(io::sink(), "{:?}", Error::from(RecvTimeoutError::Timeout)).unwrap();
        write!(io::sink(), "{:?}", send_error()).unwrap();
        write!(io::sink(), "{:?}", join_error()).unwrap();
        write!(io::sink(), "{:?}", json_error()).unwrap();
        write!(
            io::sink(),
            "{:?}",
            Error::from(io::Error::new(io::ErrorKind::NotFound, "hi"))
        )
        .unwrap();
    }
}
