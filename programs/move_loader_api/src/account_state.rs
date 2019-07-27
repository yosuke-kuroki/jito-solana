use crate::data_store::DataStore;
use bytecode_verifier::VerifiedModule;
use compiler::Compiler;
use serde_derive::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use std::convert::TryInto;
use stdlib::stdlib_modules;
use types::{
    account_address::AccountAddress,
    byte_array::ByteArray,
    write_set::{WriteOp, WriteSet},
};
use vm::{
    access::ModuleAccess, file_format::CompiledModule, transaction_metadata::TransactionMetadata,
};
use vm_cache_map::Arena;
use vm_runtime::{
    code_cache::{
        module_adapter::FakeFetcher,
        module_cache::{BlockModuleCache, VMModuleCache},
    },
    data_cache::BlockDataCache,
    txn_executor::{TransactionExecutor, ACCOUNT_MODULE, COIN_MODULE},
    value::Local,
};

// Helper function that converts a Solana Pubkey to a Libra AccountAddress (WIP)
pub fn pubkey_to_address(key: &Pubkey) -> AccountAddress {
    AccountAddress::new(*to_array_32(key.as_ref()))
}
fn to_array_32(array: &[u8]) -> &[u8; 32] {
    array.try_into().expect("slice with incorrect length")
}

/// Type of Libra account held by a Solana account
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LibraAccountState {
    /// No data for this account yet
    Unallocated,
    /// Serialized compiled program bytes
    CompiledProgram {
        script_bytes: Vec<u8>,
        modules_bytes: Vec<Vec<u8>>,
    },
    /// Serialized verified program bytes
    VerifiedProgram {
        script_bytes: Vec<u8>,
        modules_bytes: Vec<Vec<u8>>,
    },
    /// Write set containing a Libra account's data
    User(WriteSet),
    /// Write sets containing the mint and stdlib modules
    Genesis(WriteSet),
}
impl LibraAccountState {
    pub fn create_unallocated() -> Self {
        LibraAccountState::Unallocated
    }

    pub fn create_program(
        sender_address: &AccountAddress,
        code: &str,
        deps: Vec<&Vec<u8>>,
    ) -> Self {
        // Compiler needs all the dependencies all the dependency module's account's
        // data into `VerifiedModules`
        let mut extra_deps: Vec<VerifiedModule> = vec![];
        for dep in deps {
            let state: LibraAccountState = bincode::deserialize(&dep).unwrap();
            if let LibraAccountState::User(write_set) = state {
                for (_, write_op) in write_set.iter() {
                    if let WriteOp::Value(raw_bytes) = write_op {
                        extra_deps.push(
                            VerifiedModule::new(CompiledModule::deserialize(&raw_bytes).unwrap())
                                .unwrap(),
                        );
                    }
                }
            }
        }

        let compiler = Compiler {
            address: *sender_address,
            code,
            extra_deps,
            ..Compiler::default()
        };
        let compiled_program = compiler.into_compiled_program().expect("Failed to compile");

        let mut script_bytes = vec![];
        compiled_program
            .script
            .serialize(&mut script_bytes)
            .expect("Unable to serialize script");
        let mut modules_bytes = vec![];
        for module in compiled_program.modules.iter() {
            let mut buf = vec![];
            module
                .serialize(&mut buf)
                .expect("Unable to serialize module");
            modules_bytes.push(buf);
        }
        LibraAccountState::CompiledProgram {
            script_bytes,
            modules_bytes,
        }
    }

    pub fn create_user(write_set: WriteSet) -> Self {
        LibraAccountState::User(write_set)
    }

    pub fn create_genesis(mint_balance: u64) -> Self {
        let modules = stdlib_modules();
        let arena = Arena::new();
        let state_view = DataStore::default();
        let vm_cache = VMModuleCache::new(&arena);
        // Libra enforces the mint address to be 0x0 (see Libra's `mint_to_address` function)
        let mint_address = AccountAddress::default();
        // TODO: Need this?
        let genesis_auth_key = ByteArray::new(mint_address.to_vec());

        let write_set = {
            let fake_fetcher =
                FakeFetcher::new(modules.iter().map(|m| m.as_inner().clone()).collect());
            let data_cache = BlockDataCache::new(&state_view);
            let block_cache = BlockModuleCache::new(&vm_cache, fake_fetcher);

            let mut txn_data = TransactionMetadata::default();
            txn_data.sender = mint_address;

            let mut txn_executor = TransactionExecutor::new(&block_cache, &data_cache, txn_data);
            txn_executor.create_account(mint_address).unwrap().unwrap();
            txn_executor
                .execute_function(&COIN_MODULE, "initialize", vec![])
                .unwrap()
                .unwrap();

            txn_executor
                .execute_function(
                    &ACCOUNT_MODULE,
                    "mint_to_address",
                    vec![Local::address(mint_address), Local::u64(mint_balance)],
                )
                .unwrap()
                .unwrap();

            txn_executor
                .execute_function(
                    &ACCOUNT_MODULE,
                    "rotate_authentication_key",
                    vec![Local::bytearray(genesis_auth_key)],
                )
                .unwrap()
                .unwrap();

            let stdlib_modules = modules
                .iter()
                .map(|m| {
                    let mut module_vec = vec![];
                    m.serialize(&mut module_vec).unwrap();
                    (m.self_id(), module_vec)
                })
                .collect();

            txn_executor
                .make_write_set(stdlib_modules, Ok(Ok(())))
                .unwrap()
                .write_set()
                .clone()
                .into_mut()
        }
        .freeze()
        .unwrap();

        LibraAccountState::Genesis(write_set)
    }
}
