//! The `recvmmsg` module provides recvmmsg() API implementation

use crate::packet::Packet;
pub use solana_perf::packet::NUM_RCVMMSGS;
use std::cmp;
use std::io;
use std::net::UdpSocket;

#[cfg(not(target_os = "linux"))]
pub fn recv_mmsg(socket: &UdpSocket, packets: &mut [Packet]) -> io::Result<(usize, usize)> {
    let mut i = 0;
    let count = cmp::min(NUM_RCVMMSGS, packets.len());
    let mut total_size = 0;
    for p in packets.iter_mut().take(count) {
        p.meta.size = 0;
        match socket.recv_from(&mut p.data) {
            Err(_) if i > 0 => {
                break;
            }
            Err(e) => {
                return Err(e);
            }
            Ok((nrecv, from)) => {
                total_size += nrecv;
                p.meta.size = nrecv;
                p.meta.set_addr(&from);
                if i == 0 {
                    socket.set_nonblocking(true)?;
                }
            }
        }
        i += 1;
    }
    Ok((total_size, i))
}

#[cfg(target_os = "linux")]
#[allow(clippy::uninit_assumed_init)]
pub fn recv_mmsg(sock: &UdpSocket, packets: &mut [Packet]) -> io::Result<(usize, usize)> {
    use libc::{
        c_void, iovec, mmsghdr, recvmmsg, sa_family_t, sockaddr_in, sockaddr_in6, sockaddr_storage,
        socklen_t, timespec, AF_INET, AF_INET6, MSG_WAITFORONE,
    };
    use nix::sys::socket::InetAddr;
    use std::mem;
    use std::os::unix::io::AsRawFd;

    const SOCKADDR_STORAGE_SIZE: socklen_t = mem::size_of::<sockaddr_storage>() as socklen_t;
    const SOCKADDR_IN_SIZE: socklen_t = mem::size_of::<sockaddr_in>() as socklen_t;
    const SOCKADDR_IN6_SIZE: socklen_t = mem::size_of::<sockaddr_in6>() as socklen_t;

    let mut hdrs: [mmsghdr; NUM_RCVMMSGS] = unsafe { mem::zeroed() };
    let mut iovs: [iovec; NUM_RCVMMSGS] = unsafe { mem::MaybeUninit::uninit().assume_init() };
    let mut addr: [sockaddr_storage; NUM_RCVMMSGS] = unsafe { mem::zeroed() };

    let sock_fd = sock.as_raw_fd();
    let count = cmp::min(iovs.len(), packets.len());

    for i in 0..count {
        iovs[i].iov_base = packets[i].data.as_mut_ptr() as *mut c_void;
        iovs[i].iov_len = packets[i].data.len();

        hdrs[i].msg_hdr.msg_name = &mut addr[i] as *mut _ as *mut _;
        hdrs[i].msg_hdr.msg_namelen = SOCKADDR_STORAGE_SIZE;
        hdrs[i].msg_hdr.msg_iov = &mut iovs[i];
        hdrs[i].msg_hdr.msg_iovlen = 1;
    }
    let mut ts = timespec {
        tv_sec: 1,
        tv_nsec: 0,
    };

    let mut total_size = 0;
    let npkts =
        match unsafe { recvmmsg(sock_fd, &mut hdrs[0], count as u32, MSG_WAITFORONE, &mut ts) } {
            -1 => return Err(io::Error::last_os_error()),
            n => {
                let mut pkt_idx: usize = 0;
                for i in 0..n as usize {
                    let inet_addr = if addr[i].ss_family == AF_INET as sa_family_t
                        && hdrs[i].msg_hdr.msg_namelen == SOCKADDR_IN_SIZE
                    {
                        let a: *const sockaddr_in = &addr[i] as *const _ as *const _;
                        unsafe { InetAddr::V4(*a) }
                    } else if addr[i].ss_family == AF_INET6 as sa_family_t
                        && hdrs[i].msg_hdr.msg_namelen == SOCKADDR_IN6_SIZE
                    {
                        let a: *const sockaddr_in6 = &addr[i] as *const _ as *const _;
                        unsafe { InetAddr::V6(*a) }
                    } else {
                        error!(
                            "recvmmsg unexpected ss_family:{} msg_namelen:{}",
                            addr[i].ss_family, hdrs[i].msg_hdr.msg_namelen
                        );
                        continue;
                    };
                    let mut p = &mut packets[pkt_idx];
                    p.meta.size = hdrs[i].msg_len as usize;
                    total_size += p.meta.size;
                    p.meta.set_addr(&inet_addr.to_std());
                    pkt_idx += 1;
                }
                pkt_idx
            }
        };

    Ok((total_size, npkts))
}

#[cfg(test)]
mod tests {
    use crate::packet::PACKET_DATA_SIZE;
    use crate::recvmmsg::*;
    use std::net::{SocketAddr, UdpSocket};
    use std::time::{Duration, Instant};

    type TestConfig = (UdpSocket, SocketAddr, UdpSocket, SocketAddr);

