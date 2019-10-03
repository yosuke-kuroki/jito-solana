//! This account contains the current cluster rent
//!
use crate::account::Account;
use crate::account_info::AccountInfo;
use crate::rent_calculator::RentCalculator;
use crate::sysvar;
use bincode::serialized_size;

///  rent account pubkey
const ID: [u8; 32] = [
    6, 167, 213, 23, 25, 44, 92, 81, 33, 140, 201, 76, 61, 74, 241, 127, 88, 218, 238, 8, 155, 161,
    253, 68, 227, 219, 217, 138, 0, 0, 0, 0,
];

crate::solana_name_id!(ID, "SysvarRent111111111111111111111111111111111");

#[repr(C)]
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Rent {
    pub rent_calculator: RentCalculator,
}

impl Rent {
    pub fn from_account(account: &Account) -> Option<Self> {
        account.deserialize_data().ok()
    }
    pub fn to_account(&self, account: &mut Account) -> Option<()> {
        account.serialize_data(self).ok()
    }
    pub fn from_account_info(account: &AccountInfo) -> Option<Self> {
        account.deserialize_data().ok()
    }
    pub fn to_account_info(&self, account: &mut AccountInfo) -> Option<()> {
        account.serialize_data(self).ok()
    }
    pub fn size_of() -> usize {
        serialized_size(&Rent::default()).unwrap() as usize
    }
}

pub fn create_account(lamports: u64, rent_calculator: &RentCalculator) -> Account {
    Account::new_data(
        lamports,
        &Rent {
            rent_calculator: *rent_calculator,
        },
        &sysvar::id(),
    )
    .unwrap()
}

use crate::account::KeyedAccount;
use crate::instruction::InstructionError;

pub fn from_keyed_account(account: &KeyedAccount) -> Result<Rent, InstructionError> {
    if !check_id(account.unsigned_key()) {
        return Err(InstructionError::InvalidArgument);
    }
    Rent::from_account(account.account).ok_or(InstructionError::InvalidArgument)
}

pub fn verify_rent_exemption(
    account: &KeyedAccount,
    rent_sysvar_account: &KeyedAccount,
) -> Result<(), InstructionError> {
    if !from_keyed_account(rent_sysvar_account)?
        .rent_calculator
        .is_exempt(account.account.lamports, account.account.data.len())
    {
        Err(InstructionError::InsufficientFunds)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rent_create_account() {
        let lamports = 42;
        let account = create_account(lamports, &RentCalculator::default());
        let rent = Rent::from_account(&account).unwrap();
        assert_eq!(rent.rent_calculator, RentCalculator::default());
    }
}
