//! RuntimeTransaction is `runtime` facing representation of transaction, while
//! solana_sdk::SanitizedTransaction is client facing representation.
//!
//! It has two states:
//! 1. Statically Loaded: after receiving `packet` from sigverify and deserializing
//!    it into `solana_sdk::VersionedTransaction`, then sanitizing into
//!    `solana_sdk::SanitizedVersionedTransaction`, which can be wrapped into
//!    `RuntimeTransaction` with static transaction metadata extracted.
//! 2. Dynamically Loaded: after successfully loaded account addresses from onchain
//!    ALT, RuntimeTransaction<SanitizedMessage> transits into Dynamically Loaded state,
//!    with its dynamic metadata loaded.
use {
    crate::{
        compute_budget_instruction_details::*,
        signature_details::get_precompile_signature_details,
        transaction_meta::{DynamicMeta, StaticMeta, TransactionMeta},
    },
    core::ops::Deref,
    solana_compute_budget::compute_budget_limits::ComputeBudgetLimits,
    solana_sdk::{
        feature_set::FeatureSet,
        hash::Hash,
        message::{AccountKeys, AddressLoader, TransactionSignatureDetails},
        pubkey::Pubkey,
        signature::Signature,
        simple_vote_transaction_checker::is_simple_vote_transaction,
        transaction::{
            MessageHash, Result, SanitizedTransaction, SanitizedVersionedTransaction,
            VersionedTransaction,
        },
    },
    solana_svm_transaction::{
        instruction::SVMInstruction, message_address_table_lookup::SVMMessageAddressTableLookup,
        svm_message::SVMMessage, svm_transaction::SVMTransaction,
    },
    std::collections::HashSet,
};

#[cfg_attr(feature = "dev-context-only-utils", derive(Clone))]
#[derive(Debug)]
pub struct RuntimeTransaction<T> {
    transaction: T,
    // transaction meta is a collection of fields, it is updated
    // during message state transition
    meta: TransactionMeta,
}

impl<T> StaticMeta for RuntimeTransaction<T> {
    fn message_hash(&self) -> &Hash {
        &self.meta.message_hash
    }
    fn is_simple_vote_transaction(&self) -> bool {
        self.meta.is_simple_vote_transaction
    }
    fn signature_details(&self) -> &TransactionSignatureDetails {
        &self.meta.signature_details
    }
    fn compute_budget_limits(&self, _feature_set: &FeatureSet) -> Result<ComputeBudgetLimits> {
        self.meta
            .compute_budget_instruction_details
            .sanitize_and_convert_to_compute_budget_limits()
    }
}

impl<T: SVMMessage> DynamicMeta for RuntimeTransaction<T> {}

impl<T> Deref for RuntimeTransaction<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.transaction
    }
}

impl RuntimeTransaction<SanitizedVersionedTransaction> {
    pub fn try_from(
        sanitized_versioned_tx: SanitizedVersionedTransaction,
        message_hash: MessageHash,
        is_simple_vote_tx: Option<bool>,
    ) -> Result<Self> {
        let message_hash = match message_hash {
            MessageHash::Precomputed(hash) => hash,
            MessageHash::Compute => sanitized_versioned_tx.get_message().message.hash(),
        };
        let is_simple_vote_tx = is_simple_vote_tx
            .unwrap_or_else(|| is_simple_vote_transaction(&sanitized_versioned_tx));

        let precompile_signature_details = get_precompile_signature_details(
            sanitized_versioned_tx
                .get_message()
                .program_instructions_iter()
                .map(|(program_id, ix)| (program_id, SVMInstruction::from(ix))),
        );
        let signature_details = TransactionSignatureDetails::new(
            u64::from(
                sanitized_versioned_tx
                    .get_message()
                    .message
                    .header()
                    .num_required_signatures,
            ),
            precompile_signature_details.num_secp256k1_instruction_signatures,
            precompile_signature_details.num_ed25519_instruction_signatures,
        );
        let compute_budget_instruction_details = ComputeBudgetInstructionDetails::try_from(
            sanitized_versioned_tx
                .get_message()
                .program_instructions_iter()
                .map(|(program_id, ix)| (program_id, SVMInstruction::from(ix))),
        )?;

        Ok(Self {
            transaction: sanitized_versioned_tx,
            meta: TransactionMeta {
                message_hash,
                is_simple_vote_transaction: is_simple_vote_tx,
                signature_details,
                compute_budget_instruction_details,
            },
        })
    }
}

