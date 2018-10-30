//! Native loader
use bincode::deserialize;
use libc;
#[cfg(unix)]
use libloading::os::unix::*;
#[cfg(windows)]
use libloading::os::windows::*;
use solana_sdk::account::KeyedAccount;
use solana_sdk::loader_instruction::LoaderInstruction;
use solana_sdk::pubkey::Pubkey;
use std::env;
use std::path::PathBuf;
use std::str;

/// Dynamic link library prefixs
#[cfg(unix)]
const PLATFORM_FILE_PREFIX_NATIVE: &str = "lib";
#[cfg(windows)]
const PLATFORM_FILE_PREFIX_NATIVE: &str = "";

/// Dynamic link library file extension specific to the platform
#[cfg(any(target_os = "macos", target_os = "ios"))]
const PLATFORM_FILE_EXTENSION_NATIVE: &str = "dylib";
/// Dynamic link library file extension specific to the platform
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
const PLATFORM_FILE_EXTENSION_NATIVE: &str = "so";
/// Dynamic link library file extension specific to the platform
#[cfg(windows)]
const PLATFORM_FILE_EXTENSION_NATIVE: &str = "dll";

fn create_path(name: &str) -> PathBuf {
    let pathbuf = {
        let current_exe = env::current_exe().unwrap();
        PathBuf::from(current_exe.parent().unwrap())
    };

    pathbuf.join(
        PathBuf::from(PLATFORM_FILE_PREFIX_NATIVE.to_string() + name)
            .with_extension(PLATFORM_FILE_EXTENSION_NATIVE),
    )
}

const NATIVE_LOADER_PROGRAM_ID: [u8; 32] = [
    1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

// All native programs export a symbol named process()
const ENTRYPOINT: &str = "process";
type Entrypoint = unsafe extern "C" fn(keyed_accounts: &mut [KeyedAccount], data: &[u8]) -> bool;

pub fn check_id(program_id: &Pubkey) -> bool {
    program_id.as_ref() == NATIVE_LOADER_PROGRAM_ID
}

pub fn id() -> Pubkey {
    Pubkey::new(&NATIVE_LOADER_PROGRAM_ID)
}

pub fn process_transaction(keyed_accounts: &mut [KeyedAccount], tx_data: &[u8]) -> bool {
    if keyed_accounts[0].account.executable {
        // dispatch it
        let name = keyed_accounts[0].account.userdata.clone();
        let name = match str::from_utf8(&name) {
            Ok(v) => v,
            Err(e) => {
                warn!("Invalid UTF-8 sequence: {}", e);
                return false;
            }
        };
        trace!("Call native {:?}", name);
        let path = create_path(&name);
        // TODO linux tls bug can cause crash on dlclose(), workaround by never unloading
        match Library::open(Some(&path), libc::RTLD_NODELETE | libc::RTLD_NOW) {
            Ok(library) => unsafe {
                let entrypoint: Symbol<Entrypoint> = match library.get(ENTRYPOINT.as_bytes()) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("{:?}: Unable to find {:?} in program", e, ENTRYPOINT);
                        return false;
                    }
                };
                return entrypoint(&mut keyed_accounts[1..], tx_data);
            },
            Err(e) => {
                warn!("Unable to load: {:?}", e);
                return false;
            }
        }
    } else if let Ok(instruction) = deserialize(tx_data) {
        match instruction {
            LoaderInstruction::Write { offset, bytes } => {
                trace!("NativeLoader::Write offset {} bytes {:?}", offset, bytes);
                let offset = offset as usize;
                if keyed_accounts[0].account.userdata.len() < offset + bytes.len() {
                    warn!(
                        "Error: Overflow, {} < {}",
                        keyed_accounts[0].account.userdata.len(),
                        offset + bytes.len()
                    );
                    return false;
                }
                // native loader takes a name and we assume it all comes in at once
                keyed_accounts[0].account.userdata = bytes;
            }

            LoaderInstruction::Finalize => {
                keyed_accounts[0].account.executable = true;
                trace!("NativeLoader::Finalize prog: {:?}", keyed_accounts[0].key);
            }
        }
    } else {
        warn!("Invalid program transaction: {:?}", tx_data);
    }
    true
}
