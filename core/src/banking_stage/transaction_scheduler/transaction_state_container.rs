use {
    super::{
        transaction_priority_id::TransactionPriorityId,
        transaction_state::{SanitizedTransactionTTL, TransactionState},
    },
    crate::banking_stage::{
        immutable_deserialized_packet::ImmutableDeserializedPacket,
        scheduler_messages::TransactionId,
    },
    itertools::MinMaxResult,
    min_max_heap::MinMaxHeap,
    slab::Slab,
    solana_runtime_transaction::transaction_with_meta::TransactionWithMeta,
    std::sync::Arc,
};

/// This structure will hold `TransactionState` for the entirety of a
/// transaction's lifetime in the scheduler and BankingStage as a whole.
///
/// Transaction Lifetime:
/// 1. Received from `SigVerify` by `BankingStage`
/// 2. Inserted into `TransactionStateContainer` by `BankingStage`
/// 3. Popped in priority-order by scheduler, and transitioned to `Pending` state
/// 4. Processed by `ConsumeWorker`
///    a. If consumed, remove `Pending` state from the `TransactionStateContainer`
///    b. If retryable, transition back to `Unprocessed` state.
///       Re-insert to the queue, and return to step 3.
///
/// The structure is composed of two main components:
/// 1. A priority queue of wrapped `TransactionId`s, which are used to
///    order transactions by priority for selection by the scheduler.
/// 2. A map of `TransactionId` to `TransactionState`, which is used to
///    track the state of each transaction.
///
/// When `Pending`, the associated `TransactionId` is not in the queue, but
/// is still in the map.
/// The entry in the map should exist before insertion into the queue, and be
/// be removed only after the id is removed from the queue.
///
/// The container maintains a fixed capacity. If the queue is full when pushing
/// a new transaction, the lowest priority transaction will be dropped.
pub(crate) struct TransactionStateContainer<Tx: TransactionWithMeta> {
    priority_queue: MinMaxHeap<TransactionPriorityId>,
    id_to_transaction_state: Slab<TransactionState<Tx>>,
}

pub(crate) trait StateContainer<Tx: TransactionWithMeta> {
    /// Create a new `TransactionStateContainer` with the given capacity.
    fn with_capacity(capacity: usize) -> Self;

    /// Returns true if the queue is empty.
    fn is_empty(&self) -> bool;

    /// Returns the remaining capacity of the container
    fn remaining_capacity(&self) -> usize;

    /// Get the top transaction id in the priority queue.
    fn pop(&mut self) -> Option<TransactionPriorityId>;

    /// Get mutable transaction state by id.
    fn get_mut_transaction_state(&mut self, id: TransactionId)
        -> Option<&mut TransactionState<Tx>>;

    /// Get reference to `SanitizedTransactionTTL` by id.
    /// Panics if the transaction does not exist.
    fn get_transaction_ttl(&self, id: TransactionId) -> Option<&SanitizedTransactionTTL<Tx>>;

    /// Insert a new transaction into the container's queues and maps.
    /// Returns `true` if a packet was dropped due to capacity limits.
    fn insert_new_transaction(
        &mut self,
        transaction_ttl: SanitizedTransactionTTL<Tx>,
        packet: Arc<ImmutableDeserializedPacket>,
        priority: u64,
        cost: u64,
    ) -> bool;

    /// Retries a transaction - inserts transaction back into map (but not packet).
    /// This transitions the transaction to `Unprocessed` state.
    fn retry_transaction(
        &mut self,
        transaction_id: TransactionId,
        transaction_ttl: SanitizedTransactionTTL<Tx>,
    );

    /// Pushes a transaction id into the priority queue. If the queue is full, the lowest priority
    /// transaction will be dropped (removed from the queue and map).
    /// Returns `true` if a packet was dropped due to capacity limits.
    fn push_id_into_queue(&mut self, priority_id: TransactionPriorityId) -> bool;

    /// Remove transaction by id.
    fn remove_by_id(&mut self, id: TransactionId);

    fn get_min_max_priority(&self) -> MinMaxResult<u64>;
}

impl<Tx: TransactionWithMeta> StateContainer<Tx> for TransactionStateContainer<Tx> {
    fn with_capacity(capacity: usize) -> Self {
        // Extra capacity is added because some additional space is needed when
        // pushing a new transaction into the container to avoid reallocation.
        const EXTRA_CAPACITY: usize = 64;
        Self {
            priority_queue: MinMaxHeap::with_capacity(capacity),
            id_to_transaction_state: Slab::with_capacity(capacity + EXTRA_CAPACITY),
        }
    }

    fn is_empty(&self) -> bool {
        self.priority_queue.is_empty()
    }

    fn remaining_capacity(&self) -> usize {
        self.priority_queue
            .capacity()
            .saturating_sub(self.id_to_transaction_state.len())
    }

    fn pop(&mut self) -> Option<TransactionPriorityId> {
        self.priority_queue.pop_max()
    }

    fn get_mut_transaction_state(
        &mut self,
        id: TransactionId,
    ) -> Option<&mut TransactionState<Tx>> {
        self.id_to_transaction_state.get_mut(id)
    }

    fn get_transaction_ttl(&self, id: TransactionId) -> Option<&SanitizedTransactionTTL<Tx>> {
        self.id_to_transaction_state
            .get(id)
            .map(|state| state.transaction_ttl())
    }