impl RuntimeTransaction<SanitizedTransaction> {
    /// Create a new `RuntimeTransaction<SanitizedTransaction>` from an
    /// unsanitized `VersionedTransaction`.
    pub fn try_create(
        tx: VersionedTransaction,
        message_hash: MessageHash,
        is_simple_vote_tx: Option<bool>,
        address_loader: impl AddressLoader,
        reserved_account_keys: &HashSet<Pubkey>,
    ) -> Result<Self> {
        let statically_loaded_runtime_tx =
            RuntimeTransaction::<SanitizedVersionedTransaction>::try_from(
                SanitizedVersionedTransaction::try_from(tx)?,
                message_hash,
                is_simple_vote_tx,
            )?;
        Self::try_from(
            statically_loaded_runtime_tx,
            address_loader,
            reserved_account_keys,
        )
    }

    /// Create a new `RuntimeTransaction<SanitizedTransaction>` from a
    /// `RuntimeTransaction<SanitizedVersionedTransaction>` that already has
    /// static metadata loaded.
    pub fn try_from(
        statically_loaded_runtime_tx: RuntimeTransaction<SanitizedVersionedTransaction>,
        address_loader: impl AddressLoader,
        reserved_account_keys: &HashSet<Pubkey>,
    ) -> Result<Self> {
        let hash = *statically_loaded_runtime_tx.message_hash();
        let is_simple_vote_tx = statically_loaded_runtime_tx.is_simple_vote_transaction();
        let sanitized_transaction = SanitizedTransaction::try_new(
            statically_loaded_runtime_tx.transaction,
            hash,
            is_simple_vote_tx,
            address_loader,
            reserved_account_keys,
        )?;

        let mut tx = Self {
            transaction: sanitized_transaction,
            meta: statically_loaded_runtime_tx.meta,
        };
        tx.load_dynamic_metadata()?;

        Ok(tx)
    }

    fn load_dynamic_metadata(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(feature = "dev-context-only-utils")]
impl RuntimeTransaction<SanitizedTransaction> {
    pub fn from_transaction_for_tests(transaction: solana_sdk::transaction::Transaction) -> Self {
        let versioned_transaction = VersionedTransaction::from(transaction);
        Self::try_create(
            versioned_transaction,
            MessageHash::Compute,
            None,
            solana_sdk::message::SimpleAddressLoader::Disabled,
            &HashSet::new(),
        )
        .expect("failed to create RuntimeTransaction from Transaction")
    }
}

#[cfg(feature = "dev-context-only-utils")]
impl<Tx: SVMMessage> RuntimeTransaction<Tx> {
    /// Create simply wrapped transaction with a `TransactionMeta` for testing.
    /// The `TransactionMeta` is default initialized.
    pub fn new_for_tests(transaction: Tx) -> Self {
        Self {
            transaction,
            meta: TransactionMeta {
                message_hash: Hash::default(),
                is_simple_vote_transaction: false,
                signature_details: TransactionSignatureDetails::new(0, 0, 0),
                compute_budget_instruction_details: ComputeBudgetInstructionDetails::default(),
            },
        }
    }
}

impl<T: SVMMessage> SVMMessage for RuntimeTransaction<T> {
    // override to access from the cached meta instead of re-calculating
    fn num_total_signatures(&self) -> u64 {
        self.meta.signature_details.total_signatures()
    }

    fn num_write_locks(&self) -> u64 {
        self.transaction.num_write_locks()
    }

    fn recent_blockhash(&self) -> &Hash {
        self.transaction.recent_blockhash()
    }

