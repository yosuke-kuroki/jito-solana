//! The `tvu` module implements the Transaction Validation Unit, a
//! 3-stage transaction validation pipeline in software.
//!
//! ```text
//!                  .------------------------------------------.
//!                  |  TVU                                     |
//!                  |                                          |
//!                  |                                          |  .------------.
//!                  |                   .------------------------>| Validators |
//!                  |  .-------.        |                      |  `------------`
//! .--------.       |  |       |   .----+---.   .-----------.  |
//! | Leader |--------->| Blob  |   | Window |   | Replicate |  |
//! `--------`       |  | Fetch |-->| Stage  |-->|  Stage    |  |
//! .------------.   |  | Stage |   |        |   |           |  |
//! | Validators |----->|       |   `--------`   `----+------`  |
//! `------------`   |  `-------`                     |         |
//!                  |                                |         |
//!                  |                                |         |
//!                  |                                |         |
//!                  `--------------------------------|---------`
//!                                                   |
//!                                                   v
//!                                                .------.
//!                                                | Bank |
//!                                                `------`
//! ```
//!
//! 1. Fetch Stage
//! - Incoming blobs are picked up from the replicate socket and repair socket.
//! 2. Window Stage
//! - Blobs are windowed until a contiguous chunk is available.  This stage also repairs and
//! retransmits blobs that are in the queue.
//! 3. Replicate Stage
//! - Transactions in blobs are processed and applied to the bank.
//! - TODO We need to verify the signatures in the blobs.

use bank::Bank;
use blob_fetch_stage::BlobFetchStage;
use crdt::Crdt;
use packet;
use replicate_stage::ReplicateStage;
use std::net::UdpSocket;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use streamer;
use window_stage::WindowStage;

pub struct Tvu {
    pub thread_hdls: Vec<JoinHandle<()>>,
}

