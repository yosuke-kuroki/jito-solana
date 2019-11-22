use crate::{account::KeyedAccount, instruction::InstructionError, pubkey::Pubkey};
use num_traits::{FromPrimitive, ToPrimitive};

// All native programs export a symbol named process()
pub const ENTRYPOINT: &str = "process";

// Native program ENTRYPOINT prototype
pub type Entrypoint = unsafe extern "C" fn(
    program_id: &Pubkey,
    keyed_accounts: &mut [KeyedAccount],
    data: &[u8],
) -> Result<(), InstructionError>;

// Deprecated
// Convenience macro to define the native program entrypoint.  Supply a fn to this macro that
// conforms to the `Entrypoint` type signature.
#[macro_export]
macro_rules! solana_entrypoint(
    ($entrypoint:ident) => (
        #[no_mangle]
        pub extern "C" fn process(
            program_id: &$crate::pubkey::Pubkey,
            keyed_accounts: &mut [$crate::account::KeyedAccount],
            data: &[u8],
        ) -> Result<(), $crate::instruction::InstructionError> {
            $entrypoint(program_id, keyed_accounts, data)
        }
    )
);

/// Convenience macro to declare a native program
///
/// bs58_string: bs58 string representation the program's id
/// name: Name of the program, must match the library name in Cargo.toml
/// entrypoint: Program's entrypoint, must be of `type Entrypoint`
///
/// # Examples
///
/// ```
/// use solana_sdk::account::KeyedAccount;
/// use solana_sdk::instruction::InstructionError;
/// use solana_sdk::pubkey::Pubkey;
/// use solana_sdk::declare_program;
///
/// fn my_process_instruction(
///     program_id: &Pubkey,
///     keyed_accounts: &mut [KeyedAccount],
///     data: &[u8],
/// ) -> Result<(), InstructionError> {
///   // Process an instruction
///   Ok(())
/// }
///
/// solana_sdk::declare_program!(
///     "My!!!11111111111111111111111111111111111111",
///     solana_my_program,
///     my_process_instruction
/// );
/// ```
#[macro_export]
macro_rules! declare_program(
    ($bs58_string:expr, $name:ident, $entrypoint:expr) => (
        $crate::declare_id!($bs58_string);

        #[macro_export]
        macro_rules! $name {
            () => {
                (stringify!($name).to_string(), $crate::id())
            };
        }

        #[no_mangle]
        pub extern "C" fn $name(
            program_id: &$crate::pubkey::Pubkey,
            keyed_accounts: &mut [$crate::account::KeyedAccount],
            data: &[u8],
        ) -> Result<(), $crate::instruction::InstructionError> {
            $entrypoint(program_id, keyed_accounts, data)
        }
    )
);

impl<T> From<T> for InstructionError
where
    T: ToPrimitive,
{
    fn from(error: T) -> Self {
        InstructionError::CustomError(error.to_u32().unwrap_or(0xbad_c0de))
    }
}

/// Return the next KeyedAccount or a NotEnoughAccountKeys instruction error
pub fn next_keyed_account<I: Iterator>(iter: &mut I) -> Result<I::Item, InstructionError> {
    iter.next().ok_or(InstructionError::NotEnoughAccountKeys)
}

pub fn limited_deserialize<T>(data: &[u8]) -> Result<T, InstructionError>
where
    T: serde::de::DeserializeOwned,
{
    #[cfg(not(feature = "program"))]
    let limit = crate::packet::PACKET_DATA_SIZE as u64;
    #[cfg(feature = "program")]
    let limit = 1024;
    bincode::config()
        .limit(limit)
        .deserialize(data)
        .map_err(|_| InstructionError::InvalidInstructionData)
}

pub trait DecodeError<E> {
    fn decode_custom_error_to_enum(custom: u32) -> Option<E>
    where
        E: FromPrimitive,
    {
        E::from_u32(custom)
    }
    fn type_of() -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_derive::FromPrimitive;

    #[test]
    fn test_decode_custom_error_to_enum() {
        #[derive(Debug, FromPrimitive, PartialEq)]
        enum TestEnum {
            A,
            B,
            C,
        }
        impl<T> DecodeError<T> for TestEnum {
            fn type_of() -> &'static str {
                "TestEnum"
            }
        }
        assert_eq!(TestEnum::decode_custom_error_to_enum(0), Some(TestEnum::A));
        assert_eq!(TestEnum::decode_custom_error_to_enum(1), Some(TestEnum::B));
        assert_eq!(TestEnum::decode_custom_error_to_enum(2), Some(TestEnum::C));
        let option: Option<TestEnum> = TestEnum::decode_custom_error_to_enum(3);
        assert_eq!(option, None);
    }
}
