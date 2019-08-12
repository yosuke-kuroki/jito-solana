//! The `shred` module defines data structures and methods to pull MTU sized data frames from the network.
use crate::erasure::Session;
use crate::result;
use crate::result::Error;
use bincode::serialized_size;
use core::borrow::BorrowMut;
use serde::{Deserialize, Serialize};
use solana_sdk::packet::PACKET_DATA_SIZE;
use solana_sdk::signature::{Keypair, KeypairUtil, Signature};
use std::io::{Error as IOError, ErrorKind, Write};
use std::sync::Arc;
use std::{cmp, io};

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub enum Shred {
    FirstInSlot(FirstDataShred),
    FirstInFECSet(DataShred),
    Data(DataShred),
    LastInFECSet(DataShred),
    LastInSlot(DataShred),
    Coding(CodingShred),
}

impl Shred {
    pub fn slot(&self) -> u64 {
        match self {
            Shred::FirstInSlot(s) => s.header.data_header.common_header.slot,
            Shred::FirstInFECSet(s)
            | Shred::Data(s)
            | Shred::LastInFECSet(s)
            | Shred::LastInSlot(s) => s.header.common_header.slot,
            Shred::Coding(s) => s.header.common_header.slot,
        }
    }

    pub fn index(&self) -> u32 {
        match self {
            Shred::FirstInSlot(s) => s.header.data_header.common_header.index,
            Shred::FirstInFECSet(s)
            | Shred::Data(s)
            | Shred::LastInFECSet(s)
            | Shred::LastInSlot(s) => s.header.common_header.index,
            Shred::Coding(s) => s.header.common_header.index,
        }
    }

    pub fn signature(&self) -> Signature {
        match self {
            Shred::FirstInSlot(s) => s.header.data_header.common_header.signature,
            Shred::FirstInFECSet(s)
            | Shred::Data(s)
            | Shred::LastInFECSet(s)
            | Shred::LastInSlot(s) => s.header.common_header.signature,
            Shred::Coding(s) => s.header.common_header.signature,
        }
    }

    pub fn seed(&self) -> [u8; 32] {
        let mut seed = [0; 32];
        let seed_len = seed.len();
        let sig = match self {
            Shred::FirstInSlot(s) => &s.header.data_header.common_header.signature,
            Shred::FirstInFECSet(s)
            | Shred::Data(s)
            | Shred::LastInFECSet(s)
            | Shred::LastInSlot(s) => &s.header.common_header.signature,
            Shred::Coding(s) => &s.header.common_header.signature,
        }
        .as_ref();

        seed[0..seed_len].copy_from_slice(&sig[(sig.len() - seed_len)..]);
        seed
    }
}

/// A common header that is present at start of every shred
#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
pub struct ShredCommonHeader {
    pub signature: Signature,
    pub slot: u64,
    pub index: u32,
}

/// A common header that is present at start of every data shred
#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
pub struct DataShredHeader {
    _reserved: CodingShredHeader,
    pub common_header: ShredCommonHeader,
    pub last_in_slot: u8,
}

/// The first data shred also has parent slot value in it
#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
pub struct FirstDataShredHeader {
    pub data_header: DataShredHeader,
    pub parent: u64,
}

