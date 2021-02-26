use crate::{
    cluster_info_vote_listener::VerifiedLabelVotePacketsReceiver, crds_value::CrdsValueLabel,
    result::Result,
};
use solana_perf::packet::Packets;
use std::{collections::HashMap, time::Duration};

#[derive(Default)]
pub struct VerifiedVotePackets(HashMap<CrdsValueLabel, (u64, Packets)>);

impl VerifiedVotePackets {
    pub fn receive_and_process_vote_packets(
        &mut self,
        vote_packets_receiver: &VerifiedLabelVotePacketsReceiver,
        last_update_version: &mut u64,
    ) -> Result<()> {
        let timer = Duration::from_millis(200);
        let vote_packets = vote_packets_receiver.recv_timeout(timer)?;
        *last_update_version += 1;
        for (label, packet) in vote_packets {
            self.0.insert(label, (*last_update_version, packet));
        }
        while let Ok(vote_packets) = vote_packets_receiver.try_recv() {
            for (label, packet) in vote_packets {
                self.0.insert(label, (*last_update_version, packet));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn get_vote_packets(&self, key: &CrdsValueLabel) -> Option<&(u64, Packets)> {
        self.0.get(key)
    }

    pub fn get_latest_votes(&self, last_update_version: u64) -> (u64, Packets) {
        let mut new_update_version = last_update_version;
        let packets = self
            .0
            .values()
            .filter(|(v, _)| *v > last_update_version)
            .flat_map(|(v, packets)| {
                new_update_version = std::cmp::max(*v, new_update_version);
                packets.packets.clone()
            })
            .collect();
        (new_update_version, Packets::new(packets))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::Error;
    use crossbeam_channel::{unbounded, RecvTimeoutError};
    use solana_perf::packet::{Meta, Packet};

    #[test]
    fn test_get_latest_votes() {
        let pubkey = solana_sdk::pubkey::new_rand();
        let label1 = CrdsValueLabel::Vote(0, pubkey);
        let label2 = CrdsValueLabel::Vote(1, pubkey);
        let mut verified_vote_packets = VerifiedVotePackets(HashMap::new());

        let data = Packet {
            meta: Meta {
                repair: true,
                ..Meta::default()
            },
            ..Packet::default()
        };

        let none_empty_packets = Packets::new(vec![data, Packet::default()]);

        verified_vote_packets
            .0
            .insert(label1, (2, none_empty_packets));
        verified_vote_packets
            .0
            .insert(label2, (1, Packets::default()));

        // Both updates have timestamps greater than 0, so both should be returned
        let (new_update_version, updates) = verified_vote_packets.get_latest_votes(0);
        assert_eq!(new_update_version, 2);
        assert_eq!(updates.packets.len(), 2);

        // Only the nonempty packet had a timestamp greater than 1
        let (new_update_version, updates) = verified_vote_packets.get_latest_votes(1);
        assert_eq!(new_update_version, 2);
        assert_eq!(updates.packets.is_empty(), false);

        // If the given timestamp is greater than all timestamps in any update,
        // returned timestamp should be the same as the given timestamp, and
        // no updates should be returned
        let (new_update_version, updates) = verified_vote_packets.get_latest_votes(3);
        assert_eq!(new_update_version, 3);
        assert!(updates.is_empty());
    }

    #[test]
    fn test_get_and_process_vote_packets() {
        let (s, r) = unbounded();
        let pubkey = solana_sdk::pubkey::new_rand();
        let label1 = CrdsValueLabel::Vote(0, pubkey);
        let label2 = CrdsValueLabel::Vote(1, pubkey);
        let mut update_version = 0;
        s.send(vec![(label1.clone(), Packets::default())]).unwrap();
        s.send(vec![(label2.clone(), Packets::default())]).unwrap();

        let data = Packet {
            meta: Meta {
                repair: true,
                ..Meta::default()
            },
            ..Packet::default()
        };

        let later_packets = Packets::new(vec![data, Packet::default()]);
        s.send(vec![(label1.clone(), later_packets)]).unwrap();
        let mut verified_vote_packets = VerifiedVotePackets(HashMap::new());
        verified_vote_packets
            .receive_and_process_vote_packets(&r, &mut update_version)
            .unwrap();

        // Test timestamps for same batch are the same
        let update_version1 = verified_vote_packets.get_vote_packets(&label1).unwrap().0;
        assert_eq!(
            update_version1,
            verified_vote_packets.get_vote_packets(&label2).unwrap().0
        );

        // Test the later value overwrote the earlier one for this label
        assert!(
            verified_vote_packets
                .get_vote_packets(&label1)
                .unwrap()
                .1
                .packets
                .len()
                > 1
        );
        assert_eq!(
            verified_vote_packets
                .get_vote_packets(&label2)
                .unwrap()
                .1
                .packets
                .len(),
            0
        );

        // Test timestamp for next batch overwrites the original
        s.send(vec![(label2.clone(), Packets::default())]).unwrap();
        verified_vote_packets
            .receive_and_process_vote_packets(&r, &mut update_version)
            .unwrap();
        let update_version2 = verified_vote_packets.get_vote_packets(&label2).unwrap().0;
        assert!(update_version2 > update_version1);

        // Test empty doesn't bump the version
        let before = update_version;
        assert_matches!(
            verified_vote_packets.receive_and_process_vote_packets(&r, &mut update_version),
            Err(Error::CrossbeamRecvTimeoutError(RecvTimeoutError::Timeout))
        );
        assert_eq!(before, update_version);
    }
}
