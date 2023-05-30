pub use target_arch::*;
use {super::pod, crate::curve25519::ristretto::PodRistrettoPoint};

impl From<(pod::PedersenCommitment, pod::DecryptHandle)> for pod::ElGamalCiphertext {
    fn from((commitment, handle): (pod::PedersenCommitment, pod::DecryptHandle)) -> Self {
        let mut buf = [0_u8; 64];
        buf[..32].copy_from_slice(&commitment.0);
        buf[32..].copy_from_slice(&handle.0);
        pod::ElGamalCiphertext(buf)
    }
}

impl From<pod::ElGamalCiphertext> for (pod::PedersenCommitment, pod::DecryptHandle) {
    fn from(ciphertext: pod::ElGamalCiphertext) -> Self {
        let commitment: [u8; 32] = ciphertext.0[..32].try_into().unwrap();
        let handle: [u8; 32] = ciphertext.0[32..].try_into().unwrap();

        (
            pod::PedersenCommitment(commitment),
            pod::DecryptHandle(handle),
        )
    }
}

impl From<pod::PedersenCommitment> for PodRistrettoPoint {
    fn from(commitment: pod::PedersenCommitment) -> Self {
        PodRistrettoPoint(commitment.0)
    }
}

impl From<PodRistrettoPoint> for pod::PedersenCommitment {
    fn from(point: PodRistrettoPoint) -> Self {
        pod::PedersenCommitment(point.0)
    }
}

impl From<pod::DecryptHandle> for PodRistrettoPoint {
    fn from(handle: pod::DecryptHandle) -> Self {
        PodRistrettoPoint(handle.0)
    }
}

impl From<PodRistrettoPoint> for pod::DecryptHandle {
    fn from(point: PodRistrettoPoint) -> Self {
        pod::DecryptHandle(point.0)
    }
}

#[cfg(not(target_os = "solana"))]
mod target_arch {
    use {
        super::pod,
        crate::{
            curve25519::scalar::PodScalar,
            errors::ProofError,
            instruction::{
                transfer::{TransferAmountEncryption, TransferPubkeys},
                transfer_with_fee::{FeeEncryption, FeeParameters, TransferWithFeePubkeys},
            },
        },
        curve25519_dalek::{ristretto::CompressedRistretto, scalar::Scalar},
        std::convert::TryFrom,
    };

    impl From<Scalar> for PodScalar {
        fn from(scalar: Scalar) -> Self {
            Self(scalar.to_bytes())
        }
    }

    impl TryFrom<PodScalar> for Scalar {
        type Error = ProofError;

        fn try_from(pod: PodScalar) -> Result<Self, Self::Error> {
            Scalar::from_canonical_bytes(pod.0).ok_or(ProofError::CiphertextDeserialization)
        }
    }

    impl From<CompressedRistretto> for pod::CompressedRistretto {
        fn from(cr: CompressedRistretto) -> Self {
            Self(cr.to_bytes())
        }
    }

    impl From<pod::CompressedRistretto> for CompressedRistretto {
        fn from(pod: pod::CompressedRistretto) -> Self {
            Self(pod.0)
        }
    }

    impl From<TransferPubkeys> for pod::TransferPubkeys {
        fn from(keys: TransferPubkeys) -> Self {
            Self {
                source_pubkey: keys.source_pubkey.into(),
                destination_pubkey: keys.destination_pubkey.into(),
                auditor_pubkey: keys.auditor_pubkey.into(),
            }
        }
    }

    impl TryFrom<pod::TransferPubkeys> for TransferPubkeys {
        type Error = ProofError;

        fn try_from(pod: pod::TransferPubkeys) -> Result<Self, Self::Error> {
            Ok(Self {
                source_pubkey: pod.source_pubkey.try_into()?,
                destination_pubkey: pod.destination_pubkey.try_into()?,
                auditor_pubkey: pod.auditor_pubkey.try_into()?,
            })
        }
    }

    impl From<TransferWithFeePubkeys> for pod::TransferWithFeePubkeys {
        fn from(keys: TransferWithFeePubkeys) -> Self {
            Self {
                source_pubkey: keys.source_pubkey.into(),
                destination_pubkey: keys.destination_pubkey.into(),
                auditor_pubkey: keys.auditor_pubkey.into(),
                withdraw_withheld_authority_pubkey: keys.withdraw_withheld_authority_pubkey.into(),
            }
        }
    }

    impl TryFrom<pod::TransferWithFeePubkeys> for TransferWithFeePubkeys {
        type Error = ProofError;

        fn try_from(pod: pod::TransferWithFeePubkeys) -> Result<Self, Self::Error> {
            Ok(Self {
                source_pubkey: pod.source_pubkey.try_into()?,
                destination_pubkey: pod.destination_pubkey.try_into()?,
                auditor_pubkey: pod.auditor_pubkey.try_into()?,
                withdraw_withheld_authority_pubkey: pod
                    .withdraw_withheld_authority_pubkey
                    .try_into()?,
            })
        }
    }

