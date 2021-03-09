use serde::de::{Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};
use solana_sdk::{
    account::Account, account::AccountSharedData, instruction::InstructionError, pubkey::Pubkey,
};
use solana_vote_program::vote_state::VoteState;
use std::{
    borrow::Borrow,
    cmp::Ordering,
    collections::{hash_map::Entry, HashMap},
    iter::FromIterator,
    ops::Deref,
    sync::{Arc, Once, RwLock, RwLockReadGuard},
};

// The value here does not matter. It will be overwritten
// at the first call to VoteAccount::vote_state().
const INVALID_VOTE_STATE: Result<VoteState, InstructionError> =
    Err(InstructionError::InvalidAccountData);

#[derive(Clone, Debug, Default, PartialEq, AbiExample)]
pub struct ArcVoteAccount(Arc<VoteAccount>);

#[derive(Debug, AbiExample)]
pub struct VoteAccount {
    account: Account,
    vote_state: RwLock<Result<VoteState, InstructionError>>,
    vote_state_once: Once,
}

#[derive(Debug, AbiExample)]
pub struct VoteAccounts {
    vote_accounts: HashMap<Pubkey, (u64 /*stake*/, ArcVoteAccount)>,
    staked_nodes: RwLock<
        HashMap<
            Pubkey, // VoteAccount.vote_state.node_pubkey.
            u64,    // Total stake across all vote-accounts.
        >,
    >,
    staked_nodes_once: Once,
}

impl VoteAccount {
    pub fn lamports(&self) -> u64 {
        self.account.lamports
    }

    pub fn vote_state(&self) -> RwLockReadGuard<Result<VoteState, InstructionError>> {
        self.vote_state_once.call_once(|| {
            *self.vote_state.write().unwrap() = VoteState::deserialize(&self.account.data);
        });
        self.vote_state.read().unwrap()
    }

    /// VoteState.node_pubkey of this vote-account.
    fn node_pubkey(&self) -> Option<Pubkey> {
        Some(self.vote_state().as_ref().ok()?.node_pubkey)
    }
}

impl VoteAccounts {
    pub fn staked_nodes(&self) -> HashMap<Pubkey, u64> {
        self.staked_nodes_once.call_once(|| {
            let mut staked_nodes = HashMap::new();
            for (stake, vote_account) in
                self.vote_accounts.values().filter(|(stake, _)| *stake != 0)
            {
                if let Some(node_pubkey) = vote_account.node_pubkey() {
                    staked_nodes
                        .entry(node_pubkey)
                        .and_modify(|s| *s += *stake)
                        .or_insert(*stake);
                }
            }
            *self.staked_nodes.write().unwrap() = staked_nodes
        });
        self.staked_nodes.read().unwrap().clone()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Pubkey, &(u64, ArcVoteAccount))> {
        self.vote_accounts.iter()
    }

    pub fn insert(&mut self, pubkey: Pubkey, (stake, vote_account): (u64, ArcVoteAccount)) {
        self.add_node_stake(stake, &vote_account);
        if let Some((stake, vote_account)) =
            self.vote_accounts.insert(pubkey, (stake, vote_account))
        {
            self.sub_node_stake(stake, &vote_account);
        }
    }

    pub fn remove(&mut self, pubkey: &Pubkey) -> Option<(u64, ArcVoteAccount)> {
        let value = self.vote_accounts.remove(pubkey);
        if let Some((stake, ref vote_account)) = value {
            self.sub_node_stake(stake, vote_account);
        }
        value
    }

    pub fn add_stake(&mut self, pubkey: &Pubkey, delta: u64) {
        if let Some((stake, vote_account)) = self.vote_accounts.get_mut(pubkey) {
            *stake += delta;
            let vote_account = vote_account.clone();
            self.add_node_stake(delta, &vote_account);
        }
    }

    pub fn sub_stake(&mut self, pubkey: &Pubkey, delta: u64) {
        if let Some((stake, vote_account)) = self.vote_accounts.get_mut(pubkey) {
            *stake = stake
                .checked_sub(delta)
                .expect("subtraction value exceeds account's stake");
            let vote_account = vote_account.clone();
            self.sub_node_stake(delta, &vote_account);
        }
    }

    fn add_node_stake(&mut self, stake: u64, vote_account: &ArcVoteAccount) {
        if stake != 0 && self.staked_nodes_once.is_completed() {
            if let Some(node_pubkey) = vote_account.node_pubkey() {
                self.staked_nodes
                    .write()
                    .unwrap()
                    .entry(node_pubkey)
                    .and_modify(|s| *s += stake)
                    .or_insert(stake);
            }
        }
    }

