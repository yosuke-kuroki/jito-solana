pub mod bpf_verifier;

extern crate bincode;
extern crate byteorder;
extern crate env_logger;
#[macro_use]
extern crate log;
extern crate solana_rbpf;
extern crate solana_sdk;

use bincode::deserialize;
use byteorder::{ByteOrder, LittleEndian, WriteBytesExt};
use solana_rbpf::{helpers, EbpfVmRaw};
use solana_sdk::account::KeyedAccount;
use solana_sdk::loader_instruction::LoaderInstruction;
use solana_sdk::pubkey::Pubkey;
use std::io::prelude::*;
use std::io::Error;
use std::mem;
use std::sync::{Once, ONCE_INIT};

#[allow(dead_code)]
fn dump_program(key: &Pubkey, prog: &[u8]) {
    let mut eight_bytes: Vec<u8> = Vec::new();
    info!("BPF Program: {:?}", key);
    for i in prog.iter() {
        if eight_bytes.len() >= 7 {
            info!("{:02X?}", eight_bytes);
            eight_bytes.clear();
        } else {
            eight_bytes.push(i.clone());
        }
    }
}

pub fn helper_printf(arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> u64 {
    info!(
        "bpf_trace_printf: {:#x}, {:#x}, {:#x}, {:#x}, {:#x}",
        arg1, arg2, arg3, arg4, arg5
    );
    let size_arg = |x| {
        if x == 0 {
            1
        } else {
            (x as f64).log(16.0).floor() as u64 + 1
        }
    };
    "bpf_trace_printf: 0x, 0x, 0x, 0x, 0x\n".len() as u64
        + size_arg(arg1)
        + size_arg(arg2)
        + size_arg(arg3)
        + size_arg(arg4)
        + size_arg(arg5)
}

fn create_vm(prog: &[u8]) -> Result<EbpfVmRaw, Error> {
    let mut vm = EbpfVmRaw::new(None)?;
    vm.set_verifier(bpf_verifier::check)?;
    vm.set_max_instruction_count(36000)?; // 36000 is a wag, need to tune
    vm.set_program(&prog)?;
    vm.register_helper(helpers::BPF_TRACE_PRINTK_IDX, helper_printf)?;
    Ok(vm)
}

fn serialize_parameters(keyed_accounts: &mut [KeyedAccount], data: &[u8]) -> Vec<u8> {
    assert_eq!(32, mem::size_of::<Pubkey>());

    let mut v: Vec<u8> = Vec::new();
    v.write_u64::<LittleEndian>(keyed_accounts.len() as u64)
        .unwrap();
    for info in keyed_accounts.iter_mut() {
        v.write_all(info.key.as_ref()).unwrap();
        v.write_i64::<LittleEndian>(info.account.tokens).unwrap();
        v.write_u64::<LittleEndian>(info.account.userdata.len() as u64)
            .unwrap();
        v.write_all(&info.account.userdata).unwrap();
        v.write_all(info.account.program_id.as_ref()).unwrap();
    }
    v.write_u64::<LittleEndian>(data.len() as u64).unwrap();
    v.write_all(data).unwrap();
    v
}

fn deserialize_parameters(keyed_accounts: &mut [KeyedAccount], buffer: &[u8]) {
    assert_eq!(32, mem::size_of::<Pubkey>());

    let mut start = mem::size_of::<u64>();
    for info in keyed_accounts.iter_mut() {
        start += mem::size_of::<Pubkey>(); // skip pubkey
        info.account.tokens = LittleEndian::read_i64(&buffer[start..]);

        start += mem::size_of::<u64>() // skip tokens
                  + mem::size_of::<u64>(); // skip length tag
        let end = start + info.account.userdata.len();
        info.account.userdata.clone_from_slice(&buffer[start..end]);

        start += info.account.userdata.len() // skip userdata
                  + mem::size_of::<Pubkey>(); // skip program_id
    }
}

#[no_mangle]
pub extern "C" fn process(keyed_accounts: &mut [KeyedAccount], tx_data: &[u8]) -> bool {
    static INIT: Once = ONCE_INIT;
    INIT.call_once(|| {
        // env_logger can only be initialized once
        env_logger::init();
    });

    if keyed_accounts[0].account.executable {
        let prog = keyed_accounts[0].account.userdata.clone();
        trace!("Call BPF, {} instructions", prog.len() / 8);
        //dump_program(keyed_accounts[0].key, &prog);
        let mut vm = match create_vm(&prog) {
            Ok(vm) => vm,
            Err(e) => {
                warn!("create_vm failed: {}", e);
                return false;
            }
        };
        let mut v = serialize_parameters(&mut keyed_accounts[1..], &tx_data);
        match vm.execute_program(v.as_mut_slice()) {
            Ok(status) => if 0 == status {
                return false;
            },
            Err(e) => {
                warn!("execute_program failed: {}", e);
                return false;
            }
        }
        deserialize_parameters(&mut keyed_accounts[1..], &v);
        trace!(
            "BPF program executed {} instructions",
            vm.get_last_instruction_count()
        );
    } else if let Ok(instruction) = deserialize(tx_data) {
        match instruction {
            LoaderInstruction::Write { offset, bytes } => {
                let offset = offset as usize;
                let len = bytes.len();
                debug!("Write: offset={} length={}", offset, len);
                if keyed_accounts[0].account.userdata.len() < offset + len {
                    warn!(
                        "Write overflow: {} < {}",
                        keyed_accounts[0].account.userdata.len(),
                        offset + len
                    );
                    return false;
                }
                keyed_accounts[0].account.userdata[offset..offset + len].copy_from_slice(&bytes);
            }
            LoaderInstruction::Finalize => {
                keyed_accounts[0].account.executable = true;
                info!("Finalize: account {:?}", keyed_accounts[0].key);
            }
        }
    } else {
        warn!("Invalid program transaction: {:?}", tx_data);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[should_panic(expected = "Error: Execution exceeded maximum number of instructions")]
    fn test_non_terminating_program() {
        #[rustfmt::skip]
        let prog = &[
            0xb7, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // r6 = 0
            0xb7, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // r1 = 0
            0xb7, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // r2 = 0
            0xb7, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // r3 = 0
            0xb7, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // r4 = 0
            0xbf, 0x65, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // r5 = r6
            0x85, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, // call 6
            0x07, 0x06, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, // r6 + 1
            0x05, 0x00, 0xf8, 0xff, 0x00, 0x00, 0x00, 0x00, // goto -8
            0x95, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // exit
        ];
        let input = &mut [0x00];
        let mut vm = create_vm(prog).unwrap();
        vm.execute_program(input).unwrap();
    }
}