    fn num_instructions(&self) -> usize {
        self.transaction.num_instructions()
    }

    fn instructions_iter(&self) -> impl Iterator<Item = SVMInstruction> {
        self.transaction.instructions_iter()
    }

    fn program_instructions_iter(&self) -> impl Iterator<Item = (&Pubkey, SVMInstruction)> {
        self.transaction.program_instructions_iter()
    }

    fn account_keys(&self) -> AccountKeys {
        self.transaction.account_keys()
    }

    fn fee_payer(&self) -> &Pubkey {
        self.transaction.fee_payer()
    }

    fn is_writable(&self, index: usize) -> bool {
        self.transaction.is_writable(index)
    }

    fn is_signer(&self, index: usize) -> bool {
        self.transaction.is_signer(index)
    }

    fn is_invoked(&self, key_index: usize) -> bool {
        self.transaction.is_invoked(key_index)
    }

    fn num_lookup_tables(&self) -> usize {
        self.transaction.num_lookup_tables()
    }

    fn message_address_table_lookups(&self) -> impl Iterator<Item = SVMMessageAddressTableLookup> {
        self.transaction.message_address_table_lookups()
    }
}

impl<T: SVMTransaction> SVMTransaction for RuntimeTransaction<T> {
    fn signature(&self) -> &Signature {
        self.transaction.signature()
    }

    fn signatures(&self) -> &[Signature] {
        self.transaction.signatures()
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        solana_program::{
            system_instruction,
            vote::{self, state::Vote},
        },
        solana_sdk::{
            compute_budget::ComputeBudgetInstruction,
            instruction::Instruction,
            message::Message,
            reserved_account_keys::ReservedAccountKeys,
            signer::{keypair::Keypair, Signer},
            transaction::{SimpleAddressLoader, Transaction, VersionedTransaction},
        },
    };

    fn vote_sanitized_versioned_transaction() -> SanitizedVersionedTransaction {
        let bank_hash = Hash::new_unique();
        let block_hash = Hash::new_unique();
        let vote_keypair = Keypair::new();
        let node_keypair = Keypair::new();
        let auth_keypair = Keypair::new();
        let votes = Vote::new(vec![1, 2, 3], bank_hash);
        let vote_ix =
            vote::instruction::vote(&vote_keypair.pubkey(), &auth_keypair.pubkey(), votes);
        let mut vote_tx = Transaction::new_with_payer(&[vote_ix], Some(&node_keypair.pubkey()));
        vote_tx.partial_sign(&[&node_keypair], block_hash);
        vote_tx.partial_sign(&[&auth_keypair], block_hash);

        SanitizedVersionedTransaction::try_from(VersionedTransaction::from(vote_tx)).unwrap()
    }

    fn non_vote_sanitized_versioned_transaction() -> SanitizedVersionedTransaction {
        TestTransaction::new().to_sanitized_versioned_transaction()
    }

    // Simple transfer transaction for testing, it does not support vote instruction
    // because simple vote transaction will not request limits
    struct TestTransaction {
        from_keypair: Keypair,
        hash: Hash,
        instructions: Vec<Instruction>,
    }

    impl TestTransaction {
        fn new() -> Self {
            let from_keypair = Keypair::new();
            let instructions = vec![system_instruction::transfer(
                &from_keypair.pubkey(),
                &solana_sdk::pubkey::new_rand(),
                1,
            )];
            TestTransaction {
                from_keypair,
                hash: Hash::new_unique(),
                instructions,
            }
        }

        fn add_compute_unit_limit(&mut self, val: u32) -> &mut TestTransaction {
            self.instructions
                .push(ComputeBudgetInstruction::set_compute_unit_limit(val));
            self
        }

        fn add_compute_unit_price(&mut self, val: u64) -> &mut TestTransaction {
            self.instructions
                .push(ComputeBudgetInstruction::set_compute_unit_price(val));
            self
        }

        fn add_loaded_accounts_bytes(&mut self, val: u32) -> &mut TestTransaction {
            self.instructions
                .push(ComputeBudgetInstruction::set_loaded_accounts_data_size_limit(val));
            self
        }

