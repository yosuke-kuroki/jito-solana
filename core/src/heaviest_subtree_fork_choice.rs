use crate::{
    consensus::Tower, fork_choice::ForkChoice,
    latest_validator_votes_for_frozen_banks::LatestValidatorVotesForFrozenBanks,
    progress_map::ProgressMap, tree_diff::TreeDiff,
};
use solana_measure::measure::Measure;
use solana_runtime::{bank::Bank, bank_forks::BankForks, epoch_stakes::EpochStakes};
use solana_sdk::{
    clock::{Epoch, Slot},
    epoch_schedule::EpochSchedule,
    hash::Hash,
    pubkey::Pubkey,
};
use std::{
    borrow::Borrow,
    collections::{hash_map::Entry, BTreeMap, HashMap, HashSet, VecDeque},
    sync::{Arc, RwLock},
    time::Instant,
};
#[cfg(test)]
use trees::{Tree, TreeWalk};

pub type ForkWeight = u64;
pub type SlotHashKey = (Slot, Hash);
type UpdateOperations = BTreeMap<(SlotHashKey, UpdateLabel), UpdateOperation>;

const MAX_ROOT_PRINT_SECONDS: u64 = 30;

#[derive(PartialEq, Eq, Clone, Debug, PartialOrd, Ord)]
enum UpdateLabel {
    Aggregate,
    Add,
    MarkValid,
    Subtract,
}

pub trait GetSlotHash {
    fn slot_hash(&self) -> SlotHashKey;
}

impl GetSlotHash for SlotHashKey {
    fn slot_hash(&self) -> SlotHashKey {
        *self
    }
}

