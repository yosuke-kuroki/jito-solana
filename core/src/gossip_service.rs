//! The `gossip_service` module implements the network control plane.

use crate::bank_forks::BankForks;
use crate::blocktree::Blocktree;
use crate::cluster_info::ClusterInfo;
use crate::contact_info::ContactInfo;
use crate::service::Service;
use crate::streamer;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, KeypairUtil};
use std::net::SocketAddr;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, RwLock};
use std::thread::sleep;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct GossipService {
    thread_hdls: Vec<JoinHandle<()>>,
}

impl GossipService {
    pub fn new(
        cluster_info: &Arc<RwLock<ClusterInfo>>,
        blocktree: Option<Arc<Blocktree>>,
        bank_forks: Option<Arc<RwLock<BankForks>>>,
        gossip_socket: UdpSocket,
        exit: &Arc<AtomicBool>,
    ) -> Self {
        let (request_sender, request_receiver) = channel();
        let gossip_socket = Arc::new(gossip_socket);
        trace!(
            "GossipService: id: {}, listening on: {:?}",
            &cluster_info.read().unwrap().my_data().id,
            gossip_socket.local_addr().unwrap()
        );
        let t_receiver = streamer::blob_receiver(gossip_socket.clone(), &exit, request_sender);
        let (response_sender, response_receiver) = channel();
        let t_responder = streamer::responder("gossip", gossip_socket, response_receiver);
        let t_listen = ClusterInfo::listen(
            cluster_info.clone(),
            blocktree,
            request_receiver,
            response_sender.clone(),
            exit,
        );
        let t_gossip = ClusterInfo::gossip(cluster_info.clone(), bank_forks, response_sender, exit);
        let thread_hdls = vec![t_receiver, t_responder, t_listen, t_gossip];
        Self { thread_hdls }
    }
}

pub fn discover_nodes(
    gossip_addr: &SocketAddr,
    num_nodes: usize,
) -> std::io::Result<Vec<ContactInfo>> {
    discover(gossip_addr, Some(num_nodes), Some(30), None)
}

pub fn discover(
    gossip_addr: &SocketAddr,
    num_nodes: Option<usize>,
    timeout: Option<u64>,
    find_node: Option<Pubkey>,
) -> std::io::Result<Vec<ContactInfo>> {
    let exit = Arc::new(AtomicBool::new(false));
    let (gossip_service, spy_ref) = make_spy_node(gossip_addr, &exit);
    let id = spy_ref.read().unwrap().keypair.pubkey();
    trace!(
        "discover: spy_node {} looking for at least {:?} nodes",
        id,
        num_nodes
    );

    let (met_criteria, secs, tvu_peers) = spy(spy_ref.clone(), num_nodes, timeout, find_node);

    exit.store(true, Ordering::Relaxed);
    gossip_service.join().unwrap();

    if met_criteria {
        info!(
            "discover success in {}s...\n{}",
            secs,
            spy_ref.read().unwrap().contact_info_trace()
        );
        return Ok(tvu_peers);
    }

    if !tvu_peers.is_empty() {
        info!(
            "discover failed to match criteria by timeout...\n{}",
            spy_ref.read().unwrap().contact_info_trace()
        );
        return Ok(tvu_peers);
    }

    info!(
        "discover failed...\n{}",
        spy_ref.read().unwrap().contact_info_trace()
    );
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "Failed to converge",
    ))
}

fn spy(
    spy_ref: Arc<RwLock<ClusterInfo>>,
    num_nodes: Option<usize>,
    timeout: Option<u64>,
    find_node: Option<Pubkey>,
) -> (bool, u64, Vec<ContactInfo>) {
    let now = Instant::now();
    let mut met_criteria = false;
    let mut tvu_peers: Vec<ContactInfo> = Vec::new();
    let mut i = 0;
    loop {
        if let Some(secs) = timeout {
            if now.elapsed() >= Duration::from_secs(secs) {
                break;
            }
        }
        tvu_peers = spy_ref.read().unwrap().tvu_peers();
        if let Some(num) = num_nodes {
            if tvu_peers.len() >= num {
                if let Some(pubkey) = find_node {
                    if tvu_peers.iter().any(|x| x.id == pubkey) {
                        met_criteria = true;
                        break;
                    }
                } else {
                    met_criteria = true;
                    break;
                }
            }
        }
        if let Some(pubkey) = find_node {
            if num_nodes.is_none() && tvu_peers.iter().any(|x| x.id == pubkey) {
                met_criteria = true;
                break;
            }
        }
        if i % 20 == 0 {
            info!(
                "discovering...\n{}",
                spy_ref.read().unwrap().contact_info_trace()
            );
        }
        sleep(Duration::from_millis(
            crate::cluster_info::GOSSIP_SLEEP_MILLIS,
        ));
        i += 1;
    }
    (met_criteria, now.elapsed().as_secs(), tvu_peers)
}

