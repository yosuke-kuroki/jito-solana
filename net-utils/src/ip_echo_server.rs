use crate::{ip_echo_server_reply_length, HEADER_LENGTH};
use bytes::Bytes;
use log::*;
use serde_derive::{Deserialize, Serialize};
use std::{io, net::SocketAddr, time::Duration};
use tokio::{net::TcpListener, prelude::*, reactor::Handle, runtime::Runtime};

pub type IpEchoServer = Runtime;

pub const MAX_PORT_COUNT_PER_MESSAGE: usize = 4;

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct IpEchoServerMessage {
    tcp_ports: [u16; MAX_PORT_COUNT_PER_MESSAGE], // Fixed size list of ports to avoid vec serde
    udp_ports: [u16; MAX_PORT_COUNT_PER_MESSAGE], // Fixed size list of ports to avoid vec serde
}

impl IpEchoServerMessage {
    pub fn new(tcp_ports: &[u16], udp_ports: &[u16]) -> Self {
        let mut msg = Self::default();
        assert!(tcp_ports.len() <= msg.tcp_ports.len());
        assert!(udp_ports.len() <= msg.udp_ports.len());

        msg.tcp_ports[..tcp_ports.len()].copy_from_slice(tcp_ports);
        msg.udp_ports[..udp_ports.len()].copy_from_slice(udp_ports);
        msg
    }
}

pub(crate) fn ip_echo_server_request_length() -> usize {
    const REQUEST_TERMINUS_LENGTH: usize = 1;
    HEADER_LENGTH
        + bincode::serialized_size(&IpEchoServerMessage::default()).unwrap() as usize
        + REQUEST_TERMINUS_LENGTH
}

/// Starts a simple TCP server on the given port that echos the IP address of any peer that
/// connects.  Used by |get_public_ip_addr|
pub fn ip_echo_server(tcp: std::net::TcpListener) -> IpEchoServer {
    info!("bound to {:?}", tcp.local_addr());
    let tcp =
        TcpListener::from_std(tcp, &Handle::default()).expect("Failed to convert std::TcpListener");

    let server = tcp
        .incoming()
        .map_err(|err| warn!("accept failed: {:?}", err))
        .filter_map(|socket| match socket.peer_addr() {
            Ok(peer_addr) => {
                info!("connection from {:?}", peer_addr);
                Some((peer_addr, socket))
            }
            Err(err) => {
                info!("peer_addr failed for {:?}: {:?}", socket, err);
                None
            }
        })
        .for_each(move |(peer_addr, socket)| {
            let data = vec![0u8; ip_echo_server_request_length()];
            let (reader, writer) = socket.split();

            let processor = tokio::io::read_exact(reader, data)
                .and_then(move |(_, data)| {
                    if data.len() < HEADER_LENGTH {
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            format!("Request too short, received {} bytes", data.len()),
                        ));
                    }
                    let request_header: String =
                        data[0..HEADER_LENGTH].iter().map(|b| *b as char).collect();
                    if request_header != "\0\0\0\0" {
                        // Explicitly check for HTTP GET/POST requests to more gracefully handle
                        // the case where a user accidentally tried to use a gossip entrypoint in
                        // place of a JSON RPC URL:
                        if request_header == "GET " || request_header == "POST" {
                            return Ok(None); // None -> Send HTTP error response
                        }
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            format!("Bad request header: {}", request_header),
                        ));
                    }

                    let expected_len =
                        bincode::serialized_size(&IpEchoServerMessage::default()).unwrap() as usize;
                    let actual_len = data[HEADER_LENGTH..].len();
                    if actual_len < expected_len {
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            format!(
                                "Request too short, actual {} < expected {}",
                                actual_len, expected_len
                            ),
                        ));
                    }

                    bincode::deserialize::<IpEchoServerMessage>(&data[HEADER_LENGTH..])
                        .map(Some)
                        .map_err(|err| {
                            io::Error::new(
                                io::ErrorKind::Other,
                                format!("Failed to deserialize IpEchoServerMessage: {:?}", err),
                            )
                        })
                })
                .and_then(move |maybe_msg| {
                    match maybe_msg {
                        None => None, // Send HTTP error response
                        Some(msg) => {
                            // Fire a datagram at each non-zero UDP port
                            if !msg.udp_ports.is_empty() {
                                match std::net::UdpSocket::bind("0.0.0.0:0") {
                                    Ok(udp_socket) => {
                                        for udp_port in &msg.udp_ports {
                                            if *udp_port != 0 {
                                                match udp_socket.send_to(
                                                    &[0],
                                                    SocketAddr::from((peer_addr.ip(), *udp_port)),
                                                ) {
                                                    Ok(_) => debug!(
                                                        "Successful send_to udp/{}",
                                                        udp_port
                                                    ),
                                                    Err(err) => info!(
                                                        "Failed to send_to udp/{}: {}",
                                                        udp_port, err
                                                    ),
                                                }
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        warn!("Failed to bind local udp socket: {}", err);
                                    }
                                }
                            }

                            // Try to connect to each non-zero TCP port
                            let tcp_futures: Vec<_> =
                                msg.tcp_ports
                                    .iter()
                                    .filter_map(|tcp_port| {
                                        let tcp_port = *tcp_port;
                                        if tcp_port == 0 {
                                            None
                                        } else {
                                            Some(
                                                tokio::net::TcpStream::connect(&SocketAddr::new(
                                                    peer_addr.ip(),
                                                    tcp_port,
                                                ))
                                                .and_then(move |tcp_stream| {
                                                    debug!(
                                                        "Connection established to tcp/{}",
                                                        tcp_port
                                                    );
                                                    let _ = tcp_stream
                                                        .shutdown(std::net::Shutdown::Both);
                                                    Ok(())
                                                })
                                                .timeout(Duration::from_secs(5))
                                                .or_else(move |err| {
                                                    Err(io::Error::new(
                                                        io::ErrorKind::Other,
                                                        format!(
                                                            "Connection timeout to {}: {:?}",
                                                            tcp_port, err
                                                        ),
                                                    ))
                                                }),
                                            )
                                        }
                                    })
                                    .collect();
                            Some(future::join_all(tcp_futures))
                        }
                    }
                })
                .and_then(move |valid_request| {
                    let bytes = if valid_request.is_none() {
                        Bytes::from("HTTP/1.1 400 Bad Request\nContent-length: 0\n\n")
                    } else {
                        // "\0\0\0\0" header is added to ensure a valid response will never
                        // conflict with the first four bytes of a valid HTTP response.
                        let mut bytes = vec![0u8; ip_echo_server_reply_length()];
                        bincode::serialize_into(&mut bytes[HEADER_LENGTH..], &peer_addr.ip())
                            .unwrap();
                        Bytes::from(bytes)
                    };
                    tokio::io::write_all(writer, bytes)
                })
                .timeout(Duration::from_secs(5))
                .then(|result| {
                    if let Err(err) = result {
                        info!("Session failed: {:?}", err);
                    }
                    Ok(())
                });

            tokio::spawn(processor)
        });

    let mut rt = Runtime::new().expect("Failed to create Runtime");
    rt.spawn(server);
    rt
}
