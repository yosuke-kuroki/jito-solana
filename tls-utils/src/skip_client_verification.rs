use {
    crate::crypto_provider,
    rustls::{
        pki_types::{CertificateDer, UnixTime},
        server::danger::ClientCertVerified,
        DistinguishedName,
    },
    std::{fmt::Debug, sync::Arc},
};

/// Implementation of [`ClientCertVerifier`] that ignores the server
/// certificate. Yet still checks the TLS signatures.
#[derive(Debug)]
pub struct SkipClientVerification(Arc<rustls::crypto::CryptoProvider>);

impl SkipClientVerification {
    pub fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(crypto_provider())))
    }
}
impl rustls::server::danger::ClientCertVerifier for SkipClientVerification {
    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer,
        _intermediates: &[CertificateDer],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }

    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        self.offer_client_auth()
    }
}
