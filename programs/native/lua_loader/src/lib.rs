extern crate bincode;
extern crate env_logger;
#[macro_use]
extern crate log;
extern crate rlua;
extern crate solana_sdk;

use bincode::deserialize;
use rlua::{Lua, Result, Table};
use solana_sdk::account::KeyedAccount;
use solana_sdk::loader_instruction::LoaderInstruction;
use std::str;
use std::sync::{Once, ONCE_INIT};

/// Make KeyAccount values available to Lua.
fn set_accounts(lua: &Lua, name: &str, keyed_accounts: &[KeyedAccount]) -> Result<()> {
    let accounts = lua.create_table()?;
    for (i, keyed_account) in keyed_accounts.iter().enumerate() {
        let account = lua.create_table()?;
        account.set("key", keyed_account.key.to_string())?;
        account.set("tokens", keyed_account.account.tokens)?;
        let data_str = lua.create_string(&keyed_account.account.userdata)?;
        account.set("userdata", data_str)?;
        accounts.set(i + 1, account)?;
    }
    let globals = lua.globals();
    globals.set(name, accounts)
}

/// Commit the new KeyedAccount values.
fn update_accounts(lua: &Lua, name: &str, keyed_accounts: &mut [KeyedAccount]) -> Result<()> {
    let globals = lua.globals();
    let accounts: Table = globals.get(name)?;
    for (i, keyed_account) in keyed_accounts.into_iter().enumerate() {
        let account: Table = accounts.get(i + 1)?;
        keyed_account.account.tokens = account.get("tokens")?;
        let data_str: rlua::String = account.get("userdata")?;
        keyed_account.account.userdata = data_str.as_bytes().to_vec();
    }
    Ok(())
}

fn run_lua(keyed_accounts: &mut [KeyedAccount], code: &str, data: &[u8]) -> Result<()> {
    let lua = Lua::new();
    let globals = lua.globals();
    let data_str = lua.create_string(data)?;
    globals.set("data", data_str)?;

    set_accounts(&lua, "accounts", keyed_accounts)?;
    lua.exec::<_, ()>(code, None)?;
    update_accounts(&lua, "accounts", keyed_accounts)
}

