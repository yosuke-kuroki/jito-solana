//! A library for generating a message from a sequence of instructions

use crate::sanitize::{Sanitize, SanitizeError};
use crate::serialize_utils::{
    append_slice, append_u16, append_u8, read_pubkey, read_slice, read_u16, read_u8,
};
use crate::{
    hash::Hash,
    instruction::{AccountMeta, CompiledInstruction, Instruction},
    pubkey::Pubkey,
    short_vec, system_instruction,
};
use itertools::Itertools;
use std::convert::TryFrom;

fn position(keys: &[Pubkey], key: &Pubkey) -> u8 {
    keys.iter().position(|k| k == key).unwrap() as u8
}

fn compile_instruction(ix: &Instruction, keys: &[Pubkey]) -> CompiledInstruction {
    let accounts: Vec<_> = ix
        .accounts
        .iter()
        .map(|account_meta| position(keys, &account_meta.pubkey))
        .collect();

    CompiledInstruction {
        program_id_index: position(keys, &ix.program_id),
        data: ix.data.clone(),
        accounts,
    }
}

fn compile_instructions(ixs: &[Instruction], keys: &[Pubkey]) -> Vec<CompiledInstruction> {
    ixs.iter().map(|ix| compile_instruction(ix, keys)).collect()
}

/// A helper struct to collect pubkeys referenced by a set of instructions and read-only counts
#[derive(Debug, PartialEq, Eq)]
struct InstructionKeys {
    pub signed_keys: Vec<Pubkey>,
    pub unsigned_keys: Vec<Pubkey>,
    pub num_readonly_signed_accounts: u8,
    pub num_readonly_unsigned_accounts: u8,
}

impl InstructionKeys {
    fn new(
        signed_keys: Vec<Pubkey>,
        unsigned_keys: Vec<Pubkey>,
        num_readonly_signed_accounts: u8,
        num_readonly_unsigned_accounts: u8,
    ) -> Self {
        Self {
            signed_keys,
            unsigned_keys,
            num_readonly_signed_accounts,
            num_readonly_unsigned_accounts,
        }
    }
}

/// Return pubkeys referenced by all instructions, with the ones needing signatures first. If the
/// payer key is provided, it is always placed first in the list of signed keys. Read-only signed
/// accounts are placed last in the set of signed accounts. Read-only unsigned accounts,
/// including program ids, are placed last in the set. No duplicates and order is preserved.
fn get_keys(instructions: &[Instruction], payer: Option<&Pubkey>) -> InstructionKeys {
    let programs: Vec<_> = get_program_ids(instructions)
        .iter()
        .map(|program_id| AccountMeta {
            pubkey: *program_id,
            is_signer: false,
            is_writable: false,
        })
        .collect();
    let mut keys_and_signed: Vec<_> = instructions
        .iter()
        .flat_map(|ix| ix.accounts.iter())
        .collect();
    keys_and_signed.extend(&programs);
    keys_and_signed.sort_by(|x, y| {
        y.is_signer
            .cmp(&x.is_signer)
            .then(y.is_writable.cmp(&x.is_writable))
    });

    let payer_account_meta;
    if let Some(payer) = payer {
        payer_account_meta = AccountMeta {
            pubkey: *payer,
            is_signer: true,
            is_writable: true,
        };
        keys_and_signed.insert(0, &payer_account_meta);
    }

    let mut unique_metas: Vec<AccountMeta> = vec![];
    for account_meta in keys_and_signed {
        // Promote to writable if a later AccountMeta requires it
        if let Some(x) = unique_metas
            .iter_mut()
            .find(|x| x.pubkey == account_meta.pubkey)
        {
            x.is_writable |= account_meta.is_writable;
            continue;
        }
        unique_metas.push(account_meta.clone());
    }

    let mut signed_keys = vec![];
    let mut unsigned_keys = vec![];
    let mut num_readonly_signed_accounts = 0;
    let mut num_readonly_unsigned_accounts = 0;
    for account_meta in unique_metas {
        if account_meta.is_signer {
            signed_keys.push(account_meta.pubkey);
            if !account_meta.is_writable {
                num_readonly_signed_accounts += 1;
            }
        } else {
            unsigned_keys.push(account_meta.pubkey);
            if !account_meta.is_writable {
                num_readonly_unsigned_accounts += 1;
            }
        }
    }
    InstructionKeys::new(
        signed_keys,
        unsigned_keys,
        num_readonly_signed_accounts,
        num_readonly_unsigned_accounts,
    )
}

