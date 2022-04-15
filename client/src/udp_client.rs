//! Simple TPU client that communicates with the given UDP port with UDP and provides
//! an interface for sending transactions

use {
    crate::tpu_connection::{ClientStats, TpuConnection},
    core::iter::repeat,
    solana_sdk::transport::Result as TransportResult,
    solana_streamer::sendmmsg::batch_send,
    std::{
        net::{SocketAddr, UdpSocket},
        sync::Arc,
    },
};

pub struct UdpTpuConnection {
    socket: UdpSocket,
    addr: SocketAddr,
}

impl TpuConnection for UdpTpuConnection {
    fn new(client_socket: UdpSocket, tpu_addr: SocketAddr) -> Self {
        Self {
            socket: client_socket,
            addr: tpu_addr,
        }
    }

    fn tpu_addr(&self) -> &SocketAddr {
        &self.addr
    }

    fn send_wire_transaction<T>(
        &self,
        wire_transaction: T,
        _stats: &ClientStats,
    ) -> TransportResult<()>
    where
        T: AsRef<[u8]>,
    {
        self.socket.send_to(wire_transaction.as_ref(), self.addr)?;
        Ok(())
    }

    fn send_wire_transaction_async(
        &self,
        wire_transaction: Vec<u8>,
        _stats: Arc<ClientStats>,
    ) -> TransportResult<()> {
        self.socket.send_to(wire_transaction.as_ref(), self.addr)?;
        Ok(())
    }

    fn send_wire_transaction_batch<T>(
        &self,
        buffers: &[T],
        _stats: &ClientStats,
    ) -> TransportResult<()>
    where
        T: AsRef<[u8]>,
    {
        let pkts: Vec<_> = buffers.iter().zip(repeat(self.tpu_addr())).collect();
        batch_send(&self.socket, &pkts)?;
        Ok(())
    }
    fn send_wire_transaction_batch_async(
        &self,
        buffers: Vec<Vec<u8>>,
        _stats: Arc<ClientStats>,
    ) -> TransportResult<()> {
        let pkts: Vec<_> = buffers.into_iter().zip(repeat(self.tpu_addr())).collect();
        batch_send(&self.socket, &pkts)?;
        Ok(())
    }
}