    fn sub_node_stake(&mut self, stake: u64, vote_account: &ArcVoteAccount) {
        if stake != 0 && self.staked_nodes_once.is_completed() {
            if let Some(node_pubkey) = vote_account.node_pubkey() {
                match self.staked_nodes.write().unwrap().entry(node_pubkey) {
                    Entry::Vacant(_) => panic!("this should not happen!"),
                    Entry::Occupied(mut entry) => match entry.get().cmp(&stake) {
                        Ordering::Less => panic!("subtraction value exceeds node's stake"),
                        Ordering::Equal => {
                            entry.remove_entry();
                        }
                        Ordering::Greater => *entry.get_mut() -= stake,
                    },
                }
            }
        }
    }
}

impl Deref for ArcVoteAccount {
    type Target = VoteAccount;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl Serialize for ArcVoteAccount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.account.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ArcVoteAccount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let account = Account::deserialize(deserializer)?;
        Ok(Self::from(account))
    }
}

impl From<AccountSharedData> for ArcVoteAccount {
    fn from(account: AccountSharedData) -> Self {
        Self(Arc::new(VoteAccount::from(account)))
    }
}
impl From<Account> for ArcVoteAccount {
    fn from(account: Account) -> Self {
        Self(Arc::new(VoteAccount::from(account)))
    }
}

impl From<AccountSharedData> for VoteAccount {
    fn from(account: AccountSharedData) -> Self {
        Self {
            account: Account::from(account),
            vote_state: RwLock::new(INVALID_VOTE_STATE),
            vote_state_once: Once::new(),
        }
    }
}

impl From<Account> for VoteAccount {
    fn from(account: Account) -> Self {
        Self {
            account,
            vote_state: RwLock::new(INVALID_VOTE_STATE),
            vote_state_once: Once::new(),
        }
    }
}

impl Default for VoteAccount {
    fn default() -> Self {
        Self {
            account: Account::default(),
            vote_state: RwLock::new(INVALID_VOTE_STATE),
            vote_state_once: Once::new(),
        }
    }
}

impl PartialEq<VoteAccount> for VoteAccount {
    fn eq(&self, other: &Self) -> bool {
        self.account == other.account
    }
}

impl Default for VoteAccounts {
    fn default() -> Self {
        Self {
            vote_accounts: HashMap::default(),
            staked_nodes: RwLock::default(),
            staked_nodes_once: Once::new(),
        }
    }
}

impl Clone for VoteAccounts {
    fn clone(&self) -> Self {
        if self.staked_nodes_once.is_completed() {
            let staked_nodes = self.staked_nodes.read().unwrap().clone();
            let other = Self {
                vote_accounts: self.vote_accounts.clone(),
                staked_nodes: RwLock::new(staked_nodes),
                staked_nodes_once: Once::new(),
            };
            other.staked_nodes_once.call_once(|| {});
            other
        } else {
            Self {
                vote_accounts: self.vote_accounts.clone(),
                staked_nodes: RwLock::default(),
                staked_nodes_once: Once::new(),
            }
        }
    }
}

impl PartialEq<VoteAccounts> for VoteAccounts {
    fn eq(&self, other: &Self) -> bool {
        self.vote_accounts == other.vote_accounts
    }
}

type VoteAccountsHashMap = HashMap<Pubkey, (u64 /*stake*/, ArcVoteAccount)>;

impl From<VoteAccountsHashMap> for VoteAccounts {
    fn from(vote_accounts: VoteAccountsHashMap) -> Self {
        Self {
            vote_accounts,
            staked_nodes: RwLock::default(),
            staked_nodes_once: Once::new(),
        }
    }
}

impl Borrow<VoteAccountsHashMap> for VoteAccounts {
    fn borrow(&self) -> &VoteAccountsHashMap {
        &self.vote_accounts
    }
}

impl FromIterator<(Pubkey, (u64 /*stake*/, ArcVoteAccount))> for VoteAccounts {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = (Pubkey, (u64, ArcVoteAccount))>,
    {
        Self::from(HashMap::from_iter(iter))
    }
}