/// Return program ids referenced by all instructions.  No duplicates and order is preserved.
fn get_program_ids(instructions: &[Instruction]) -> Vec<Pubkey> {
    instructions
        .iter()
        .map(|ix| ix.program_id)
        .unique()
        .collect()
}

#[frozen_abi(digest = "BVC5RhetsNpheGipt5rUrkR6RDDUHtD5sCLK1UjymL4S")]
#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Eq, Clone, AbiExample)]
#[serde(rename_all = "camelCase")]
pub struct MessageHeader {
    /// The number of signatures required for this message to be considered valid. The
    /// signatures must match the first `num_required_signatures` of `account_keys`.
    /// NOTE: Serialization-related changes must be paired with the direct read at sigverify.
    pub num_required_signatures: u8,

    /// The last num_readonly_signed_accounts of the signed keys are read-only accounts. Programs
    /// may process multiple transactions that load read-only accounts within a single PoH entry,
    /// but are not permitted to credit or debit lamports or modify account data. Transactions
    /// targeting the same read-write account are evaluated sequentially.
    pub num_readonly_signed_accounts: u8,

    /// The last num_readonly_unsigned_accounts of the unsigned keys are read-only accounts.
    pub num_readonly_unsigned_accounts: u8,
}

#[frozen_abi(digest = "BPBJZxpRQ4JS7LGJtsgoyctg4BXyBbbY4uc7FjowtxLV")]
#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Eq, Clone, AbiExample)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// The message header, identifying signed and read-only `account_keys`
    /// NOTE: Serialization-related changes must be paired with the direct read at sigverify.
    pub header: MessageHeader,

    /// All the account keys used by this transaction
    #[serde(with = "short_vec")]
    pub account_keys: Vec<Pubkey>,

    /// The id of a recent ledger entry.
    pub recent_blockhash: Hash,

    /// Programs that will be executed in sequence and committed in one atomic transaction if all
    /// succeed.
    #[serde(with = "short_vec")]
    pub instructions: Vec<CompiledInstruction>,
}

impl Sanitize for Message {
    fn sanitize(&self) -> std::result::Result<(), SanitizeError> {
        // signing area and read-only non-signing area should not overlap
        if self.header.num_required_signatures as usize
            + self.header.num_readonly_unsigned_accounts as usize
            > self.account_keys.len()
        {
            return Err(SanitizeError::IndexOutOfBounds);
        }

        // there should be at least 1 RW fee-payer account.
        if self.header.num_readonly_signed_accounts >= self.header.num_required_signatures {
            return Err(SanitizeError::IndexOutOfBounds);
        }

        for ci in &self.instructions {
            if ci.program_id_index as usize >= self.account_keys.len() {
                return Err(SanitizeError::IndexOutOfBounds);
            }
            // A program cannot be a payer.
            if ci.program_id_index == 0 {
                return Err(SanitizeError::IndexOutOfBounds);
            }
            for ai in &ci.accounts {
                if *ai as usize >= self.account_keys.len() {
                    return Err(SanitizeError::IndexOutOfBounds);
                }
            }
        }
        self.account_keys.sanitize()?;
        self.recent_blockhash.sanitize()?;
        self.instructions.sanitize()?;
        Ok(())
    }
}

impl Message {
    pub fn new_with_compiled_instructions(
        num_required_signatures: u8,
        num_readonly_signed_accounts: u8,
        num_readonly_unsigned_accounts: u8,
        account_keys: Vec<Pubkey>,
        recent_blockhash: Hash,
        instructions: Vec<CompiledInstruction>,
    ) -> Self {
        Self {
            header: MessageHeader {
                num_required_signatures,
                num_readonly_signed_accounts,
                num_readonly_unsigned_accounts,
            },
            account_keys,
            recent_blockhash,
            instructions,
        }
    }

