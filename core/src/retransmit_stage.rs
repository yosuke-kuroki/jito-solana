//! The `retransmit_stage` retransmits shreds between validators
#![allow(clippy::rc_buffer)]

use crate::{
    cluster_info::{compute_retransmit_peers, ClusterInfo, DATA_PLANE_FANOUT},
    cluster_info_vote_listener::VerifiedVoteReceiver,
    cluster_slots::ClusterSlots,
    cluster_slots_service::ClusterSlotsService,
    completed_data_sets_service::CompletedDataSetsSender,
    contact_info::ContactInfo,
    max_slots::MaxSlots,
    repair_service::DuplicateSlotsResetSender,
    repair_service::RepairInfo,
    result::{Error, Result},
    rpc_completed_slots_service::RpcCompletedSlotsService,
    rpc_subscriptions::RpcSubscriptions,
    window_service::{should_retransmit_and_persist, WindowService},
};
use crossbeam_channel::Receiver;
use lru::LruCache;
use solana_client::rpc_response::SlotUpdate;
use solana_ledger::shred::{get_shred_slot_index_type, ShredFetchStats};
use solana_ledger::{
    blockstore::{Blockstore, CompletedSlotsReceiver},
    leader_schedule_cache::LeaderScheduleCache,
};
use solana_measure::measure::Measure;
use solana_metrics::inc_new_counter_error;
use solana_perf::packet::{Packet, Packets};
use solana_runtime::{bank::Bank, bank_forks::BankForks};
use solana_sdk::{
    clock::Slot, epoch_schedule::EpochSchedule, feature_set, pubkey::Pubkey, timing::timestamp,
};
use solana_streamer::streamer::PacketReceiver;
use std::{
    cmp,
    collections::hash_set::HashSet,
    collections::{BTreeMap, BTreeSet, HashMap},
    net::UdpSocket,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    sync::mpsc::channel,
    sync::mpsc::RecvTimeoutError,
    sync::Mutex,
    sync::{Arc, RwLock},
    thread::{self, Builder, JoinHandle},
    time::Duration,
};

const MAX_DUPLICATE_COUNT: usize = 2;
const DEFAULT_LRU_SIZE: usize = 10_000;

// Limit a given thread to consume about this many packets so that
// it doesn't pull up too much work.
const MAX_PACKET_BATCH_SIZE: usize = 100;

#[derive(Default)]
struct RetransmitStats {
    total_packets: AtomicU64,
    total_batches: AtomicU64,
    total_time: AtomicU64,
    epoch_fetch: AtomicU64,
    epoch_cache_update: AtomicU64,
    repair_total: AtomicU64,
    discard_total: AtomicU64,
    retransmit_total: AtomicU64,
    last_ts: AtomicU64,
    compute_turbine_peers_total: AtomicU64,
    packets_by_slot: Mutex<BTreeMap<Slot, usize>>,
    packets_by_source: Mutex<BTreeMap<String, usize>>,
}