impl Serialize for VoteAccounts {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.vote_accounts.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for VoteAccounts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let vote_accounts = VoteAccountsHashMap::deserialize(deserializer)?;
        Ok(Self::from(vote_accounts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bincode::Options;
    use rand::Rng;
    use solana_sdk::{pubkey::Pubkey, sysvar::clock::Clock};
    use solana_vote_program::vote_state::{VoteInit, VoteStateVersions};
    use std::iter::repeat_with;

    fn new_rand_vote_account<R: Rng>(
        rng: &mut R,
        node_pubkey: Option<Pubkey>,
    ) -> (AccountSharedData, VoteState) {
        let vote_init = VoteInit {
            node_pubkey: node_pubkey.unwrap_or_else(Pubkey::new_unique),
            authorized_voter: Pubkey::new_unique(),
            authorized_withdrawer: Pubkey::new_unique(),
            commission: rng.gen(),
        };
        let clock = Clock {
            slot: rng.gen(),
            epoch_start_timestamp: rng.gen(),
            epoch: rng.gen(),
            leader_schedule_epoch: rng.gen(),
            unix_timestamp: rng.gen(),
        };
        let vote_state = VoteState::new(&vote_init, &clock);
        let account = AccountSharedData::new_data(
            rng.gen(), // lamports
            &VoteStateVersions::new_current(vote_state.clone()),
            &Pubkey::new_unique(), // owner
        )
        .unwrap();
        (account, vote_state)
    }

    fn new_rand_vote_accounts<R: Rng>(
        rng: &mut R,
        num_nodes: usize,
    ) -> impl Iterator<Item = (Pubkey, (u64 /*stake*/, ArcVoteAccount))> + '_ {
        let nodes: Vec<_> = repeat_with(Pubkey::new_unique).take(num_nodes).collect();
        repeat_with(move || {
            let node = nodes[rng.gen_range(0, nodes.len())];
            let (account, _) = new_rand_vote_account(rng, Some(node));
            let stake = rng.gen_range(0, 997);
            (Pubkey::new_unique(), (stake, ArcVoteAccount::from(account)))
        })
    }

    fn staked_nodes<'a, I>(vote_accounts: I) -> HashMap<Pubkey, u64>
    where
        I: IntoIterator<Item = &'a (Pubkey, (u64, ArcVoteAccount))>,
    {
        let mut staked_nodes = HashMap::new();
        for (_, (stake, vote_account)) in vote_accounts
            .into_iter()
            .filter(|(_, (stake, _))| *stake != 0)
        {
            if let Some(node_pubkey) = vote_account.node_pubkey() {
                staked_nodes
                    .entry(node_pubkey)
                    .and_modify(|s| *s += *stake)
                    .or_insert(*stake);
            }
        }
        staked_nodes
    }

    #[test]
    fn test_vote_account() {
        let mut rng = rand::thread_rng();
        let (account, vote_state) = new_rand_vote_account(&mut rng, None);
        let lamports = account.lamports;
        let vote_account = ArcVoteAccount::from(account);
        assert_eq!(lamports, vote_account.lamports());
        assert_eq!(vote_state, *vote_account.vote_state().as_ref().unwrap());
        // 2nd call to .vote_state() should return the cached value.
        assert_eq!(vote_state, *vote_account.vote_state().as_ref().unwrap());
    }

    #[test]
    fn test_vote_account_serialize() {
        let mut rng = rand::thread_rng();
        let (account, vote_state) = new_rand_vote_account(&mut rng, None);
        let vote_account = ArcVoteAccount::from(account.clone());
        assert_eq!(vote_state, *vote_account.vote_state().as_ref().unwrap());
        // Assert than ArcVoteAccount has the same wire format as Account.
        assert_eq!(
            bincode::serialize(&account).unwrap(),
            bincode::serialize(&vote_account).unwrap()
        );
    }

    #[test]
    fn test_vote_account_deserialize() {
        let mut rng = rand::thread_rng();
        let (account, vote_state) = new_rand_vote_account(&mut rng, None);
        let data = bincode::serialize(&account).unwrap();
        let vote_account = ArcVoteAccount::from(account);
        assert_eq!(vote_state, *vote_account.vote_state().as_ref().unwrap());
        let other_vote_account: ArcVoteAccount = bincode::deserialize(&data).unwrap();
        assert_eq!(vote_account, other_vote_account);
        assert_eq!(
            vote_state,
            *other_vote_account.vote_state().as_ref().unwrap()
        );
    }

    #[test]
    fn test_vote_account_round_trip() {
        let mut rng = rand::thread_rng();
        let (account, vote_state) = new_rand_vote_account(&mut rng, None);
        let vote_account = ArcVoteAccount::from(account);
        assert_eq!(vote_state, *vote_account.vote_state().as_ref().unwrap());
        let data = bincode::serialize(&vote_account).unwrap();
        let other_vote_account: ArcVoteAccount = bincode::deserialize(&data).unwrap();
        // Assert that serialize->deserialized returns the same ArcVoteAccount.
        assert_eq!(vote_account, other_vote_account);
        assert_eq!(
            vote_state,
            *other_vote_account.vote_state().as_ref().unwrap()
        );
    }

    #[test]
    fn test_vote_accounts_serialize() {
        let mut rng = rand::thread_rng();
        let vote_accounts_hash_map: HashMap<Pubkey, (u64, ArcVoteAccount)> =
            new_rand_vote_accounts(&mut rng, 64).take(1024).collect();
        let vote_accounts = VoteAccounts::from(vote_accounts_hash_map.clone());
        assert!(vote_accounts.staked_nodes().len() > 32);
        assert_eq!(
            bincode::serialize(&vote_accounts).unwrap(),
            bincode::serialize(&vote_accounts_hash_map).unwrap(),
        );
        assert_eq!(
            bincode::options().serialize(&vote_accounts).unwrap(),
            bincode::options()
                .serialize(&vote_accounts_hash_map)
                .unwrap(),
        )
    }

    #[test]
    fn test_vote_accounts_deserialize() {
        let mut rng = rand::thread_rng();
        let vote_accounts_hash_map: HashMap<Pubkey, (u64, ArcVoteAccount)> =
            new_rand_vote_accounts(&mut rng, 64).take(1024).collect();
        let data = bincode::serialize(&vote_accounts_hash_map).unwrap();
        let vote_accounts: VoteAccounts = bincode::deserialize(&data).unwrap();
        assert!(vote_accounts.staked_nodes().len() > 32);
        assert_eq!(vote_accounts.vote_accounts, vote_accounts_hash_map);
        let data = bincode::options()
            .serialize(&vote_accounts_hash_map)
            .unwrap();
        let vote_accounts: VoteAccounts = bincode::options().deserialize(&data).unwrap();
        assert_eq!(vote_accounts.vote_accounts, vote_accounts_hash_map);
    }

    #[test]
    fn test_staked_nodes() {
        let mut rng = rand::thread_rng();
        let mut accounts: Vec<_> = new_rand_vote_accounts(&mut rng, 64).take(1024).collect();
        let mut vote_accounts = VoteAccounts::default();
        // Add vote accounts.
        for (k, (pubkey, (stake, vote_account))) in accounts.iter().enumerate() {
            vote_accounts.insert(*pubkey, (*stake, vote_account.clone()));
            if (k + 1) % 128 == 0 {
                assert_eq!(
                    staked_nodes(&accounts[..k + 1]),
                    vote_accounts.staked_nodes()
                );
            }
        }
        // Remove some of the vote accounts.
        for k in 0..256 {
            let index = rng.gen_range(0, accounts.len());
            let (pubkey, (_, _)) = accounts.swap_remove(index);
            vote_accounts.remove(&pubkey);
            if (k + 1) % 32 == 0 {
                assert_eq!(staked_nodes(&accounts), vote_accounts.staked_nodes());
            }
        }
        // Modify the stakes for some of the accounts.
        for k in 0..2048 {
            let index = rng.gen_range(0, accounts.len());
            let (pubkey, (stake, _)) = &mut accounts[index];
            let new_stake = rng.gen_range(0, 997);
            if new_stake < *stake {
                vote_accounts.sub_stake(pubkey, *stake - new_stake);
            } else {
                vote_accounts.add_stake(pubkey, new_stake - *stake);
            }
            *stake = new_stake;
            if (k + 1) % 128 == 0 {
                assert_eq!(staked_nodes(&accounts), vote_accounts.staked_nodes());
            }
        }
        // Remove everything.
        while !accounts.is_empty() {
            let index = rng.gen_range(0, accounts.len());
            let (pubkey, (_, _)) = accounts.swap_remove(index);
            vote_accounts.remove(&pubkey);
            if accounts.len() % 32 == 0 {
                assert_eq!(staked_nodes(&accounts), vote_accounts.staked_nodes());
            }
        }
        assert!(vote_accounts.staked_nodes.read().unwrap().is_empty());
    }
}