    pub fn new(instructions: &[Instruction], payer: Option<&Pubkey>) -> Self {
        let InstructionKeys {
            mut signed_keys,
            unsigned_keys,
            num_readonly_signed_accounts,
            num_readonly_unsigned_accounts,
        } = get_keys(instructions, payer);
        let num_required_signatures = signed_keys.len() as u8;
        signed_keys.extend(&unsigned_keys);
        let instructions = compile_instructions(instructions, &signed_keys);
        Self::new_with_compiled_instructions(
            num_required_signatures,
            num_readonly_signed_accounts,
            num_readonly_unsigned_accounts,
            signed_keys,
            Hash::default(),
            instructions,
        )
    }

    pub fn new_with_nonce(
        mut instructions: Vec<Instruction>,
        payer: Option<&Pubkey>,
        nonce_account_pubkey: &Pubkey,
        nonce_authority_pubkey: &Pubkey,
    ) -> Self {
        let nonce_ix = system_instruction::advance_nonce_account(
            &nonce_account_pubkey,
            &nonce_authority_pubkey,
        );
        instructions.insert(0, nonce_ix);
        Self::new(&instructions, payer)
    }

    pub fn compile_instruction(&self, ix: &Instruction) -> CompiledInstruction {
        compile_instruction(ix, &self.account_keys)
    }

    pub fn serialize(&self) -> Vec<u8> {
        bincode::serialize(self).unwrap()
    }

    pub fn program_id(&self, instruction_index: usize) -> Option<&Pubkey> {
        Some(
            &self.account_keys[self.instructions.get(instruction_index)?.program_id_index as usize],
        )
    }

    pub fn program_index(&self, instruction_index: usize) -> Option<usize> {
        Some(self.instructions.get(instruction_index)?.program_id_index as usize)
    }

    pub fn program_ids(&self) -> Vec<&Pubkey> {
        self.instructions
            .iter()
            .map(|ix| &self.account_keys[ix.program_id_index as usize])
            .collect()
    }

    pub fn is_key_passed_to_program(&self, index: usize) -> bool {
        if let Ok(index) = u8::try_from(index) {
            for ix in self.instructions.iter() {
                if ix.accounts.contains(&index) {
                    return true;
                }
            }
        }
        false
    }

    pub fn is_non_loader_key(&self, key: &Pubkey, key_index: usize) -> bool {
        !self.program_ids().contains(&key) || self.is_key_passed_to_program(key_index)
    }

    pub fn program_position(&self, index: usize) -> Option<usize> {
        let program_ids = self.program_ids();
        program_ids
            .iter()
            .position(|&&pubkey| pubkey == self.account_keys[index])
    }

    pub fn is_writable(&self, i: usize) -> bool {
        i < (self.header.num_required_signatures - self.header.num_readonly_signed_accounts)
            as usize
            || (i >= self.header.num_required_signatures as usize
                && i < self.account_keys.len()
                    - self.header.num_readonly_unsigned_accounts as usize)
    }

    pub fn is_signer(&self, i: usize) -> bool {
        i < self.header.num_required_signatures as usize
    }

    pub fn get_account_keys_by_lock_type(&self) -> (Vec<&Pubkey>, Vec<&Pubkey>) {
        let mut writable_keys = vec![];
        let mut readonly_keys = vec![];
        for (i, key) in self.account_keys.iter().enumerate() {
            if self.is_writable(i) {
                writable_keys.push(key);
            } else {
                readonly_keys.push(key);
            }
        }
        (writable_keys, readonly_keys)
    }

