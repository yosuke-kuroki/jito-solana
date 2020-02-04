use crate::{instruction::InstructionError, instruction_processor_utils::DecodeError};
use num_traits::{FromPrimitive, ToPrimitive};
use thiserror::Error;

#[cfg(feature = "program")]
use crate::info;
#[cfg(not(feature = "program"))]
use log::info;

/// Reasons the program may fail
#[derive(Clone, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
pub enum ProgramError {
    /// Allows on-chain programs to implement program-specific error types and see them returned
    /// by the Solana runtime. A program-specific error may be any type that is represented as
    /// or serialized to a u32 integer.
    #[error("Custom program error: {0}")]
    CustomError(u32),
    #[error("The arguments provided to a program instruction where invalid")]
    InvalidArgument,
    #[error("An instruction's data contents was invalid")]
    InvalidInstructionData,
    #[error("An account's data contents was invalid")]
    InvalidAccountData,
    #[error("An account's data was too small")]
    AccountDataTooSmall,
    #[error("An account's balance was too small to complete the instruction")]
    InsufficientFunds,
    #[error("The account did not have the expected program id")]
    IncorrectProgramId,
    #[error("A signature was required but not found")]
    MissingRequiredSignature,
    #[error("An initialize instruction was sent to an account that has already been initialized")]
    AccountAlreadyInitialized,
    #[error("An attempt to operate on an account that hasn't been initialized")]
    UninitializedAccount,
    #[error("The instruction expected additional account keys")]
    NotEnoughAccountKeys,
    #[error("Failed to borrow a reference to account data, already borrowed")]
    AccountBorrowFailed,
}

pub trait PrintProgramError {
    fn print<E>(&self)
    where
        E: 'static + std::error::Error + DecodeError<E> + PrintProgramError + FromPrimitive;
}

impl PrintProgramError for ProgramError {
    fn print<E>(&self)
    where
        E: 'static + std::error::Error + DecodeError<E> + PrintProgramError + FromPrimitive,
    {
        match self {
            ProgramError::CustomError(error) => {
                if let Some(custom_error) = E::decode_custom_error_to_enum(*error) {
                    custom_error.print::<E>();
                } else {
                    info!("Error: Unknown");
                }
            }
            ProgramError::InvalidArgument => info!("Error: InvalidArgument"),
            ProgramError::InvalidInstructionData => info!("Error: InvalidInstructionData"),
            ProgramError::InvalidAccountData => info!("Error: InvalidAccountData"),
            ProgramError::AccountDataTooSmall => info!("Error: AccountDataTooSmall"),
            ProgramError::InsufficientFunds => info!("Error: InsufficientFunds"),
            ProgramError::IncorrectProgramId => info!("Error: IncorrectProgramId"),
            ProgramError::MissingRequiredSignature => info!("Error: MissingRequiredSignature"),
            ProgramError::AccountAlreadyInitialized => info!("Error: AccountAlreadyInitialized"),
            ProgramError::UninitializedAccount => info!("Error: UninitializedAccount"),
            ProgramError::NotEnoughAccountKeys => info!("Error: NotEnoughAccountKeys"),
            ProgramError::AccountBorrowFailed => info!("Error: AccountBorrowFailed"),
        }
    }
}

/// Builtin return values occupy the upper 32 bits
const BUILTIN_BIT_SHIFT: usize = 32;
macro_rules! to_builtin {
    ($error:expr) => {
        ($error as u64) << BUILTIN_BIT_SHIFT
    };
}

const CUSTOM_ZERO: u64 = to_builtin!(1);
const INVALID_ARGUMENT: u64 = to_builtin!(2);
const INVALID_INSTRUCTION_DATA: u64 = to_builtin!(3);
const INVALID_ACCOUNT_DATA: u64 = to_builtin!(4);
const ACCOUNT_DATA_TOO_SMALL: u64 = to_builtin!(5);
const INSUFFICIENT_FUNDS: u64 = to_builtin!(6);
const INCORRECT_PROGRAM_ID: u64 = to_builtin!(7);
const MISSING_REQUIRED_SIGNATURES: u64 = to_builtin!(8);
const ACCOUNT_ALREADY_INITIALIZED: u64 = to_builtin!(9);
const UNINITIALIZED_ACCOUNT: u64 = to_builtin!(10);
const NOT_ENOUGH_ACCOUNT_KEYS: u64 = to_builtin!(11);
const ACCOUNT_BORROW_FAILED: u64 = to_builtin!(12);

impl From<ProgramError> for u64 {
    fn from(error: ProgramError) -> Self {
        match error {
            ProgramError::InvalidArgument => INVALID_ARGUMENT,
            ProgramError::InvalidInstructionData => INVALID_INSTRUCTION_DATA,
            ProgramError::InvalidAccountData => INVALID_ACCOUNT_DATA,
            ProgramError::AccountDataTooSmall => ACCOUNT_DATA_TOO_SMALL,
            ProgramError::InsufficientFunds => INSUFFICIENT_FUNDS,
            ProgramError::IncorrectProgramId => INCORRECT_PROGRAM_ID,
            ProgramError::MissingRequiredSignature => MISSING_REQUIRED_SIGNATURES,
            ProgramError::AccountAlreadyInitialized => ACCOUNT_ALREADY_INITIALIZED,
            ProgramError::UninitializedAccount => UNINITIALIZED_ACCOUNT,
            ProgramError::NotEnoughAccountKeys => NOT_ENOUGH_ACCOUNT_KEYS,
            ProgramError::AccountBorrowFailed => ACCOUNT_BORROW_FAILED,
            ProgramError::CustomError(error) => {
                if error == 0 {
                    CUSTOM_ZERO
                } else {
                    error as u64
                }
            }
        }
    }
}

impl<T> From<T> for InstructionError
where
    T: ToPrimitive,
{
    fn from(error: T) -> Self {
        let error = error.to_u64().unwrap_or(0xbad_c0de);
        match error {
            CUSTOM_ZERO => InstructionError::CustomError(0),
            INVALID_ARGUMENT => InstructionError::InvalidArgument,
            INVALID_INSTRUCTION_DATA => InstructionError::InvalidInstructionData,
            INVALID_ACCOUNT_DATA => InstructionError::InvalidAccountData,
            ACCOUNT_DATA_TOO_SMALL => InstructionError::AccountDataTooSmall,
            INSUFFICIENT_FUNDS => InstructionError::InsufficientFunds,
            INCORRECT_PROGRAM_ID => InstructionError::IncorrectProgramId,
            MISSING_REQUIRED_SIGNATURES => InstructionError::MissingRequiredSignature,
            ACCOUNT_ALREADY_INITIALIZED => InstructionError::AccountAlreadyInitialized,
            UNINITIALIZED_ACCOUNT => InstructionError::UninitializedAccount,
            NOT_ENOUGH_ACCOUNT_KEYS => InstructionError::NotEnoughAccountKeys,
            ACCOUNT_BORROW_FAILED => InstructionError::AccountBorrowFailed,
            _ => {
                // A valid custom error has no bits set in the upper 32
                if error >> BUILTIN_BIT_SHIFT == 0 {
                    InstructionError::CustomError(error as u32)
                } else {
                    InstructionError::InvalidError
                }
            }
        }
    }
}