impl Tvu {
    /// This service receives messages from a leader in the network and processes the transactions
    /// on the bank state.
    /// # Arguments
    /// * `bank` - The bank state.
    /// * `crdt` - The crdt state.
    /// * `window` - The window state.
    /// * `replicate_socket` - my replicate socket
    /// * `repair_socket` - my repair socket
    /// * `retransmit_socket` - my retransmit socket
    /// * `exit` - The exit signal.
    pub fn new(
        bank: Arc<Bank>,
        crdt: Arc<RwLock<Crdt>>,
        window: streamer::Window,
        replicate_socket: UdpSocket,
        repair_socket: UdpSocket,
        retransmit_socket: UdpSocket,
        exit: Arc<AtomicBool>,
    ) -> Self {
        let blob_recycler = packet::BlobRecycler::default();
        let fetch_stage = BlobFetchStage::new_multi_socket(
            vec![replicate_socket, repair_socket],
            exit.clone(),
            blob_recycler.clone(),
        );
        //TODO
        //the packets coming out of blob_receiver need to be sent to the GPU and verified
        //then sent to the window, which does the erasure coding reconstruction
        let window_stage = WindowStage::new(
            crdt,
            window,
            retransmit_socket,
            exit.clone(),
            blob_recycler.clone(),
            fetch_stage.blob_receiver,
        );

        let replicate_stage =
            ReplicateStage::new(bank, exit, window_stage.blob_receiver, blob_recycler);

        let mut threads = vec![replicate_stage.thread_hdl];
        threads.extend(fetch_stage.thread_hdls.into_iter());
        threads.extend(window_stage.thread_hdls.into_iter());
        Tvu {
            thread_hdls: threads,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use bank::Bank;
    use bincode::serialize;
    use crdt::{Crdt, TestNode};
    use entry::Entry;
    use hash::{hash, Hash};
    use logger;
    use mint::Mint;
    use ncp::Ncp;
    use packet::BlobRecycler;
    use result::Result;
    use signature::{KeyPair, KeyPairUtil};
    use std::collections::VecDeque;
    use std::net::UdpSocket;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::{Arc, RwLock};
    use std::time::Duration;
    use streamer;
    use transaction::Transaction;
    use tvu::Tvu;

    fn new_ncp(
        crdt: Arc<RwLock<Crdt>>,
        listen: UdpSocket,
        exit: Arc<AtomicBool>,
    ) -> Result<(Ncp, streamer::Window)> {
        let window = streamer::default_window();
        let send_sock = UdpSocket::bind("0.0.0.0:0").expect("bind 0");
        let ncp = Ncp::new(crdt, window.clone(), listen, send_sock, exit)?;
        Ok((ncp, window))
    }
    /// Test that message sent from leader to target1 and replicated to target2
    #[test]
    fn test_replicate() {
        logger::setup();
        let leader = TestNode::new();
        let target1 = TestNode::new();
        let target2 = TestNode::new();
        let exit = Arc::new(AtomicBool::new(false));

        //start crdt_leader
        let mut crdt_l = Crdt::new(leader.data.clone());
        crdt_l.set_leader(leader.data.id);

        let cref_l = Arc::new(RwLock::new(crdt_l));
        let dr_l = new_ncp(cref_l, leader.sockets.gossip, exit.clone()).unwrap();

        //start crdt2
        let mut crdt2 = Crdt::new(target2.data.clone());
        crdt2.insert(&leader.data);
        crdt2.set_leader(leader.data.id);
        let leader_id = leader.data.id;
        let cref2 = Arc::new(RwLock::new(crdt2));
        let dr_2 = new_ncp(cref2, target2.sockets.gossip, exit.clone()).unwrap();

        // setup some blob services to send blobs into the socket
        // to simulate the source peer and get blobs out of the socket to
        // simulate target peer
        let recv_recycler = BlobRecycler::default();
        let resp_recycler = BlobRecycler::default();
        let (s_reader, r_reader) = channel();
        let t_receiver = streamer::blob_receiver(
            exit.clone(),
            recv_recycler.clone(),
            target2.sockets.replicate,
            s_reader,
        ).unwrap();

        // simulate leader sending messages
        let (s_responder, r_responder) = channel();
        let t_responder = streamer::responder(
            leader.sockets.requests,
            exit.clone(),
            resp_recycler.clone(),
            r_responder,
        );

        let starting_balance = 10_000;
        let mint = Mint::new(starting_balance);
        let replicate_addr = target1.data.replicate_addr;
        let bank = Arc::new(Bank::new(&mint));

        //start crdt1
        let mut crdt1 = Crdt::new(target1.data.clone());
        crdt1.insert(&leader.data);
        crdt1.set_leader(leader.data.id);
        let cref1 = Arc::new(RwLock::new(crdt1));
        let dr_1 = new_ncp(cref1.clone(), target1.sockets.gossip, exit.clone()).unwrap();

        let tvu = Tvu::new(
            bank.clone(),
            cref1,
            dr_1.1,
            target1.sockets.replicate,
            target1.sockets.repair,
            target1.sockets.retransmit,
            exit.clone(),
        );

        let mut alice_ref_balance = starting_balance;
        let mut msgs = VecDeque::new();
        let mut cur_hash = Hash::default();
        let num_blobs = 10;
        let transfer_amount = 501;
        let bob_keypair = KeyPair::new();
        for i in 0..num_blobs {
            let b = resp_recycler.allocate();
            let b_ = b.clone();
            let mut w = b.write().unwrap();
            w.set_index(i).unwrap();
            w.set_id(leader_id).unwrap();

            let entry0 = Entry::new(&cur_hash, i, vec![]);
            bank.register_entry_id(&cur_hash);
            cur_hash = hash(&cur_hash);

            let tx0 = Transaction::new(
                &mint.keypair(),
                bob_keypair.pubkey(),
                transfer_amount,
                cur_hash,
            );
            bank.register_entry_id(&cur_hash);
            cur_hash = hash(&cur_hash);
            let entry1 = Entry::new(&cur_hash, i + num_blobs, vec![tx0]);
            bank.register_entry_id(&cur_hash);
            cur_hash = hash(&cur_hash);

            alice_ref_balance -= transfer_amount;

            let serialized_entry = serialize(&vec![entry0, entry1]).unwrap();

            w.data_mut()[..serialized_entry.len()].copy_from_slice(&serialized_entry);
            w.set_size(serialized_entry.len());
            w.meta.set_addr(&replicate_addr);
            drop(w);
            msgs.push_back(b_);
        }

        // send the blobs into the socket
        s_responder.send(msgs).expect("send");

        // receive retransmitted messages
        let timer = Duration::new(1, 0);
        let mut msgs: Vec<_> = Vec::new();
        while let Ok(msg) = r_reader.recv_timeout(timer) {
            trace!("msg: {:?}", msg);
            msgs.push(msg);
        }

        let alice_balance = bank.get_balance(&mint.keypair().pubkey()).unwrap();
        assert_eq!(alice_balance, alice_ref_balance);

        let bob_balance = bank.get_balance(&bob_keypair.pubkey()).unwrap();
        assert_eq!(bob_balance, starting_balance - alice_ref_balance);

        exit.store(true, Ordering::Relaxed);
        for t in tvu.thread_hdls {
            t.join().expect("join");
        }
        for t in dr_l.0.thread_hdls {
            t.join().expect("join");
        }
        for t in dr_2.0.thread_hdls {
            t.join().expect("join");
        }
        for t in dr_1.0.thread_hdls {
            t.join().expect("join");
        }
        t_receiver.join().expect("join");
        t_responder.join().expect("join");
    }
}
