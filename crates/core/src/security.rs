//! TLS 1.3 identity + fingerprint-pinned mutual authentication.
//!
//! ScreenLink does not use a CA/PKI. Every device generates a long-term
//! self-signed certificate at first run; its identity *is* the SHA-256
//! fingerprint of that certificate. Pairing records the peer's fingerprint;
//! reconnection requires the peer to present a certificate with that exact
//! fingerprint and to prove possession of the matching private key (the latter
//! is enforced by the TLS handshake signature, which we *do* verify).
//!
//! This gives mutual authentication with a tiny trust model: trust is the set of
//! fingerprints you paired with, nothing more.

use crate::error::{Error, Result};
use crate::protocol::DeviceId;
use crate::trust::TrustStore;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A device's long-term cryptographic identity.
#[derive(Clone)]
pub struct Identity {
    cert_der: CertificateDer<'static>,
    key_der: Vec<u8>, // PKCS#8
    fingerprint: String,
}

impl Identity {
    /// Generate a fresh self-signed identity.
    pub fn generate() -> Result<Self> {
        let ck = rcgen::generate_simple_self_signed(vec!["screenlink.local".to_string()])?;
        let cert_der = ck.cert.der().clone();
        let key_der = ck.key_pair.serialize_der();
        let fingerprint = fingerprint_hex(&cert_der);
        Ok(Self {
            cert_der,
            key_der,
            fingerprint,
        })
    }

    /// Load the identity from `dir` (cert.der + key.der), generating and saving a
    /// new one if absent.
    pub fn load_or_generate(dir: &Path) -> Result<Self> {
        let cert_path = dir.join("identity.cert.der");
        let key_path = dir.join("identity.key.der");
        if cert_path.exists() && key_path.exists() {
            let cert = std::fs::read(&cert_path)?;
            let key_der = std::fs::read(&key_path)?;
            let cert_der = CertificateDer::from(cert);
            let fingerprint = fingerprint_hex(&cert_der);
            return Ok(Self {
                cert_der,
                key_der,
                fingerprint,
            });
        }
        let id = Self::generate()?;
        std::fs::create_dir_all(dir)?;
        std::fs::write(&cert_path, id.cert_der.as_ref())?;
        std::fs::write(&key_path, &id.key_der)?;
        Ok(id)
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId(self.fingerprint.clone())
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn cert_der(&self) -> CertificateDer<'static> {
        self.cert_der.clone()
    }

    fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone()))
    }
}

/// Lowercase hex SHA-256 of a certificate's DER bytes.
pub fn fingerprint_hex(cert: &CertificateDer<'_>) -> String {
    let digest = Sha256::digest(cert.as_ref());
    hex::encode(digest)
}

/// What the verifier should do with a peer certificate it sees.
#[derive(Clone)]
pub enum VerifyMode {
    /// Pairing / trust-on-first-use: accept any self-signed peer and record its
    /// fingerprint. Safe only because the user then confirms the comparison PIN.
    PairTofu,
    /// Normal reconnect: accept only fingerprints already in the trust store.
    RequireTrusted(Arc<TrustStore>),
}

/// Implements both rustls verifier traits (client-side server-cert and
/// server-side client-cert) with fingerprint pinning. The handshake signature is
/// always cryptographically verified; only the *chain/PKI* validation is replaced
/// by our fingerprint policy.
#[derive(Clone)]
pub struct PinnedVerifier {
    algs: WebPkiSupportedAlgorithms,
    mode: VerifyMode,
    no_subjects: Arc<Vec<DistinguishedName>>,
    /// Fingerprint of the peer certificate observed during the handshake.
    observed: Arc<Mutex<Option<String>>>,
}

impl std::fmt::Debug for PinnedVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedVerifier").finish_non_exhaustive()
    }
}

impl PinnedVerifier {
    pub fn new(mode: VerifyMode) -> Arc<Self> {
        let provider = rustls::crypto::ring::default_provider();
        Arc::new(Self {
            algs: provider.signature_verification_algorithms,
            mode,
            no_subjects: Arc::new(Vec::new()),
            observed: Arc::new(Mutex::new(None)),
        })
    }

    /// The peer fingerprint captured during the most recent handshake, if any.
    pub fn observed_fingerprint(&self) -> Option<String> {
        self.observed.lock().unwrap().clone()
    }

    fn check(&self, end_entity: &CertificateDer<'_>) -> std::result::Result<(), rustls::Error> {
        let fp = fingerprint_hex(end_entity);
        *self.observed.lock().unwrap() = Some(fp.clone());
        match &self.mode {
            VerifyMode::PairTofu => Ok(()),
            VerifyMode::RequireTrusted(store) => {
                if store.is_trusted(&fp) {
                    Ok(())
                } else {
                    Err(rustls::Error::General(
                        "peer certificate is not a trusted (paired) device".to_string(),
                    ))
                }
            }
        }
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        self.check(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

impl ClientCertVerifier for PinnedVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &self.no_subjects
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        self.check(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// Build a TLS server config that presents `id` and pins clients per `verifier`.
pub fn server_config(
    id: &Identity,
    verifier: Arc<PinnedVerifier>,
) -> Result<Arc<rustls::ServerConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let client_verifier: Arc<dyn ClientCertVerifier> = verifier;
    let cfg = rustls::ServerConfig::builder_with_provider(provider)
        // TLS 1.3 only: both peers run ScreenLink, so there is no need to allow
        // the older 1.2 handshake.
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(Error::Tls)?
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(vec![id.cert_der()], id.private_key())?;
    Ok(Arc::new(cfg))
}

/// Build a TLS client config that presents `id` and pins the server per `verifier`.
pub fn client_config(
    id: &Identity,
    verifier: Arc<PinnedVerifier>,
) -> Result<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server_verifier: Arc<dyn ServerCertVerifier> = verifier;
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(Error::Tls)?
        .dangerous()
        .with_custom_certificate_verifier(server_verifier)
        .with_client_auth_cert(vec![id.cert_der()], id.private_key())?;
    Ok(Arc::new(cfg))
}

/// A constant SNI name to use when connecting; the verifier ignores it (we pin by
/// fingerprint, not hostname), but rustls requires *a* name.
pub fn sni_name() -> ServerName<'static> {
    ServerName::try_from("screenlink.local").expect("static name is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_fingerprint_is_stable_and_64_hex() {
        crate::init_crypto();
        let id = Identity::generate().unwrap();
        assert_eq!(id.fingerprint().len(), 64);
        assert!(id.fingerprint().bytes().all(|b| b.is_ascii_hexdigit()));
        // Recomputing from the cert gives the same value.
        assert_eq!(fingerprint_hex(&id.cert_der()), id.fingerprint());
    }

    #[test]
    fn distinct_identities_have_distinct_fingerprints() {
        crate::init_crypto();
        let a = Identity::generate().unwrap();
        let b = Identity::generate().unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
