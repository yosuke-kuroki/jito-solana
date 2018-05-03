// Support erasure coding

use packet::{BlobRecycler, SharedBlob};
use std::result;

//TODO(sakridge) pick these values
const NUM_CODED: usize = 10;
const MAX_MISSING: usize = 2;
const NUM_DATA: usize = NUM_CODED - MAX_MISSING;

#[derive(Debug, PartialEq, Eq)]
pub enum ErasureError {
    NotEnoughBlocksToDecode,
    DecodeError,
    InvalidBlockSize,
}

pub type Result<T> = result::Result<T, ErasureError>;

// k = number of data devices
// m = number of coding devices
// w = word size

extern "C" {
    fn jerasure_matrix_encode(
        k: i32,
        m: i32,
        w: i32,
        matrix: *const i32,
        data_ptrs: *const *const u8,
        coding_ptrs: *const *mut u8,
        size: i32,
    );
    fn jerasure_matrix_decode(
        k: i32,
        m: i32,
        w: i32,
        matrix: *const i32,
        row_k_ones: i32,
        erasures: *const i32,
        data_ptrs: *const *mut u8,
        coding_ptrs: *const *const u8,
        size: i32,
    ) -> i32;
    fn galois_single_divide(a: i32, b: i32, w: i32) -> i32;
}

fn get_matrix(m: i32, k: i32, w: i32) -> Vec<i32> {
    let mut matrix = vec![0; (m * k) as usize];
    for i in 0..m {
        for j in 0..k {
            unsafe {
                matrix[(i * k + j) as usize] = galois_single_divide(1, i ^ (m + j), w);
            }
        }
    }
    matrix
}

pub const ERASURE_W: i32 = 32;

// Generate coding blocks into coding
//   There are some alignment restrictions, blocks should be aligned by 16 bytes
//   which means their size should be >= 16 bytes
pub fn generate_coding_blocks(coding: &mut [&mut [u8]], data: &[&[u8]]) -> Result<()> {
    if data.len() == 0 {
        return Ok(());
    }
    let m = coding.len() as i32;
    let block_len = data[0].len();
    let matrix: Vec<i32> = get_matrix(m, data.len() as i32, ERASURE_W);
    let mut coding_arg = Vec::new();
    let mut data_arg = Vec::new();
    for block in data {
        if block_len != block.len() {
            return Err(ErasureError::InvalidBlockSize);
        }
        data_arg.push(block.as_ptr());
    }
    for mut block in coding {
        if block_len != block.len() {
            return Err(ErasureError::InvalidBlockSize);
        }
        coding_arg.push(block.as_mut_ptr());
    }

    unsafe {
        jerasure_matrix_encode(
            data.len() as i32,
            m,
            ERASURE_W,
            matrix.as_ptr(),
            data_arg.as_ptr(),
            coding_arg.as_ptr(),
            data[0].len() as i32,
        );
    }
    Ok(())
}

// Recover data + coding blocks into data blocks
//   data: array of blocks to recover into
//   coding: arry of coding blocks
//   erasures: list of indices in data where blocks should be recovered
pub fn decode_blocks(data: &mut [&mut [u8]], coding: &[&[u8]], erasures: &[i32]) -> Result<()> {
    if data.len() == 0 {
        return Ok(());
    }
    let block_len = data[0].len();
    let matrix: Vec<i32> = get_matrix(coding.len() as i32, data.len() as i32, ERASURE_W);

    // generate coding pointers, blocks should be the same size
    let mut coding_arg: Vec<*const u8> = Vec::new();
    for x in coding.iter() {
        if x.len() != block_len {
            return Err(ErasureError::InvalidBlockSize);
        }
        coding_arg.push(x.as_ptr());
    }

    // generate data pointers, blocks should be the same size
    let mut data_arg: Vec<*mut u8> = Vec::new();
    for x in data.iter_mut() {
        if x.len() != block_len {
            return Err(ErasureError::InvalidBlockSize);
        }
        data_arg.push(x.as_mut_ptr());
    }
    unsafe {
        let ret = jerasure_matrix_decode(
            data.len() as i32,
            coding.len() as i32,
            ERASURE_W,
            matrix.as_ptr(),
            0,
            erasures.as_ptr(),
            data_arg.as_ptr(),
            coding_arg.as_ptr(),
            data[0].len() as i32,
        );
        trace!("jerasure_matrix_decode ret: {}", ret);
        for x in data[erasures[0] as usize][0..8].iter() {
            trace!("{} ", x)
        }
        trace!("");
        if ret < 0 {
            return Err(ErasureError::DecodeError);
        }
    }
    Ok(())
}

