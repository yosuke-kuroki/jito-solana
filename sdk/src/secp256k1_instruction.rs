#![allow(clippy::integer_arithmetic)]
#![cfg(feature = "full")]

use crate::instruction::Instruction;
use digest::Digest;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, PartialEq)]
pub enum Secp256k1Error {
    InvalidSignature,
    InvalidRecoveryId,
    InvalidDataOffsets,
    InvalidInstructionDataSize,
}

pub const HASHED_PUBKEY_SERIALIZED_SIZE: usize = 20;
pub const SIGNATURE_SERIALIZED_SIZE: usize = 64;
pub const SIGNATURE_OFFSETS_SERIALIZED_SIZE: usize = 11;

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct SecpSignatureOffsets {
    pub signature_offset: u16, // offset to [signature,recovery_id] of 64+1 bytes
    pub signature_instruction_index: u8,
    pub eth_address_offset: u16, // offset to eth_address of 20 bytes
    pub eth_address_instruction_index: u8,
    pub message_data_offset: u16, // offset to start of message data
    pub message_data_size: u16,   // size of message data
    pub message_instruction_index: u8,
}

pub fn new_secp256k1_instruction(
    priv_key: &secp256k1::SecretKey,
    message_arr: &[u8],
) -> Instruction {
    let secp_pubkey = secp256k1::PublicKey::from_secret_key(priv_key);
    let eth_pubkey = construct_eth_pubkey(&secp_pubkey);
    let mut hasher = sha3::Keccak256::new();
    hasher.update(&message_arr);
    let message_hash = hasher.finalize();
    let mut message_hash_arr = [0u8; 32];
    message_hash_arr.copy_from_slice(&message_hash.as_slice());
    let message = secp256k1::Message::parse(&message_hash_arr);
    let (signature, recovery_id) = secp256k1::sign(&message, priv_key);
    let signature_arr = signature.serialize();
    assert_eq!(signature_arr.len(), SIGNATURE_SERIALIZED_SIZE);

    let mut instruction_data = vec![];
    let data_start = 1 + SIGNATURE_OFFSETS_SERIALIZED_SIZE;
    instruction_data.resize(
        data_start + eth_pubkey.len() + signature_arr.len() + message_arr.len() + 1,
        0,
    );
    let eth_address_offset = data_start;
    instruction_data[eth_address_offset..eth_address_offset + eth_pubkey.len()]
        .copy_from_slice(&eth_pubkey);

    let signature_offset = data_start + eth_pubkey.len();
    instruction_data[signature_offset..signature_offset + signature_arr.len()]
        .copy_from_slice(&signature_arr);

    instruction_data[signature_offset + signature_arr.len()] = recovery_id.serialize();

    let message_data_offset = signature_offset + signature_arr.len() + 1;
    instruction_data[message_data_offset..].copy_from_slice(message_arr);

    let num_signatures = 1;
    instruction_data[0] = num_signatures;
    let offsets = SecpSignatureOffsets {
        signature_offset: signature_offset as u16,
        signature_instruction_index: 0,
        eth_address_offset: eth_address_offset as u16,
        eth_address_instruction_index: 0,
        message_data_offset: message_data_offset as u16,
        message_data_size: message_arr.len() as u16,
        message_instruction_index: 0,
    };
    let writer = std::io::Cursor::new(&mut instruction_data[1..data_start]);
    bincode::serialize_into(writer, &offsets).unwrap();

    Instruction {
        program_id: solana_sdk::secp256k1_program::id(),
        accounts: vec![],
        data: instruction_data,
    }
}

pub fn construct_eth_pubkey(pubkey: &secp256k1::PublicKey) -> [u8; HASHED_PUBKEY_SERIALIZED_SIZE] {
    let mut addr = [0u8; HASHED_PUBKEY_SERIALIZED_SIZE];
    addr.copy_from_slice(&sha3::Keccak256::digest(&pubkey.serialize()[1..])[12..]);
    assert_eq!(addr.len(), HASHED_PUBKEY_SERIALIZED_SIZE);
    addr
}

