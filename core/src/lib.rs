//! The `solana` library implements the Solana high-performance blockchain architecture.
//! It includes a full Rust implementation of the architecture (see
//! [Validator](server/struct.Validator.html)) as well as hooks to GPU implementations of its most
//! paralellizable components (i.e. [SigVerify](sigverify/index.html)).  It also includes
//! command-line tools to spin up validators and a Rust library
//!

pub mod banking_stage;
pub mod broadcast_stage;
pub mod chacha;
pub mod chacha_cuda;
pub mod cluster_info_vote_listener;
pub mod commitment;
pub mod shred_fetch_stage;
pub mod thread_mem_usage;
#[macro_use]
pub mod contact_info;
pub mod archiver;
pub mod blockstream;
pub mod blockstream_service;
pub mod cluster_info;
pub mod cluster_info_repair_listener;
pub mod consensus;
pub mod crds;
pub mod crds_gossip;
pub mod crds_gossip_error;
pub mod crds_gossip_pull;
pub mod crds_gossip_push;
pub mod crds_value;
pub mod fetch_stage;
pub mod gen_keys;
pub mod genesis_utils;
pub mod gossip_service;
pub mod ledger_cleanup_service;
pub mod local_vote_signer_service;
pub mod packet;
pub mod partition_cfg;
pub mod poh_recorder;
pub mod poh_service;
pub mod recvmmsg;
pub mod repair_service;
pub mod replay_stage;
pub mod result;
pub mod retransmit_stage;
pub mod rpc;
pub mod rpc_pubsub;
pub mod rpc_pubsub_service;
pub mod rpc_service;
pub mod rpc_subscriptions;
pub mod sendmmsg;
pub mod sigverify;
pub mod sigverify_shreds;
pub mod sigverify_stage;
pub mod snapshot_packager_service;
pub mod storage_stage;
pub mod streamer;
pub mod tpu;
pub mod tvu;
pub mod validator;
pub mod weighted_shuffle;
pub mod window_service;

#[macro_use]
extern crate solana_budget_program;

#[cfg(test)]
#[macro_use]
extern crate hex_literal;

#[macro_use]
extern crate log;

#[macro_use]
extern crate serde_derive;

#[cfg(test)]
#[macro_use]
extern crate serde_json;

#[macro_use]
extern crate solana_metrics;

#[cfg(test)]
#[macro_use]
extern crate matches;

extern crate jemallocator;

#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;