// Generate coding blocks in window from consumed to consumed+NUM_DATA
pub fn generate_coding(
    re: &BlobRecycler,
    window: &mut Vec<SharedBlob>,
    consumed: usize,
) -> Result<()> {
    let mut data_blobs = Vec::new();
    let mut coding_blobs = Vec::new();
    let mut data_locks = Vec::new();
    let mut data_ptrs: Vec<&[u8]> = Vec::new();
    let mut coding_locks = Vec::new();
    let mut coding_ptrs: Vec<&mut [u8]> = Vec::new();
    for i in consumed..consumed + NUM_DATA {
        let n = i % window.len();
        data_blobs.push(window[n].clone().unwrap());
    }
    for b in &data_blobs {
        data_locks.push(b.write().unwrap());
    }
    for (i, l) in data_locks.iter_mut().enumerate() {
        trace!("i: {} data: {}", i, l.data[0]);
        data_ptrs.push(&l.data);
    }

    // generate coding ptr array
    let coding_start = consumed + NUM_DATA;
    let coding_end = consumed + NUM_CODED;
    for i in coding_start..coding_end {
        let n = i % window.len();
        window[n] = re.allocate();
        coding_blobs.push(window[n].clone().unwrap());
    }
    for b in &coding_blobs {
        coding_locks.push(b.write().unwrap());
    }
    for (i, l) in coding_locks.iter_mut().enumerate() {
        trace!("i: {} data: {}", i, l.data[0]);
        coding_ptrs.push(&mut l.data);
    }

    generate_coding_blocks(coding_ptrs.as_mut_slice(), &data_ptrs)?;
    trace!("consumed: {}", consumed);
    Ok(())
}