pub fn verify_eth_addresses(
    data: &[u8],
    instruction_datas: &[&[u8]],
) -> Result<(), Secp256k1Error> {
    if data.is_empty() {
        return Err(Secp256k1Error::InvalidInstructionDataSize);
    }
    let count = data[0] as usize;
    let expected_data_size = 1 + count * SIGNATURE_OFFSETS_SERIALIZED_SIZE;
    if data.len() < expected_data_size {
        return Err(Secp256k1Error::InvalidInstructionDataSize);
    }
    for i in 0..count {
        let start = 1 + i * SIGNATURE_OFFSETS_SERIALIZED_SIZE;
        let end = start + SIGNATURE_OFFSETS_SERIALIZED_SIZE;

        let offsets: SecpSignatureOffsets = bincode::deserialize(&data[start..end])
            .map_err(|_| Secp256k1Error::InvalidSignature)?;

        // Parse out signature
        let signature_index = offsets.signature_instruction_index as usize;
        if signature_index >= instruction_datas.len() {
            return Err(Secp256k1Error::InvalidInstructionDataSize);
        }
        let signature_instruction = instruction_datas[signature_index];
        let sig_start = offsets.signature_offset as usize;
        let sig_end = sig_start + SIGNATURE_SERIALIZED_SIZE;
        if sig_end >= signature_instruction.len() {
            return Err(Secp256k1Error::InvalidSignature);
        }
        let signature =
            secp256k1::Signature::parse_slice(&signature_instruction[sig_start..sig_end])
                .map_err(|_| Secp256k1Error::InvalidSignature)?;

        let recovery_id = secp256k1::RecoveryId::parse(signature_instruction[sig_end])
            .map_err(|_| Secp256k1Error::InvalidRecoveryId)?;

        // Parse out pubkey
        let eth_address_slice = get_data_slice(
            &instruction_datas,
            offsets.eth_address_instruction_index,
            offsets.eth_address_offset,
            HASHED_PUBKEY_SERIALIZED_SIZE,
        )?;

        // Parse out message
        let message_slice = get_data_slice(
            &instruction_datas,
            offsets.message_instruction_index,
            offsets.message_data_offset,
            offsets.message_data_size as usize,
        )?;

        let mut hasher = sha3::Keccak256::new();
        hasher.update(message_slice);
        let message_hash = hasher.finalize();

        let pubkey = secp256k1::recover(
            &secp256k1::Message::parse_slice(&message_hash).unwrap(),
            &signature,
            &recovery_id,
        )
        .map_err(|_| Secp256k1Error::InvalidSignature)?;
        let eth_address = construct_eth_pubkey(&pubkey);

        if eth_address_slice != eth_address {
            return Err(Secp256k1Error::InvalidSignature);
        }
    }
    Ok(())
}

fn get_data_slice<'a>(
    instruction_datas: &'a [&[u8]],
    instruction_index: u8,
    offset_start: u16,
    size: usize,
) -> Result<&'a [u8], Secp256k1Error> {
    let signature_index = instruction_index as usize;
    if signature_index >= instruction_datas.len() {
        return Err(Secp256k1Error::InvalidDataOffsets);
    }
    let signature_instruction = &instruction_datas[signature_index];
    let start = offset_start as usize;
    let end = start + size;
    if end > signature_instruction.len() {
        return Err(Secp256k1Error::InvalidSignature);
    }

    Ok(&instruction_datas[signature_index][start..end])
}

#[cfg(test)]
pub mod test {
    use super::*;

    fn test_case(num_signatures: u8, offsets: &SecpSignatureOffsets) -> Result<(), Secp256k1Error> {
        let mut instruction_data = vec![0u8; 1 + SIGNATURE_OFFSETS_SERIALIZED_SIZE];
        instruction_data[0] = num_signatures;
        let writer = std::io::Cursor::new(&mut instruction_data[1..]);
        bincode::serialize_into(writer, &offsets).unwrap();

        verify_eth_addresses(&instruction_data, &[&[0u8; 100]])
    }

