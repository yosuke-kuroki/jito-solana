use crate::transaction::TransactionError;
use std::io;

#[derive(Debug)]
pub enum TransportError {
    IoError(io::Error),
    TransactionError(TransactionError),
}

impl TransportError {
    pub fn unwrap(&self) -> TransactionError {
        if let TransportError::TransactionError(err) = self {
            err.clone()
        } else {
            panic!("unexpected transport error")
        }
    }
}

impl From<io::Error> for TransportError {
    fn from(err: io::Error) -> TransportError {
        TransportError::IoError(err)
    }
}

impl From<TransactionError> for TransportError {
    fn from(err: TransactionError) -> TransportError {
        TransportError::TransactionError(err)
    }
}

pub type Result<T> = std::result::Result<T, TransportError>;