        fn to_sanitized_versioned_transaction(&self) -> SanitizedVersionedTransaction {
            let message = Message::new(&self.instructions, Some(&self.from_keypair.pubkey()));
            let tx = Transaction::new(&[&self.from_keypair], message, self.hash);
            SanitizedVersionedTransaction::try_from(VersionedTransaction::from(tx)).unwrap()
        }
    }

    #[test]
    fn test_runtime_transaction_is_vote_meta() {
        fn get_is_simple_vote(
            svt: SanitizedVersionedTransaction,
            is_simple_vote: Option<bool>,
        ) -> bool {
            RuntimeTransaction::<SanitizedVersionedTransaction>::try_from(
                svt,
                MessageHash::Compute,
                is_simple_vote,
            )
            .unwrap()
            .meta
            .is_simple_vote_transaction
        }

        assert!(!get_is_simple_vote(
            non_vote_sanitized_versioned_transaction(),
            None
        ));

        assert!(get_is_simple_vote(
            non_vote_sanitized_versioned_transaction(),
            Some(true), // override
        ));

        assert!(get_is_simple_vote(
            vote_sanitized_versioned_transaction(),
            None
        ));

        assert!(!get_is_simple_vote(
            vote_sanitized_versioned_transaction(),
            Some(false), // override
        ));
    }

    #[test]
    fn test_advancing_transaction_type() {
        let hash = Hash::new_unique();

        let statically_loaded_transaction =
            RuntimeTransaction::<SanitizedVersionedTransaction>::try_from(
                non_vote_sanitized_versioned_transaction(),
                MessageHash::Precomputed(hash),
                None,
            )
            .unwrap();

        assert_eq!(hash, *statically_loaded_transaction.message_hash());
        assert!(!statically_loaded_transaction.is_simple_vote_transaction());

        let dynamically_loaded_transaction = RuntimeTransaction::<SanitizedTransaction>::try_from(
            statically_loaded_transaction,
            SimpleAddressLoader::Disabled,
            &ReservedAccountKeys::empty_key_set(),
        );
        let dynamically_loaded_transaction =
            dynamically_loaded_transaction.expect("created from statically loaded tx");

        assert_eq!(hash, *dynamically_loaded_transaction.message_hash());
        assert!(!dynamically_loaded_transaction.is_simple_vote_transaction());
    }

    #[test]
    fn test_runtime_transaction_static_meta() {
        let hash = Hash::new_unique();
        let compute_unit_limit = 250_000;
        let compute_unit_price = 1_000;
        let loaded_accounts_bytes = 1_024;
        let mut test_transaction = TestTransaction::new();

        let runtime_transaction_static =
            RuntimeTransaction::<SanitizedVersionedTransaction>::try_from(
                test_transaction
                    .add_compute_unit_limit(compute_unit_limit)
                    .add_compute_unit_price(compute_unit_price)
                    .add_loaded_accounts_bytes(loaded_accounts_bytes)
                    .to_sanitized_versioned_transaction(),
                MessageHash::Precomputed(hash),
                None,
            )
            .unwrap();

        assert_eq!(&hash, runtime_transaction_static.message_hash());
        assert!(!runtime_transaction_static.is_simple_vote_transaction());

        let signature_details = &runtime_transaction_static.meta.signature_details;
        assert_eq!(1, signature_details.num_transaction_signatures());
        assert_eq!(0, signature_details.num_secp256k1_instruction_signatures());
        assert_eq!(0, signature_details.num_ed25519_instruction_signatures());

        let compute_budget_limits = runtime_transaction_static
            .compute_budget_limits(&FeatureSet::default())
            .unwrap();
        assert_eq!(compute_unit_limit, compute_budget_limits.compute_unit_limit);
        assert_eq!(compute_unit_price, compute_budget_limits.compute_unit_price);
        assert_eq!(
            loaded_accounts_bytes,
            compute_budget_limits.loaded_accounts_bytes.get()
        );
    }
}
