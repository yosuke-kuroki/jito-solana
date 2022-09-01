//! Deserializes packets from sigverify stage. Owned by banking stage.

use {
    crate::{
        immutable_deserialized_packet::ImmutableDeserializedPacket,
        sigverify::SigverifyTracerPacketStats,
    },
    crossbeam_channel::{Receiver as CrossbeamReceiver, RecvTimeoutError},
    solana_perf::packet::PacketBatch,
    std::time::{Duration, Instant},
};

pub type BankingPacketBatch = (Vec<PacketBatch>, Option<SigverifyTracerPacketStats>);
pub type BankingPacketReceiver = CrossbeamReceiver<BankingPacketBatch>;

/// Results from deserializing packet batches.
pub struct ReceivePacketResults {
    /// Deserialized packets from all received packet batches
    pub deserialized_packets: Vec<ImmutableDeserializedPacket>,
    /// Aggregate tracer stats for all received packet batches
    pub new_tracer_stats_option: Option<SigverifyTracerPacketStats>,
    /// Number of packets passing sigverify
    pub passed_sigverify_count: u64,
    /// Number of packets failing sigverify
    pub failed_sigverify_count: u64,
}

pub struct PacketDeserializer {
    /// Receiver for packet batches from sigverify stage
    packet_batch_receiver: BankingPacketReceiver,
}

impl PacketDeserializer {
    pub fn new(packet_batch_receiver: BankingPacketReceiver) -> Self {
        Self {
            packet_batch_receiver,
        }
    }

    /// Handles receiving packet batches from sigverify and returns a vector of deserialized packets
    pub fn handle_received_packets(
        &self,
        recv_timeout: Duration,
        capacity: usize,
    ) -> Result<ReceivePacketResults, RecvTimeoutError> {
        let (packet_batches, sigverify_tracer_stats_option) =
            self.receive_until(recv_timeout, capacity)?;

        let packet_count: usize = packet_batches.iter().map(|x| x.len()).sum();
        let mut passed_sigverify_count: usize = 0;
        let mut failed_sigverify_count: usize = 0;
        let mut deserialized_packets = Vec::with_capacity(packet_count);
        for packet_batch in packet_batches {
            let packet_indexes = Self::generate_packet_indexes(&packet_batch);

            passed_sigverify_count += packet_indexes.len();
            failed_sigverify_count += packet_batch.len().saturating_sub(packet_indexes.len());

            deserialized_packets.extend(Self::deserialize_packets(&packet_batch, &packet_indexes));
        }

        Ok(ReceivePacketResults {
            deserialized_packets,
            new_tracer_stats_option: sigverify_tracer_stats_option,
            passed_sigverify_count: passed_sigverify_count as u64,
            failed_sigverify_count: failed_sigverify_count as u64,
        })
    }

    /// Receives packet batches from sigverify stage with a timeout, and aggregates tracer packet stats
    fn receive_until(
        &self,
        recv_timeout: Duration,
        packet_count_upperbound: usize,
    ) -> Result<(Vec<PacketBatch>, Option<SigverifyTracerPacketStats>), RecvTimeoutError> {
        let start = Instant::now();
        let (mut packet_batches, mut aggregated_tracer_packet_stats_option) =
            self.packet_batch_receiver.recv_timeout(recv_timeout)?;

        let mut num_packets_received: usize = packet_batches.iter().map(|batch| batch.len()).sum();
        while let Ok((packet_batch, tracer_packet_stats_option)) =
            self.packet_batch_receiver.try_recv()
        {
            trace!("got more packet batches in packet deserializer");
            let (packets_received, packet_count_overflowed) = num_packets_received
                .overflowing_add(packet_batch.iter().map(|batch| batch.len()).sum());
            packet_batches.extend(packet_batch);

            if let Some(tracer_packet_stats) = &tracer_packet_stats_option {
                if let Some(aggregated_tracer_packet_stats) =
                    &mut aggregated_tracer_packet_stats_option
                {
                    aggregated_tracer_packet_stats.aggregate(tracer_packet_stats);
                } else {
                    aggregated_tracer_packet_stats_option = tracer_packet_stats_option;
                }
            }

            if start.elapsed() >= recv_timeout
                || packet_count_overflowed
                || packets_received >= packet_count_upperbound
            {
                break;
            }
            num_packets_received = packets_received;
        }

        Ok((packet_batches, aggregated_tracer_packet_stats_option))
    }

    fn generate_packet_indexes(packet_batch: &PacketBatch) -> Vec<usize> {
        packet_batch
            .iter()
            .enumerate()
            .filter(|(_, pkt)| !pkt.meta.discard())
            .map(|(index, _)| index)
            .collect()
    }

    fn deserialize_packets<'a>(
        packet_batch: &'a PacketBatch,
        packet_indexes: &'a [usize],
    ) -> impl Iterator<Item = ImmutableDeserializedPacket> + 'a {
        packet_indexes.iter().filter_map(move |packet_index| {
            ImmutableDeserializedPacket::new(packet_batch[*packet_index].clone(), None).ok()
        })
    }
}