    impl From<TransferAmountEncryption> for pod::TransferAmountEncryption {
        fn from(ciphertext: TransferAmountEncryption) -> Self {
            Self {
                commitment: ciphertext.commitment.into(),
                source_handle: ciphertext.source_handle.into(),
                destination_handle: ciphertext.destination_handle.into(),
                auditor_handle: ciphertext.auditor_handle.into(),
            }
        }
    }

    impl TryFrom<pod::TransferAmountEncryption> for TransferAmountEncryption {
        type Error = ProofError;

        fn try_from(pod: pod::TransferAmountEncryption) -> Result<Self, Self::Error> {
            Ok(Self {
                commitment: pod.commitment.try_into()?,
                source_handle: pod.source_handle.try_into()?,
                destination_handle: pod.destination_handle.try_into()?,
                auditor_handle: pod.auditor_handle.try_into()?,
            })
        }
    }

    impl From<FeeEncryption> for pod::FeeEncryption {
        fn from(ciphertext: FeeEncryption) -> Self {
            Self {
                commitment: ciphertext.commitment.into(),
                destination_handle: ciphertext.destination_handle.into(),
                withdraw_withheld_authority_handle: ciphertext
                    .withdraw_withheld_authority_handle
                    .into(),
            }
        }
    }

    impl TryFrom<pod::FeeEncryption> for FeeEncryption {
        type Error = ProofError;

        fn try_from(pod: pod::FeeEncryption) -> Result<Self, Self::Error> {
            Ok(Self {
                commitment: pod.commitment.try_into()?,
                destination_handle: pod.destination_handle.try_into()?,
                withdraw_withheld_authority_handle: pod
                    .withdraw_withheld_authority_handle
                    .try_into()?,
            })
        }
    }

    impl From<FeeParameters> for pod::FeeParameters {
        fn from(parameters: FeeParameters) -> Self {
            Self {
                fee_rate_basis_points: parameters.fee_rate_basis_points.into(),
                maximum_fee: parameters.maximum_fee.into(),
            }
        }
    }

    impl From<pod::FeeParameters> for FeeParameters {
        fn from(pod: pod::FeeParameters) -> Self {
            Self {
                fee_rate_basis_points: pod.fee_rate_basis_points.into(),
                maximum_fee: pod.maximum_fee.into(),
            }
        }
    }
}

#[cfg(target_os = "solana")]
#[allow(unused_variables)]
mod target_arch {}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{encryption::pedersen::Pedersen, range_proof::RangeProof},
        merlin::Transcript,
        std::convert::TryInto,
    };

    #[test]
    fn test_pod_range_proof_64() {
        let (comm, open) = Pedersen::new(55_u64);

        let mut transcript_create = Transcript::new(b"Test");
        let mut transcript_verify = Transcript::new(b"Test");

        let proof = RangeProof::new(vec![55], vec![64], vec![&open], &mut transcript_create);

        let proof_serialized: pod::RangeProof64 = proof.try_into().unwrap();
        let proof_deserialized: RangeProof = proof_serialized.try_into().unwrap();

        assert!(proof_deserialized
            .verify(vec![&comm], vec![64], &mut transcript_verify)
            .is_ok());

        // should fail to serialize to pod::RangeProof128
        let proof = RangeProof::new(vec![55], vec![64], vec![&open], &mut transcript_create);

        assert!(TryInto::<pod::RangeProof128>::try_into(proof).is_err());
    }

    #[test]
    fn test_pod_range_proof_128() {
        let (comm_1, open_1) = Pedersen::new(55_u64);
        let (comm_2, open_2) = Pedersen::new(77_u64);
        let (comm_3, open_3) = Pedersen::new(99_u64);

        let mut transcript_create = Transcript::new(b"Test");
        let mut transcript_verify = Transcript::new(b"Test");

        let proof = RangeProof::new(
            vec![55, 77, 99],
            vec![64, 32, 32],
            vec![&open_1, &open_2, &open_3],
            &mut transcript_create,
        );

        let proof_serialized: pod::RangeProof128 = proof.try_into().unwrap();
        let proof_deserialized: RangeProof = proof_serialized.try_into().unwrap();

        assert!(proof_deserialized
            .verify(
                vec![&comm_1, &comm_2, &comm_3],
                vec![64, 32, 32],
                &mut transcript_verify,
            )
            .is_ok());

        // should fail to serialize to pod::RangeProof64
        let proof = RangeProof::new(
            vec![55, 77, 99],
            vec![64, 32, 32],
            vec![&open_1, &open_2, &open_3],
            &mut transcript_create,
        );

        assert!(TryInto::<pod::RangeProof64>::try_into(proof).is_err());
    }
}