#[allow(clippy::too_many_arguments)]
fn update_retransmit_stats(
    stats: &RetransmitStats,
    total_time: u64,
    total_packets: usize,
    retransmit_total: u64,
    discard_total: u64,
    repair_total: u64,
    compute_turbine_peers_total: u64,
    peers_len: usize,
    packets_by_slot: HashMap<Slot, usize>,
    packets_by_source: HashMap<String, usize>,
    epoch_fetch: u64,
    epoch_cach_update: u64,
) {
    stats.total_time.fetch_add(total_time, Ordering::Relaxed);
    stats
        .total_packets
        .fetch_add(total_packets as u64, Ordering::Relaxed);
    stats
        .retransmit_total
        .fetch_add(retransmit_total, Ordering::Relaxed);
    stats
        .repair_total
        .fetch_add(repair_total, Ordering::Relaxed);
    stats
        .discard_total
        .fetch_add(discard_total, Ordering::Relaxed);
    stats
        .compute_turbine_peers_total
        .fetch_add(compute_turbine_peers_total, Ordering::Relaxed);
    stats.total_batches.fetch_add(1, Ordering::Relaxed);
    stats.epoch_fetch.fetch_add(epoch_fetch, Ordering::Relaxed);
    stats
        .epoch_cache_update
        .fetch_add(epoch_cach_update, Ordering::Relaxed);
    {
        let mut stats_packets_by_slot = stats.packets_by_slot.lock().unwrap();
        for (slot, count) in packets_by_slot {
            *stats_packets_by_slot.entry(slot).or_insert(0) += count;
        }
    }
    {
        let mut stats_packets_by_source = stats.packets_by_source.lock().unwrap();
        for (source, count) in packets_by_source {
            *stats_packets_by_source.entry(source).or_insert(0) += count;
        }
    }

    let now = timestamp();
    let last = stats.last_ts.load(Ordering::Relaxed);
    #[allow(deprecated)]
    if now.saturating_sub(last) > 2000
        && stats.last_ts.compare_and_swap(last, now, Ordering::Relaxed) == last
    {
        datapoint_info!("retransmit-num_nodes", ("count", peers_len, i64));
        datapoint_info!(
            "retransmit-stage",
            (
                "total_time",
                stats.total_time.swap(0, Ordering::Relaxed) as i64,
                i64
            ),
            (
                "epoch_fetch",
                stats.epoch_fetch.swap(0, Ordering::Relaxed) as i64,
                i64
            ),
            (
                "epoch_cache_update",
                stats.epoch_cache_update.swap(0, Ordering::Relaxed) as i64,
                i64
            ),
            (
                "total_batches",
                stats.total_batches.swap(0, Ordering::Relaxed) as i64,
                i64
            ),
            (
                "total_packets",
                stats.total_packets.swap(0, Ordering::Relaxed) as i64,
                i64
            ),
            (
                "retransmit_total",
                stats.retransmit_total.swap(0, Ordering::Relaxed) as i64,
                i64
            ),
            (
                "compute_turbine",
                stats.compute_turbine_peers_total.swap(0, Ordering::Relaxed) as i64,
                i64
            ),
            (
                "repair_total",
                stats.repair_total.swap(0, Ordering::Relaxed) as i64,
                i64
            ),
            (
                "discard_total",
                stats.discard_total.swap(0, Ordering::Relaxed) as i64,
                i64
            ),
        );
        let mut packets_by_slot = stats.packets_by_slot.lock().unwrap();
        let old_packets_by_slot = std::mem::replace(&mut *packets_by_slot, BTreeMap::new());
        drop(packets_by_slot);

        for (slot, num_shreds) in old_packets_by_slot {
            datapoint_info!(
                "retransmit-slot-num-packets",
                ("slot", slot, i64),
                ("num_shreds", num_shreds, i64)
            );
        }
        let mut packets_by_source = stats.packets_by_source.lock().unwrap();
        let mut top = BTreeMap::new();
        let mut max = 0;
        for (source, num) in packets_by_source.iter() {
            if *num > max {
                top.insert(*num, source.clone());
                if top.len() > 5 {
                    let last = *top.iter().next().unwrap().0;
                    top.remove(&last);
                }
                max = *top.iter().next().unwrap().0;
            }
        }
        info!(
            "retransmit: top packets_by_source: {:?} len: {}",
            top,
            packets_by_source.len()
        );
        packets_by_source.clear();
    }
}

#[derive(Default)]
struct EpochStakesCache {
    peers: Vec<ContactInfo>,
    stakes_and_index: Vec<(u64, usize)>,
}

use crate::packet_hasher::PacketHasher;
// Map of shred (slot, index, is_data) => list of hash values seen for that key.
pub type ShredFilter = LruCache<(Slot, u32, bool), Vec<u64>>;

pub type ShredFilterAndHasher = (ShredFilter, PacketHasher);

// Returns None if shred is already received and should skip retransmit.
// Otherwise returns shred's slot and whether the shred is a data shred.
fn check_if_already_received(
    packet: &Packet,
    shreds_received: &Mutex<ShredFilterAndHasher>,
) -> Option<Slot> {
    let shred = get_shred_slot_index_type(packet, &mut ShredFetchStats::default())?;
    let mut shreds_received = shreds_received.lock().unwrap();
    let (cache, hasher) = shreds_received.deref_mut();
    match cache.get_mut(&shred) {
        Some(sent) if sent.len() >= MAX_DUPLICATE_COUNT => None,
        Some(sent) => {
            let hash = hasher.hash_packet(packet);
            if sent.contains(&hash) {
                None
            } else {
                sent.push(hash);
                Some(shred.0)
            }
        }
        None => {
            let hash = hasher.hash_packet(packet);
            cache.put(shred, vec![hash]);
            Some(shred.0)
        }
    }
}

