# Blocktree

After a block reaches finality, all blocks from that one on down
to the genesis block form a linear chain with the familiar name
blockchain. Until that point, however, the validator must maintain all
potentially valid chains, called *forks*. The process by which forks
naturally form as a result of leader rotation is described in
[fork generation](fork-generation.md). The *blocktree* data structure
described here is how a validator copes with those forks until blocks
are finalized.

The blocktree allows a validator to record every blob it observes
on the network, in any order, as long as the blob is signed by the expected
leader for a given slot.

Blobs are moved to a fork-able key space the tuple of `leader slot` + `blob
index` (within the slot).  This permits the skip-list structure of the Solana
protocol to be stored in its entirety, without a-priori choosing which fork to
follow, which Entries to persist or when to persist them.

Repair requests for recent blobs are served out of RAM or recent files and out
of deeper storage for less recent blobs, as implemented by the store backing
Blocktree.

### Functionalities of Blocktree

1. Persistence: the Blocktree lives in the front of the nodes verification
   pipeline, right behind network receive and signature verification.  If the
blob received is consistent with the leader schedule (i.e. was signed by the
leader for the indicated slot), it is immediately stored.
2. Repair: repair is the same as window repair above, but able to serve any
   blob that's been received. Blocktree stores blobs with signatures,
preserving the chain of origination.
3. Forks: Blocktree supports random access of blobs, so can support a
   validator's need to rollback and replay from a Bank checkpoint.
4. Restart: with proper pruning/culling, the Blocktree can be replayed by
   ordered enumeration of entries from slot 0.  The logic of the replay stage
(i.e. dealing with forks) will have to be used for the most recent entries in
the Blocktree.

### Blocktree Design

1. Entries in the Blocktree are stored as key-value pairs, where the key is the concatenated
slot index and blob index for an entry, and the value is the entry data. Note blob indexes are zero-based for each slot (i.e. they're slot-relative).

2. The Blocktree maintains metadata for each slot, in the `SlotMeta` struct containing:
      * `slot_index` - The index of this slot
      * `num_blocks` - The number of blocks in the slot (used for chaining to a previous slot)
      * `consumed` - The highest blob index `n`, such that for all `m < n`, there exists a blob in this slot with blob index equal to `n` (i.e. the highest consecutive blob index).
      * `received` - The highest received blob index for the slot
      * `next_slots` - A list of future slots this slot could chain to. Used when rebuilding
      the ledger to find possible fork points.
      * `last_index` - The index of the blob that is flagged as the last blob for this slot. This flag on a blob will be set by the leader for a slot when they are transmitting the last blob for a slot.
      * `is_rooted` - True iff every block from 0...slot forms a full sequence without any holes. We can derive is_rooted for each slot with the following rules. Let slot(n) be the slot with index `n`, and slot(n).is_full() is true if the slot with index `n` has all the ticks expected for that slot. Let is_rooted(n) be the statement that "the slot(n).is_rooted is true". Then:

      is_rooted(0)
      is_rooted(n+1) iff (is_rooted(n) and slot(n).is_full()

3. Chaining - When a blob for a new slot `x` arrives, we check the number of blocks (`num_blocks`) for that new slot (this information is encoded in the blob). We then know that this new slot chains to slot `x - num_blocks`.

4. Subscriptions - The Blocktree records a set of slots that have been "subscribed" to. This means entries that chain to these slots will be sent on the Blocktree channel for consumption by the ReplayStage. See the `Blocktree APIs` for details.

5. Update notifications - The Blocktree notifies listeners when slot(n).is_rooted is flipped from false to true for any `n`.

### Blocktree APIs

The Blocktree offers a subscription based API that ReplayStage uses to ask for entries it's interested in. The entries will be sent on a channel exposed by the Blocktree. These subscription API's are as follows:
   1. `fn get_slots_since(slot_indexes: &[u64]) -> Vec<SlotMeta>`: Returns new slots connecting to any element of the list `slot_indexes`.

   2. `fn get_slot_entries(slot_index: u64, entry_start_index: usize, max_entries: Option<u64>) -> Vec<Entry>`: Returns the entry vector for the slot starting with `entry_start_index`, capping the result at `max` if `max_entries == Some(max)`, otherwise, no upper limit on the length of the return vector is imposed.

Note: Cumulatively, this means that the replay stage will now have to know when a slot is finished, and subscribe to the next slot it's interested in to get the next set of entries. Previously, the burden of chaining slots fell on the Blocktree.

### Interfacing with Bank

The bank exposes to replay stage:

 1. `prev_hash`: which PoH chain it's working on as indicated by the hash of the last
    entry it processed
 2. `tick_height`: the ticks in the PoH chain currently being verified by this
    bank
 3. `votes`: a stack of records that contain:

    1. `prev_hashes`: what anything after this vote must chain to in PoH
    2. `tick_height`: the tick height at which this vote was cast
    3. `lockout period`: how long a chain must be observed to be in the ledger to
       be able to be chained below this vote

Replay stage uses Blocktree APIs to find the longest chain of entries it can
hang off a previous vote.  If that chain of entries does not hang off the
latest vote, the replay stage rolls back the bank to that vote and replays the
chain from there.

### Pruning Blocktree

Once Blocktree entries are old enough, representing all the possible forks
becomes less useful, perhaps even problematic for replay upon restart.  Once a
validator's votes have reached max lockout, however, any Blocktree contents
that are not on the PoH chain for that vote for can be pruned, expunged.

Replicator nodes will be responsible for storing really old ledger contents,
and validators need only persist their bank periodically.