    #[test]
    fn test_invalid_offsets() {
        solana_logger::setup();

        let mut instruction_data = vec![0u8; 1 + SIGNATURE_OFFSETS_SERIALIZED_SIZE];
        let offsets = SecpSignatureOffsets::default();
        instruction_data[0] = 1;
        let writer = std::io::Cursor::new(&mut instruction_data[1..]);
        bincode::serialize_into(writer, &offsets).unwrap();
        instruction_data.truncate(instruction_data.len() - 1);

        assert_eq!(
            verify_eth_addresses(&instruction_data, &[&[0u8; 100]]),
            Err(Secp256k1Error::InvalidInstructionDataSize)
        );

        let offsets = SecpSignatureOffsets {
            signature_instruction_index: 1,
            ..SecpSignatureOffsets::default()
        };
        assert_eq!(
            test_case(1, &offsets),
            Err(Secp256k1Error::InvalidInstructionDataSize)
        );

        let offsets = SecpSignatureOffsets {
            message_instruction_index: 1,
            ..SecpSignatureOffsets::default()
        };
        assert_eq!(
            test_case(1, &offsets),
            Err(Secp256k1Error::InvalidDataOffsets)
        );

        let offsets = SecpSignatureOffsets {
            eth_address_instruction_index: 1,
            ..SecpSignatureOffsets::default()
        };
        assert_eq!(
            test_case(1, &offsets),
            Err(Secp256k1Error::InvalidDataOffsets)
        );
    }

    #[test]
    fn test_message_data_offsets() {
        let offsets = SecpSignatureOffsets {
            message_data_offset: 99,
            message_data_size: 1,
            ..SecpSignatureOffsets::default()
        };
        assert_eq!(
            test_case(1, &offsets),
            Err(Secp256k1Error::InvalidSignature)
        );

        let offsets = SecpSignatureOffsets {
            message_data_offset: 100,
            message_data_size: 1,
            ..SecpSignatureOffsets::default()
        };
        assert_eq!(
            test_case(1, &offsets),
            Err(Secp256k1Error::InvalidSignature)
        );

        let offsets = SecpSignatureOffsets {
            message_data_offset: 100,
            message_data_size: 1000,
            ..SecpSignatureOffsets::default()
        };
        assert_eq!(
            test_case(1, &offsets),
            Err(Secp256k1Error::InvalidSignature)
        );

        let offsets = SecpSignatureOffsets {
            message_data_offset: std::u16::MAX,
            message_data_size: std::u16::MAX,
            ..SecpSignatureOffsets::default()
        };
        assert_eq!(
            test_case(1, &offsets),
            Err(Secp256k1Error::InvalidSignature)
        );
    }

    #[test]
    fn test_eth_offset() {
        let offsets = SecpSignatureOffsets {
            eth_address_offset: std::u16::MAX,
            ..SecpSignatureOffsets::default()
        };
        assert_eq!(
            test_case(1, &offsets),
            Err(Secp256k1Error::InvalidSignature)
        );

        let offsets = SecpSignatureOffsets {
            eth_address_offset: 100 - HASHED_PUBKEY_SERIALIZED_SIZE as u16 + 1,
            ..SecpSignatureOffsets::default()
        };
        assert_eq!(
            test_case(1, &offsets),
            Err(Secp256k1Error::InvalidSignature)
        );
    }

    #[test]
    fn test_signature_offset() {
        let offsets = SecpSignatureOffsets {
            signature_offset: std::u16::MAX,
            ..SecpSignatureOffsets::default()
        };
        assert_eq!(
            test_case(1, &offsets),
            Err(Secp256k1Error::InvalidSignature)
        );

        let offsets = SecpSignatureOffsets {
            signature_offset: 100 - SIGNATURE_SERIALIZED_SIZE as u16 + 1,
            ..SecpSignatureOffsets::default()
        };
        assert_eq!(
            test_case(1, &offsets),
            Err(Secp256k1Error::InvalidSignature)
        );
    }
}