// Returns true if this is the first time receiving a shred for `shred_slot`.
fn check_if_first_shred_received(
    shred_slot: Slot,
    first_shreds_received: &Mutex<BTreeSet<Slot>>,
    root_bank: &Bank,
) -> bool {
    if shred_slot <= root_bank.slot() {
        return false;
    }

    let mut first_shreds_received_locked = first_shreds_received.lock().unwrap();
    if !first_shreds_received_locked.contains(&shred_slot) {
        datapoint_info!("retransmit-first-shred", ("slot", shred_slot, i64));
        first_shreds_received_locked.insert(shred_slot);
        if first_shreds_received_locked.len() > 100 {
            let mut slots_before_root =
                first_shreds_received_locked.split_off(&(root_bank.slot() + 1));
            // `slots_before_root` now contains all slots <= root
            std::mem::swap(&mut slots_before_root, &mut first_shreds_received_locked);
        }
        true
    } else {
        false
    }
}

// Returns true if turbine retransmit peers patch (#14565) is enabled.
fn enable_turbine_retransmit_peers_patch(shred_slot: Slot, root_bank: &Bank) -> bool {
    let feature_slot = root_bank
        .feature_set
        .activated_slot(&feature_set::turbine_retransmit_peers_patch::id());
    match feature_slot {
        None => false,
        Some(feature_slot) => {
            let epoch_schedule = root_bank.epoch_schedule();
            let feature_epoch = epoch_schedule.get_epoch(feature_slot);
            let shred_epoch = epoch_schedule.get_epoch(shred_slot);
            feature_epoch < shred_epoch
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn retransmit(
    bank_forks: &RwLock<BankForks>,
    leader_schedule_cache: &LeaderScheduleCache,
    cluster_info: &ClusterInfo,
    r: &Mutex<PacketReceiver>,
    sock: &UdpSocket,
    id: u32,
    stats: &RetransmitStats,
    epoch_stakes_cache: &RwLock<EpochStakesCache>,
    last_peer_update: &AtomicU64,
    shreds_received: &Mutex<ShredFilterAndHasher>,
    max_slots: &MaxSlots,
    first_shreds_received: &Mutex<BTreeSet<Slot>>,
    rpc_subscriptions: &Option<Arc<RpcSubscriptions>>,
) -> Result<()> {
    let timer = Duration::new(1, 0);
    let r_lock = r.lock().unwrap();
    let packets = r_lock.recv_timeout(timer)?;
    let mut timer_start = Measure::start("retransmit");
    let mut total_packets = packets.packets.len();
    let mut packet_v = vec![packets];
    while let Ok(nq) = r_lock.try_recv() {
        total_packets += nq.packets.len();
        packet_v.push(nq);
        if total_packets >= MAX_PACKET_BATCH_SIZE {
            break;
        }
    }
    drop(r_lock);

    let mut epoch_fetch = Measure::start("retransmit_epoch_fetch");
    let (r_bank, root_bank) = {
        let bank_forks = bank_forks.read().unwrap();
        (bank_forks.working_bank(), bank_forks.root_bank())
    };
    let bank_epoch = r_bank.get_leader_schedule_epoch(r_bank.slot());
    epoch_fetch.stop();

    let mut epoch_cache_update = Measure::start("retransmit_epoch_cach_update");
    let now = timestamp();
    let last = last_peer_update.load(Ordering::Relaxed);
    #[allow(deprecated)]
    if now.saturating_sub(last) > 1000
        && last_peer_update.compare_and_swap(last, now, Ordering::Relaxed) == last
    {
        let epoch_staked_nodes = r_bank.epoch_staked_nodes(bank_epoch);
        let (peers, stakes_and_index) =
            cluster_info.sorted_retransmit_peers_and_stakes(epoch_staked_nodes.as_ref());
        {
            let mut epoch_stakes_cache = epoch_stakes_cache.write().unwrap();
            epoch_stakes_cache.peers = peers;
            epoch_stakes_cache.stakes_and_index = stakes_and_index;
        }
        {
            let mut sr = shreds_received.lock().unwrap();
            sr.0.clear();
            sr.1.reset();
        }
    }
    let r_epoch_stakes_cache = epoch_stakes_cache.read().unwrap();
    let mut peers_len = 0;
    epoch_cache_update.stop();

    let my_id = cluster_info.id();
    let mut discard_total = 0;
    let mut repair_total = 0;
    let mut retransmit_total = 0;
    let mut compute_turbine_peers_total = 0;
    let mut packets_by_slot: HashMap<Slot, usize> = HashMap::new();
    let mut packets_by_source: HashMap<String, usize> = HashMap::new();
    let mut max_slot = 0;
    for mut packets in packet_v {
        for packet in packets.packets.iter_mut() {
            // skip discarded packets and repair packets
            if packet.meta.discard {
                total_packets -= 1;
                discard_total += 1;
                continue;
            }
            if packet.meta.repair {
                total_packets -= 1;
                repair_total += 1;
                continue;
            }
            let shred_slot = match check_if_already_received(packet, shreds_received) {
                Some(slot) => slot,
                None => continue,
            };
            max_slot = max_slot.max(shred_slot);

            if let Some(rpc_subscriptions) = rpc_subscriptions {
                if check_if_first_shred_received(shred_slot, first_shreds_received, &root_bank) {
                    rpc_subscriptions.notify_slot_update(SlotUpdate::FirstShredReceived {
                        slot: shred_slot,
                        timestamp: timestamp(),
                    });
                }
            }

            let mut compute_turbine_peers = Measure::start("turbine_start");
            let (my_index, mut shuffled_stakes_and_index) = ClusterInfo::shuffle_peers_and_index(
                &my_id,
                &r_epoch_stakes_cache.peers,
                &r_epoch_stakes_cache.stakes_and_index,
                packet.meta.seed,
            );
            peers_len = cmp::max(peers_len, shuffled_stakes_and_index.len());
            // Until the patch is activated, do the old buggy thing.
            if !enable_turbine_retransmit_peers_patch(shred_slot, root_bank.deref()) {
                shuffled_stakes_and_index.remove(my_index);
            }
            // split off the indexes, we don't need the stakes anymore
            let indexes: Vec<_> = shuffled_stakes_and_index
                .into_iter()
                .map(|(_, index)| index)
                .collect();

            let (neighbors, children) =
                compute_retransmit_peers(DATA_PLANE_FANOUT, my_index, &indexes);
            let neighbors: Vec<_> = neighbors
                .into_iter()
                .filter_map(|index| {
                    let peer = &r_epoch_stakes_cache.peers[index];
                    if peer.id == my_id {
                        None
                    } else {
                        Some(peer)
                    }
                })
                .collect();
            let children: Vec<_> = children
                .into_iter()
                .map(|index| &r_epoch_stakes_cache.peers[index])
                .collect();
            compute_turbine_peers.stop();
            compute_turbine_peers_total += compute_turbine_peers.as_us();

            *packets_by_slot.entry(packet.meta.slot).or_insert(0) += 1;
            *packets_by_source
                .entry(packet.meta.addr().to_string())
                .or_insert(0) += 1;

            let leader =
                leader_schedule_cache.slot_leader_at(packet.meta.slot, Some(r_bank.as_ref()));
            let mut retransmit_time = Measure::start("retransmit_to");
            if !packet.meta.forward {
                ClusterInfo::retransmit_to(&neighbors, packet, leader, sock, true)?;
                ClusterInfo::retransmit_to(&children, packet, leader, sock, false)?;
            } else {
                ClusterInfo::retransmit_to(&children, packet, leader, sock, true)?;
            }
            retransmit_time.stop();
            retransmit_total += retransmit_time.as_us();
        }
    }
    max_slots.retransmit.fetch_max(max_slot, Ordering::Relaxed);
    timer_start.stop();
    debug!(
        "retransmitted {} packets in {}ms retransmit_time: {}ms id: {}",
        total_packets,
        timer_start.as_ms(),
        retransmit_total,
        id,
    );
    update_retransmit_stats(
        stats,
        timer_start.as_us(),
        total_packets,
        retransmit_total,
        discard_total,
        repair_total,
        compute_turbine_peers_total,
        peers_len,
        packets_by_slot,
        packets_by_source,
        epoch_fetch.as_us(),
        epoch_cache_update.as_us(),
    );

    Ok(())
}

/// Service to retransmit messages from the leader or layer 1 to relevant peer nodes.
/// See `cluster_info` for network layer definitions.
/// # Arguments
/// * `sockets` - Sockets to read from.
/// * `bank_forks` - The BankForks structure
/// * `leader_schedule_cache` - The leader schedule to verify shreds
/// * `cluster_info` - This structure needs to be updated and populated by the bank and via gossip.
/// * `r` - Receive channel for shreds to be retransmitted to all the layer 1 nodes.
pub fn retransmitter(
    sockets: Arc<Vec<UdpSocket>>,
    bank_forks: Arc<RwLock<BankForks>>,
    leader_schedule_cache: &Arc<LeaderScheduleCache>,
    cluster_info: Arc<ClusterInfo>,
    r: Arc<Mutex<PacketReceiver>>,
    max_slots: &Arc<MaxSlots>,
    rpc_subscriptions: Option<Arc<RpcSubscriptions>>,
) -> Vec<JoinHandle<()>> {
    let stats = Arc::new(RetransmitStats::default());
    let shreds_received = Arc::new(Mutex::new((
        LruCache::new(DEFAULT_LRU_SIZE),
        PacketHasher::default(),
    )));
    let first_shreds_received = Arc::new(Mutex::new(BTreeSet::new()));
    (0..sockets.len())
        .map(|s| {
            let sockets = sockets.clone();
            let bank_forks = bank_forks.clone();
            let leader_schedule_cache = leader_schedule_cache.clone();
            let r = r.clone();
            let cluster_info = cluster_info.clone();
            let stats = stats.clone();
            let epoch_stakes_cache = Arc::new(RwLock::new(EpochStakesCache::default()));
            let last_peer_update = Arc::new(AtomicU64::new(0));
            let shreds_received = shreds_received.clone();
            let max_slots = max_slots.clone();
            let first_shreds_received = first_shreds_received.clone();
            let rpc_subscriptions = rpc_subscriptions.clone();

            Builder::new()
                .name("solana-retransmitter".to_string())
                .spawn(move || {
                    trace!("retransmitter started");
                    loop {
                        if let Err(e) = retransmit(
                            &bank_forks,
                            &leader_schedule_cache,
                            &cluster_info,
                            &r,
                            &sockets[s],
                            s as u32,
                            &stats,
                            &epoch_stakes_cache,
                            &last_peer_update,
                            &shreds_received,
                            &max_slots,
                            &first_shreds_received,
                            &rpc_subscriptions,
                        ) {
                            match e {
                                Error::RecvTimeoutError(RecvTimeoutError::Disconnected) => break,
                                Error::RecvTimeoutError(RecvTimeoutError::Timeout) => (),
                                _ => {
                                    inc_new_counter_error!("streamer-retransmit-error", 1, 1);
                                }
                            }
                        }
                    }
                    trace!("exiting retransmitter");
                })
                .unwrap()
        })
        .collect()
}

pub struct RetransmitStage {
    thread_hdls: Vec<JoinHandle<()>>,
    window_service: WindowService,
    cluster_slots_service: ClusterSlotsService,
}

impl RetransmitStage {
    #[allow(clippy::new_ret_no_self)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bank_forks: Arc<RwLock<BankForks>>,
        leader_schedule_cache: &Arc<LeaderScheduleCache>,
        blockstore: Arc<Blockstore>,
        cluster_info: &Arc<ClusterInfo>,
        retransmit_sockets: Arc<Vec<UdpSocket>>,
        repair_socket: Arc<UdpSocket>,
        verified_receiver: Receiver<Vec<Packets>>,
        exit: &Arc<AtomicBool>,
        completed_slots_receivers: [CompletedSlotsReceiver; 2],
        epoch_schedule: EpochSchedule,
        cfg: Option<Arc<AtomicBool>>,
        shred_version: u16,
        cluster_slots: Arc<ClusterSlots>,
        duplicate_slots_reset_sender: DuplicateSlotsResetSender,
        verified_vote_receiver: VerifiedVoteReceiver,
        repair_validators: Option<HashSet<Pubkey>>,
        completed_data_sets_sender: CompletedDataSetsSender,
        max_slots: &Arc<MaxSlots>,
        rpc_subscriptions: Option<Arc<RpcSubscriptions>>,
    ) -> Self {
        let (retransmit_sender, retransmit_receiver) = channel();

        let retransmit_receiver = Arc::new(Mutex::new(retransmit_receiver));
        let t_retransmit = retransmitter(
            retransmit_sockets,
            bank_forks.clone(),
            leader_schedule_cache,
            cluster_info.clone(),
            retransmit_receiver,
            max_slots,
            rpc_subscriptions.clone(),
        );

        let [rpc_completed_slots_receiver, cluster_completed_slots_receiver] =
            completed_slots_receivers;
        let rpc_completed_slots_hdl =
            RpcCompletedSlotsService::spawn(rpc_completed_slots_receiver, rpc_subscriptions);
        let cluster_slots_service = ClusterSlotsService::new(
            blockstore.clone(),
            cluster_slots.clone(),
            bank_forks.clone(),
            cluster_info.clone(),
            cluster_completed_slots_receiver,
            exit.clone(),
        );

        let leader_schedule_cache_clone = leader_schedule_cache.clone();
        let repair_info = RepairInfo {
            bank_forks,
            epoch_schedule,
            duplicate_slots_reset_sender,
            repair_validators,
        };
        let window_service = WindowService::new(
            blockstore,
            cluster_info.clone(),
            verified_receiver,
            retransmit_sender,
            repair_socket,
            exit,
            repair_info,
            leader_schedule_cache,
            move |id, shred, working_bank, last_root| {
                let is_connected = cfg
                    .as_ref()
                    .map(|x| x.load(Ordering::Relaxed))
                    .unwrap_or(true);
                let rv = should_retransmit_and_persist(
                    shred,
                    working_bank,
                    &leader_schedule_cache_clone,
                    id,
                    last_root,
                    shred_version,
                );
                rv && is_connected
            },
            cluster_slots,
            verified_vote_receiver,
            completed_data_sets_sender,
        );

        let mut thread_hdls = t_retransmit;
        if let Some(thread_hdl) = rpc_completed_slots_hdl {
            thread_hdls.push(thread_hdl);
        }

        Self {
            thread_hdls,
            window_service,
            cluster_slots_service,
        }
    }

    pub fn join(self) -> thread::Result<()> {
        for thread_hdl in self.thread_hdls {
            thread_hdl.join()?;
        }
        self.window_service.join()?;
        self.cluster_slots_service.join()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contact_info::ContactInfo;
    use solana_ledger::blockstore_processor::{process_blockstore, ProcessOptions};
    use solana_ledger::create_new_tmp_ledger;
    use solana_ledger::genesis_utils::{create_genesis_config, GenesisConfigInfo};
    use solana_ledger::shred::Shred;
    use solana_net_utils::find_available_port_in_range;
    use solana_perf::packet::{Packet, Packets};
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_skip_repair() {
        solana_logger::setup();
        let GenesisConfigInfo { genesis_config, .. } = create_genesis_config(123);
        let (ledger_path, _blockhash) = create_new_tmp_ledger!(&genesis_config);
        let blockstore = Blockstore::open(&ledger_path).unwrap();
        let opts = ProcessOptions {
            full_leader_cache: true,
            ..ProcessOptions::default()
        };
        let (bank_forks, cached_leader_schedule) =
            process_blockstore(&genesis_config, &blockstore, Vec::new(), opts).unwrap();
        let leader_schedule_cache = Arc::new(cached_leader_schedule);
        let bank_forks = Arc::new(RwLock::new(bank_forks));

        let mut me = ContactInfo::new_localhost(&solana_sdk::pubkey::new_rand(), 0);
        let ip_addr = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));
        let port = find_available_port_in_range(ip_addr, (8000, 10000)).unwrap();
        let me_retransmit = UdpSocket::bind(format!("127.0.0.1:{}", port)).unwrap();
        // need to make sure tvu and tpu are valid addresses
        me.tvu_forwards = me_retransmit.local_addr().unwrap();
        let port = find_available_port_in_range(ip_addr, (8000, 10000)).unwrap();
        me.tvu = UdpSocket::bind(format!("127.0.0.1:{}", port))
            .unwrap()
            .local_addr()
            .unwrap();

        let other = ContactInfo::new_localhost(&solana_sdk::pubkey::new_rand(), 0);
        let cluster_info = ClusterInfo::new_with_invalid_keypair(other);
        cluster_info.insert_info(me);

        let retransmit_socket = Arc::new(vec![UdpSocket::bind("0.0.0.0:0").unwrap()]);
        let cluster_info = Arc::new(cluster_info);

        let (retransmit_sender, retransmit_receiver) = channel();
        let t_retransmit = retransmitter(
            retransmit_socket,
            bank_forks,
            &leader_schedule_cache,
            cluster_info,
            Arc::new(Mutex::new(retransmit_receiver)),
            &Arc::new(MaxSlots::default()),
            None,
        );
        let _thread_hdls = vec![t_retransmit];

        let mut shred = Shred::new_from_data(0, 0, 0, None, true, true, 0, 0x20, 0);
        let mut packet = Packet::default();
        shred.copy_to_packet(&mut packet);

        let packets = Packets::new(vec![packet.clone()]);
        // it should send this over the sockets.
        retransmit_sender.send(packets).unwrap();
        let mut packets = Packets::new(vec![]);
        solana_streamer::packet::recv_from(&mut packets, &me_retransmit, 1).unwrap();
        assert_eq!(packets.packets.len(), 1);
        assert_eq!(packets.packets[0].meta.repair, false);

        let mut repair = packet.clone();
        repair.meta.repair = true;

        shred.set_slot(1);
        shred.copy_to_packet(&mut packet);
        // send 1 repair and 1 "regular" packet so that we don't block forever on the recv_from
        let packets = Packets::new(vec![repair, packet]);
        retransmit_sender.send(packets).unwrap();
        let mut packets = Packets::new(vec![]);
        solana_streamer::packet::recv_from(&mut packets, &me_retransmit, 1).unwrap();
        assert_eq!(packets.packets.len(), 1);
        assert_eq!(packets.packets[0].meta.repair, false);
    }

    #[test]
    fn test_already_received() {
        let mut packet = Packet::default();
        let slot = 1;
        let index = 5;
        let version = 0x40;
        let shred = Shred::new_from_data(slot, index, 0, None, true, true, 0, version, 0);
        shred.copy_to_packet(&mut packet);
        let shreds_received = Arc::new(Mutex::new((LruCache::new(100), PacketHasher::default())));
        // unique shred for (1, 5) should pass
        assert_eq!(
            check_if_already_received(&packet, &shreds_received),
            Some(slot)
        );
        // duplicate shred for (1, 5) blocked
        assert_eq!(check_if_already_received(&packet, &shreds_received), None);

        let shred = Shred::new_from_data(slot, index, 2, None, true, true, 0, version, 0);
        shred.copy_to_packet(&mut packet);
        // first duplicate shred for (1, 5) passed
        assert_eq!(
            check_if_already_received(&packet, &shreds_received),
            Some(slot)
        );
        // then blocked
        assert_eq!(check_if_already_received(&packet, &shreds_received), None);

        let shred = Shred::new_from_data(slot, index, 8, None, true, true, 0, version, 0);
        shred.copy_to_packet(&mut packet);
        // 2nd duplicate shred for (1, 5) blocked
        assert_eq!(check_if_already_received(&packet, &shreds_received), None);
        assert_eq!(check_if_already_received(&packet, &shreds_received), None);

        let shred = Shred::new_empty_coding(slot, index, 0, 1, 1, 0, version);
        shred.copy_to_packet(&mut packet);
        // Coding at (1, 5) passes
        assert_eq!(
            check_if_already_received(&packet, &shreds_received),
            Some(slot)
        );
        // then blocked
        assert_eq!(check_if_already_received(&packet, &shreds_received), None);

        let shred = Shred::new_empty_coding(slot, index, 2, 1, 1, 0, version);
        shred.copy_to_packet(&mut packet);
        // 2nd unique coding at (1, 5) passes
        assert_eq!(
            check_if_already_received(&packet, &shreds_received),
            Some(slot)
        );
        // same again is blocked
        assert_eq!(check_if_already_received(&packet, &shreds_received), None);

        let shred = Shred::new_empty_coding(slot, index, 3, 1, 1, 0, version);
        shred.copy_to_packet(&mut packet);
        // Another unique coding at (1, 5) always blocked
        assert_eq!(check_if_already_received(&packet, &shreds_received), None);
        assert_eq!(check_if_already_received(&packet, &shreds_received), None);
    }
}