    fn insert_new_transaction(
        &mut self,
        transaction_ttl: SanitizedTransactionTTL<Tx>,
        packet: Arc<ImmutableDeserializedPacket>,
        priority: u64,
        cost: u64,
    ) -> bool {
        // cache the remaining capacity **before** we take ownership of
        // the next vacant entry. i.e. get the size before we insert.
        let remaining_capacity = self.remaining_capacity();
        let priority_id = {
            let entry = self.id_to_transaction_state.vacant_entry();
            let transaction_id = entry.key();
            entry.insert(TransactionState::new(
                transaction_ttl,
                packet,
                priority,
                cost,
            ));
            TransactionPriorityId::new(priority, transaction_id)
        };

        self.push_id_into_queue_with_remaining_capacity(priority_id, remaining_capacity)
    }

    fn retry_transaction(
        &mut self,
        transaction_id: TransactionId,
        transaction_ttl: SanitizedTransactionTTL<Tx>,
    ) {
        let transaction_state = self
            .get_mut_transaction_state(transaction_id)
            .expect("transaction must exist");
        let priority_id = TransactionPriorityId::new(transaction_state.priority(), transaction_id);
        transaction_state.transition_to_unprocessed(transaction_ttl);
        self.push_id_into_queue(priority_id);
    }

    fn push_id_into_queue(&mut self, priority_id: TransactionPriorityId) -> bool {
        self.push_id_into_queue_with_remaining_capacity(priority_id, self.remaining_capacity())
    }

    fn remove_by_id(&mut self, id: TransactionId) {
        self.id_to_transaction_state.remove(id);
    }

    fn get_min_max_priority(&self) -> MinMaxResult<u64> {
        match self.priority_queue.peek_min() {
            Some(min) => match self.priority_queue.peek_max() {
                Some(max) => MinMaxResult::MinMax(min.priority, max.priority),
                None => MinMaxResult::OneElement(min.priority),
            },
            None => MinMaxResult::NoElements,
        }
    }
}

impl<Tx: TransactionWithMeta> TransactionStateContainer<Tx> {
    fn push_id_into_queue_with_remaining_capacity(
        &mut self,
        priority_id: TransactionPriorityId,
        remaining_capacity: usize,
    ) -> bool {
        if remaining_capacity == 0 {
            let popped_id = self.priority_queue.push_pop_min(priority_id);
            self.remove_by_id(popped_id.id);
            true
        } else {
            self.priority_queue.push(priority_id);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::banking_stage::scheduler_messages::MaxAge,
        solana_runtime_transaction::runtime_transaction::RuntimeTransaction,
        solana_sdk::{
            compute_budget::ComputeBudgetInstruction,
            hash::Hash,
            message::Message,
            packet::Packet,
            signature::Keypair,
            signer::Signer,
            system_instruction,
            transaction::{SanitizedTransaction, Transaction},
        },
    };

    /// Returns (transaction_ttl, priority, cost)
    fn test_transaction(
        priority: u64,
    ) -> (
        SanitizedTransactionTTL<RuntimeTransaction<SanitizedTransaction>>,
        Arc<ImmutableDeserializedPacket>,
        u64,
        u64,
    ) {
        let from_keypair = Keypair::new();
        let ixs = vec![
            system_instruction::transfer(
                &from_keypair.pubkey(),
                &solana_sdk::pubkey::new_rand(),
                1,
            ),
            ComputeBudgetInstruction::set_compute_unit_price(priority),
        ];
        let message = Message::new(&ixs, Some(&from_keypair.pubkey()));
        let tx = RuntimeTransaction::from_transaction_for_tests(Transaction::new(
            &[&from_keypair],
            message,
            Hash::default(),
        ));
        let packet = Arc::new(
            ImmutableDeserializedPacket::new(
                Packet::from_data(None, tx.to_versioned_transaction()).unwrap(),
            )
            .unwrap(),
        );
        let transaction_ttl = SanitizedTransactionTTL {
            transaction: tx,
            max_age: MaxAge::MAX,
        };
        const TEST_TRANSACTION_COST: u64 = 5000;
        (transaction_ttl, packet, priority, TEST_TRANSACTION_COST)
    }

    fn push_to_container(
        container: &mut TransactionStateContainer<RuntimeTransaction<SanitizedTransaction>>,
        num: usize,
    ) {
        for priority in 0..num as u64 {
            let (transaction_ttl, packet, priority, cost) = test_transaction(priority);
            container.insert_new_transaction(transaction_ttl, packet, priority, cost);
        }
    }

    #[test]
    fn test_is_empty() {
        let mut container = TransactionStateContainer::with_capacity(1);
        assert!(container.is_empty());

        push_to_container(&mut container, 1);
        assert!(!container.is_empty());
    }

    #[test]
    fn test_priority_queue_capacity() {
        let mut container = TransactionStateContainer::with_capacity(1);
        push_to_container(&mut container, 5);

        assert_eq!(container.priority_queue.len(), 1);
        assert_eq!(container.id_to_transaction_state.len(), 1);
        assert_eq!(
            container
                .id_to_transaction_state
                .iter()
                .map(|ts| ts.1.priority())
                .next()
                .unwrap(),
            4
        );
    }

    #[test]
    fn test_get_mut_transaction_state() {
        let mut container = TransactionStateContainer::with_capacity(5);
        push_to_container(&mut container, 5);

        let existing_id = 3;
        let non_existing_id = 7;
        assert!(container.get_mut_transaction_state(existing_id).is_some());
        assert!(container.get_mut_transaction_state(existing_id).is_some());
        assert!(container
            .get_mut_transaction_state(non_existing_id)
            .is_none());
    }
}