    // First encode the number of instructions:
    // [0..2 - num_instructions
    //
    // Then a table of offsets of where to find them in the data
    //  3..2*num_instructions table of instruction offsets
    //
    // Each instruction is then encoded as:
    //   0..2 - num_accounts
    //   3 - meta_byte -> (bit 0 signer, bit 1 is_writable)
    //   4..36 - pubkey - 32 bytes
    //   36..64 - program_id
    //     33..34 - data len - u16
    //     35..data_len - data
    pub fn serialize_instructions(&self) -> Vec<u8> {
        // 64 bytes is a reasonable guess, calculating exactly is slower in benchmarks
        let mut data = Vec::with_capacity(self.instructions.len() * (32 * 2));
        append_u16(&mut data, self.instructions.len() as u16);
        for _ in 0..self.instructions.len() {
            append_u16(&mut data, 0);
        }
        for (i, instruction) in self.instructions.iter().enumerate() {
            let start_instruction_offset = data.len() as u16;
            let start = 2 + (2 * i);
            data[start..start + 2].copy_from_slice(&start_instruction_offset.to_le_bytes());
            append_u16(&mut data, instruction.accounts.len() as u16);
            for account_index in &instruction.accounts {
                let account_index = *account_index as usize;
                let is_signer = self.is_signer(account_index);
                let is_writable = self.is_writable(account_index);
                let mut meta_byte = 0;
                if is_signer {
                    meta_byte |= 1 << Self::IS_SIGNER_BIT;
                }
                if is_writable {
                    meta_byte |= 1 << Self::IS_WRITABLE_BIT;
                }
                append_u8(&mut data, meta_byte);
                append_slice(&mut data, self.account_keys[account_index].as_ref());
            }

            let program_id = &self.account_keys[instruction.program_id_index as usize];
            append_slice(&mut data, program_id.as_ref());
            append_u16(&mut data, instruction.data.len() as u16);
            append_slice(&mut data, &instruction.data);
        }
        data
    }

    const IS_SIGNER_BIT: usize = 0;
    const IS_WRITABLE_BIT: usize = 1;

