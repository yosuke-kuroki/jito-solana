#![cfg(feature = "full")]

use {
    crate::{
        hash::Hash,
        message::{v0, MappedAddresses, MappedMessage, SanitizedMessage, VersionedMessage},
        nonce::NONCED_TX_MARKER_IX_INDEX,
        precompiles::verify_if_precompile,
        program_utils::limited_deserialize,
        pubkey::Pubkey,
        sanitize::Sanitize,
        signature::Signature,
        solana_sdk::feature_set,
        transaction::{Result, Transaction, TransactionError, VersionedTransaction},
    },
    solana_program::{system_instruction::SystemInstruction, system_program},
    std::sync::Arc,
};

/// Sanitized transaction and the hash of its message
#[derive(Debug, Clone)]
pub struct SanitizedTransaction {
    message: SanitizedMessage,
    message_hash: Hash,
    is_simple_vote_tx: bool,
    signatures: Vec<Signature>,
}

/// Set of accounts that must be locked for safe transaction processing
#[derive(Debug, Clone, Default)]
pub struct TransactionAccountLocks<'a> {
    /// List of readonly account key locks
    pub readonly: Vec<&'a Pubkey>,
    /// List of writable account key locks
    pub writable: Vec<&'a Pubkey>,
}

impl SanitizedTransaction {
    /// Create a sanitized transaction from an unsanitized transaction.
    /// If the input transaction uses address maps, attempt to map the
    /// transaction keys to full addresses.
    pub fn try_create(
        tx: VersionedTransaction,
        message_hash: Hash,
        is_simple_vote_tx: Option<bool>,
        address_mapper: impl Fn(&v0::Message) -> Result<MappedAddresses>,
    ) -> Result<Self> {
        tx.sanitize()?;

        let signatures = tx.signatures;
        let message = match tx.message {
            VersionedMessage::Legacy(message) => SanitizedMessage::Legacy(message),
            VersionedMessage::V0(message) => SanitizedMessage::V0(MappedMessage {
                mapped_addresses: address_mapper(&message)?,
                message,
            }),
        };

        if message.has_duplicates() {
            return Err(TransactionError::AccountLoadedTwice);
        }

        let is_simple_vote_tx = is_simple_vote_tx.unwrap_or_else(|| {
            let mut ix_iter = message.program_instructions_iter();
            ix_iter.next().map(|(program_id, _ix)| program_id) == Some(&crate::vote::program::id())
        });

        Ok(Self {
            message,
            message_hash,
            is_simple_vote_tx,
            signatures,
        })
    }

    /// Create a sanitized transaction from a legacy transaction. Used for tests only.
    pub fn from_transaction_for_tests(tx: Transaction) -> Self {
        tx.sanitize().unwrap();

        if tx.message.has_duplicates() {
            Result::<Self>::Err(TransactionError::AccountLoadedTwice).unwrap();
        }

        Self {
            message_hash: tx.message.hash(),
            message: SanitizedMessage::Legacy(tx.message),
            is_simple_vote_tx: false,
            signatures: tx.signatures,
        }
    }

    /// Return the first signature for this transaction.
    ///
    /// Notes:
    ///
    /// Sanitized transactions must have at least one signature because the
    /// number of signatures must be greater than or equal to the message header
    /// value `num_required_signatures` which must be greater than 0 itself.
    pub fn signature(&self) -> &Signature {
        &self.signatures[0]
    }

    /// Return the list of signatures for this transaction
    pub fn signatures(&self) -> &[Signature] {
        &self.signatures
    }

    /// Return the signed message
    pub fn message(&self) -> &SanitizedMessage {
        &self.message
    }

    /// Return the hash of the signed message
    pub fn message_hash(&self) -> &Hash {
        &self.message_hash
    }

    /// Returns true if this transaction is a simple vote
    pub fn is_simple_vote_transaction(&self) -> bool {
        self.is_simple_vote_tx
    }

    /// Convert this sanitized transaction into a versioned transaction for
    /// recording in the ledger.
    pub fn to_versioned_transaction(&self) -> VersionedTransaction {
        let signatures = self.signatures.clone();
        match &self.message {
            SanitizedMessage::V0(mapped_msg) => VersionedTransaction {
                signatures,
                message: VersionedMessage::V0(mapped_msg.message.clone()),
            },
            SanitizedMessage::Legacy(message) => VersionedTransaction {
                signatures,
                message: VersionedMessage::Legacy(message.clone()),
            },
        }
    }

    /// Return the list of accounts that must be locked during processing this transaction.
    pub fn get_account_locks(&self, demote_program_write_locks: bool) -> TransactionAccountLocks {
        let message = &self.message;
        let num_readonly_accounts = message.num_readonly_accounts();
        let num_writable_accounts = message
            .account_keys_len()
            .saturating_sub(num_readonly_accounts);

        let mut account_locks = TransactionAccountLocks {
            writable: Vec::with_capacity(num_writable_accounts),
            readonly: Vec::with_capacity(num_readonly_accounts),
        };

        for (i, key) in message.account_keys_iter().enumerate() {
            if message.is_writable(i, demote_program_write_locks) {
                account_locks.writable.push(key);
            } else {
                account_locks.readonly.push(key);
            }
        }

        account_locks
    }

    /// If the transaction uses a durable nonce, return the pubkey of the nonce account
    pub fn get_durable_nonce(&self, nonce_must_be_writable: bool) -> Option<&Pubkey> {
        self.message
            .instructions()
            .get(NONCED_TX_MARKER_IX_INDEX as usize)
            .filter(
                |ix| match self.message.get_account_key(ix.program_id_index as usize) {
                    Some(program_id) => system_program::check_id(program_id),
                    _ => false,
                },
            )
            .filter(|ix| {
                matches!(
                    limited_deserialize(&ix.data),
                    Ok(SystemInstruction::AdvanceNonceAccount)
                )
            })
            .and_then(|ix| {
                ix.accounts.get(0).and_then(|idx| {
                    let idx = *idx as usize;
                    if nonce_must_be_writable && !self.message.is_writable(idx, true) {
                        None
                    } else {
                        self.message.get_account_key(idx)
                    }
                })
            })
    }

    /// Return the serialized message data to sign.
    fn message_data(&self) -> Vec<u8> {
        match &self.message {
            SanitizedMessage::Legacy(message) => message.serialize(),
            SanitizedMessage::V0(mapped_msg) => mapped_msg.message.serialize(),
        }
    }

    /// Verify the length of signatures matches the value in the message header
    pub fn verify_signatures_len(&self) -> bool {
        self.signatures.len() == self.message.header().num_required_signatures as usize
    }

    /// Verify the transaction signatures
    pub fn verify(&self) -> Result<()> {
        let message_bytes = self.message_data();
        if self
            .signatures
            .iter()
            .zip(self.message.account_keys_iter())
            .map(|(signature, pubkey)| signature.verify(pubkey.as_ref(), &message_bytes))
            .any(|verified| !verified)
        {
            Err(TransactionError::SignatureFailure)
        } else {
            Ok(())
        }
    }

    /// Verify the precompiled programs in this transaction
    pub fn verify_precompiles(&self, feature_set: &Arc<feature_set::FeatureSet>) -> Result<()> {
        for (program_id, instruction) in self.message.program_instructions_iter() {
            verify_if_precompile(
                program_id,
                instruction,
                self.message().instructions(),
                feature_set,
            )
            .map_err(|_| TransactionError::InvalidAccountIndex)?;
        }
        Ok(())
    }
}