impl GetSlotHash for Slot {
    fn slot_hash(&self) -> SlotHashKey {
        (*self, Hash::default())
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
enum UpdateOperation {
    Add(u64),
    MarkValid,
    Subtract(u64),
    Aggregate,
}

impl UpdateOperation {
    fn update_stake(&mut self, new_stake: u64) {
        match self {
            Self::Aggregate => panic!("Should not get here"),
            Self::Add(stake) => *stake += new_stake,
            Self::MarkValid => panic!("Should not get here"),
            Self::Subtract(stake) => *stake += new_stake,
        }
    }
}

struct ForkInfo {
    // Amount of stake that has voted for exactly this slot
    stake_voted_at: ForkWeight,
    // Amount of stake that has voted for this slot and the subtree
    // rooted at this slot
    stake_voted_subtree: ForkWeight,
    // Best slot in the subtree rooted at this slot, does not
    // have to be a direct child in `children`
    best_slot: SlotHashKey,
    parent: Option<SlotHashKey>,
    children: Vec<SlotHashKey>,
    // Whether the fork rooted at this slot is a valid contender
    // for the best fork
    is_candidate: bool,
}

pub struct HeaviestSubtreeForkChoice {
    fork_infos: HashMap<SlotHashKey, ForkInfo>,
    latest_votes: HashMap<Pubkey, SlotHashKey>,
    root: SlotHashKey,
    last_root_time: Instant,
}

impl HeaviestSubtreeForkChoice {
    pub(crate) fn new(root: SlotHashKey) -> Self {
        let mut heaviest_subtree_fork_choice = Self {
            root,
            // Doesn't implement default because `root` must
            // exist in all the fields
            fork_infos: HashMap::new(),
            latest_votes: HashMap::new(),
            last_root_time: Instant::now(),
        };
        heaviest_subtree_fork_choice.add_new_leaf_slot(root, None);
        heaviest_subtree_fork_choice
    }

    // Given a root and a list of `frozen_banks` sorted smallest to greatest by slot,
    // return a new HeaviestSubtreeForkChoice
    pub(crate) fn new_from_frozen_banks(root: SlotHashKey, frozen_banks: &[Arc<Bank>]) -> Self {
        let mut heaviest_subtree_fork_choice = HeaviestSubtreeForkChoice::new(root);
        let mut prev_slot = root.0;
        for bank in frozen_banks.iter() {
            assert!(bank.is_frozen());
            if bank.slot() > root.0 {
                // Make sure the list is sorted
                assert!(bank.slot() > prev_slot);
                prev_slot = bank.slot();
                let bank_hash = bank.hash();
                assert_ne!(bank_hash, Hash::default());
                let parent_bank_hash = bank.parent_hash();
                assert_ne!(parent_bank_hash, Hash::default());
                heaviest_subtree_fork_choice.add_new_leaf_slot(
                    (bank.slot(), bank_hash),
                    Some((bank.parent_slot(), parent_bank_hash)),
                );
            }
        }

        heaviest_subtree_fork_choice
    }

    #[cfg(test)]
    pub(crate) fn new_from_bank_forks(bank_forks: &BankForks) -> Self {
        let mut frozen_banks: Vec<_> = bank_forks.frozen_banks().values().cloned().collect();

        frozen_banks.sort_by_key(|bank| bank.slot());
        let root_bank = bank_forks.root_bank();
        Self::new_from_frozen_banks((root_bank.slot(), root_bank.hash()), &frozen_banks)
    }

    #[cfg(test)]
    pub(crate) fn new_from_tree<T: GetSlotHash>(forks: Tree<T>) -> Self {
        let root = forks.root().data.slot_hash();
        let mut walk = TreeWalk::from(forks);
        let mut heaviest_subtree_fork_choice = HeaviestSubtreeForkChoice::new(root);
        while let Some(visit) = walk.get() {
            let slot_hash = visit.node().data.slot_hash();
            if heaviest_subtree_fork_choice
                .fork_infos
                .contains_key(&slot_hash)
            {
                walk.forward();
                continue;
            }
            let parent_slot_hash = walk.get_parent().map(|n| n.data.slot_hash());
            heaviest_subtree_fork_choice.add_new_leaf_slot(slot_hash, parent_slot_hash);
            walk.forward();
        }

        heaviest_subtree_fork_choice
    }

    pub fn contains_block(&self, key: &SlotHashKey) -> bool {
        self.fork_infos.contains_key(key)
    }

    pub fn best_slot(&self, key: &SlotHashKey) -> Option<SlotHashKey> {
        self.fork_infos
            .get(key)
            .map(|fork_info| fork_info.best_slot)
    }

    pub fn best_overall_slot(&self) -> SlotHashKey {
        self.best_slot(&self.root).unwrap()
    }

    pub fn stake_voted_subtree(&self, key: &SlotHashKey) -> Option<u64> {
        self.fork_infos
            .get(key)
            .map(|fork_info| fork_info.stake_voted_subtree)
    }

    pub fn is_candidate_slot(&self, key: &SlotHashKey) -> Option<bool> {
        self.fork_infos
            .get(key)
            .map(|fork_info| fork_info.is_candidate)
    }

    pub fn root(&self) -> SlotHashKey {
        self.root
    }

    pub fn max_by_weight(&self, slot1: SlotHashKey, slot2: SlotHashKey) -> std::cmp::Ordering {
        let weight1 = self.stake_voted_subtree(&slot1).unwrap();
        let weight2 = self.stake_voted_subtree(&slot2).unwrap();
        if weight1 == weight2 {
            slot1.cmp(&slot2).reverse()
        } else {
            weight1.cmp(&weight2)
        }
    }

    // Add new votes, returns the best slot
    pub fn add_votes<'a, 'b>(
        &'a mut self,
        // newly updated votes on a fork
        pubkey_votes: impl Iterator<Item = impl Borrow<(Pubkey, SlotHashKey)> + 'b>,
        epoch_stakes: &HashMap<Epoch, EpochStakes>,
        epoch_schedule: &EpochSchedule,
    ) -> SlotHashKey {
        // Generate the set of updates
        let update_operations_batch =
            self.generate_update_operations(pubkey_votes, epoch_stakes, epoch_schedule);

        // Finalize all updates
        self.process_update_operations(update_operations_batch);
        self.best_overall_slot()
    }

    pub fn set_root(&mut self, new_root: SlotHashKey) {
        // Remove everything reachable from `self.root` but not `new_root`,
        // as those are now unrooted.
        let remove_set = self.subtree_diff(self.root, new_root);
        for node_key in remove_set {
            self.fork_infos
                .remove(&node_key)
                .expect("Slots reachable from old root must exist in tree");
        }
        let root_fork_info = self.fork_infos.get_mut(&new_root);

        root_fork_info
            .unwrap_or_else(|| panic!("New root: {:?}, didn't exist in fork choice", new_root))
            .parent = None;
        self.root = new_root;
        self.last_root_time = Instant::now();
    }

    pub fn add_root_parent(&mut self, root_parent: SlotHashKey) {
        assert!(root_parent.0 < self.root.0);
        assert!(self.fork_infos.get(&root_parent).is_none());
        let root_info = self
            .fork_infos
            .get_mut(&self.root)
            .expect("entry for root must exist");
        root_info.parent = Some(root_parent);
        let root_parent_info = ForkInfo {
            stake_voted_at: 0,
            stake_voted_subtree: root_info.stake_voted_subtree,
            // The `best_slot` of a leaf is itself
            best_slot: root_info.best_slot,
            children: vec![self.root],
            parent: None,
            is_candidate: true,
        };
        self.fork_infos.insert(root_parent, root_parent_info);
        self.root = root_parent;
    }

    pub fn add_new_leaf_slot(&mut self, slot: SlotHashKey, parent: Option<SlotHashKey>) {
        if self.last_root_time.elapsed().as_secs() > MAX_ROOT_PRINT_SECONDS {
            self.print_state();
            self.last_root_time = Instant::now();
        }

        if self.fork_infos.contains_key(&slot) {
            // Can potentially happen if we repair the same version of the duplicate slot, after
            // dumping the original version
            return;
        }

        self.fork_infos
            .entry(slot)
            .and_modify(|slot_info| slot_info.parent = parent)
            .or_insert(ForkInfo {
                stake_voted_at: 0,
                stake_voted_subtree: 0,
                // The `best_slot` of a leaf is itself
                best_slot: slot,
                children: vec![],
                parent,
                is_candidate: true,
            });

        if parent.is_none() {
            return;
        }

        let parent = parent.unwrap();

        // Parent must already exist by time child is being added
        self.fork_infos
            .get_mut(&parent)
            .unwrap()
            .children
            .push(slot);

        // Propagate leaf up the tree to any ancestors who considered the previous leaf
        // the `best_slot`
        self.propagate_new_leaf(&slot, &parent)
    }

    // Returns if the given `maybe_best_child` is the heaviest among the children
    // it's parent
    fn is_best_child(&self, maybe_best_child: &SlotHashKey) -> bool {
        let maybe_best_child_weight = self.stake_voted_subtree(maybe_best_child).unwrap();
        let parent = self.parent(maybe_best_child);
        // If there's no parent, this must be the root
        if parent.is_none() {
            return true;
        }
        for child in self.children(&parent.unwrap()).unwrap() {
            let child_weight = self
                .stake_voted_subtree(child)
                .expect("child must exist in `self.fork_infos`");

            // Don't count children currently marked as invalid
            if !self
                .is_candidate_slot(child)
                .expect("child must exist in tree")
            {
                continue;
            }

            if child_weight > maybe_best_child_weight
                || (maybe_best_child_weight == child_weight && *child < *maybe_best_child)
            {
                return false;
            }
        }

        true
    }

    pub fn all_slots_stake_voted_subtree(&self) -> impl Iterator<Item = (&SlotHashKey, u64)> {
        self.fork_infos
            .iter()
            .map(|(slot_hash, fork_info)| (slot_hash, fork_info.stake_voted_subtree))
    }

    #[cfg(test)]
    pub fn ancestors(&self, start_slot_hash_key: SlotHashKey) -> Vec<SlotHashKey> {
        AncestorIterator::new(start_slot_hash_key, &self.fork_infos).collect()
    }

    pub fn merge(
        &mut self,
        other: HeaviestSubtreeForkChoice,
        merge_leaf: &SlotHashKey,
        epoch_stakes: &HashMap<Epoch, EpochStakes>,
        epoch_schedule: &EpochSchedule,
    ) {
        assert!(self.fork_infos.contains_key(merge_leaf));

        // Add all the nodes from `other` into our tree
        let mut other_slots_nodes: Vec<_> = other
            .fork_infos
            .iter()
            .map(|(slot_hash_key, fork_info)| {
                (slot_hash_key, fork_info.parent.unwrap_or(*merge_leaf))
            })
            .collect();

        other_slots_nodes.sort_by_key(|(slot_hash_key, _)| *slot_hash_key);
        for (slot_hash_key, parent) in other_slots_nodes {
            self.add_new_leaf_slot(*slot_hash_key, Some(parent));
        }

        // Add all votes, the outdated ones should be filtered out by
        // self.add_votes()
        self.add_votes(other.latest_votes.into_iter(), epoch_stakes, epoch_schedule);
    }

    pub fn stake_voted_at(&self, slot: &SlotHashKey) -> Option<u64> {
        self.fork_infos
            .get(slot)
            .map(|fork_info| fork_info.stake_voted_at)
    }

    fn propagate_new_leaf(
        &mut self,
        slot_hash_key: &SlotHashKey,
        parent_slot_hash_key: &SlotHashKey,
    ) {
        let parent_best_slot_hash_key = self
            .best_slot(parent_slot_hash_key)
            .expect("parent must exist in self.fork_infos after its child leaf was created");

        // If this new leaf is the direct parent's best child, then propagate
        // it up the tree
        if self.is_best_child(slot_hash_key) {
            let mut ancestor = Some(*parent_slot_hash_key);
            loop {
                if ancestor.is_none() {
                    break;
                }
                let ancestor_fork_info = self.fork_infos.get_mut(&ancestor.unwrap()).unwrap();
                if ancestor_fork_info.best_slot == parent_best_slot_hash_key {
                    ancestor_fork_info.best_slot = *slot_hash_key;
                } else {
                    break;
                }
                ancestor = ancestor_fork_info.parent;
            }
        }
    }

    fn insert_mark_valid_aggregate_operations(
        &self,
        update_operations: &mut BTreeMap<(SlotHashKey, UpdateLabel), UpdateOperation>,
        slot_hash_key: SlotHashKey,
    ) {
        self.do_insert_aggregate_operations(update_operations, true, slot_hash_key);
    }

    fn insert_aggregate_operations(
        &self,
        update_operations: &mut BTreeMap<(SlotHashKey, UpdateLabel), UpdateOperation>,
        slot_hash_key: SlotHashKey,
    ) {
        self.do_insert_aggregate_operations(update_operations, false, slot_hash_key);
    }

    #[allow(clippy::map_entry)]
    fn do_insert_aggregate_operations(
        &self,
        update_operations: &mut BTreeMap<(SlotHashKey, UpdateLabel), UpdateOperation>,
        should_mark_valid: bool,
        slot_hash_key: SlotHashKey,
    ) {
        for parent_slot_hash_key in self.ancestor_iterator(slot_hash_key) {
            let aggregate_label = (parent_slot_hash_key, UpdateLabel::Aggregate);
            if update_operations.contains_key(&aggregate_label) {
                break;
            } else {
                if should_mark_valid {
                    update_operations.insert(
                        (parent_slot_hash_key, UpdateLabel::MarkValid),
                        UpdateOperation::MarkValid,
                    );
                }
                update_operations.insert(aggregate_label, UpdateOperation::Aggregate);
            }
        }
    }

    fn ancestor_iterator(&self, start_slot_hash_key: SlotHashKey) -> AncestorIterator {
        AncestorIterator::new(start_slot_hash_key, &self.fork_infos)
    }

    fn aggregate_slot(&mut self, slot_hash_key: SlotHashKey) {
        let mut stake_voted_subtree;
        let mut best_slot_hash_key = slot_hash_key;
        if let Some(fork_info) = self.fork_infos.get(&slot_hash_key) {
            stake_voted_subtree = fork_info.stake_voted_at;
            let mut best_child_stake_voted_subtree = 0;
            let mut best_child_slot = slot_hash_key;
            for child in &fork_info.children {
                let child_stake_voted_subtree = self.stake_voted_subtree(child).unwrap();

                // Child forks that are not candidates still contribute to the weight
                // of the subtree rooted at `slot_hash_key`. For instance:
                /*
                    Build fork structure:
                          slot 0
                            |
                          slot 1
                          /    \
                    slot 2     |
                        |     slot 3 (34%)
                slot 4 (66%)

                    If slot 4 is a duplicate slot, so no longer qualifies as a candidate until
                    the slot is confirmed, the weight of votes on slot 4 should still count towards
                    slot 2, otherwise we might pick slot 3 as the heaviest fork to build blocks on
                    instead of slot 2.
                */

                // See comment above for why this check is outside of the `is_candidate` check.
                stake_voted_subtree += child_stake_voted_subtree;

                // Note: If there's no valid children, then the best slot should default to the
                // input `slot` itself.
                if self
                    .is_candidate_slot(child)
                    .expect("Child must exist in fork_info map")
                    && (best_child_slot == slot_hash_key ||
                    child_stake_voted_subtree > best_child_stake_voted_subtree ||
                // tiebreaker by slot height, prioritize earlier slot
                (child_stake_voted_subtree == best_child_stake_voted_subtree && child < &best_child_slot))
                {
                    best_child_stake_voted_subtree = child_stake_voted_subtree;
                    best_child_slot = *child;
                    best_slot_hash_key = self
                        .best_slot(child)
                        .expect("`child` must exist in `self.fork_infos`");
                }
            }
        } else {
            return;
        }

        let fork_info = self.fork_infos.get_mut(&slot_hash_key).unwrap();
        fork_info.stake_voted_subtree = stake_voted_subtree;
        fork_info.best_slot = best_slot_hash_key;
    }

    fn mark_slot_valid(&mut self, valid_slot_hash_key: (Slot, Hash)) {
        if let Some(fork_info) = self.fork_infos.get_mut(&valid_slot_hash_key) {
            if !fork_info.is_candidate {
                info!(
                    "marked previously invalid fork starting at slot: {:?} as valid",
                    valid_slot_hash_key
                );
            }
            fork_info.is_candidate = true;
        }
    }

    fn generate_update_operations<'a, 'b>(
        &'a mut self,
        pubkey_votes: impl Iterator<Item = impl Borrow<(Pubkey, SlotHashKey)> + 'b>,
        epoch_stakes: &HashMap<Epoch, EpochStakes>,
        epoch_schedule: &EpochSchedule,
    ) -> UpdateOperations {
        let mut update_operations: BTreeMap<(SlotHashKey, UpdateLabel), UpdateOperation> =
            BTreeMap::new();
        let mut observed_pubkeys: HashMap<Pubkey, Slot> = HashMap::new();
        // Sort the `pubkey_votes` in a BTreeMap by the slot voted
        for pubkey_vote in pubkey_votes {
            let (pubkey, new_vote_slot_hash) = pubkey_vote.borrow();
            let (new_vote_slot, new_vote_hash) = *new_vote_slot_hash;
            if new_vote_slot < self.root.0 {
                // If the new vote is less than the root we can ignore it. This is because there
                // are two cases. Either:
                // 1) The validator's latest vote was bigger than the new vote, so we can ignore it
                // 2) The validator's latest vote was less than the new vote, then the validator's latest
                // vote was also less than root. This means either every node in the current tree has the
                // validators stake counted toward it (if the latest vote was an ancestor of the current root),
                // OR every node doesn't have this validator's vote counting toward it (if the latest vote
                // was not an ancestor of the current root). Thus this validator is essentially a no-op
                // and won't affect fork choice.
                continue;
            }

            // A pubkey cannot appear in pubkey votes more than once.
            match observed_pubkeys.entry(*pubkey) {
                Entry::Occupied(_occupied_entry) => {
                    panic!("Should not get multiple votes for same pubkey in the same batch");
                }
                Entry::Vacant(vacant_entry) => {
                    vacant_entry.insert(new_vote_slot);
                    false
                }
            };

            let mut pubkey_latest_vote = self.latest_votes.get_mut(pubkey);

            // Filter out any votes or slots < any slot this pubkey has
            // already voted for, we only care about the latest votes.
            //
            // If the new vote is for the same slot, but a different, smaller hash,
            // then allow processing to continue as this is a duplicate version
            // of the same slot.
            match pubkey_latest_vote.as_mut() {
                Some((pubkey_latest_vote_slot, pubkey_latest_vote_hash))
                    if (new_vote_slot < *pubkey_latest_vote_slot)
                        || (new_vote_slot == *pubkey_latest_vote_slot
                            && &new_vote_hash >= pubkey_latest_vote_hash) =>
                {
                    continue;
                }

                _ => {
                    // We either:
                    // 1) don't have a vote yet for this pubkey,
                    // 2) or the new vote slot is bigger than the old vote slot
                    // 3) or the new vote slot == old_vote slot, but for a smaller bank hash.
                    // In all above cases, we need to remove this pubkey stake from the previous fork
                    // of the previous vote

                    if let Some((old_latest_vote_slot, old_latest_vote_hash)) =
                        self.latest_votes.insert(*pubkey, *new_vote_slot_hash)
                    {
                        assert!(if new_vote_slot == old_latest_vote_slot {
                            warn!(
                                "Got a duplicate vote for
                                    validator: {},
                                    slot_hash: {:?}",
                                pubkey, new_vote_slot_hash
                            );
                            // If the slots are equal, then the new
                            // vote must be for a smaller hash
                            new_vote_hash < old_latest_vote_hash
                        } else {
                            new_vote_slot > old_latest_vote_slot
                        });

                        let epoch = epoch_schedule.get_epoch(old_latest_vote_slot);
                        let stake_update = epoch_stakes
                            .get(&epoch)
                            .map(|epoch_stakes| epoch_stakes.vote_account_stake(pubkey))
                            .unwrap_or(0);

                        if stake_update > 0 {
                            update_operations
                                .entry((
                                    (old_latest_vote_slot, old_latest_vote_hash),
                                    UpdateLabel::Subtract,
                                ))
                                .and_modify(|update| update.update_stake(stake_update))
                                .or_insert(UpdateOperation::Subtract(stake_update));
                            self.insert_aggregate_operations(
                                &mut update_operations,
                                (old_latest_vote_slot, old_latest_vote_hash),
                            );
                        }
                    }
                }
            }

            // Add this pubkey stake to new fork
            let epoch = epoch_schedule.get_epoch(new_vote_slot_hash.0);
            let stake_update = epoch_stakes
                .get(&epoch)
                .map(|epoch_stakes| epoch_stakes.vote_account_stake(&pubkey))
                .unwrap_or(0);

            update_operations
                .entry((*new_vote_slot_hash, UpdateLabel::Add))
                .and_modify(|update| update.update_stake(stake_update))
                .or_insert(UpdateOperation::Add(stake_update));
            self.insert_aggregate_operations(&mut update_operations, *new_vote_slot_hash);
        }

        update_operations
    }

    fn process_update_operations(&mut self, update_operations: UpdateOperations) {
        // Iterate through the update operations from greatest to smallest slot
        for ((slot_hash_key, _), operation) in update_operations.into_iter().rev() {
            match operation {
                UpdateOperation::MarkValid => self.mark_slot_valid(slot_hash_key),
                UpdateOperation::Aggregate => self.aggregate_slot(slot_hash_key),
                UpdateOperation::Add(stake) => self.add_slot_stake(&slot_hash_key, stake),
                UpdateOperation::Subtract(stake) => self.subtract_slot_stake(&slot_hash_key, stake),
            }
        }
    }

    fn add_slot_stake(&mut self, slot_hash_key: &SlotHashKey, stake: u64) {
        if let Some(fork_info) = self.fork_infos.get_mut(slot_hash_key) {
            fork_info.stake_voted_at += stake;
            fork_info.stake_voted_subtree += stake;
        }
    }

    fn subtract_slot_stake(&mut self, slot_hash_key: &SlotHashKey, stake: u64) {
        if let Some(fork_info) = self.fork_infos.get_mut(slot_hash_key) {
            fork_info.stake_voted_at -= stake;
            fork_info.stake_voted_subtree -= stake;
        }
    }

    fn parent(&self, slot_hash_key: &SlotHashKey) -> Option<SlotHashKey> {
        self.fork_infos
            .get(slot_hash_key)
            .map(|fork_info| fork_info.parent)
            .unwrap_or(None)
    }

    fn print_state(&self) {
        let best_slot_hash_key = self.best_overall_slot();
        let mut best_path: VecDeque<_> = self.ancestor_iterator(best_slot_hash_key).collect();
        best_path.push_front(best_slot_hash_key);
        info!(
            "Latest known votes by vote pubkey: {:#?}, best path: {:?}",
            self.latest_votes,
            best_path.iter().rev().collect::<Vec<&SlotHashKey>>()
        );
    }

    fn heaviest_slot_on_same_voted_fork(&self, tower: &Tower) -> Option<SlotHashKey> {
        tower
            .last_voted_slot_hash()
            .and_then(|last_voted_slot_hash| {
                let heaviest_slot_hash_on_same_voted_fork = self.best_slot(&last_voted_slot_hash);
                if heaviest_slot_hash_on_same_voted_fork.is_none() {
                    if !tower.is_stray_last_vote() {
                        // Unless last vote is stray and stale, self.bast_slot(last_voted_slot) must return
                        // Some(_), justifying to panic! here.
                        // Also, adjust_lockouts_after_replay() correctly makes last_voted_slot None,
                        // if all saved votes are ancestors of replayed_root_slot. So this code shouldn't be
                        // touched in that case as well.
                        // In other words, except being stray, all other slots have been voted on while this
                        // validator has been running, so we must be able to fetch best_slots for all of
                        // them.
                        panic!(
                            "a bank at last_voted_slot({:?}) is a frozen bank so must have been \
                            added to heaviest_subtree_fork_choice at time of freezing",
                            last_voted_slot_hash,
                        )
                    } else {
                        // fork_infos doesn't have corresponding data for the stale stray last vote,
                        // meaning some inconsistency between saved tower and ledger.
                        // (newer snapshot, or only a saved tower is moved over to new setup?)
                        return None;
                    }
                }
                let heaviest_slot_hash_on_same_voted_fork =
                    heaviest_slot_hash_on_same_voted_fork.unwrap();

                if heaviest_slot_hash_on_same_voted_fork == last_voted_slot_hash {
                    None
                } else {
                    Some(heaviest_slot_hash_on_same_voted_fork)
                }
            })
    }

    #[cfg(test)]
    fn set_stake_voted_at(&mut self, slot_hash_key: SlotHashKey, stake_voted_at: u64) {
        self.fork_infos
            .get_mut(&slot_hash_key)
            .unwrap()
            .stake_voted_at = stake_voted_at;
    }

    #[cfg(test)]
    fn is_leaf(&self, slot_hash_key: SlotHashKey) -> bool {
        self.fork_infos
            .get(&slot_hash_key)
            .unwrap()
            .children
            .is_empty()
    }
}

impl TreeDiff for HeaviestSubtreeForkChoice {
    type TreeKey = SlotHashKey;
    fn contains_slot(&self, slot_hash_key: &SlotHashKey) -> bool {
        self.fork_infos.contains_key(slot_hash_key)
    }

    fn children(&self, slot_hash_key: &SlotHashKey) -> Option<&[SlotHashKey]> {
        self.fork_infos
            .get(&slot_hash_key)
            .map(|fork_info| &fork_info.children[..])
    }
}

impl ForkChoice for HeaviestSubtreeForkChoice {
    type ForkChoiceKey = SlotHashKey;
    fn compute_bank_stats(
        &mut self,
        bank: &Bank,
        _tower: &Tower,
        latest_validator_votes_for_frozen_banks: &mut LatestValidatorVotesForFrozenBanks,
    ) {
        let mut start = Measure::start("compute_bank_stats_time");
        // Update `heaviest_subtree_fork_choice` to find the best fork to build on
        let root = self.root.0;
        let new_votes = latest_validator_votes_for_frozen_banks.take_votes_dirty_set(root);
        let (best_overall_slot, best_overall_hash) = self.add_votes(
            new_votes.into_iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        start.stop();

        datapoint_info!(
            "compute_bank_stats-best_slot",
            ("computed_slot", bank.slot(), i64),
            ("overall_best_slot", best_overall_slot, i64),
            ("overall_best_hash", best_overall_hash.to_string(), String),
            ("elapsed", start.as_us(), i64),
        );
    }

    // Returns:
    // 1) The heaviest overall bank
    // 2) The heaviest bank on the same fork as the last vote (doesn't require a
    // switching proof to vote for)
    fn select_forks(
        &self,
        _frozen_banks: &[Arc<Bank>],
        tower: &Tower,
        _progress: &ProgressMap,
        _ancestors: &HashMap<u64, HashSet<u64>>,
        bank_forks: &RwLock<BankForks>,
    ) -> (Arc<Bank>, Option<Arc<Bank>>) {
        let r_bank_forks = bank_forks.read().unwrap();
        (
            // BankForks should only contain one valid version of this slot
            r_bank_forks
                .get_with_checked_hash(self.best_overall_slot())
                .unwrap()
                .clone(),
            self.heaviest_slot_on_same_voted_fork(tower)
                .map(|slot_hash| {
                    // BankForks should only contain one valid version of this slot
                    r_bank_forks
                        .get_with_checked_hash(slot_hash)
                        .unwrap()
                        .clone()
                }),
        )
    }

    fn mark_fork_invalid_candidate(&mut self, invalid_slot_hash_key: &SlotHashKey) {
        info!(
            "marking fork starting at slot: {:?} invalid candidate",
            invalid_slot_hash_key
        );
        let fork_info = self.fork_infos.get_mut(invalid_slot_hash_key);
        if let Some(fork_info) = fork_info {
            if fork_info.is_candidate {
                fork_info.is_candidate = false;
                // Aggregate to find the new best slots excluding this fork
                let mut update_operations = UpdateOperations::default();
                self.insert_aggregate_operations(&mut update_operations, *invalid_slot_hash_key);
                self.process_update_operations(update_operations);
            }
        }
    }

    fn mark_fork_valid_candidate(&mut self, valid_slot_hash_key: &SlotHashKey) {
        let mut update_operations = UpdateOperations::default();
        let fork_info = self.fork_infos.get_mut(valid_slot_hash_key);
        if let Some(fork_info) = fork_info {
            // If a bunch of slots on the same fork are confirmed at once, then only the latest
            // slot will incur this aggregation operation
            fork_info.is_candidate = true;
            self.insert_mark_valid_aggregate_operations(
                &mut update_operations,
                *valid_slot_hash_key,
            );
        }

        // Aggregate to find the new best slots including this fork
        self.process_update_operations(update_operations);
    }
}

struct AncestorIterator<'a> {
    current_slot_hash_key: SlotHashKey,
    fork_infos: &'a HashMap<SlotHashKey, ForkInfo>,
}

impl<'a> AncestorIterator<'a> {
    fn new(
        start_slot_hash_key: SlotHashKey,
        fork_infos: &'a HashMap<SlotHashKey, ForkInfo>,
    ) -> Self {
        Self {
            current_slot_hash_key: start_slot_hash_key,
            fork_infos,
        }
    }
}

impl<'a> Iterator for AncestorIterator<'a> {
    type Item = SlotHashKey;
    fn next(&mut self) -> Option<Self::Item> {
        let parent_slot_hash_key = self
            .fork_infos
            .get(&self.current_slot_hash_key)
            .map(|fork_info| fork_info.parent)
            .unwrap_or(None);

        parent_slot_hash_key
            .map(|parent_slot_hash_key| {
                self.current_slot_hash_key = parent_slot_hash_key;
                Some(self.current_slot_hash_key)
            })
            .unwrap_or(None)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::consensus::test::VoteSimulator;
    use solana_runtime::{bank::Bank, bank_utils};
    use solana_sdk::{hash::Hash, slot_history::SlotHistory};
    use std::{collections::HashSet, ops::Range};
    use trees::tr;

    #[test]
    fn test_max_by_weight() {
        /*
            Build fork structure:
                 slot 0
                   |
                 slot 4
                   |
                 slot 5
        */
        let forks = tr(0) / (tr(4) / (tr(5)));
        let mut heaviest_subtree_fork_choice = HeaviestSubtreeForkChoice::new_from_tree(forks);

        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(1, stake);
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[0], (4, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        assert_eq!(
            heaviest_subtree_fork_choice.max_by_weight((4, Hash::default()), (5, Hash::default())),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            heaviest_subtree_fork_choice.max_by_weight((4, Hash::default()), (0, Hash::default())),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_add_root_parent() {
        /*
            Build fork structure:
                 slot 3
                   |
                 slot 4
                   |
                 slot 5
        */
        let forks = tr(3) / (tr(4) / (tr(5)));
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(1, stake);
        let mut heaviest_subtree_fork_choice = HeaviestSubtreeForkChoice::new_from_tree(forks);
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[0], (5, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        heaviest_subtree_fork_choice.add_root_parent((2, Hash::default()));
        assert_eq!(
            heaviest_subtree_fork_choice
                .parent(&(3, Hash::default()))
                .unwrap(),
            (2, Hash::default())
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&(3, Hash::default()))
                .unwrap(),
            stake
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&(2, Hash::default()))
                .unwrap(),
            0
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .children(&(2, Hash::default()))
                .unwrap()
                .to_vec(),
            vec![(3, Hash::default())]
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .best_slot(&(2, Hash::default()))
                .unwrap()
                .0,
            5
        );
        assert!(heaviest_subtree_fork_choice
            .parent(&(2, Hash::default()))
            .is_none());
    }

    #[test]
    fn test_ancestor_iterator() {
        let mut heaviest_subtree_fork_choice = setup_forks();
        let parents: Vec<_> = heaviest_subtree_fork_choice
            .ancestor_iterator((6, Hash::default()))
            .collect();
        assert_eq!(
            parents,
            vec![5, 3, 1, 0]
                .into_iter()
                .map(|s| (s, Hash::default()))
                .collect::<Vec<_>>()
        );
        let parents: Vec<_> = heaviest_subtree_fork_choice
            .ancestor_iterator((4, Hash::default()))
            .collect();
        assert_eq!(
            parents,
            vec![2, 1, 0]
                .into_iter()
                .map(|s| (s, Hash::default()))
                .collect::<Vec<_>>()
        );
        let parents: Vec<_> = heaviest_subtree_fork_choice
            .ancestor_iterator((1, Hash::default()))
            .collect();
        assert_eq!(
            parents,
            vec![0]
                .into_iter()
                .map(|s| (s, Hash::default()))
                .collect::<Vec<_>>()
        );
        let parents: Vec<_> = heaviest_subtree_fork_choice
            .ancestor_iterator((0, Hash::default()))
            .collect();
        assert!(parents.is_empty());

        // Set a root, everything but slots 2, 4 should be removed
        heaviest_subtree_fork_choice.set_root((2, Hash::default()));
        let parents: Vec<_> = heaviest_subtree_fork_choice
            .ancestor_iterator((4, Hash::default()))
            .collect();
        assert_eq!(
            parents,
            vec![2]
                .into_iter()
                .map(|s| (s, Hash::default()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_new_from_frozen_banks() {
        /*
            Build fork structure:
                 slot 0
                   |
                 slot 1
                 /    \
            slot 2    |
               |    slot 3
            slot 4
        */
        let forks = tr(0) / (tr(1) / (tr(2) / (tr(4))) / (tr(3)));
        let mut vote_simulator = VoteSimulator::new(1);
        vote_simulator.fill_bank_forks(forks, &HashMap::new());
        let bank_forks = vote_simulator.bank_forks;
        let mut frozen_banks: Vec<_> = bank_forks
            .read()
            .unwrap()
            .frozen_banks()
            .values()
            .cloned()
            .collect();
        frozen_banks.sort_by_key(|bank| bank.slot());

        let root_bank = bank_forks.read().unwrap().root_bank();
        let root = root_bank.slot();
        let root_hash = root_bank.hash();
        let heaviest_subtree_fork_choice =
            HeaviestSubtreeForkChoice::new_from_frozen_banks((root, root_hash), &frozen_banks);

        let bank0_hash = bank_forks.read().unwrap().get(0).unwrap().hash();
        assert!(heaviest_subtree_fork_choice
            .parent(&(0, bank0_hash))
            .is_none());

        let bank1_hash = bank_forks.read().unwrap().get(1).unwrap().hash();
        assert_eq!(
            heaviest_subtree_fork_choice
                .children(&(0, bank0_hash))
                .unwrap(),
            &[(1, bank1_hash)]
        );

        assert_eq!(
            heaviest_subtree_fork_choice.parent(&(1, bank1_hash)),
            Some((0, bank0_hash))
        );
        let bank2_hash = bank_forks.read().unwrap().get(2).unwrap().hash();
        let bank3_hash = bank_forks.read().unwrap().get(3).unwrap().hash();
        assert_eq!(
            heaviest_subtree_fork_choice
                .children(&(1, bank1_hash))
                .unwrap(),
            &[(2, bank2_hash), (3, bank3_hash)]
        );
        assert_eq!(
            heaviest_subtree_fork_choice.parent(&(2, bank2_hash)),
            Some((1, bank1_hash))
        );
        let bank4_hash = bank_forks.read().unwrap().get(4).unwrap().hash();
        assert_eq!(
            heaviest_subtree_fork_choice
                .children(&(2, bank2_hash))
                .unwrap(),
            &[(4, bank4_hash)]
        );
        // Check parent and children of invalid hash don't exist
        let invalid_hash = Hash::new_unique();
        assert!(heaviest_subtree_fork_choice
            .children(&(2, invalid_hash))
            .is_none());
        assert!(heaviest_subtree_fork_choice
            .parent(&(2, invalid_hash))
            .is_none());

        assert_eq!(
            heaviest_subtree_fork_choice.parent(&(3, bank3_hash)),
            Some((1, bank1_hash))
        );
        assert!(heaviest_subtree_fork_choice
            .children(&(3, bank3_hash))
            .unwrap()
            .is_empty());
        assert_eq!(
            heaviest_subtree_fork_choice.parent(&(4, bank4_hash)),
            Some((2, bank2_hash))
        );
        assert!(heaviest_subtree_fork_choice
            .children(&(4, bank4_hash))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_set_root() {
        let mut heaviest_subtree_fork_choice = setup_forks();

        // Set root to 1, should only purge 0
        heaviest_subtree_fork_choice.set_root((1, Hash::default()));
        for i in 0..=6 {
            let exists = i != 0;
            assert_eq!(
                heaviest_subtree_fork_choice
                    .fork_infos
                    .contains_key(&(i, Hash::default())),
                exists
            );
        }

        // Set root to 5, should purge everything except 5, 6
        heaviest_subtree_fork_choice.set_root((5, Hash::default()));
        for i in 0..=6 {
            let exists = i == 5 || i == 6;
            assert_eq!(
                heaviest_subtree_fork_choice
                    .fork_infos
                    .contains_key(&(i, Hash::default())),
                exists
            );
        }
    }

    #[test]
    fn test_set_root_and_add_votes() {
        let mut heaviest_subtree_fork_choice = setup_forks();
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(1, stake);

        // Vote for slot 2
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[0], (1, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 4);

        // Set a root
        heaviest_subtree_fork_choice.set_root((1, Hash::default()));

        // Vote again for slot 3 on a different fork than the last vote,
        // verify this fork is now the best fork
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[0], (3, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 6);
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&(1, Hash::default()))
                .unwrap(),
            0
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&(3, Hash::default()))
                .unwrap(),
            stake
        );
        for slot in &[1, 3] {
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_subtree(&(*slot, Hash::default()))
                    .unwrap(),
                stake
            );
        }

        // Set a root at last vote
        heaviest_subtree_fork_choice.set_root((3, Hash::default()));
        // Check new leaf 7 is still propagated properly
        heaviest_subtree_fork_choice
            .add_new_leaf_slot((7, Hash::default()), Some((6, Hash::default())));
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 7);
    }

    #[test]
    fn test_set_root_and_add_outdated_votes() {
        let mut heaviest_subtree_fork_choice = setup_forks();
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(1, stake);

        // Vote for slot 0
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[0], (0, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        // Set root to 1, should purge 0 from the tree, but
        // there's still an outstanding vote for slot 0 in `pubkey_votes`.
        heaviest_subtree_fork_choice.set_root((1, Hash::default()));

        // Vote again for slot 3, verify everything is ok
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[0], (3, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&(3, Hash::default()))
                .unwrap(),
            stake
        );
        for slot in &[1, 3] {
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_subtree(&(*slot, Hash::default()))
                    .unwrap(),
                stake
            );
        }
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 6);

        // Set root again on different fork than the last vote
        heaviest_subtree_fork_choice.set_root((2, Hash::default()));
        // Smaller vote than last vote 3 should be ignored
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[0], (2, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&(2, Hash::default()))
                .unwrap(),
            0
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&(2, Hash::default()))
                .unwrap(),
            0
        );
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 4);

        // New larger vote than last vote 3 should be processed
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[0], (4, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&(2, Hash::default()))
                .unwrap(),
            0
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&(4, Hash::default()))
                .unwrap(),
            stake
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&(2, Hash::default()))
                .unwrap(),
            stake
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&(4, Hash::default()))
                .unwrap(),
            stake
        );
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 4);
    }

    #[test]
    fn test_best_overall_slot() {
        let heaviest_subtree_fork_choice = setup_forks();
        // Best overall path is 0 -> 1 -> 2 -> 4, so best leaf
        // should be 4
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 4);
    }

    #[test]
    fn test_add_new_leaf_duplicate() {
        let (
            mut heaviest_subtree_fork_choice,
            duplicate_leaves_descended_from_4,
            duplicate_leaves_descended_from_5,
        ) = setup_duplicate_forks();

        // Add a child to one of the duplicates
        let duplicate_parent = duplicate_leaves_descended_from_4[0];
        let child = (11, Hash::new_unique());
        heaviest_subtree_fork_choice.add_new_leaf_slot(child, Some(duplicate_parent));
        assert_eq!(
            heaviest_subtree_fork_choice
                .children(&duplicate_parent)
                .unwrap(),
            &[child]
        );
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot(), child);

        // All the other duplicates should have no children
        for duplicate_leaf in duplicate_leaves_descended_from_5
            .iter()
            .chain(std::iter::once(&duplicate_leaves_descended_from_4[1]))
        {
            assert!(heaviest_subtree_fork_choice
                .children(&duplicate_leaf)
                .unwrap()
                .is_empty(),);
        }

        // Re-adding same duplicate slot should not overwrite existing one
        heaviest_subtree_fork_choice
            .add_new_leaf_slot(duplicate_parent, Some((4, Hash::default())));
        assert_eq!(
            heaviest_subtree_fork_choice
                .children(&duplicate_parent)
                .unwrap(),
            &[child]
        );
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot(), child);
    }

    #[test]
    fn test_propagate_new_leaf() {
        let mut heaviest_subtree_fork_choice = setup_forks();

        // Add a leaf 10, it should be the best choice
        heaviest_subtree_fork_choice
            .add_new_leaf_slot((10, Hash::default()), Some((4, Hash::default())));
        let ancestors = heaviest_subtree_fork_choice
            .ancestor_iterator((10, Hash::default()))
            .collect::<Vec<_>>();
        for a in ancestors
            .into_iter()
            .chain(std::iter::once((10, Hash::default())))
        {
            assert_eq!(heaviest_subtree_fork_choice.best_slot(&a).unwrap().0, 10);
        }

        // Add a smaller leaf 9, it should be the best choice
        heaviest_subtree_fork_choice
            .add_new_leaf_slot((9, Hash::default()), Some((4, Hash::default())));
        let ancestors = heaviest_subtree_fork_choice
            .ancestor_iterator((9, Hash::default()))
            .collect::<Vec<_>>();
        for a in ancestors
            .into_iter()
            .chain(std::iter::once((9, Hash::default())))
        {
            assert_eq!(heaviest_subtree_fork_choice.best_slot(&a).unwrap().0, 9);
        }

        // Add a higher leaf 11, should not change the best choice
        heaviest_subtree_fork_choice
            .add_new_leaf_slot((11, Hash::default()), Some((4, Hash::default())));
        let ancestors = heaviest_subtree_fork_choice
            .ancestor_iterator((11, Hash::default()))
            .collect::<Vec<_>>();
        for a in ancestors
            .into_iter()
            .chain(std::iter::once((9, Hash::default())))
        {
            assert_eq!(heaviest_subtree_fork_choice.best_slot(&a).unwrap().0, 9);
        }

        // Add a vote for the other branch at slot 3.
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(2, stake);
        let leaf6 = 6;
        // Leaf slot 9 stops being the `best_slot` at slot 1 because there
        // are now votes for the branch at slot 3
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[0], (leaf6, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        // Because slot 1 now sees the child branch at slot 3 has non-zero
        // weight, adding smaller leaf slot 8 in the other child branch at slot 2
        // should not propagate past slot 1
        heaviest_subtree_fork_choice
            .add_new_leaf_slot((8, Hash::default()), Some((4, Hash::default())));
        let ancestors = heaviest_subtree_fork_choice
            .ancestor_iterator((8, Hash::default()))
            .collect::<Vec<_>>();
        for a in ancestors
            .into_iter()
            .chain(std::iter::once((8, Hash::default())))
        {
            let best_slot = if a.0 > 1 { 8 } else { leaf6 };
            assert_eq!(
                heaviest_subtree_fork_choice.best_slot(&a).unwrap().0,
                best_slot
            );
        }

        // Add vote for slot 8, should now be the best slot (has same weight
        // as fork containing slot 6, but slot 2 is smaller than slot 3).
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[1], (8, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 8);

        // Because slot 4 now sees the child leaf 8 has non-zero
        // weight, adding smaller leaf slots should not propagate past slot 4
        heaviest_subtree_fork_choice
            .add_new_leaf_slot((7, Hash::default()), Some((4, Hash::default())));
        let ancestors = heaviest_subtree_fork_choice
            .ancestor_iterator((7, Hash::default()))
            .collect::<Vec<_>>();
        for a in ancestors
            .into_iter()
            .chain(std::iter::once((8, Hash::default())))
        {
            assert_eq!(heaviest_subtree_fork_choice.best_slot(&a).unwrap().0, 8);
        }

        // All the leaves should think they are their own best choice
        for leaf in [8, 9, 10, 11].iter() {
            assert_eq!(
                heaviest_subtree_fork_choice
                    .best_slot(&(*leaf, Hash::default()))
                    .unwrap()
                    .0,
                *leaf
            );
        }
    }

    #[test]
    fn test_propagate_new_leaf_2() {
        /*
            Build fork structure:
                 slot 0
                   |
                 slot 4
                   |
                 slot 6
        */
        let forks = tr(0) / (tr(4) / (tr(6)));
        let mut heaviest_subtree_fork_choice = HeaviestSubtreeForkChoice::new_from_tree(forks);
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(1, stake);

        // slot 6 should be the best because it's the only leaf
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 6);

        // Add a leaf slot 5. Even though 5 is less than the best leaf 6,
        // it's not less than it's sibling slot 4, so the best overall
        // leaf should remain unchanged
        heaviest_subtree_fork_choice
            .add_new_leaf_slot((5, Hash::default()), Some((0, Hash::default())));
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 6);

        // Add a leaf slot 2 on a different fork than leaf 6. Slot 2 should
        // be the new best because it's for a lesser slot
        heaviest_subtree_fork_choice
            .add_new_leaf_slot((2, Hash::default()), Some((0, Hash::default())));
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 2);

        // Add a vote for slot 4, so leaf 6 should be the best again
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[0], (4, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 6);

        // Adding a slot 1 that is less than the current best leaf 6 should not change the best
        // slot because the fork slot 5 is on has a higher weight
        heaviest_subtree_fork_choice
            .add_new_leaf_slot((1, Hash::default()), Some((0, Hash::default())));
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 6);
    }

    #[test]
    fn test_aggregate_slot() {
        let mut heaviest_subtree_fork_choice = setup_forks();

        // No weights are present, weights should be zero
        heaviest_subtree_fork_choice.aggregate_slot((1, Hash::default()));
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&(1, Hash::default()))
                .unwrap(),
            0
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&(1, Hash::default()))
                .unwrap(),
            0
        );
        // The best leaf when weights are equal should prioritize the lower leaf
        assert_eq!(
            heaviest_subtree_fork_choice
                .best_slot(&(1, Hash::default()))
                .unwrap()
                .0,
            4
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .best_slot(&(2, Hash::default()))
                .unwrap()
                .0,
            4
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .best_slot(&(3, Hash::default()))
                .unwrap()
                .0,
            6
        );

        // Update the weights that have voted *exactly* at each slot, the
        // branch containing slots {5, 6} has weight 11, so should be heavier
        // than the branch containing slots {2, 4}
        let mut total_stake = 0;
        let staked_voted_slots: HashSet<_> = vec![2, 4, 5, 6].into_iter().collect();
        for slot in &staked_voted_slots {
            heaviest_subtree_fork_choice.set_stake_voted_at((*slot, Hash::default()), *slot);
            total_stake += *slot;
        }

        // Aggregate up each of the two forks (order matters, has to be
        // reverse order for each fork, and aggregating a slot multiple times
        // is fine)
        let slots_to_aggregate: Vec<_> = std::iter::once((6, Hash::default()))
            .chain(heaviest_subtree_fork_choice.ancestor_iterator((6, Hash::default())))
            .chain(std::iter::once((4, Hash::default())))
            .chain(heaviest_subtree_fork_choice.ancestor_iterator((4, Hash::default())))
            .collect();

        for slot_hash in slots_to_aggregate {
            heaviest_subtree_fork_choice.aggregate_slot(slot_hash);
        }

        // The best path is now 0 -> 1 -> 3 -> 5 -> 6, so leaf 6
        // should be the best choice
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 6);

        // Verify `stake_voted_at`
        for slot in 0..=6 {
            let expected_stake = if staked_voted_slots.contains(&slot) {
                slot
            } else {
                0
            };

            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_at(&(slot, Hash::default()))
                    .unwrap(),
                expected_stake
            );
        }

        // Verify `stake_voted_subtree` for common fork
        for slot in &[0, 1] {
            // Subtree stake is sum of the `stake_voted_at` across
            // all slots in the subtree
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_subtree(&(*slot, Hash::default()))
                    .unwrap(),
                total_stake
            );
        }
        // Verify `stake_voted_subtree` for fork 1
        let mut total_expected_stake = 0;
        for slot in &[4, 2] {
            total_expected_stake += heaviest_subtree_fork_choice
                .stake_voted_at(&(*slot, Hash::default()))
                .unwrap();
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_subtree(&(*slot, Hash::default()))
                    .unwrap(),
                total_expected_stake
            );
        }
        // Verify `stake_voted_subtree` for fork 2
        total_expected_stake = 0;
        for slot in &[6, 5, 3] {
            total_expected_stake += heaviest_subtree_fork_choice
                .stake_voted_at(&(*slot, Hash::default()))
                .unwrap();
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_subtree(&(*slot, Hash::default()))
                    .unwrap(),
                total_expected_stake
            );
        }
    }

    #[test]
    fn test_process_update_operations() {
        let mut heaviest_subtree_fork_choice = setup_forks();
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(3, stake);

        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], (3, Hash::default())),
            (vote_pubkeys[1], (2, Hash::default())),
            (vote_pubkeys[2], (1, Hash::default())),
        ];
        let expected_best_slot =
            |slot, heaviest_subtree_fork_choice: &HeaviestSubtreeForkChoice| -> Slot {
                if !heaviest_subtree_fork_choice.is_leaf((slot, Hash::default())) {
                    // Both branches have equal weight, so should pick the lesser leaf
                    if heaviest_subtree_fork_choice
                        .ancestor_iterator((4, Hash::default()))
                        .collect::<HashSet<SlotHashKey>>()
                        .contains(&(slot, Hash::default()))
                    {
                        4
                    } else {
                        6
                    }
                } else {
                    slot
                }
            };

        check_process_update_correctness(
            &mut heaviest_subtree_fork_choice,
            &pubkey_votes,
            0..7,
            &bank,
            stake,
            expected_best_slot,
        );

        // Everyone makes newer votes
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], (4, Hash::default())),
            (vote_pubkeys[1], (3, Hash::default())),
            (vote_pubkeys[2], (3, Hash::default())),
        ];

        let expected_best_slot =
            |slot, heaviest_subtree_fork_choice: &HeaviestSubtreeForkChoice| -> Slot {
                if !heaviest_subtree_fork_choice.is_leaf((slot, Hash::default())) {
                    // The branch with leaf 6 now has two votes, so should pick that one
                    if heaviest_subtree_fork_choice
                        .ancestor_iterator((6, Hash::default()))
                        .collect::<HashSet<SlotHashKey>>()
                        .contains(&(slot, Hash::default()))
                    {
                        6
                    } else {
                        4
                    }
                } else {
                    slot
                }
            };

        check_process_update_correctness(
            &mut heaviest_subtree_fork_choice,
            &pubkey_votes,
            0..7,
            &bank,
            stake,
            expected_best_slot,
        );
    }

    #[test]
    fn test_generate_update_operations() {
        let mut heaviest_subtree_fork_choice = setup_forks();
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(3, stake);
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], (3, Hash::default())),
            (vote_pubkeys[1], (4, Hash::default())),
            (vote_pubkeys[2], (1, Hash::default())),
        ];

        let expected_update_operations: UpdateOperations = vec![
            // Add/remove from new/old forks
            (
                ((1, Hash::default()), UpdateLabel::Add),
                UpdateOperation::Add(stake),
            ),
            (
                ((3, Hash::default()), UpdateLabel::Add),
                UpdateOperation::Add(stake),
            ),
            (
                ((4, Hash::default()), UpdateLabel::Add),
                UpdateOperation::Add(stake),
            ),
            // Aggregate all ancestors of changed slots
            (
                ((0, Hash::default()), UpdateLabel::Aggregate),
                UpdateOperation::Aggregate,
            ),
            (
                ((1, Hash::default()), UpdateLabel::Aggregate),
                UpdateOperation::Aggregate,
            ),
            (
                ((2, Hash::default()), UpdateLabel::Aggregate),
                UpdateOperation::Aggregate,
            ),
        ]
        .into_iter()
        .collect();

        let generated_update_operations = heaviest_subtree_fork_choice.generate_update_operations(
            pubkey_votes.iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        assert_eq!(expected_update_operations, generated_update_operations);

        // Everyone makes older/same votes, should be ignored
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], (3, Hash::default())),
            (vote_pubkeys[1], (2, Hash::default())),
            (vote_pubkeys[2], (1, Hash::default())),
        ];
        let generated_update_operations = heaviest_subtree_fork_choice.generate_update_operations(
            pubkey_votes.iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        assert!(generated_update_operations.is_empty());

        // Some people make newer votes
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            // old, ignored
            (vote_pubkeys[0], (3, Hash::default())),
            // new, switched forks
            (vote_pubkeys[1], (5, Hash::default())),
            // new, same fork
            (vote_pubkeys[2], (3, Hash::default())),
        ];

        let expected_update_operations: BTreeMap<(SlotHashKey, UpdateLabel), UpdateOperation> =
            vec![
                // Add/remove to/from new/old forks
                (
                    ((3, Hash::default()), UpdateLabel::Add),
                    UpdateOperation::Add(stake),
                ),
                (
                    ((5, Hash::default()), UpdateLabel::Add),
                    UpdateOperation::Add(stake),
                ),
                (
                    ((1, Hash::default()), UpdateLabel::Subtract),
                    UpdateOperation::Subtract(stake),
                ),
                (
                    ((4, Hash::default()), UpdateLabel::Subtract),
                    UpdateOperation::Subtract(stake),
                ),
                // Aggregate all ancestors of changed slots
                (
                    ((0, Hash::default()), UpdateLabel::Aggregate),
                    UpdateOperation::Aggregate,
                ),
                (
                    ((1, Hash::default()), UpdateLabel::Aggregate),
                    UpdateOperation::Aggregate,
                ),
                (
                    ((2, Hash::default()), UpdateLabel::Aggregate),
                    UpdateOperation::Aggregate,
                ),
                (
                    ((3, Hash::default()), UpdateLabel::Aggregate),
                    UpdateOperation::Aggregate,
                ),
            ]
            .into_iter()
            .collect();

        let generated_update_operations = heaviest_subtree_fork_choice.generate_update_operations(
            pubkey_votes.iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        assert_eq!(expected_update_operations, generated_update_operations);

        // People make new votes
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            // new, switch forks
            (vote_pubkeys[0], (4, Hash::default())),
            // new, same fork
            (vote_pubkeys[1], (6, Hash::default())),
            // new, same fork
            (vote_pubkeys[2], (6, Hash::default())),
        ];

        let expected_update_operations: BTreeMap<(SlotHashKey, UpdateLabel), UpdateOperation> =
            vec![
                // Add/remove from new/old forks
                (
                    ((4, Hash::default()), UpdateLabel::Add),
                    UpdateOperation::Add(stake),
                ),
                (
                    ((6, Hash::default()), UpdateLabel::Add),
                    UpdateOperation::Add(2 * stake),
                ),
                (
                    ((3, Hash::default()), UpdateLabel::Subtract),
                    UpdateOperation::Subtract(2 * stake),
                ),
                (
                    ((5, Hash::default()), UpdateLabel::Subtract),
                    UpdateOperation::Subtract(stake),
                ),
                // Aggregate all ancestors of changed slots
                (
                    ((0, Hash::default()), UpdateLabel::Aggregate),
                    UpdateOperation::Aggregate,
                ),
                (
                    ((1, Hash::default()), UpdateLabel::Aggregate),
                    UpdateOperation::Aggregate,
                ),
                (
                    ((2, Hash::default()), UpdateLabel::Aggregate),
                    UpdateOperation::Aggregate,
                ),
                (
                    ((3, Hash::default()), UpdateLabel::Aggregate),
                    UpdateOperation::Aggregate,
                ),
                (
                    ((5, Hash::default()), UpdateLabel::Aggregate),
                    UpdateOperation::Aggregate,
                ),
            ]
            .into_iter()
            .collect();

        let generated_update_operations = heaviest_subtree_fork_choice.generate_update_operations(
            pubkey_votes.iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        assert_eq!(expected_update_operations, generated_update_operations);
    }

    #[test]
    fn test_add_votes() {
        let mut heaviest_subtree_fork_choice = setup_forks();
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(3, stake);

        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], (3, Hash::default())),
            (vote_pubkeys[1], (2, Hash::default())),
            (vote_pubkeys[2], (1, Hash::default())),
        ];
        assert_eq!(
            heaviest_subtree_fork_choice
                .add_votes(
                    pubkey_votes.iter(),
                    bank.epoch_stakes_map(),
                    bank.epoch_schedule()
                )
                .0,
            4
        );

        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 4)
    }

    #[test]
    fn test_add_votes_duplicate_tie() {
        let (mut heaviest_subtree_fork_choice, duplicate_leaves_descended_from_4, _) =
            setup_duplicate_forks();
        let stake = 10;
        let num_validators = 2;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(num_validators, stake);

        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], duplicate_leaves_descended_from_4[0]),
            (vote_pubkeys[1], duplicate_leaves_descended_from_4[1]),
        ];

        // duplicate_leaves_descended_from_4 are sorted, and fork choice will pick the smaller
        // one in the event of a tie
        let expected_best_slot_hash = duplicate_leaves_descended_from_4[0];
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            expected_best_slot_hash
        );

        assert_eq!(
            heaviest_subtree_fork_choice.best_overall_slot(),
            expected_best_slot_hash
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&duplicate_leaves_descended_from_4[1])
                .unwrap(),
            stake
        );

        // Adding the same vote again will not do anything
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> =
            vec![(vote_pubkeys[1], duplicate_leaves_descended_from_4[1])];
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            expected_best_slot_hash
        );

        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&duplicate_leaves_descended_from_4[1])
                .unwrap(),
            stake
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&duplicate_leaves_descended_from_4[1])
                .unwrap(),
            stake
        );

        // All common ancestors should have subtree voted stake == 2 * stake, but direct
        // voted stake == 0
        let expected_ancestors_stake = 2 * stake;
        for ancestor in
            heaviest_subtree_fork_choice.ancestor_iterator(duplicate_leaves_descended_from_4[1])
        {
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_subtree(&ancestor)
                    .unwrap(),
                expected_ancestors_stake
            );
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_at(&ancestor)
                    .unwrap(),
                0,
            );
        }
    }

    #[test]
    fn test_add_votes_duplicate_greater_hash_ignored() {
        let (mut heaviest_subtree_fork_choice, duplicate_leaves_descended_from_4, _) =
            setup_duplicate_forks();
        let stake = 10;
        let num_validators = 2;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(num_validators, stake);

        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], duplicate_leaves_descended_from_4[0]),
            (vote_pubkeys[1], duplicate_leaves_descended_from_4[1]),
        ];

        // duplicate_leaves_descended_from_4 are sorted, and fork choice will pick the smaller
        // one in the event of a tie
        let expected_best_slot_hash = duplicate_leaves_descended_from_4[0];
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            expected_best_slot_hash
        );
        // Adding a duplicate vote for a validator, for another a greater bank hash,
        // should be ignored as we prioritize the smaller bank hash. Thus nothing
        // should change.
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> =
            vec![(vote_pubkeys[0], duplicate_leaves_descended_from_4[1])];
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            expected_best_slot_hash
        );

        // Still only has one validator voting on it
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&duplicate_leaves_descended_from_4[1])
                .unwrap(),
            stake
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&duplicate_leaves_descended_from_4[1])
                .unwrap(),
            stake
        );
        // All common ancestors should have subtree voted stake == 2 * stake, but direct
        // voted stake == 0
        let expected_ancestors_stake = 2 * stake;
        for ancestor in
            heaviest_subtree_fork_choice.ancestor_iterator(duplicate_leaves_descended_from_4[1])
        {
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_subtree(&ancestor)
                    .unwrap(),
                expected_ancestors_stake
            );
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_at(&ancestor)
                    .unwrap(),
                0,
            );
        }
    }

    #[test]
    fn test_add_votes_duplicate_smaller_hash_prioritized() {
        let (mut heaviest_subtree_fork_choice, duplicate_leaves_descended_from_4, _) =
            setup_duplicate_forks();
        let stake = 10;
        let num_validators = 2;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(num_validators, stake);

        // Both voters voted on duplicate_leaves_descended_from_4[1], so thats the heaviest
        // branch
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], duplicate_leaves_descended_from_4[1]),
            (vote_pubkeys[1], duplicate_leaves_descended_from_4[1]),
        ];

        let expected_best_slot_hash = duplicate_leaves_descended_from_4[1];
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            expected_best_slot_hash
        );

        // BEFORE, both validators voting on this leaf
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&duplicate_leaves_descended_from_4[1])
                .unwrap(),
            2 * stake,
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&duplicate_leaves_descended_from_4[1])
                .unwrap(),
            2 * stake,
        );

        // Adding a duplicate vote for a validator, for another a smaller bank hash,
        // should be proritized and replace the vote for the greater bank hash.
        // Now because both duplicate nodes are tied, the best leaf is the smaller one.
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> =
            vec![(vote_pubkeys[0], duplicate_leaves_descended_from_4[0])];
        let expected_best_slot_hash = duplicate_leaves_descended_from_4[0];
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            expected_best_slot_hash
        );

        // AFTER, only one of the validators is voting on this leaf
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&duplicate_leaves_descended_from_4[1])
                .unwrap(),
            stake,
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&duplicate_leaves_descended_from_4[1])
                .unwrap(),
            stake,
        );

        // The other leaf now has one of the votes
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&duplicate_leaves_descended_from_4[0])
                .unwrap(),
            stake,
        );
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_at(&duplicate_leaves_descended_from_4[0])
                .unwrap(),
            stake,
        );

        // All common ancestors should have subtree voted stake == 2 * stake, but direct
        // voted stake == 0
        let expected_ancestors_stake = 2 * stake;
        for ancestor in
            heaviest_subtree_fork_choice.ancestor_iterator(duplicate_leaves_descended_from_4[0])
        {
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_subtree(&ancestor)
                    .unwrap(),
                expected_ancestors_stake
            );
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_at(&ancestor)
                    .unwrap(),
                0,
            );
        }
    }

    #[test]
    fn test_add_votes_duplicate_then_outdated() {
        let (mut heaviest_subtree_fork_choice, duplicate_leaves_descended_from_4, _) =
            setup_duplicate_forks();
        let stake = 10;
        let num_validators = 3;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(num_validators, stake);

        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], duplicate_leaves_descended_from_4[0]),
            (vote_pubkeys[1], duplicate_leaves_descended_from_4[1]),
        ];

        // duplicate_leaves_descended_from_4 are sorted, and fork choice will pick the smaller
        // one in the event of a tie
        let expected_best_slot_hash = duplicate_leaves_descended_from_4[0];
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            expected_best_slot_hash
        );

        // Create two children for slots greater than the duplicate slot,
        // 1) descended from the current best slot (which also happens to be a duplicate slot)
        // 2) another descended from a non-duplicate slot.
        assert_eq!(
            heaviest_subtree_fork_choice.best_overall_slot(),
            duplicate_leaves_descended_from_4[0]
        );
        // Create new child with heaviest duplicate parent
        let duplicate_parent = duplicate_leaves_descended_from_4[0];
        let duplicate_slot = duplicate_parent.0;

        // Create new child with non-duplicate parent
        let nonduplicate_parent = (2, Hash::default());
        let higher_child_with_duplicate_parent = (duplicate_slot + 1, Hash::new_unique());
        let higher_child_with_nonduplicate_parent = (duplicate_slot + 2, Hash::new_unique());
        heaviest_subtree_fork_choice
            .add_new_leaf_slot(higher_child_with_duplicate_parent, Some(duplicate_parent));
        heaviest_subtree_fork_choice.add_new_leaf_slot(
            higher_child_with_nonduplicate_parent,
            Some(nonduplicate_parent),
        );

        // vote_pubkeys[0] and vote_pubkeys[1] should both have their latest votes
        // erased after a vote for a higher parent
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], higher_child_with_duplicate_parent),
            (vote_pubkeys[1], higher_child_with_nonduplicate_parent),
            (vote_pubkeys[2], higher_child_with_nonduplicate_parent),
        ];
        let expected_best_slot_hash = higher_child_with_nonduplicate_parent;
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            expected_best_slot_hash
        );

        // All the stake dirctly voting on the duplicates have been outdated
        for (i, duplicate_leaf) in duplicate_leaves_descended_from_4.iter().enumerate() {
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_at(duplicate_leaf)
                    .unwrap(),
                0,
            );

            if i == 0 {
                // The subtree stake of the first duplicate however, has one vote still
                // because it's the parent of the `higher_child_with_duplicate_parent`,
                // which has one vote
                assert_eq!(
                    heaviest_subtree_fork_choice
                        .stake_voted_subtree(duplicate_leaf)
                        .unwrap(),
                    stake,
                );
            } else {
                assert_eq!(
                    heaviest_subtree_fork_choice
                        .stake_voted_subtree(duplicate_leaf)
                        .unwrap(),
                    0,
                );
            }
        }

        // Node 4 has subtree voted stake == stake since it only has one voter on it
        let node4 = (4, Hash::default());
        assert_eq!(
            heaviest_subtree_fork_choice
                .stake_voted_subtree(&node4)
                .unwrap(),
            stake,
        );
        assert_eq!(
            heaviest_subtree_fork_choice.stake_voted_at(&node4).unwrap(),
            0,
        );

        // All ancestors of 4 should have subtree voted stake == num_validators * stake,
        // but direct voted stake == 0
        let expected_ancestors_stake = num_validators as u64 * stake;
        for ancestor in heaviest_subtree_fork_choice.ancestor_iterator(node4) {
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_subtree(&ancestor)
                    .unwrap(),
                expected_ancestors_stake
            );
            assert_eq!(
                heaviest_subtree_fork_choice
                    .stake_voted_at(&ancestor)
                    .unwrap(),
                0,
            );
        }
    }

    #[test]
    fn test_add_votes_duplicate_zero_stake() {
        let (mut heaviest_subtree_fork_choice, duplicate_leaves_descended_from_4, _): (
            HeaviestSubtreeForkChoice,
            Vec<SlotHashKey>,
            Vec<SlotHashKey>,
        ) = setup_duplicate_forks();

        let stake = 0;
        let num_validators = 2;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(num_validators, stake);

        // Make new vote with vote_pubkeys[0] for a higher slot
        // Create new child with heaviest duplicate parent
        let duplicate_parent = duplicate_leaves_descended_from_4[0];
        let duplicate_slot = duplicate_parent.0;
        let higher_child_with_duplicate_parent = (duplicate_slot + 1, Hash::new_unique());
        heaviest_subtree_fork_choice
            .add_new_leaf_slot(higher_child_with_duplicate_parent, Some(duplicate_parent));

        // Vote for pubkey 0 on one of the duplicate slots
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> =
            vec![(vote_pubkeys[0], duplicate_leaves_descended_from_4[1])];

        // Stake is zero, so because duplicate_leaves_descended_from_4[0] and
        // duplicate_leaves_descended_from_4[1] are tied, the child of the smaller
        // node duplicate_leaves_descended_from_4[0] is the one that is picked
        let expected_best_slot_hash = higher_child_with_duplicate_parent;
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            expected_best_slot_hash
        );
        assert_eq!(
            *heaviest_subtree_fork_choice
                .latest_votes
                .get(&vote_pubkeys[0])
                .unwrap(),
            duplicate_leaves_descended_from_4[1]
        );

        // Now add a vote for a higher slot, and ensure the latest votes
        // for this pubkey were updated
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> =
            vec![(vote_pubkeys[0], higher_child_with_duplicate_parent)];

        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            expected_best_slot_hash
        );
        assert_eq!(
            *heaviest_subtree_fork_choice
                .latest_votes
                .get(&vote_pubkeys[0])
                .unwrap(),
            higher_child_with_duplicate_parent
        );
    }

    #[test]
    fn test_is_best_child() {
        /*
            Build fork structure:
                 slot 0
                   |
                 slot 4
                /      \
          slot 10     slot 9
        */
        let forks = tr(0) / (tr(4) / (tr(9)) / (tr(10)));
        let mut heaviest_subtree_fork_choice = HeaviestSubtreeForkChoice::new_from_tree(forks);
        assert!(heaviest_subtree_fork_choice.is_best_child(&(0, Hash::default())));
        assert!(heaviest_subtree_fork_choice.is_best_child(&(4, Hash::default())));

        // 9 is better than 10
        assert!(heaviest_subtree_fork_choice.is_best_child(&(9, Hash::default())));
        assert!(!heaviest_subtree_fork_choice.is_best_child(&(10, Hash::default())));

        // Add new leaf 8, which is better than 9, as both have weight 0
        heaviest_subtree_fork_choice
            .add_new_leaf_slot((8, Hash::default()), Some((4, Hash::default())));
        assert!(heaviest_subtree_fork_choice.is_best_child(&(8, Hash::default())));
        assert!(!heaviest_subtree_fork_choice.is_best_child(&(9, Hash::default())));
        assert!(!heaviest_subtree_fork_choice.is_best_child(&(10, Hash::default())));

        // Add vote for 9, it's the best again
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(3, 100);
        heaviest_subtree_fork_choice.add_votes(
            [(vote_pubkeys[0], (9, Hash::default()))].iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );
        assert!(heaviest_subtree_fork_choice.is_best_child(&(9, Hash::default())));
        assert!(!heaviest_subtree_fork_choice.is_best_child(&(8, Hash::default())));
        assert!(!heaviest_subtree_fork_choice.is_best_child(&(10, Hash::default())));
    }

    #[test]
    fn test_merge() {
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(4, stake);
        /*
            Build fork structure:
                 slot 0
                   |
                 slot 3
                 /    \
            slot 5    |
               |    slot 9
            slot 7    |
                    slot 11
                      |
                    slot 12 (vote pubkey 2)
        */
        let forks = tr(0) / (tr(3) / (tr(5) / (tr(7))) / (tr(9) / (tr(11) / (tr(12)))));
        let mut tree1 = HeaviestSubtreeForkChoice::new_from_tree(forks);
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], (5, Hash::default())),
            (vote_pubkeys[1], (3, Hash::default())),
            (vote_pubkeys[2], (12, Hash::default())),
        ];
        tree1.add_votes(
            pubkey_votes.iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        /*
                    Build fork structure:
                          slot 10
                             |
                          slot 15
                         /       \
        (vote pubkey 0) slot 16   |
                       |       slot 18
                    slot 17       |
                               slot 19 (vote pubkey 1)
                                  |
                               slot 20 (vote pubkey 3)
        */
        let forks = tr(10) / (tr(15) / (tr(16) / (tr(17))) / (tr(18) / (tr(19) / (tr(20)))));
        let mut tree2 = HeaviestSubtreeForkChoice::new_from_tree(forks);
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            // more than tree 1
            (vote_pubkeys[0], (16, Hash::default())),
            // more than tree1
            (vote_pubkeys[1], (19, Hash::default())),
            // less than tree1
            (vote_pubkeys[2], (10, Hash::default())),
            // Add a pubkey that only voted on this tree
            (vote_pubkeys[3], (20, Hash::default())),
        ];
        tree2.add_votes(
            pubkey_votes.iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        // Merge tree2 at leaf 7 of tree1
        tree1.merge(
            tree2,
            &(7, Hash::default()),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        // Check ancestry information is correct
        let ancestors: Vec<_> = tree1.ancestor_iterator((20, Hash::default())).collect();
        assert_eq!(
            ancestors,
            vec![19, 18, 15, 10, 7, 5, 3, 0]
                .into_iter()
                .map(|s| (s, Hash::default()))
                .collect::<Vec<_>>()
        );
        let ancestors: Vec<_> = tree1.ancestor_iterator((17, Hash::default())).collect();
        assert_eq!(
            ancestors,
            vec![16, 15, 10, 7, 5, 3, 0]
                .into_iter()
                .map(|s| (s, Hash::default()))
                .collect::<Vec<_>>()
        );

        // Check correctness of votes
        // Pubkey 0
        assert_eq!(tree1.stake_voted_at(&(16, Hash::default())).unwrap(), stake);
        assert_eq!(tree1.stake_voted_at(&(5, Hash::default())).unwrap(), 0);
        // Pubkey 1
        assert_eq!(tree1.stake_voted_at(&(19, Hash::default())).unwrap(), stake);
        assert_eq!(tree1.stake_voted_at(&(3, Hash::default())).unwrap(), 0);
        // Pubkey 2
        assert_eq!(tree1.stake_voted_at(&(10, Hash::default())).unwrap(), 0);
        assert_eq!(tree1.stake_voted_at(&(12, Hash::default())).unwrap(), stake);
        // Pubkey 3
        assert_eq!(tree1.stake_voted_at(&(20, Hash::default())).unwrap(), stake);

        for slot in &[0, 3] {
            assert_eq!(
                tree1
                    .stake_voted_subtree(&(*slot, Hash::default()))
                    .unwrap(),
                4 * stake
            );
        }
        for slot in &[5, 7, 10, 15] {
            assert_eq!(
                tree1
                    .stake_voted_subtree(&(*slot, Hash::default()))
                    .unwrap(),
                3 * stake
            );
        }
        for slot in &[18, 19] {
            assert_eq!(
                tree1
                    .stake_voted_subtree(&(*slot, Hash::default()))
                    .unwrap(),
                2 * stake
            );
        }
        for slot in &[9, 11, 12, 16, 20] {
            assert_eq!(
                tree1
                    .stake_voted_subtree(&(*slot, Hash::default()))
                    .unwrap(),
                stake
            );
        }
        for slot in &[17] {
            assert_eq!(
                tree1
                    .stake_voted_subtree(&(*slot, Hash::default()))
                    .unwrap(),
                0
            );
        }

        assert_eq!(tree1.best_overall_slot().0, 20);
    }

    #[test]
    fn test_merge_duplicate() {
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(2, stake);
        let mut slot_5_duplicate_hashes = std::iter::repeat_with(|| (5, Hash::new_unique()))
            .take(2)
            .collect::<Vec<_>>();
        slot_5_duplicate_hashes.sort();

        /*
            Build fork structure:
                 slot 0
                /     \
           slot 2     slot 5 (bigger hash)
        */
        let forks =
            tr((0, Hash::default())) / tr((2, Hash::default())) / tr(slot_5_duplicate_hashes[1]);
        let mut tree1 = HeaviestSubtreeForkChoice::new_from_tree(forks);
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], (2, Hash::default())),
            (vote_pubkeys[1], slot_5_duplicate_hashes[1]),
        ];
        tree1.add_votes(
            pubkey_votes.iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        /*
                    Build fork structure:
                          slot 3
                             |
                          slot 5 (smaller hash, prioritized over previous version)
        */
        let forks = tr((3, Hash::default())) / tr(slot_5_duplicate_hashes[0]);
        let mut tree2 = HeaviestSubtreeForkChoice::new_from_tree(forks);
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], (3, Hash::default())),
            // Pubkey 1 voted on another version of slot 5
            (vote_pubkeys[1], slot_5_duplicate_hashes[0]),
        ];

        tree2.add_votes(
            pubkey_votes.iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        // Merge tree2 at leaf 2 of tree1
        tree1.merge(
            tree2,
            &(2, Hash::default()),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        // Pubkey 1 voted on both versions of slot 5, but should prioritize the one in
        // the merged branch because it's for a smaller hash
        assert_eq!(
            tree1.stake_voted_at(&slot_5_duplicate_hashes[1]).unwrap(),
            0
        );
        assert_eq!(
            tree1.stake_voted_at(&slot_5_duplicate_hashes[0]).unwrap(),
            stake
        );
        assert_eq!(tree1.best_overall_slot(), slot_5_duplicate_hashes[0]);

        // Check the ancestors are correct
        let ancestors: Vec<_> = tree1
            .ancestor_iterator(slot_5_duplicate_hashes[1])
            .collect();
        assert_eq!(ancestors, vec![(0, Hash::default())]);
        let ancestors: Vec<_> = tree1
            .ancestor_iterator(slot_5_duplicate_hashes[0])
            .collect();
        assert_eq!(
            ancestors,
            vec![
                (3, Hash::default()),
                (2, Hash::default()),
                (0, Hash::default())
            ]
        );
    }

    #[test]
    fn test_subtree_diff() {
        let mut heaviest_subtree_fork_choice = setup_forks();

        // Diff of same root is empty, no matter root, intermediate node, or leaf
        assert!(heaviest_subtree_fork_choice
            .subtree_diff((0, Hash::default()), (0, Hash::default()))
            .is_empty());
        assert!(heaviest_subtree_fork_choice
            .subtree_diff((5, Hash::default()), (5, Hash::default()))
            .is_empty());
        assert!(heaviest_subtree_fork_choice
            .subtree_diff((6, Hash::default()), (6, Hash::default()))
            .is_empty());

        // The set reachable from slot 3, excluding subtree 1, is just everything
        // in slot 3 since subtree 1 is an ancestor
        assert_eq!(
            heaviest_subtree_fork_choice.subtree_diff((3, Hash::default()), (1, Hash::default())),
            vec![3, 5, 6]
                .into_iter()
                .map(|s| (s, Hash::default()))
                .collect::<HashSet<_>>()
        );

        // The set reachable from slot 1, excluding subtree 3, is just 1 and
        // the subtree at 2
        assert_eq!(
            heaviest_subtree_fork_choice.subtree_diff((1, Hash::default()), (3, Hash::default())),
            vec![1, 2, 4]
                .into_iter()
                .map(|s| (s, Hash::default()))
                .collect::<HashSet<_>>()
        );

        // The set reachable from slot 1, excluding leaf 6, is just everything
        // except leaf 6
        assert_eq!(
            heaviest_subtree_fork_choice.subtree_diff((0, Hash::default()), (6, Hash::default())),
            vec![0, 1, 3, 5, 2, 4]
                .into_iter()
                .map(|s| (s, Hash::default()))
                .collect::<HashSet<_>>()
        );

        // Set root at 1
        heaviest_subtree_fork_choice.set_root((1, Hash::default()));

        // Zero no longer exists, set reachable from 0 is empty
        assert!(heaviest_subtree_fork_choice
            .subtree_diff((0, Hash::default()), (6, Hash::default()))
            .is_empty());
    }

    #[test]
    fn test_stray_restored_slot() {
        let forks = tr(0) / (tr(1) / tr(2));
        let heaviest_subtree_fork_choice = HeaviestSubtreeForkChoice::new_from_tree(forks);

        let mut tower = Tower::new_for_tests(10, 0.9);
        tower.record_vote(1, Hash::default());

        assert_eq!(tower.is_stray_last_vote(), false);
        assert_eq!(
            heaviest_subtree_fork_choice.heaviest_slot_on_same_voted_fork(&tower),
            Some((2, Hash::default()))
        );

        // Make slot 1 (existing in bank_forks) a restored stray slot
        let mut slot_history = SlotHistory::default();
        slot_history.add(0);
        // Work around TooOldSlotHistory
        slot_history.add(999);
        tower = tower
            .adjust_lockouts_after_replay(0, &slot_history)
            .unwrap();

        assert_eq!(tower.is_stray_last_vote(), true);
        assert_eq!(
            heaviest_subtree_fork_choice.heaviest_slot_on_same_voted_fork(&tower),
            Some((2, Hash::default()))
        );

        // Make slot 3 (NOT existing in bank_forks) a restored stray slot
        tower.record_vote(3, Hash::default());
        tower = tower
            .adjust_lockouts_after_replay(0, &slot_history)
            .unwrap();

        assert_eq!(tower.is_stray_last_vote(), true);
        assert_eq!(
            heaviest_subtree_fork_choice.heaviest_slot_on_same_voted_fork(&tower),
            None
        );
    }

    #[test]
    fn test_mark_valid_invalid_forks() {
        let mut heaviest_subtree_fork_choice = setup_forks();
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(3, stake);

        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], (6, Hash::default())),
            (vote_pubkeys[1], (6, Hash::default())),
            (vote_pubkeys[2], (2, Hash::default())),
        ];
        let expected_best_slot = 6;
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            (expected_best_slot, Hash::default()),
        );

        // Mark slot 5 as invalid, the best fork should be its ancestor 3,
        // not the other for at 4.
        let invalid_candidate = (5, Hash::default());
        heaviest_subtree_fork_choice.mark_fork_invalid_candidate(&invalid_candidate);
        assert_eq!(heaviest_subtree_fork_choice.best_overall_slot().0, 3);
        assert!(!heaviest_subtree_fork_choice
            .is_candidate_slot(&invalid_candidate)
            .unwrap());

        // The ancestor is still a candidate
        assert!(heaviest_subtree_fork_choice
            .is_candidate_slot(&(3, Hash::default()))
            .unwrap());

        // Adding another descendant to the invalid candidate won't
        // update the best slot, even if it contains votes
        let new_leaf_slot7 = 7;
        heaviest_subtree_fork_choice.add_new_leaf_slot(
            (new_leaf_slot7, Hash::default()),
            Some((6, Hash::default())),
        );
        let invalid_slot_ancestor = 3;
        assert_eq!(
            heaviest_subtree_fork_choice.best_overall_slot().0,
            invalid_slot_ancestor
        );
        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> =
            vec![(vote_pubkeys[0], (new_leaf_slot7, Hash::default()))];
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule()
            ),
            (invalid_slot_ancestor, Hash::default()),
        );

        // Adding a descendant to the ancestor of the invalid candidate *should* update
        // the best slot though, since the ancestor is on the heaviest fork
        let new_leaf_slot8 = 8;
        heaviest_subtree_fork_choice.add_new_leaf_slot(
            (new_leaf_slot8, Hash::default()),
            Some((invalid_slot_ancestor, Hash::default())),
        );
        assert_eq!(
            heaviest_subtree_fork_choice.best_overall_slot().0,
            new_leaf_slot8,
        );

        // If we mark slot a descendant of `invalid_candidate` as valid, then that
        // should also mark `invalid_candidate` as valid, and the best slot should
        // be the leaf of the heaviest fork, `new_leaf_slot`.
        heaviest_subtree_fork_choice.mark_fork_valid_candidate(&invalid_candidate);
        assert!(heaviest_subtree_fork_choice
            .is_candidate_slot(&invalid_candidate)
            .unwrap());
        assert_eq!(
            heaviest_subtree_fork_choice.best_overall_slot().0,
            // Should pick the smaller slot of the two new equally weighted leaves
            new_leaf_slot7
        );
    }

    #[test]
    fn test_mark_valid_invalid_forks_duplicate() {
        let (
            mut heaviest_subtree_fork_choice,
            duplicate_leaves_descended_from_4,
            duplicate_leaves_descended_from_5,
        ) = setup_duplicate_forks();
        let stake = 100;
        let (bank, vote_pubkeys) = bank_utils::setup_bank_and_vote_pubkeys(3, stake);

        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], duplicate_leaves_descended_from_4[0]),
            (vote_pubkeys[1], duplicate_leaves_descended_from_5[0]),
        ];

        // The best slot should be the the smallest leaf descended from 4
        assert_eq!(
            heaviest_subtree_fork_choice.add_votes(
                pubkey_votes.iter(),
                bank.epoch_stakes_map(),
                bank.epoch_schedule(),
            ),
            duplicate_leaves_descended_from_4[0]
        );

        // If we mark slot 4 as invalid, the ancestor 2 should be the heaviest, not
        // the other branch at slot 5
        let invalid_candidate = (4, Hash::default());
        heaviest_subtree_fork_choice.mark_fork_invalid_candidate(&invalid_candidate);

        assert_eq!(
            heaviest_subtree_fork_choice.best_overall_slot(),
            (2, Hash::default())
        );

        // Marking candidate as valid again will choose the the heaviest leaf of
        // the newly valid branch
        let duplicate_slot = duplicate_leaves_descended_from_4[0].0;
        let duplicate_descendant = (duplicate_slot + 1, Hash::new_unique());
        heaviest_subtree_fork_choice.add_new_leaf_slot(
            duplicate_descendant,
            Some(duplicate_leaves_descended_from_4[0]),
        );
        heaviest_subtree_fork_choice.mark_fork_valid_candidate(&invalid_candidate);
        assert_eq!(
            heaviest_subtree_fork_choice.best_overall_slot(),
            duplicate_descendant
        );

        // Mark the current heaviest branch as invalid again
        heaviest_subtree_fork_choice.mark_fork_invalid_candidate(&invalid_candidate);

        // If we add a new version of the duplicate slot that is not descended from the invalid
        // candidate and votes for that duplicate slot, the new duplicate slot should be picked
        // once it has more weight
        let new_duplicate_hash = Hash::default();
        // The hash has to be smaller in order for the votes to be counted
        assert!(new_duplicate_hash < duplicate_leaves_descended_from_4[0].1);
        let new_duplicate = (duplicate_slot, new_duplicate_hash);
        heaviest_subtree_fork_choice.add_new_leaf_slot(new_duplicate, Some((3, Hash::default())));

        let pubkey_votes: Vec<(Pubkey, SlotHashKey)> = vec![
            (vote_pubkeys[0], new_duplicate),
            (vote_pubkeys[1], new_duplicate),
        ];

        heaviest_subtree_fork_choice.add_votes(
            pubkey_votes.iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        assert_eq!(
            heaviest_subtree_fork_choice.best_overall_slot(),
            new_duplicate
        );
    }

    fn setup_forks() -> HeaviestSubtreeForkChoice {
        /*
            Build fork structure:
                 slot 0
                   |
                 slot 1
                 /    \
            slot 2    |
               |    slot 3
            slot 4    |
                    slot 5
                      |
                    slot 6
        */
        let forks = tr(0) / (tr(1) / (tr(2) / (tr(4))) / (tr(3) / (tr(5) / (tr(6)))));
        HeaviestSubtreeForkChoice::new_from_tree(forks)
    }

    fn setup_duplicate_forks() -> (
        HeaviestSubtreeForkChoice,
        Vec<SlotHashKey>,
        Vec<SlotHashKey>,
    ) {
        /*
                Build fork structure:
                     slot 0
                       |
                     slot 1
                     /       \
                slot 2        |
                   |          slot 3
                slot 4               \
                /    \                slot 5
        slot 10      slot 10        /     |     \
                             slot 6   slot 10   slot 10
            */

        let mut heaviest_subtree_fork_choice = setup_forks();
        let duplicate_slot = 10;
        let mut duplicate_leaves_descended_from_4 =
            std::iter::repeat_with(|| (duplicate_slot, Hash::new_unique()))
                .take(2)
                .collect::<Vec<_>>();
        let mut duplicate_leaves_descended_from_5 =
            std::iter::repeat_with(|| (duplicate_slot, Hash::new_unique()))
                .take(2)
                .collect::<Vec<_>>();
        duplicate_leaves_descended_from_4.sort();
        duplicate_leaves_descended_from_5.sort();

        // Add versions of leaf 10, some with different ancestors, some with the same
        // ancestors
        for duplicate_leaf in &duplicate_leaves_descended_from_4 {
            heaviest_subtree_fork_choice
                .add_new_leaf_slot(*duplicate_leaf, Some((4, Hash::default())));
        }
        for duplicate_leaf in &duplicate_leaves_descended_from_5 {
            heaviest_subtree_fork_choice
                .add_new_leaf_slot(*duplicate_leaf, Some((5, Hash::default())));
        }

        let mut dup_children = heaviest_subtree_fork_choice
            .children(&(4, Hash::default()))
            .unwrap()
            .to_vec();
        dup_children.sort();
        assert_eq!(dup_children, duplicate_leaves_descended_from_4);
        let mut dup_children: Vec<_> = heaviest_subtree_fork_choice
            .children(&(5, Hash::default()))
            .unwrap()
            .iter()
            .copied()
            .filter(|(slot, _)| *slot == duplicate_slot)
            .collect();
        dup_children.sort();
        assert_eq!(dup_children, duplicate_leaves_descended_from_5);

        (
            heaviest_subtree_fork_choice,
            duplicate_leaves_descended_from_4,
            duplicate_leaves_descended_from_5,
        )
    }

    fn check_process_update_correctness<F>(
        heaviest_subtree_fork_choice: &mut HeaviestSubtreeForkChoice,
        pubkey_votes: &[(Pubkey, SlotHashKey)],
        slots_range: Range<Slot>,
        bank: &Bank,
        stake: u64,
        mut expected_best_slot: F,
    ) where
        F: FnMut(Slot, &HeaviestSubtreeForkChoice) -> Slot,
    {
        let unique_votes: HashSet<Slot> = pubkey_votes.iter().map(|(_, (slot, _))| *slot).collect();
        let vote_ancestors: HashMap<Slot, HashSet<SlotHashKey>> = unique_votes
            .iter()
            .map(|v| {
                (
                    *v,
                    heaviest_subtree_fork_choice
                        .ancestor_iterator((*v, Hash::default()))
                        .collect(),
                )
            })
            .collect();
        let mut vote_count: HashMap<Slot, usize> = HashMap::new();
        for (_, vote) in pubkey_votes {
            vote_count
                .entry(vote.0)
                .and_modify(|c| *c += 1)
                .or_insert(1);
        }

        // Maps a slot to the number of descendants of that slot
        // that have been voted on
        let num_voted_descendants: HashMap<Slot, usize> = slots_range
            .clone()
            .map(|slot| {
                let num_voted_descendants = vote_ancestors
                    .iter()
                    .map(|(vote_slot, ancestors)| {
                        (ancestors.contains(&(slot, Hash::default())) || *vote_slot == slot)
                            as usize
                            * vote_count.get(vote_slot).unwrap()
                    })
                    .sum();
                (slot, num_voted_descendants)
            })
            .collect();

        let update_operations_batch = heaviest_subtree_fork_choice.generate_update_operations(
            pubkey_votes.iter(),
            bank.epoch_stakes_map(),
            bank.epoch_schedule(),
        );

        heaviest_subtree_fork_choice.process_update_operations(update_operations_batch);
        for slot in slots_range {
            let expected_stake_voted_at =
                vote_count.get(&slot).cloned().unwrap_or(0) as u64 * stake;
            let expected_stake_voted_subtree =
                *num_voted_descendants.get(&slot).unwrap() as u64 * stake;
            assert_eq!(
                expected_stake_voted_at,
                heaviest_subtree_fork_choice
                    .stake_voted_at(&(slot, Hash::default()))
                    .unwrap()
            );
            assert_eq!(
                expected_stake_voted_subtree,
                heaviest_subtree_fork_choice
                    .stake_voted_subtree(&(slot, Hash::default()))
                    .unwrap()
            );
            assert_eq!(
                expected_best_slot(slot, heaviest_subtree_fork_choice),
                heaviest_subtree_fork_choice
                    .best_slot(&(slot, Hash::default()))
                    .unwrap()
                    .0
            );
        }
    }
}