#[no_mangle]
pub extern "C" fn process(keyed_accounts: &mut [KeyedAccount], tx_data: &[u8]) -> bool {
    static INIT: Once = ONCE_INIT;
    INIT.call_once(|| {
        // env_logger can only be initialized once
        env_logger::init();
    });

    if keyed_accounts[0].account.executable {
        let code = keyed_accounts[0].account.userdata.clone();
        let code = str::from_utf8(&code).unwrap();
        match run_lua(&mut keyed_accounts[1..], &code, tx_data) {
            Ok(()) => {
                trace!("Lua success");
                return true;
            }
            Err(e) => {
                warn!("Lua Error: {:#?}", e);
                return false;
            }
        }
    } else if let Ok(instruction) = deserialize(tx_data) {
        match instruction {
            LoaderInstruction::Write { offset, bytes } => {
                let offset = offset as usize;
                let len = bytes.len();
                trace!("LuaLoader::Write offset {} length {:?}", offset, len);
                if keyed_accounts[0].account.userdata.len() < offset + len {
                    warn!(
                        "Write overflow {} < {}",
                        keyed_accounts[0].account.userdata.len(),
                        offset + len
                    );
                    return false;
                }
                keyed_accounts[0].account.userdata[offset..offset + len].copy_from_slice(&bytes);
            }

            LoaderInstruction::Finalize => {
                keyed_accounts[0].account.executable = true;
                trace!("LuaLoader::Finalize prog: {:?}", keyed_accounts[0].key);
            }
        }
    } else {
        warn!("Invalid program transaction: {:?}", tx_data);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    extern crate bincode;

    use self::bincode::serialize;
    use super::*;
    use solana_sdk::account::{create_keyed_accounts, Account};
    use solana_sdk::pubkey::Pubkey;
    use std::fs::File;
    use std::io::prelude::*;
    use std::path::PathBuf;

    #[test]
    fn test_update_accounts() -> Result<()> {
        let mut accounts = [(Pubkey::default(), Account::default())];
        let mut keyed_accounts = create_keyed_accounts(&mut accounts);
        let lua = Lua::new();
        set_accounts(&lua, "xs", &keyed_accounts)?;
        keyed_accounts[0].account.tokens = 42;
        keyed_accounts[0].account.userdata = vec![];
        update_accounts(&lua, "xs", &mut keyed_accounts)?;

        // Ensure update_accounts() overwrites the local value 42.
        assert_eq!(keyed_accounts[0].account.tokens, 0);

        Ok(())
    }

    #[test]
    fn test_credit_with_lua() -> Result<()> {
        let code = r#"accounts[1].tokens = accounts[1].tokens + 1"#;
        let mut accounts = [(Pubkey::default(), Account::default())];
        run_lua(&mut create_keyed_accounts(&mut accounts), code, &[])?;
        assert_eq!(accounts[0].1.tokens, 1);
        Ok(())
    }

    #[test]
    fn test_error_with_lua() {
        let code = r#"accounts[1].tokens += 1"#;
        let mut accounts = [(Pubkey::default(), Account::default())];
        assert!(run_lua(&mut create_keyed_accounts(&mut accounts), code, &[]).is_err());
    }

    #[test]
    fn test_move_funds_with_lua_via_process() {
        let userdata = r#"
            local tokens, _ = string.unpack("I", data)
            accounts[1].tokens = accounts[1].tokens - tokens
            accounts[2].tokens = accounts[2].tokens + tokens
        "#.as_bytes()
        .to_vec();

        let alice_pubkey = Pubkey::default();
        let bob_pubkey = Pubkey::default();
        let program_id = Pubkey::default();

        let mut accounts = [
            (
                Pubkey::default(),
                Account {
                    tokens: 1,
                    userdata,
                    program_id,
                    executable: true,
                    loader_program_id: Pubkey::default(),
                },
            ),
            (alice_pubkey, Account::new(100, 0, program_id)),
            (bob_pubkey, Account::new(1, 0, program_id)),
        ];
        let data = serialize(&10u64).unwrap();
        process(&mut create_keyed_accounts(&mut accounts), &data);
        assert_eq!(accounts[1].1.tokens, 90);
        assert_eq!(accounts[2].1.tokens, 11);

        process(&mut create_keyed_accounts(&mut accounts), &data);
        assert_eq!(accounts[1].1.tokens, 80);
        assert_eq!(accounts[2].1.tokens, 21);
    }

    fn read_test_file(name: &str) -> Vec<u8> {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push(name);
        let mut file = File::open(path).unwrap();
        let mut contents = vec![];
        file.read_to_end(&mut contents).unwrap();
        contents
    }

    #[test]
    fn test_load_lua_library() {
        let userdata = r#"
            local serialize = load(accounts[2].userdata)().serialize
            accounts[3].userdata = serialize({a=1, b=2, c=3}, nil, "s")
        "#.as_bytes()
        .to_vec();
        let program_id = Pubkey::default();
        let program_account = Account {
            tokens: 1,
            userdata,
            program_id,
            executable: true,
            loader_program_id: Pubkey::default(),
        };
        let alice_account = Account::new(100, 0, program_id);
        let serialize_account = Account {
            tokens: 100,
            userdata: read_test_file("serialize.lua"),
            program_id,
            executable: false,
            loader_program_id: Pubkey::default(),
        };
        let mut accounts = [
            (Pubkey::default(), program_account),
            (Pubkey::default(), alice_account),
            (Pubkey::default(), serialize_account),
            (Pubkey::default(), Account::new(1, 0, program_id)),
        ];
        let mut keyed_accounts = create_keyed_accounts(&mut accounts);
        process(&mut keyed_accounts, &[]);
        // Verify deterministic ordering of a serialized Lua table.
        assert_eq!(
            str::from_utf8(&keyed_accounts[3].account.userdata).unwrap(),
            "{a=1,b=2,c=3}"
        );
    }

    #[test]
    fn test_lua_multisig() {
        let program_id = Pubkey::default();

        let alice_pubkey = Pubkey::new(&[0; 32]);
        let serialize_pubkey = Pubkey::new(&[1; 32]);
        let state_pubkey = Pubkey::new(&[2; 32]);
        let bob_pubkey = Pubkey::new(&[3; 32]);
        let carol_pubkey = Pubkey::new(&[4; 32]);
        let dan_pubkey = Pubkey::new(&[5; 32]);
        let erin_pubkey = Pubkey::new(&[6; 32]);

        let program_account = Account {
            tokens: 1,
            userdata: read_test_file("multisig.lua"),
            program_id,
            executable: true,
            loader_program_id: Pubkey::default(),
        };

        let alice_account = Account {
            tokens: 100,
            userdata: Vec::new(),
            program_id,
            executable: true,
            loader_program_id: Pubkey::default(),
        };

        let serialize_account = Account {
            tokens: 100,
            userdata: read_test_file("serialize.lua"),
            program_id,
            executable: true,
            loader_program_id: Pubkey::default(),
        };

        let mut accounts = [
            (Pubkey::default(), program_account), // Account holding the program
            (alice_pubkey, alice_account),        // The payer
            (serialize_pubkey, serialize_account), // Where the serialize library is stored.
            (state_pubkey, Account::new(1, 0, program_id)), // Where program state is stored.
            (bob_pubkey, Account::new(1, 0, program_id)), // The payee once M signatures are collected.
        ];
        let mut keyed_accounts = create_keyed_accounts(&mut accounts);

        let data = format!(
            r#"{{m=2, n={{"{}","{}","{}"}}, tokens=100}}"#,
            carol_pubkey, dan_pubkey, erin_pubkey
        ).as_bytes()
        .to_vec();

        process(&mut keyed_accounts, &data);
        assert_eq!(keyed_accounts[4].account.tokens, 1);

        let data = format!(r#""{}""#, carol_pubkey).into_bytes();
        process(&mut keyed_accounts, &data);
        assert_eq!(keyed_accounts[4].account.tokens, 1);

        let data = format!(r#""{}""#, dan_pubkey).into_bytes();
        process(&mut keyed_accounts, &data);
        assert_eq!(keyed_accounts[4].account.tokens, 101); // Pay day!

        let data = format!(r#""{}""#, erin_pubkey).into_bytes();
        process(&mut keyed_accounts, &data);
        assert_eq!(keyed_accounts[4].account.tokens, 101); // No change!
    }
}
