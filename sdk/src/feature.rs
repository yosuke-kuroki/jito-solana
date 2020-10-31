use crate::account::Account;
pub use solana_program::feature::*;

pub fn from_account(account: &Account) -> Option<Feature> {
    if account.owner != id() {
        None
    } else {
        bincode::deserialize(&account.data).ok()
    }
}

pub fn to_account(feature: &Feature, account: &mut Account) -> Option<()> {
    bincode::serialize_into(&mut account.data[..], feature).ok()
}

pub fn create_account(feature: &Feature, lamports: u64) -> Account {
    let data_len = Feature::size_of().max(bincode::serialized_size(feature).unwrap() as usize);
    let mut account = Account::new(lamports, data_len, &id());
    to_account(feature, &mut account).unwrap();
    account
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn feature_deserialize_none() {
        let just_initialized = Account::new(42, Feature::size_of(), &id());
        assert_eq!(
            from_account(&just_initialized),
            Some(Feature { activated_at: None })
        );
    }
}