fn make_spy_node(
    gossip_addr: &SocketAddr,
    exit: &Arc<AtomicBool>,
) -> (GossipService, Arc<RwLock<ClusterInfo>>) {
    let keypair = Arc::new(Keypair::new());
    let (node, gossip_socket) = ClusterInfo::spy_node(&keypair.pubkey());
    let mut cluster_info = ClusterInfo::new(node, keypair);
    cluster_info.set_entrypoint(ContactInfo::new_gossip_entry_point(gossip_addr));
    let cluster_info = Arc::new(RwLock::new(cluster_info));
    let gossip_service =
        GossipService::new(&cluster_info.clone(), None, None, gossip_socket, &exit);
    (gossip_service, cluster_info)
}

impl Service for GossipService {
    type JoinReturnType = ();

    fn join(self) -> thread::Result<()> {
        for thread_hdl in self.thread_hdls {
            thread_hdl.join()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster_info::{ClusterInfo, Node};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, RwLock};

    #[test]
    #[ignore]
    // test that stage will exit when flag is set
    fn test_exit() {
        let exit = Arc::new(AtomicBool::new(false));
        let tn = Node::new_localhost();
        let cluster_info = ClusterInfo::new_with_invalid_keypair(tn.info.clone());
        let c = Arc::new(RwLock::new(cluster_info));
        let d = GossipService::new(&c, None, None, tn.sockets.gossip, &exit);
        exit.store(true, Ordering::Relaxed);
        d.join().unwrap();
    }

    #[test]
    fn test_gossip_services_spy() {
        let keypair = Keypair::new();
        let peer0 = Pubkey::new_rand();
        let peer1 = Pubkey::new_rand();
        let contact_info = ContactInfo::new_localhost(&keypair.pubkey(), 0);
        let peer0_info = ContactInfo::new_localhost(&peer0, 0);
        let peer1_info = ContactInfo::new_localhost(&peer1, 0);
        let mut cluster_info = ClusterInfo::new(contact_info.clone(), Arc::new(keypair));
        cluster_info.insert_info(peer0_info);
        cluster_info.insert_info(peer1_info);

        let spy_ref = Arc::new(RwLock::new(cluster_info));

        let (met_criteria, secs, tvu_peers) = spy(spy_ref.clone(), None, Some(1), None);
        assert_eq!(met_criteria, false);
        assert_eq!(secs, 1);
        assert_eq!(tvu_peers, spy_ref.read().unwrap().tvu_peers());

        // Find num_nodes
        let (met_criteria, _, _) = spy(spy_ref.clone(), Some(1), None, None);
        assert_eq!(met_criteria, true);
        let (met_criteria, _, _) = spy(spy_ref.clone(), Some(2), None, None);
        assert_eq!(met_criteria, true);

        // Find specific node by pubkey
        let (met_criteria, _, _) = spy(spy_ref.clone(), None, None, Some(peer0));
        assert_eq!(met_criteria, true);
        let (met_criteria, _, _) = spy(spy_ref.clone(), None, Some(0), Some(Pubkey::new_rand()));
        assert_eq!(met_criteria, false);

        // Find num_nodes *and* specific node by pubkey
        let (met_criteria, _, _) = spy(spy_ref.clone(), Some(1), None, Some(peer0));
        assert_eq!(met_criteria, true);
        let (met_criteria, _, _) = spy(spy_ref.clone(), Some(3), Some(0), Some(peer0));
        assert_eq!(met_criteria, false);
        let (met_criteria, _, _) = spy(spy_ref.clone(), Some(1), Some(0), Some(Pubkey::new_rand()));
        assert_eq!(met_criteria, false);
    }
}