// Recover missing blocks into window
//   missing blocks should be None, will use re
//   to allocate new ones. Returns err if not enough
//   coding blocks are present to restore
pub fn recover(
    re: &BlobRecycler,
    window: &mut Vec<Option<SharedBlob>>,
    consumed: usize,
) -> Result<()> {
    //recover with erasure coding
    let mut data_missing = 0;
    let mut coded_missing = 0;
    let coding_start = consumed + NUM_DATA;
    let coding_end = consumed + NUM_CODED;
    for i in consumed..coding_end {
        let n = i % window.len();
        if window[n].is_none() {
            if i >= coding_start {
                coded_missing += 1;
            } else {
                data_missing += 1;
            }
        }
    }
    trace!("missing: data: {} coding: {}", data_missing, coded_missing);
    if data_missing > 0 {
        if (data_missing + coded_missing) <= MAX_MISSING {
            let mut blobs: Vec<SharedBlob> = Vec::new();
            let mut locks = Vec::new();
            let mut data_ptrs: Vec<&mut [u8]> = Vec::new();
            let mut coding_ptrs: Vec<&[u8]> = Vec::new();
            let mut erasures: Vec<i32> = Vec::new();
            for i in consumed..coding_end {
                let j = i % window.len();
                let mut b = &mut window[j];
                if b.is_some() {
                    blobs.push(b.clone().unwrap());
                    continue;
                }
                let n = re.allocate();
                *b = Some(n.clone());
                //mark the missing memory
                blobs.push(n);
                erasures.push((i - consumed) as i32);
            }
            erasures.push(-1);
            trace!("erasures: {:?}", erasures);
            //lock everything
            for b in &blobs {
                locks.push(b.write().unwrap());
            }
            for (i, l) in locks.iter_mut().enumerate() {
                if i >= NUM_DATA {
                    trace!("pushing coding: {}", i);
                    coding_ptrs.push(&l.data);
                } else {
                    trace!("pushing data: {}", i);
                    data_ptrs.push(&mut l.data);
                }
            }
            trace!(
                "coding_ptrs.len: {} data_ptrs.len {}",
                coding_ptrs.len(),
                data_ptrs.len()
            );
            decode_blocks(data_ptrs.as_mut_slice(), &coding_ptrs, &erasures)?;
        } else {
            return Err(ErasureError::NotEnoughBlocksToDecode);
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use erasure;
    use packet::{BlobRecycler, SharedBlob, PACKET_DATA_SIZE};

    #[test]
    pub fn test_coding() {
        let zero_vec = vec![0; 16];
        let mut vs: Vec<Vec<u8>> = (0..4).map(|i| (i..(16 + i)).collect()).collect();
        let v_orig: Vec<u8> = vs[0].clone();

        let m = 2;
        let mut coding_blocks: Vec<_> = (0..m).map(|_| vec![0u8; 16]).collect();

        {
            let mut coding_blocks_slices: Vec<_> =
                coding_blocks.iter_mut().map(|x| x.as_mut_slice()).collect();
            let v_slices: Vec<_> = vs.iter().map(|x| x.as_slice()).collect();

            assert!(
                erasure::generate_coding_blocks(
                    coding_blocks_slices.as_mut_slice(),
                    v_slices.as_slice()
                ).is_ok()
            );
        }
        trace!("coding blocks:");
        for b in &coding_blocks {
            trace!("{:?}", b);
        }
        let erasure: i32 = 1;
        let erasures = vec![erasure, -1];
        // clear an entry
        vs[erasure as usize].copy_from_slice(zero_vec.as_slice());

        {
            let coding_blocks_slices: Vec<_> = coding_blocks.iter().map(|x| x.as_slice()).collect();
            let mut v_slices: Vec<_> = vs.iter_mut().map(|x| x.as_mut_slice()).collect();

            assert!(
                erasure::decode_blocks(
                    v_slices.as_mut_slice(),
                    coding_blocks_slices.as_slice(),
                    erasures.as_slice(),
                ).is_ok()
            );
        }

        trace!("vs:");
        for v in &vs {
            trace!("{:?}", v);
        }
        assert_eq!(v_orig, vs[0]);
    }

    fn print_window(window: &Vec<Option<SharedBlob>>) {
        for (i, w) in window.iter().enumerate() {
            print!("window({}): ", i);
            if w.is_some() {
                let window_lock = w.clone().unwrap();
                let window_data = window_lock.read().unwrap().data;
                for i in 0..8 {
                    print!("{} ", window_data[i]);
                }
            } else {
                print!("null");
            }
            println!("");
        }
    }

    #[test]
    pub fn test_window_recover() {
        let mut window = Vec::new();
        let blob_recycler = BlobRecycler::default();
        let offset = 4;
        for i in 0..(4 * erasure::NUM_CODED + 1) {
            let b = blob_recycler.allocate();
            let b_ = b.clone();
            let data_len = b.read().unwrap().data.len();
            let mut w = b.write().unwrap();
            w.set_index(i as u64).unwrap();
            assert_eq!(i as u64, w.get_index().unwrap());
            w.meta.size = PACKET_DATA_SIZE;
            for k in 0..data_len {
                w.data[k] = (k + i) as u8;
            }
            window.push(Some(b_));
        }
        println!("** after-gen:");
        print_window(&window);
        assert!(erasure::generate_coding(&blob_recycler, &mut window, offset).is_ok());
        assert!(
            erasure::generate_coding(&blob_recycler, &mut window, offset + erasure::NUM_CODED)
                .is_ok()
        );
        assert!(
            erasure::generate_coding(
                &blob_recycler,
                &mut window,
                offset + (2 * erasure::NUM_CODED)
            ).is_ok()
        );
        assert!(
            erasure::generate_coding(
                &blob_recycler,
                &mut window,
                offset + (3 * erasure::NUM_CODED)
            ).is_ok()
        );
        println!("** after-coding:");
        print_window(&window);
        let refwindow = window[offset + 1].clone();
        window[offset + 1] = None;
        window[offset + 2] = None;
        window[offset + erasure::NUM_CODED + 3] = None;
        window[offset + (2 * erasure::NUM_CODED) + 0] = None;
        window[offset + (2 * erasure::NUM_CODED) + 1] = None;
        window[offset + (2 * erasure::NUM_CODED) + 2] = None;
        let window_l0 = &(window[offset + (3 * erasure::NUM_CODED)]).clone().unwrap();
        window_l0.write().unwrap().data[0] = 55;
        println!("** after-nulling:");
        print_window(&window);
        assert!(erasure::recover(&blob_recycler, &mut window, offset).is_ok());
        assert!(erasure::recover(&blob_recycler, &mut window, offset + erasure::NUM_CODED).is_ok());
        assert!(
            erasure::recover(
                &blob_recycler,
                &mut window,
                offset + (2 * erasure::NUM_CODED)
            ).is_err()
        );
        assert!(
            erasure::recover(
                &blob_recycler,
                &mut window,
                offset + (3 * erasure::NUM_CODED)
            ).is_ok()
        );
        println!("** after-restore:");
        print_window(&window);
        let window_l = window[offset + 1].clone().unwrap();
        let ref_l = refwindow.clone().unwrap();
        assert_eq!(
            window_l.read().unwrap().data.to_vec(),
            ref_l.read().unwrap().data.to_vec()
        );
    }
}
