use crate::rpc_subscriptions::RpcSubscriptions;
use solana_client::rpc_response::SlotUpdate;
use solana_ledger::blockstore::CompletedSlotsReceiver;
use solana_sdk::timing::timestamp;
use std::{
    sync::Arc,
    thread::{Builder, JoinHandle},
};

pub struct RpcCompletedSlotsService;
impl RpcCompletedSlotsService {
    pub fn spawn(
        completed_slots_receiver: CompletedSlotsReceiver,
        rpc_subscriptions: Option<Arc<RpcSubscriptions>>,
    ) -> Option<JoinHandle<()>> {
        let rpc_subscriptions = rpc_subscriptions?;
        Some(
            Builder::new()
                .name("solana-rpc-completed-slots-service".to_string())
                .spawn(move || {
                    for slots in completed_slots_receiver.iter() {
                        for slot in slots {
                            rpc_subscriptions.notify_slot_update(SlotUpdate::Completed {
                                slot,
                                timestamp: timestamp(),
                            });
                        }
                    }
                })
                .unwrap(),
        )
    }
}
