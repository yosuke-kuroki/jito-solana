use crate::{
    account::AccountSharedData,
    account_utils::StateMut,
    fee_calculator::FeeCalculator,
    hash::Hash,
    nonce::{state::Versions, State},
};
use std::cell::RefCell;

pub fn create_account(lamports: u64) -> RefCell<AccountSharedData> {
    RefCell::new(
        AccountSharedData::new_data_with_space(
            lamports,
            &Versions::new_current(State::Uninitialized),
            State::size(),
            &crate::system_program::id(),
        )
        .expect("nonce_account"),
    )
}

pub fn verify_nonce_account(acc: &AccountSharedData, hash: &Hash) -> bool {
    if acc.owner != crate::system_program::id() {
        return false;
    }
    match StateMut::<Versions>::state(acc).map(|v| v.convert_to_current()) {
        Ok(State::Initialized(ref data)) => *hash == data.blockhash,
        _ => false,
    }
}

pub fn fee_calculator_of(account: &AccountSharedData) -> Option<FeeCalculator> {
    let state = StateMut::<Versions>::state(account)
        .ok()?
        .convert_to_current();
    match state {
        State::Initialized(data) => Some(data.fee_calculator),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pubkey::Pubkey;

    #[test]
    fn test_verify_bad_account_owner_fails() {
        let program_id = Pubkey::new_unique();
        assert_ne!(program_id, crate::system_program::id());
        let account = AccountSharedData::new_data_with_space(
            42,
            &Versions::new_current(State::Uninitialized),
            State::size(),
            &program_id,
        )
        .expect("nonce_account");
        assert!(!verify_nonce_account(&account, &Hash::default()));
    }
}
