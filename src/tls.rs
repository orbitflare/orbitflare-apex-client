//! Client certificate derived from the API key, and the QUIC endpoint.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::QuicClientConfig;
use quinn::{ClientConfig, Endpoint, IdleTimeout, TransportConfig};
use sha2::Sha256;
use solana_keypair::Keypair;

use crate::Error;

/// Must match the server; see the sender's `auth.rs`.
const CLIENT_CERT_SALT: &[u8] = b"orbitflare-apex";
const CLIENT_CERT_INFO: &[u8] = b"orbitflare-apex/quic-client-cert/v1";
const ALPN_TPU: &[u8] = b"solana-tpu";
const MAX_IDLE: Duration = Duration::from_secs(10);

pub fn derive_client_keypair(api_key: &str) -> Keypair {
    let hk = hkdf::Hkdf::<Sha256>::new(Some(CLIENT_CERT_SALT), api_key.as_bytes());
    let mut seed = [0u8; 32];
    hk.expand(CLIENT_CERT_INFO, &mut seed)
        .expect("32 bytes is a valid hkdf length");
    Keypair::new_from_array(seed)
}

pub fn build_endpoint(
    bind: SocketAddr,
    identity: &Keypair,
    keep_alive: Duration,
) -> Result<Endpoint, Error> {
    let (cert, key) = solana_tls_utils::new_dummy_x509_certificate(identity);
    #[allow(deprecated)]
    let mut crypto = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| Error::Tls(e.to_string()))?
    .dangerous()
    .with_custom_certificate_verifier(solana_tls_utils::SkipServerVerification::new())
    .with_client_auth_cert(vec![cert], key)
    .map_err(|e| Error::Tls(e.to_string()))?;
    crypto.enable_early_data = true;
    crypto.alpn_protocols = vec![ALPN_TPU.to_vec()];

    let mut transport = TransportConfig::default();
    transport.max_idle_timeout(Some(
        IdleTimeout::try_from(MAX_IDLE).map_err(|e| Error::Tls(e.to_string()))?,
    ));
    transport.keep_alive_interval(Some(keep_alive));
    transport.send_fairness(false);

    let mut config = ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(crypto).map_err(|e| Error::Tls(e.to_string()))?,
    ));
    config.transport_config(Arc::new(transport));

    let socket = std::net::UdpSocket::bind(bind).map_err(Error::Bind)?;
    let mut endpoint = Endpoint::new(
        quinn::EndpointConfig::default(),
        None,
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .map_err(Error::Bind)?;
    endpoint.set_default_client_config(config);
    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_signer::Signer;

    /// Pinned against the server's `derived_keypair_vector` test.
    #[test]
    fn derivation_matches_the_server() {
        assert_eq!(
            derive_client_keypair("test-api-key").pubkey().to_string(),
            "2ovF9aU8fszHXpZwxaC1S9NF2m5VVDRBUvDUVQvXs4Tv"
        );
    }
}