    pub fn deserialize_instruction(
        index: usize,
        data: &[u8],
    ) -> Result<Instruction, SanitizeError> {
        let mut current = 0;
        let _num_instructions = read_u16(&mut current, &data)?;

        // index into the instruction byte-offset table.
        current += index * 2;
        let start = read_u16(&mut current, &data)?;

        current = start as usize;
        let num_accounts = read_u16(&mut current, &data)?;
        let mut accounts = Vec::with_capacity(num_accounts as usize);
        for _ in 0..num_accounts {
            let meta_byte = read_u8(&mut current, &data)?;
            let mut is_signer = false;
            let mut is_writable = false;
            if meta_byte & (1 << Self::IS_SIGNER_BIT) != 0 {
                is_signer = true;
            }
            if meta_byte & (1 << Self::IS_WRITABLE_BIT) != 0 {
                is_writable = true;
            }
            let pubkey = read_pubkey(&mut current, &data)?;
            accounts.push(AccountMeta {
                pubkey,
                is_signer,
                is_writable,
            });
        }
        let program_id = read_pubkey(&mut current, &data)?;
        let data_len = read_u16(&mut current, &data)?;
        let data = read_slice(&mut current, &data, data_len as usize)?;
        Ok(Instruction {
            program_id,
            data,
            accounts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instruction::AccountMeta;

    #[test]
    fn test_message_unique_program_ids() {
        let program_id0 = Pubkey::default();
        let program_ids = get_program_ids(&[
            Instruction::new_with_bincode(program_id0, &0, vec![]),
            Instruction::new_with_bincode(program_id0, &0, vec![]),
        ]);
        assert_eq!(program_ids, vec![program_id0]);
    }

    #[test]
    fn test_message_unique_program_ids_not_adjacent() {
        let program_id0 = Pubkey::default();
        let program_id1 = Pubkey::new_unique();
        let program_ids = get_program_ids(&[
            Instruction::new_with_bincode(program_id0, &0, vec![]),
            Instruction::new_with_bincode(program_id1, &0, vec![]),
            Instruction::new_with_bincode(program_id0, &0, vec![]),
        ]);
        assert_eq!(program_ids, vec![program_id0, program_id1]);
    }

    #[test]
    fn test_message_unique_program_ids_order_preserved() {
        let program_id0 = Pubkey::new_unique();
        let program_id1 = Pubkey::default(); // Key less than program_id0
        let program_ids = get_program_ids(&[
            Instruction::new_with_bincode(program_id0, &0, vec![]),
            Instruction::new_with_bincode(program_id1, &0, vec![]),
            Instruction::new_with_bincode(program_id0, &0, vec![]),
        ]);
        assert_eq!(program_ids, vec![program_id0, program_id1]);
    }

    #[test]
    fn test_message_unique_keys_both_signed() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
            ],
            None,
        );
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_signed_and_payer() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[Instruction::new_with_bincode(
                program_id,
                &0,
                vec![AccountMeta::new(id0, true)],
            )],
            Some(&id0),
        );
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_unsigned_and_payer() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[Instruction::new_with_bincode(
                program_id,
                &0,
                vec![AccountMeta::new(id0, false)],
            )],
            Some(&id0),
        );
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_one_signed() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
            ],
            None,
        );
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_one_readonly_signed() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id0, true)],
                ),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
            ],
            None,
        );

        // Ensure the key is no longer readonly
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_one_readonly_unsigned() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id0, false)],
                ),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
            ],
            None,
        );

        // Ensure the key is no longer readonly
        assert_eq!(keys, InstructionKeys::new(vec![], vec![id0], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_order_preserved() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::new_unique();
        let id1 = Pubkey::default(); // Key less than id0
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, false)]),
            ],
            None,
        );
        assert_eq!(keys, InstructionKeys::new(vec![], vec![id0, id1], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_not_adjacent() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let id1 = Pubkey::new_unique();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
            ],
            None,
        );
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![id1], 0, 0));
    }

    #[test]
    fn test_message_signed_keys_first() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let id1 = Pubkey::new_unique();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, true)]),
            ],
            None,
        );
        assert_eq!(keys, InstructionKeys::new(vec![id1], vec![id0], 0, 0));
    }

    #[test]
    // Ensure there's a way to calculate the number of required signatures.
    fn test_message_signed_keys_len() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]);
        let message = Message::new(&[ix], None);
        assert_eq!(message.header.num_required_signatures, 0);

        let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]);
        let message = Message::new(&[ix], Some(&id0));
        assert_eq!(message.header.num_required_signatures, 1);
    }

    #[test]
    fn test_message_readonly_keys_last() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default(); // Identical key/program_id should be de-duped
        let id1 = Pubkey::new_unique();
        let id2 = Pubkey::new_unique();
        let id3 = Pubkey::new_unique();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id0, false)],
                ),
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id1, true)],
                ),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id2, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id3, true)]),
            ],
            None,
        );
        assert_eq!(
            keys,
            InstructionKeys::new(vec![id3, id1], vec![id2, id0], 1, 1)
        );
    }

    #[test]
    fn test_message_kitchen_sink() {
        let program_id0 = Pubkey::new_unique();
        let program_id1 = Pubkey::new_unique();
        let id0 = Pubkey::default();
        let id1 = Pubkey::new_unique();
        let message = Message::new(
            &[
                Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id1, &0, vec![AccountMeta::new(id1, true)]),
                Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id1, false)]),
            ],
            Some(&id1),
        );
        assert_eq!(
            message.instructions[0],
            CompiledInstruction::new(2, &0, vec![1])
        );
        assert_eq!(
            message.instructions[1],
            CompiledInstruction::new(3, &0, vec![0])
        );
        assert_eq!(
            message.instructions[2],
            CompiledInstruction::new(2, &0, vec![0])
        );
    }

    #[test]
    fn test_message_payer_first() {
        let program_id = Pubkey::default();
        let payer = Pubkey::new_unique();
        let id0 = Pubkey::default();

        let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]);
        let message = Message::new(&[ix], Some(&payer));
        assert_eq!(message.header.num_required_signatures, 1);

        let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]);
        let message = Message::new(&[ix], Some(&payer));
        assert_eq!(message.header.num_required_signatures, 2);

        let ix = Instruction::new_with_bincode(
            program_id,
            &0,
            vec![AccountMeta::new(payer, true), AccountMeta::new(id0, true)],
        );
        let message = Message::new(&[ix], Some(&payer));
        assert_eq!(message.header.num_required_signatures, 2);
    }

    #[test]
    fn test_message_program_last() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::new_unique();
        let id1 = Pubkey::new_unique();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id0, false)],
                ),
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id1, true)],
                ),
            ],
            None,
        );
        assert_eq!(
            keys,
            InstructionKeys::new(vec![id1], vec![id0, program_id], 1, 2)
        );
    }

    #[test]
    fn test_program_position() {
        let program_id0 = Pubkey::default();
        let program_id1 = Pubkey::new_unique();
        let id = Pubkey::new_unique();
        let message = Message::new(
            &[
                Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id, false)]),
                Instruction::new_with_bincode(program_id1, &0, vec![AccountMeta::new(id, true)]),
            ],
            Some(&id),
        );
        assert_eq!(message.program_position(0), None);
        assert_eq!(message.program_position(1), Some(0));
        assert_eq!(message.program_position(2), Some(1));
    }

    #[test]
    fn test_is_writable() {
        let key0 = Pubkey::new_unique();
        let key1 = Pubkey::new_unique();
        let key2 = Pubkey::new_unique();
        let key3 = Pubkey::new_unique();
        let key4 = Pubkey::new_unique();
        let key5 = Pubkey::new_unique();

        let message = Message {
            header: MessageHeader {
                num_required_signatures: 3,
                num_readonly_signed_accounts: 2,
                num_readonly_unsigned_accounts: 1,
            },
            account_keys: vec![key0, key1, key2, key3, key4, key5],
            recent_blockhash: Hash::default(),
            instructions: vec![],
        };
        assert_eq!(message.is_writable(0), true);
        assert_eq!(message.is_writable(1), false);
        assert_eq!(message.is_writable(2), false);
        assert_eq!(message.is_writable(3), true);
        assert_eq!(message.is_writable(4), true);
        assert_eq!(message.is_writable(5), false);
    }

    #[test]
    fn test_get_account_keys_by_lock_type() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::new_unique();
        let id1 = Pubkey::new_unique();
        let id2 = Pubkey::new_unique();
        let id3 = Pubkey::new_unique();
        let message = Message::new(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, true)]),
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id2, false)],
                ),
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id3, true)],
                ),
            ],
            Some(&id1),
        );
        assert_eq!(
            message.get_account_keys_by_lock_type(),
            (vec![&id1, &id0], vec![&id3, &id2, &program_id])
        );
    }

    #[test]
    fn test_decompile_instructions() {
        solana_logger::setup();
        let program_id0 = Pubkey::new_unique();
        let program_id1 = Pubkey::new_unique();
        let id0 = Pubkey::new_unique();
        let id1 = Pubkey::new_unique();
        let id2 = Pubkey::new_unique();
        let id3 = Pubkey::new_unique();
        let instructions = vec![
            Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id0, false)]),
            Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id1, true)]),
            Instruction::new_with_bincode(
                program_id1,
                &0,
                vec![AccountMeta::new_readonly(id2, false)],
            ),
            Instruction::new_with_bincode(
                program_id1,
                &0,
                vec![AccountMeta::new_readonly(id3, true)],
            ),
        ];

        let message = Message::new(&instructions, Some(&id1));
        let serialized = message.serialize_instructions();
        for (i, instruction) in instructions.iter().enumerate() {
            assert_eq!(
                Message::deserialize_instruction(i, &serialized).unwrap(),
                *instruction
            );
        }
    }

    #[test]
    fn test_program_ids() {
        let key0 = Pubkey::new_unique();
        let key1 = Pubkey::new_unique();
        let loader2 = Pubkey::new_unique();
        let instructions = vec![CompiledInstruction::new(2, &(), vec![0, 1])];
        let message = Message::new_with_compiled_instructions(
            1,
            0,
            2,
            vec![key0, key1, loader2],
            Hash::default(),
            instructions,
        );
        assert_eq!(message.program_ids(), vec![&loader2]);
    }

    #[test]
    fn test_is_key_passed_to_program() {
        let key0 = Pubkey::new_unique();
        let key1 = Pubkey::new_unique();
        let loader2 = Pubkey::new_unique();
        let instructions = vec![CompiledInstruction::new(2, &(), vec![0, 1])];
        let message = Message::new_with_compiled_instructions(
            1,
            0,
            2,
            vec![key0, key1, loader2],
            Hash::default(),
            instructions,
        );

        assert!(message.is_key_passed_to_program(0));
        assert!(message.is_key_passed_to_program(1));
        assert!(!message.is_key_passed_to_program(2));
    }

    #[test]
    fn test_is_non_loader_key() {
        let key0 = Pubkey::new_unique();
        let key1 = Pubkey::new_unique();
        let loader2 = Pubkey::new_unique();
        let instructions = vec![CompiledInstruction::new(2, &(), vec![0, 1])];
        let message = Message::new_with_compiled_instructions(
            1,
            0,
            2,
            vec![key0, key1, loader2],
            Hash::default(),
            instructions,
        );

        assert!(message.is_non_loader_key(&key0, 0));
        assert!(message.is_non_loader_key(&key1, 1));
        assert!(!message.is_non_loader_key(&loader2, 2));
    }
}