/// The coding shred header has FEC information
#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
pub struct CodingShredHeader {
    pub common_header: ShredCommonHeader,
    pub num_data_shreds: u16,
    pub num_coding_shreds: u16,
    pub position: u16,
    pub payload: Vec<u8>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct FirstDataShred {
    pub header: FirstDataShredHeader,
    pub payload: Vec<u8>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct DataShred {
    pub header: DataShredHeader,
    pub payload: Vec<u8>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct CodingShred {
    pub header: CodingShredHeader,
}

/// Default shred is sized correctly to meet MTU/Packet size requirements
impl Default for FirstDataShred {
    fn default() -> Self {
        let size = PACKET_DATA_SIZE
            - serialized_size(&Shred::FirstInSlot(Self::empty_shred())).unwrap() as usize;
        FirstDataShred {
            header: FirstDataShredHeader::default(),
            payload: vec![0; size],
        }
    }
}

/// Default shred is sized correctly to meet MTU/Packet size requirements
impl Default for DataShred {
    fn default() -> Self {
        let size =
            PACKET_DATA_SIZE - serialized_size(&Shred::Data(Self::empty_shred())).unwrap() as usize;
        DataShred {
            header: DataShredHeader::default(),
            payload: vec![0; size],
        }
    }
}

/// Default shred is sized correctly to meet MTU/Packet size requirements
impl Default for CodingShred {
    fn default() -> Self {
        let size = PACKET_DATA_SIZE
            - serialized_size(&Shred::Coding(Self::empty_shred())).unwrap() as usize;
        CodingShred {
            header: CodingShredHeader {
                common_header: ShredCommonHeader::default(),
                num_data_shreds: 0,
                num_coding_shreds: 0,
                position: 0,
                payload: vec![0; size],
            },
        }
    }
}

/// Common trait implemented by all types of shreds
pub trait ShredCommon {
    /// Write at a particular offset in the shred
    fn write_at(&mut self, offset: usize, buf: &[u8]) -> usize;
    /// Overhead of shred enum and headers
    fn overhead() -> usize;
    /// Utility function to create an empty shred
    fn empty_shred() -> Self;
}

impl ShredCommon for FirstDataShred {
    fn write_at(&mut self, offset: usize, buf: &[u8]) -> usize {
        let slice_len = cmp::min(self.payload.len().saturating_sub(offset), buf.len());
        if slice_len > 0 {
            self.payload[offset..offset + slice_len].copy_from_slice(&buf[..slice_len]);
        }
        slice_len
    }

    fn overhead() -> usize {
        (bincode::serialized_size(&Shred::FirstInSlot(Self::empty_shred())).unwrap()
            - bincode::serialized_size(&vec![0u8; 0]).unwrap()) as usize
    }

    fn empty_shred() -> Self {
        FirstDataShred {
            header: FirstDataShredHeader::default(),
            payload: vec![],
        }
    }
}

impl ShredCommon for DataShred {
    fn write_at(&mut self, offset: usize, buf: &[u8]) -> usize {
        let slice_len = cmp::min(self.payload.len().saturating_sub(offset), buf.len());
        if slice_len > 0 {
            self.payload[offset..offset + slice_len].copy_from_slice(&buf[..slice_len]);
        }
        slice_len
    }

    fn overhead() -> usize {
        (bincode::serialized_size(&Shred::Data(Self::empty_shred())).unwrap()
            - bincode::serialized_size(&vec![0u8; 0]).unwrap()) as usize
    }

    fn empty_shred() -> Self {
        DataShred {
            header: DataShredHeader::default(),
            payload: vec![],
        }
    }
}

impl ShredCommon for CodingShred {
    fn write_at(&mut self, offset: usize, buf: &[u8]) -> usize {
        let slice_len = cmp::min(self.header.payload.len().saturating_sub(offset), buf.len());
        if slice_len > 0 {
            self.header.payload[offset..offset + slice_len].copy_from_slice(&buf[..slice_len]);
        }
        slice_len
    }

    fn overhead() -> usize {
        bincode::serialized_size(&Shred::Coding(Self::empty_shred())).unwrap() as usize
    }

    fn empty_shred() -> Self {
        CodingShred {
            header: CodingShredHeader::default(),
        }
    }
}

#[derive(Default, Debug)]
pub struct Shredder {
    slot: u64,
    index: u32,
    parent: Option<u64>,
    fec_rate: f32,
    signer: Arc<Keypair>,
    pub shreds: Vec<Vec<u8>>,
    pub active_shred: Option<Shred>,
    pub active_offset: usize,
}

impl Write for Shredder {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut current_shred = self
            .active_shred
            .take()
            .or_else(|| {
                Some(
                    self.parent
                        .take()
                        .map(|parent| {
                            // If parent slot is provided, assume it's first shred in slot
                            Shred::FirstInSlot(self.new_first_shred(parent))
                        })
                        .unwrap_or_else(||
                            // If parent slot is not provided, and since there's no existing shred,
                            // assume it's first shred in FEC block
                            Shred::FirstInFECSet(self.new_data_shred())),
                )
            })
            .unwrap();

        let written = self.active_offset;
        let slice_len = match current_shred.borrow_mut() {
            Shred::FirstInSlot(s) => s.write_at(written, buf),
            Shred::FirstInFECSet(s)
            | Shred::Data(s)
            | Shred::LastInFECSet(s)
            | Shred::LastInSlot(s) => s.write_at(written, buf),
            Shred::Coding(s) => s.write_at(written, buf),
        };

        let active_shred = if buf.len() > slice_len {
            self.finalize_data_shred(current_shred);
            // Continue generating more data shreds.
            // If the caller decides to finalize the FEC block or Slot, the data shred will
            // morph into appropriate shred accordingly
            Shred::Data(self.new_data_shred())
        } else {
            self.active_offset += slice_len;
            current_shred
        };

        self.active_shred = Some(active_shred);

        Ok(slice_len)
    }

    fn flush(&mut self) -> io::Result<()> {
        unimplemented!()
    }
}

#[derive(Default, Debug, PartialEq)]
pub struct DeshredResult {
    pub payload: Vec<u8>,
    pub recovered_data: Vec<Shred>,
    pub recovered_code: Vec<Shred>,
}

impl Shredder {
    pub fn new(
        slot: u64,
        parent: Option<u64>,
        fec_rate: f32,
        signer: &Arc<Keypair>,
        index: u32,
    ) -> result::Result<Self> {
        if fec_rate > 1.0 || fec_rate < 0.0 {
            Err(Error::IO(IOError::new(
                ErrorKind::Other,
                format!(
                    "FEC rate {:?} must be more than 0.0 and less than 1.0",
                    fec_rate
                ),
            )))
        } else {
            Ok(Shredder {
                slot,
                index,
                parent,
                fec_rate,
                signer: signer.clone(),
                ..Shredder::default()
            })
        }
    }

    /// Serialize the payload, sign it and store the signature in the shred
    /// Store the signed shred in the vector of shreds
    fn finalize_shred(&mut self, mut shred: Vec<u8>, signature_offset: usize) {
        let data_offset =
            signature_offset + bincode::serialized_size(&Signature::default()).unwrap() as usize;
        let signature = bincode::serialize(&self.signer.sign_message(&shred[data_offset..]))
            .expect("Failed to generate serialized signature");
        shred[signature_offset..signature_offset + signature.len()].copy_from_slice(&signature);
        self.shreds.push(shred);
    }

    /// Finalize a data shred. Update the shred index for the next shred
    fn finalize_data_shred(&mut self, shred: Shred) {
        let data = bincode::serialize(&shred).expect("Failed to serialize shred");

        self.finalize_shred(data, CodingShred::overhead());
        self.active_offset = 0;
        self.index += 1;
    }

    /// Creates a new data shred
    fn new_data_shred(&self) -> DataShred {
        let mut data_shred = DataShred::default();
        data_shred.header.common_header.slot = self.slot;
        data_shred.header.common_header.index = self.index;
        data_shred
    }

    /// Create a new data shred that's also first in the slot
    fn new_first_shred(&self, parent: u64) -> FirstDataShred {
        let mut first_shred = FirstDataShred::default();
        first_shred.header.parent = parent;
        first_shred.header.data_header.common_header.slot = self.slot;
        first_shred.header.data_header.common_header.index = self.index;
        first_shred
    }

    fn new_coding_shred(
        slot: u64,
        index: u32,
        num_data: usize,
        num_code: usize,
        position: usize,
    ) -> CodingShred {
        let mut coding_shred = CodingShred::default();
        coding_shred.header.common_header.slot = slot;
        coding_shred.header.common_header.index = index;
        coding_shred.header.num_data_shreds = num_data as u16;
        coding_shred.header.num_coding_shreds = num_code as u16;
        coding_shred.header.position = position as u16;
        coding_shred
    }

    /// Generates coding shreds for the data shreds in the current FEC set
    fn generate_coding_shreds(&mut self) {
        if self.fec_rate != 0.0 {
            let num_data = self.shreds.len();
            let num_coding = (self.fec_rate * num_data as f32) as usize;
            let session =
                Session::new(num_data, num_coding).expect("Failed to create erasure session");
            let start_index = self.index - num_data as u32;

            // All information after "reserved" field (coding shred header) in a data shred is encoded
            let coding_block_offset = CodingShred::overhead();
            let data_ptrs: Vec<_> = self
                .shreds
                .iter()
                .map(|data| &data[coding_block_offset..])
                .collect();

            // Create empty coding shreds, with correctly populated headers
            let mut coding_shreds = Vec::with_capacity(num_coding);
            (0..num_coding).for_each(|i| {
                let shred = bincode::serialize(&Shred::Coding(Self::new_coding_shred(
                    self.slot,
                    start_index + i as u32,
                    num_data,
                    num_coding,
                    i,
                )))
                .unwrap();
                coding_shreds.push(shred);
            });

            // Grab pointers for the coding blocks
            let mut coding_ptrs: Vec<_> = coding_shreds
                .iter_mut()
                .map(|buffer| &mut buffer[coding_block_offset..])
                .collect();

            // Create coding blocks
            session
                .encode(&data_ptrs, coding_ptrs.as_mut_slice())
                .expect("Failed in erasure encode");

            // Offset of coding shred header in the Coding Shred (i.e. overhead of enum variant)
            let coding_header_offset = (serialized_size(&Shred::Coding(CodingShred::empty_shred()))
                .unwrap()
                - serialized_size(&CodingShred::empty_shred()).unwrap())
                as usize;

            // Finalize the coding blocks (sign and append to the shred list)
            coding_shreds
                .into_iter()
                .for_each(|code| self.finalize_shred(code, coding_header_offset))
        }
    }

    /// Create the final data shred for the current FEC set or slot
    /// If there's an active data shred, morph it into the final shred
    /// If the current active data shred is first in slot, finalize it and create a new shred
    fn make_final_data_shred(&mut self) -> DataShred {
        self.active_shred.take().map_or(
            self.new_data_shred(),
            |current_shred| match current_shred {
                Shred::FirstInSlot(s) => {
                    self.finalize_data_shred(Shred::FirstInSlot(s));
                    self.new_data_shred()
                }
                Shred::FirstInFECSet(s)
                | Shred::Data(s)
                | Shred::LastInFECSet(s)
                | Shred::LastInSlot(s) => s,
                Shred::Coding(_) => self.new_data_shred(),
            },
        )
    }

    /// Finalize the current FEC block, and generate coding shreds
    pub fn finalize_fec_block(&mut self) {
        let final_shred = self.make_final_data_shred();
        self.finalize_data_shred(Shred::LastInFECSet(final_shred));
        self.generate_coding_shreds();
    }

    /// Finalize the current slot (i.e. add last slot shred) and generate coding shreds
    pub fn finalize_slot(&mut self) {
        let mut final_shred = self.make_final_data_shred();
        final_shred.header.last_in_slot = 1;
        self.finalize_data_shred(Shred::LastInSlot(final_shred));
        self.generate_coding_shreds();
    }

    fn fill_in_missing_shreds(
        shred: &Shred,
        num_data: usize,
        num_coding: usize,
        slot: u64,
        first_index: usize,
        expected_index: usize,
        present: &mut [bool],
    ) -> (Vec<Vec<u8>>, bool, usize) {
        let (index, mut first_shred_in_slot) = Self::get_shred_index(shred, num_data);

        let mut missing_blocks: Vec<Vec<u8>> = (expected_index..index)
            .map(|missing| {
                present[missing] = false;
                // If index 0 shred is missing, then first shred in slot will also be recovered
                first_shred_in_slot |= missing == 0;
                Shredder::new_empty_missing_shred(num_data, num_coding, slot, first_index, missing)
            })
            .collect();
        let shred_buf = bincode::serialize(shred).unwrap();
        missing_blocks.push(shred_buf);
        (missing_blocks, first_shred_in_slot, index)
    }

    fn new_empty_missing_shred(
        num_data: usize,
        num_coding: usize,
        slot: u64,
        first_index: usize,
        missing: usize,
    ) -> Vec<u8> {
        let missing_shred = if missing == 0 {
            let mut data_shred = FirstDataShred::default();
            data_shred.header.data_header.common_header.slot = slot;
            data_shred.header.data_header.common_header.index = 0;
            Shred::FirstInSlot(data_shred)
        } else if missing < first_index + num_data {
            let mut data_shred = DataShred::default();
            data_shred.header.common_header.slot = slot;
            data_shred.header.common_header.index = missing as u32;
            if missing == first_index + num_data - 1 {
                Shred::LastInFECSet(data_shred)
            } else {
                Shred::Data(data_shred)
            }
        } else {
            Shred::Coding(Self::new_coding_shred(
                slot,
                missing.saturating_sub(num_data) as u32,
                num_data,
                num_coding,
                missing - first_index - num_data,
            ))
        };
        bincode::serialize(&missing_shred).unwrap()
    }

    /// Combines all shreds to recreate the original buffer
    /// If the shreds include coding shreds, and if not all shreds are present, it tries
    /// to reconstruct missing shreds using erasure
    /// Note: The shreds are expected to be sorted
    /// (lower to higher index, and data shreds before coding shreds)
    pub fn deshred(shreds: &[Shred]) -> Result<DeshredResult, reed_solomon_erasure::Error> {
        // If coding is enabled, the last shred must be a coding shred.
        let (num_data, num_coding, first_index, slot) =
            if let Shred::Coding(code) = shreds.last().unwrap() {
                (
                    code.header.num_data_shreds as usize,
                    code.header.num_coding_shreds as usize,
                    code.header.common_header.index as usize - code.header.position as usize,
                    code.header.common_header.slot,
                )
            } else {
                (shreds.len(), 0, 0, 0)
            };

        let mut recovered_data = vec![];
        let mut recovered_code = vec![];
        let fec_set_size = num_data + num_coding;
        let (data_shred_bufs, first_shred) = if num_coding > 0 && shreds.len() < fec_set_size {
            let coding_block_offset = CodingShred::overhead();

            // Let's try recovering missing shreds using erasure
            let mut present = &mut vec![true; fec_set_size];
            let mut first_shred_in_slot = false;
            let mut next_expected_index = first_index;
            let mut shred_bufs: Vec<Vec<u8>> = shreds
                .iter()
                .flat_map(|shred| {
                    let (blocks, first_shred, last_index) = Self::fill_in_missing_shreds(
                        shred,
                        num_data,
                        num_coding,
                        slot,
                        first_index,
                        next_expected_index,
                        &mut present,
                    );
                    first_shred_in_slot |= first_shred;
                    next_expected_index = last_index + 1;
                    blocks
                })
                .collect();

            let mut pending_shreds: Vec<Vec<u8>> = (next_expected_index
                ..first_index + fec_set_size)
                .map(|missing| {
                    present[missing] = false;
                    Self::new_empty_missing_shred(num_data, num_coding, slot, first_index, missing)
                })
                .collect();
            shred_bufs.append(&mut pending_shreds);

            let session = Session::new(num_data, num_coding).unwrap();

            let mut blocks: Vec<&mut [u8]> = shred_bufs
                .iter_mut()
                .map(|x| x[coding_block_offset..].as_mut())
                .collect();
            session.decode_blocks(&mut blocks, &present)?;

            present.iter().enumerate().for_each(|(index, was_present)| {
                if !was_present {
                    let shred: Shred = bincode::deserialize(&shred_bufs[index]).unwrap();
                    if index < first_index + num_data {
                        // Check if the last recovered data shred is also last in Slot.
                        // If so, it needs to be morphed into the correct type
                        let shred = if let Shred::Data(s) = shred {
                            if s.header.last_in_slot == 1 {
                                Shred::LastInSlot(s)
                            } else {
                                Shred::Data(s)
                            }
                        } else if let Shred::LastInFECSet(s) = shred {
                            if s.header.last_in_slot == 1 {
                                Shred::LastInSlot(s)
                            } else {
                                Shred::LastInFECSet(s)
                            }
                        } else {
                            shred
                        };
                        recovered_data.push(shred)
                    } else {
                        recovered_code.push(shred)
                    }
                }
            });
            (shred_bufs, first_shred_in_slot)
        } else {
            let (first_index, first_shred_in_slot) =
                Shredder::get_shred_index(shreds.first().unwrap(), num_data);

            let last_index = match shreds.last().unwrap() {
                Shred::LastInFECSet(s) | Shred::LastInSlot(s) => {
                    s.header.common_header.index as usize
                }
                _ => 0,
            };

            if num_data.saturating_add(first_index) != last_index.saturating_add(1) {
                Err(reed_solomon_erasure::Error::TooFewDataShards)?;
            }

            let shred_bufs: Vec<Vec<u8>> = shreds
                .iter()
                .map(|shred| bincode::serialize(shred).unwrap())
                .collect();
            (shred_bufs, first_shred_in_slot)
        };

        Ok(DeshredResult {
            payload: Self::reassemble_payload(num_data, data_shred_bufs, first_shred),
            recovered_data,
            recovered_code,
        })
    }

    fn get_shred_index(shred: &Shred, num_data: usize) -> (usize, bool) {
        let (first_index, first_shred_in_slot) = match shred {
            Shred::FirstInSlot(s) => (s.header.data_header.common_header.index as usize, true),
            Shred::FirstInFECSet(s)
            | Shred::Data(s)
            | Shred::LastInFECSet(s)
            | Shred::LastInSlot(s) => (s.header.common_header.index as usize, false),
            Shred::Coding(s) => (s.header.common_header.index as usize + num_data, false),
        };
        (first_index, first_shred_in_slot)
    }

    fn reassemble_payload(
        num_data: usize,
        data_shred_bufs: Vec<Vec<u8>>,
        first_shred: bool,
    ) -> Vec<u8> {
        data_shred_bufs[..num_data]
            .iter()
            .enumerate()
            .flat_map(|(i, data)| {
                let offset = if i == 0 && first_shred {
                    bincode::serialized_size(&Shred::FirstInSlot(FirstDataShred::empty_shred()))
                        .unwrap()
                } else {
                    bincode::serialized_size(&Shred::Data(DataShred::empty_shred())).unwrap()
                };
                data[offset as usize..].iter()
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_shredder() {
        let keypair = Arc::new(Keypair::new());
        let slot = 0x123456789abcdef0;
        let mut shredder =
            Shredder::new(slot, Some(5), 0.0, &keypair, 0).expect("Failed in creating shredder");

        assert!(shredder.shreds.is_empty());
        assert_eq!(shredder.active_shred, None);
        assert_eq!(shredder.active_offset, 0);

        assert!(FirstDataShred::overhead() < PACKET_DATA_SIZE);
        assert!(DataShred::overhead() < PACKET_DATA_SIZE);
        assert!(CodingShred::overhead() < PACKET_DATA_SIZE);

        // Test0: Write some data to shred. Not enough to create a signed shred
        let data: Vec<u8> = (0..25).collect();
        assert_eq!(shredder.write(&data).unwrap(), data.len());
        assert!(shredder.shreds.is_empty());
        assert_ne!(shredder.active_shred, None);
        assert_eq!(shredder.active_offset, 25);

        // Test1: Write some more data to shred. Not enough to create a signed shred
        assert_eq!(shredder.write(&data).unwrap(), data.len());
        assert!(shredder.shreds.is_empty());
        assert_eq!(shredder.active_offset, 50);

        // Test2: Write enough data to create a shred (> PACKET_DATA_SIZE)
        let data: Vec<_> = (0..PACKET_DATA_SIZE).collect();
        let data: Vec<u8> = data.iter().map(|x| *x as u8).collect();
        let offset = shredder.write(&data).unwrap();
        assert_ne!(offset, data.len());
        // Assert that we have atleast one signed shred
        assert!(!shredder.shreds.is_empty());
        // Assert that a new active shred was also created
        assert_ne!(shredder.active_shred, None);
        // Assert that the new active shred was not populated
        assert_eq!(shredder.active_offset, 0);

        let data_offset = CodingShred::overhead()
            + bincode::serialized_size(&Signature::default()).unwrap() as usize;

        // Test3: Assert that the first shred in slot was created (since we gave a parent to shredder)
        let shred = shredder.shreds.pop().unwrap();
        assert_eq!(shred.len(), PACKET_DATA_SIZE);
        info!("Len: {}", shred.len());
        info!("{:?}", shred);

        // Test4: Try deserialize the PDU and assert that it matches the original shred
        let deserialized_shred: Shred =
            bincode::deserialize(&shred).expect("Failed in deserializing the PDU");
        assert_matches!(deserialized_shred, Shred::FirstInSlot(_));
        assert_eq!(deserialized_shred.index(), 0);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
        let seed0 = deserialized_shred.seed();
        // Test that same seed is generated for a given shred
        assert_eq!(seed0, deserialized_shred.seed());

        // Test5: Write left over data, and assert that a data shred is being created
        shredder.write(&data[offset..]).unwrap();

        // It shouldn't generate a signed shred
        assert!(shredder.shreds.is_empty());

        // Test6: Let's finalize the FEC block. That should result in the current shred to morph into
        // a signed LastInFECSetData shred
        shredder.finalize_fec_block();

        // We should have a new signed shred
        assert!(!shredder.shreds.is_empty());

        // Must be Last in FEC Set
        let shred = shredder.shreds.pop().unwrap();
        assert_eq!(shred.len(), PACKET_DATA_SIZE);

        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::LastInFECSet(_));
        assert_eq!(deserialized_shred.index(), 1);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
        // Test that same seed is NOT generated for two different shreds
        assert_ne!(seed0, deserialized_shred.seed());

        // Test7: Let's write some more data to the shredder.
        // Now we should get a new FEC block
        let data: Vec<_> = (0..PACKET_DATA_SIZE).collect();
        let data: Vec<u8> = data.iter().map(|x| *x as u8).collect();
        let offset = shredder.write(&data).unwrap();
        assert_ne!(offset, data.len());

        // We should have a new signed shred
        assert!(!shredder.shreds.is_empty());

        // Must be FirstInFECSet
        let shred = shredder.shreds.pop().unwrap();
        assert_eq!(shred.len(), PACKET_DATA_SIZE);

        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::FirstInFECSet(_));
        assert_eq!(deserialized_shred.index(), 2);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));

        // Test8: Write more data to generate an intermediate data shred
        let offset = shredder.write(&data).unwrap();
        assert_ne!(offset, data.len());

        // We should have a new signed shred
        assert!(!shredder.shreds.is_empty());

        // Must be a Data shred
        let shred = shredder.shreds.pop().unwrap();
        assert_eq!(shred.len(), PACKET_DATA_SIZE);

        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::Data(_));
        assert_eq!(deserialized_shred.index(), 3);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));

        // Test9: Write some data to shredder
        let data: Vec<u8> = (0..25).collect();
        assert_eq!(shredder.write(&data).unwrap(), data.len());

        // And, finish the slot
        shredder.finalize_slot();

        // We should have a new signed shred
        assert!(!shredder.shreds.is_empty());

        // Must be LastInSlot
        let shred = shredder.shreds.pop().unwrap();
        assert_eq!(shred.len(), PACKET_DATA_SIZE);

        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::LastInSlot(_));
        assert_eq!(deserialized_shred.index(), 4);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
    }

    #[test]
    fn test_small_data_shredder() {
        let keypair = Arc::new(Keypair::new());

        let slot = 0x123456789abcdef0;
        let mut shredder =
            Shredder::new(slot, Some(5), 0.0, &keypair, 0).expect("Failed in creating shredder");

        assert!(shredder.shreds.is_empty());
        assert_eq!(shredder.active_shred, None);
        assert_eq!(shredder.active_offset, 0);

        let data: Vec<_> = (0..25).collect();
        let data: Vec<u8> = data.iter().map(|x| *x as u8).collect();
        let _ = shredder.write(&data).unwrap();

        // We should have 0 shreds now
        assert_eq!(shredder.shreds.len(), 0);

        shredder.finalize_fec_block();

        // We should have 2 shreds now (FirstInSlot, and LastInFECSet)
        assert_eq!(shredder.shreds.len(), 2);

        let data_offset = CodingShred::overhead()
            + bincode::serialized_size(&Signature::default()).unwrap() as usize;

        let shred = shredder.shreds.remove(0);
        assert_eq!(shred.len(), PACKET_DATA_SIZE);
        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::FirstInSlot(_));
        assert_eq!(deserialized_shred.index(), 0);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));

        let shred = shredder.shreds.remove(0);
        assert_eq!(shred.len(), PACKET_DATA_SIZE);
        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::LastInFECSet(_));
        assert_eq!(deserialized_shred.index(), 1);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));

        // Try shredder when no parent is provided
        let mut shredder = Shredder::new(0x123456789abcdef0, None, 0.0, &keypair, 2)
            .expect("Failed in creating shredder");

        assert!(shredder.shreds.is_empty());
        assert_eq!(shredder.active_shred, None);
        assert_eq!(shredder.active_offset, 0);

        let data: Vec<_> = (0..25).collect();
        let data: Vec<u8> = data.iter().map(|x| *x as u8).collect();
        let _ = shredder.write(&data).unwrap();

        // We should have 0 shreds now
        assert_eq!(shredder.shreds.len(), 0);

        shredder.finalize_fec_block();

        // We should have 1 shred now (LastInFECSet)
        assert_eq!(shredder.shreds.len(), 1);
        let shred = shredder.shreds.remove(0);
        assert_eq!(shred.len(), PACKET_DATA_SIZE);
        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::LastInFECSet(_));
        assert_eq!(deserialized_shred.index(), 2);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
    }

    #[test]
    fn test_data_and_code_shredder() {
        let keypair = Arc::new(Keypair::new());

        let slot = 0x123456789abcdef0;
        // Test that FEC rate cannot be > 1.0
        assert_matches!(Shredder::new(slot, Some(5), 1.001, &keypair, 0), Err(_));

        let mut shredder = Shredder::new(0x123456789abcdef0, Some(5), 1.0, &keypair, 0)
            .expect("Failed in creating shredder");

        assert!(shredder.shreds.is_empty());
        assert_eq!(shredder.active_shred, None);
        assert_eq!(shredder.active_offset, 0);

        // Write enough data to create a shred (> PACKET_DATA_SIZE)
        let data: Vec<_> = (0..PACKET_DATA_SIZE).collect();
        let data: Vec<u8> = data.iter().map(|x| *x as u8).collect();
        let _ = shredder.write(&data).unwrap();
        let _ = shredder.write(&data).unwrap();

        // We should have 2 shreds now
        assert_eq!(shredder.shreds.len(), 2);

        shredder.finalize_fec_block();

        let data_offset = CodingShred::overhead()
            + bincode::serialized_size(&Signature::default()).unwrap() as usize;

        // Finalize must have created 1 final data shred and 3 coding shreds
        // assert_eq!(shredder.shreds.len(), 6);
        let shred = shredder.shreds.remove(0);
        assert_eq!(shred.len(), PACKET_DATA_SIZE);
        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::FirstInSlot(_));
        assert_eq!(deserialized_shred.index(), 0);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));

        let shred = shredder.shreds.remove(0);
        assert_eq!(shred.len(), PACKET_DATA_SIZE);
        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::Data(_));
        assert_eq!(deserialized_shred.index(), 1);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));

        let shred = shredder.shreds.remove(0);
        assert_eq!(shred.len(), PACKET_DATA_SIZE);
        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::LastInFECSet(_));
        assert_eq!(deserialized_shred.index(), 2);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));

        let coding_data_offset =
            (serialized_size(&Shred::Coding(CodingShred::empty_shred())).unwrap()
                - serialized_size(&CodingShred::empty_shred()).unwrap()
                + serialized_size(&Signature::default()).unwrap()) as usize as usize;

        let shred = shredder.shreds.remove(0);
        assert_eq!(shred.len(), PACKET_DATA_SIZE);
        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::Coding(_));
        assert_eq!(deserialized_shred.index(), 0);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[coding_data_offset..]));

        let shred = shredder.shreds.remove(0);
        assert_eq!(shred.len(), PACKET_DATA_SIZE);
        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::Coding(_));
        assert_eq!(deserialized_shred.index(), 1);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[coding_data_offset..]));

        let shred = shredder.shreds.remove(0);
        assert_eq!(shred.len(), PACKET_DATA_SIZE);
        let deserialized_shred: Shred = bincode::deserialize(&shred).unwrap();
        assert_matches!(deserialized_shred, Shred::Coding(_));
        assert_eq!(deserialized_shred.index(), 2);
        assert_eq!(deserialized_shred.slot(), slot);
        assert!(deserialized_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[coding_data_offset..]));
    }

    #[test]
    fn test_recovery_and_reassembly() {
        let keypair = Arc::new(Keypair::new());
        let slot = 0x123456789abcdef0;
        let mut shredder =
            Shredder::new(slot, Some(5), 1.0, &keypair, 0).expect("Failed in creating shredder");

        assert!(shredder.shreds.is_empty());
        assert_eq!(shredder.active_shred, None);
        assert_eq!(shredder.active_offset, 0);

        let data: Vec<_> = (0..5000).collect();
        let data: Vec<u8> = data.iter().map(|x| *x as u8).collect();
        let mut offset = shredder.write(&data).unwrap();
        let approx_shred_payload_size = offset;
        offset += shredder.write(&data[offset..]).unwrap();
        offset += shredder.write(&data[offset..]).unwrap();
        offset += shredder.write(&data[offset..]).unwrap();
        offset += shredder.write(&data[offset..]).unwrap();

        // We should have some shreds now
        assert_eq!(
            shredder.shreds.len(),
            data.len() / approx_shred_payload_size
        );
        assert_eq!(offset, data.len());

        shredder.finalize_fec_block();

        // We should have 10 shreds now (one additional final shred, and equal number of coding shreds)
        let expected_shred_count = ((data.len() / approx_shred_payload_size) + 1) * 2;
        assert_eq!(shredder.shreds.len(), expected_shred_count);

        let shreds: Vec<Shred> = shredder
            .shreds
            .iter()
            .map(|s| bincode::deserialize(s).unwrap())
            .collect();

        // Test0: Try recovery/reassembly with only data shreds, but not all data shreds. Hint: should fail
        assert_matches!(
            Shredder::deshred(&shreds[..4]),
            Err(reed_solomon_erasure::Error::TooFewDataShards)
        );

        // Test1: Try recovery/reassembly with only data shreds. Hint: should work
        let result = Shredder::deshred(&shreds[..5]).unwrap();
        assert_ne!(DeshredResult::default(), result);
        assert!(result.payload.len() >= data.len());
        assert!(result.recovered_data.is_empty());
        assert!(result.recovered_code.is_empty());
        assert_eq!(data[..], result.payload[..data.len()]);

        // Test2: Try recovery/reassembly with missing data shreds + coding shreds. Hint: should work
        let shreds: Vec<Shred> = shredder
            .shreds
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                if i % 2 == 0 {
                    Some(bincode::deserialize(s).unwrap())
                } else {
                    None
                }
            })
            .collect();

        let data_offset = CodingShred::overhead()
            + bincode::serialized_size(&Signature::default()).unwrap() as usize;

        let mut result = Shredder::deshred(&shreds).unwrap();
        assert!(result.payload.len() >= data.len());
        assert_eq!(result.recovered_data.len(), 2); // Data shreds 1 and 3 were missing
        let recovered_shred = result.recovered_data.remove(0);
        let shred = bincode::serialize(&recovered_shred).unwrap();
        assert_matches!(recovered_shred, Shred::Data(_));
        assert_eq!(recovered_shred.index(), 1);
        assert_eq!(recovered_shred.slot(), slot);
        assert!(recovered_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
        let recovered_shred = result.recovered_data.remove(0);
        let shred = bincode::serialize(&recovered_shred).unwrap();
        assert_matches!(recovered_shred, Shred::Data(_));
        assert_eq!(recovered_shred.index(), 3);
        assert_eq!(recovered_shred.slot(), slot);
        assert!(recovered_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
        assert_eq!(result.recovered_code.len(), 3); // Coding shreds 5, 7, 9 were missing
        let recovered_shred = result.recovered_code.remove(0);
        if let Shred::Coding(code) = recovered_shred {
            assert_eq!(code.header.num_data_shreds, 5);
            assert_eq!(code.header.num_coding_shreds, 5);
            assert_eq!(code.header.position, 0);
            assert_eq!(code.header.common_header.slot, slot);
            assert_eq!(code.header.common_header.index, 0);
        }
        let recovered_shred = result.recovered_code.remove(0);
        if let Shred::Coding(code) = recovered_shred {
            assert_eq!(code.header.num_data_shreds, 5);
            assert_eq!(code.header.num_coding_shreds, 5);
            assert_eq!(code.header.position, 2);
            assert_eq!(code.header.common_header.slot, slot);
            assert_eq!(code.header.common_header.index, 2);
        }
        let recovered_shred = result.recovered_code.remove(0);
        if let Shred::Coding(code) = recovered_shred {
            assert_eq!(code.header.num_data_shreds, 5);
            assert_eq!(code.header.num_coding_shreds, 5);
            assert_eq!(code.header.position, 4);
            assert_eq!(code.header.common_header.slot, slot);
            assert_eq!(code.header.common_header.index, 4);
        }
        assert_eq!(data[..], result.payload[..data.len()]);

        // Test3: Try recovery/reassembly with 3 missing data shreds + 2 coding shreds. Hint: should work
        let shreds: Vec<Shred> = shredder
            .shreds
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                if i % 2 != 0 {
                    Some(bincode::deserialize(s).unwrap())
                } else {
                    None
                }
            })
            .collect();

        let mut result = Shredder::deshred(&shreds).unwrap();
        assert!(result.payload.len() >= data.len());
        assert_eq!(result.recovered_data.len(), 3); // Data shreds 0, 2 and 4 were missing
        let recovered_shred = result.recovered_data.remove(0);
        let shred = bincode::serialize(&recovered_shred).unwrap();
        assert_matches!(recovered_shred, Shred::FirstInSlot(_));
        assert_eq!(recovered_shred.index(), 0);
        assert_eq!(recovered_shred.slot(), slot);
        assert!(recovered_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
        let recovered_shred = result.recovered_data.remove(0);
        let shred = bincode::serialize(&recovered_shred).unwrap();
        assert_matches!(recovered_shred, Shred::Data(_));
        assert_eq!(recovered_shred.index(), 2);
        assert_eq!(recovered_shred.slot(), slot);
        assert!(recovered_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
        let recovered_shred = result.recovered_data.remove(0);
        let shred = bincode::serialize(&recovered_shred).unwrap();
        assert_matches!(recovered_shred, Shred::LastInFECSet(_));
        assert_eq!(recovered_shred.index(), 4);
        assert_eq!(recovered_shred.slot(), slot);
        assert!(recovered_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
        assert_eq!(result.recovered_code.len(), 2); // Coding shreds 6, 8 were missing
        let recovered_shred = result.recovered_code.remove(0);
        if let Shred::Coding(code) = recovered_shred {
            assert_eq!(code.header.num_data_shreds, 5);
            assert_eq!(code.header.num_coding_shreds, 5);
            assert_eq!(code.header.position, 1);
            assert_eq!(code.header.common_header.slot, slot);
            assert_eq!(code.header.common_header.index, 1);
        }
        let recovered_shred = result.recovered_code.remove(0);
        if let Shred::Coding(code) = recovered_shred {
            assert_eq!(code.header.num_data_shreds, 5);
            assert_eq!(code.header.num_coding_shreds, 5);
            assert_eq!(code.header.position, 3);
            assert_eq!(code.header.common_header.slot, slot);
            assert_eq!(code.header.common_header.index, 3);
        }
        assert_eq!(data[..], result.payload[..data.len()]);

        // Test4: Try recovery/reassembly full slot with 3 missing data shreds + 2 coding shreds. Hint: should work
        let mut shredder =
            Shredder::new(slot, Some(5), 1.0, &keypair, 0).expect("Failed in creating shredder");

        let mut offset = shredder.write(&data).unwrap();
        let approx_shred_payload_size = offset;
        offset += shredder.write(&data[offset..]).unwrap();
        offset += shredder.write(&data[offset..]).unwrap();
        offset += shredder.write(&data[offset..]).unwrap();
        offset += shredder.write(&data[offset..]).unwrap();

        // We should have some shreds now
        assert_eq!(
            shredder.shreds.len(),
            data.len() / approx_shred_payload_size
        );
        assert_eq!(offset, data.len());

        shredder.finalize_slot();

        // We should have 10 shreds now (one additional final shred, and equal number of coding shreds)
        let expected_shred_count = ((data.len() / approx_shred_payload_size) + 1) * 2;
        assert_eq!(shredder.shreds.len(), expected_shred_count);

        let shreds: Vec<Shred> = shredder
            .shreds
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                if i % 2 != 0 {
                    Some(bincode::deserialize(s).unwrap())
                } else {
                    None
                }
            })
            .collect();

        let mut result = Shredder::deshred(&shreds).unwrap();
        assert!(result.payload.len() >= data.len());
        assert_eq!(result.recovered_data.len(), 3); // Data shreds 0, 2 and 4 were missing
        let recovered_shred = result.recovered_data.remove(0);
        let shred = bincode::serialize(&recovered_shred).unwrap();
        assert_matches!(recovered_shred, Shred::FirstInSlot(_));
        assert_eq!(recovered_shred.index(), 0);
        assert_eq!(recovered_shred.slot(), slot);
        assert!(recovered_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
        let recovered_shred = result.recovered_data.remove(0);
        let shred = bincode::serialize(&recovered_shred).unwrap();
        assert_matches!(recovered_shred, Shred::Data(_));
        assert_eq!(recovered_shred.index(), 2);
        assert_eq!(recovered_shred.slot(), slot);
        assert!(recovered_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
        let recovered_shred = result.recovered_data.remove(0);
        let shred = bincode::serialize(&recovered_shred).unwrap();
        assert_matches!(recovered_shred, Shred::LastInSlot(_));
        assert_eq!(recovered_shred.index(), 4);
        assert_eq!(recovered_shred.slot(), slot);
        assert!(recovered_shred
            .signature()
            .verify(keypair.pubkey().as_ref(), &shred[data_offset..]));
        assert_eq!(result.recovered_code.len(), 2); // Coding shreds 6, 8 were missing
        let recovered_shred = result.recovered_code.remove(0);
        if let Shred::Coding(code) = recovered_shred {
            assert_eq!(code.header.num_data_shreds, 5);
            assert_eq!(code.header.num_coding_shreds, 5);
            assert_eq!(code.header.position, 1);
            assert_eq!(code.header.common_header.slot, slot);
            assert_eq!(code.header.common_header.index, 1);
        }
        let recovered_shred = result.recovered_code.remove(0);
        if let Shred::Coding(code) = recovered_shred {
            assert_eq!(code.header.num_data_shreds, 5);
            assert_eq!(code.header.num_coding_shreds, 5);
            assert_eq!(code.header.position, 3);
            assert_eq!(code.header.common_header.slot, slot);
            assert_eq!(code.header.common_header.index, 3);
        }
        assert_eq!(data[..], result.payload[..data.len()]);

        // Test5: Try recovery/reassembly with 3 missing data shreds + 3 coding shreds. Hint: should fail
        let shreds: Vec<Shred> = shredder
            .shreds
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                if (i < 5 && i % 2 != 0) || (i >= 5 && i % 2 == 0) {
                    Some(bincode::deserialize(s).unwrap())
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(shreds.len(), 4);
        assert_matches!(
            Shredder::deshred(&shreds),
            Err(reed_solomon_erasure::Error::TooFewShardsPresent)
        );
    }
}