    fn test_setup_reader_sender(ip_str: &str) -> io::Result<TestConfig> {
        let reader = UdpSocket::bind(ip_str)?;
        let addr = reader.local_addr()?;
        let sender = UdpSocket::bind(ip_str)?;
        let saddr = sender.local_addr()?;
        Ok((reader, addr, sender, saddr))
    }

    const TEST_NUM_MSGS: usize = 32;
    #[test]
    pub fn test_recv_mmsg_one_iter() {
        let test_one_iter = |(reader, addr, sender, saddr): TestConfig| {
            let sent = TEST_NUM_MSGS - 1;
            for _ in 0..sent {
                let data = [0; PACKET_DATA_SIZE];
                sender.send_to(&data[..], &addr).unwrap();
            }

            let mut packets = vec![Packet::default(); TEST_NUM_MSGS];
            let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
            assert_eq!(sent, recv);
            for packet in packets.iter().take(recv) {
                assert_eq!(packet.meta.size, PACKET_DATA_SIZE);
                assert_eq!(packet.meta.addr(), saddr);
            }
        };

        test_one_iter(test_setup_reader_sender("127.0.0.1:0").unwrap());

        match test_setup_reader_sender("::1:0") {
            Ok(config) => test_one_iter(config),
            Err(e) => warn!("Failed to configure IPv6: {:?}", e),
        }
    }

    #[test]
    pub fn test_recv_mmsg_multi_iter() {
        let test_multi_iter = |(reader, addr, sender, saddr): TestConfig| {
            let sent = TEST_NUM_MSGS + 10;
            for _ in 0..sent {
                let data = [0; PACKET_DATA_SIZE];
                sender.send_to(&data[..], &addr).unwrap();
            }

            let mut packets = vec![Packet::default(); TEST_NUM_MSGS];
            let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
            assert_eq!(TEST_NUM_MSGS, recv);
            for packet in packets.iter().take(recv) {
                assert_eq!(packet.meta.size, PACKET_DATA_SIZE);
                assert_eq!(packet.meta.addr(), saddr);
            }

            let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
            assert_eq!(sent - TEST_NUM_MSGS, recv);
            for packet in packets.iter().take(recv) {
                assert_eq!(packet.meta.size, PACKET_DATA_SIZE);
                assert_eq!(packet.meta.addr(), saddr);
            }
        };

        test_multi_iter(test_setup_reader_sender("127.0.0.1:0").unwrap());

        match test_setup_reader_sender("::1:0") {
            Ok(config) => test_multi_iter(config),
            Err(e) => warn!("Failed to configure IPv6: {:?}", e),
        }
    }

    #[test]
    pub fn test_recv_mmsg_multi_iter_timeout() {
        let reader = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let addr = reader.local_addr().unwrap();
        reader.set_read_timeout(Some(Duration::new(5, 0))).unwrap();
        reader.set_nonblocking(false).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let saddr = sender.local_addr().unwrap();
        let sent = TEST_NUM_MSGS;
        for _ in 0..sent {
            let data = [0; PACKET_DATA_SIZE];
            sender.send_to(&data[..], &addr).unwrap();
        }

        let start = Instant::now();
        let mut packets = vec![Packet::default(); TEST_NUM_MSGS];
        let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
        assert_eq!(TEST_NUM_MSGS, recv);
        for packet in packets.iter().take(recv) {
            assert_eq!(packet.meta.size, PACKET_DATA_SIZE);
            assert_eq!(packet.meta.addr(), saddr);
        }
        reader.set_nonblocking(true).unwrap();

        let _recv = recv_mmsg(&reader, &mut packets[..]);
        assert!(start.elapsed().as_secs() < 5);
    }

    #[test]
    pub fn test_recv_mmsg_multi_addrs() {
        let reader = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let addr = reader.local_addr().unwrap();

        let sender1 = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let saddr1 = sender1.local_addr().unwrap();
        let sent1 = TEST_NUM_MSGS - 1;

        let sender2 = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let saddr2 = sender2.local_addr().unwrap();
        let sent2 = TEST_NUM_MSGS + 1;

        for _ in 0..sent1 {
            let data = [0; PACKET_DATA_SIZE];
            sender1.send_to(&data[..], &addr).unwrap();
        }

        for _ in 0..sent2 {
            let data = [0; PACKET_DATA_SIZE];
            sender2.send_to(&data[..], &addr).unwrap();
        }

        let mut packets = vec![Packet::default(); TEST_NUM_MSGS];

        let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
        assert_eq!(TEST_NUM_MSGS, recv);
        for packet in packets.iter().take(sent1) {
            assert_eq!(packet.meta.size, PACKET_DATA_SIZE);
            assert_eq!(packet.meta.addr(), saddr1);
        }
        for packet in packets.iter().skip(sent1).take(recv - sent1) {
            assert_eq!(packet.meta.size, PACKET_DATA_SIZE);
            assert_eq!(packet.meta.addr(), saddr2);
        }

        let recv = recv_mmsg(&reader, &mut packets[..]).unwrap().1;
        assert_eq!(sent1 + sent2 - TEST_NUM_MSGS, recv);
        for packet in packets.iter().take(recv) {
            assert_eq!(packet.meta.size, PACKET_DATA_SIZE);
            assert_eq!(packet.meta.addr(), saddr2);
        }
    }
}
