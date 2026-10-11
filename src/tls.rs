//! Client certificate derived from the API key, and the QUIC endpoint.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::QuicClientConfig;
use quinn::{AckFrequencyConfig, ClientConfig, Endpoint, IdleTimeout, TransportConfig, VarInt};
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
    // Ask the endpoint to acknowledge every packet as it arrives. Left alone
    // it holds the acknowledgement of a lone packet for 25 ms, which is what
    // one transaction or one keep-alive is.
    let mut ack = AckFrequencyConfig::default();
    ack.ack_eliciting_threshold(VarInt::from_u32(0))
        .max_ack_delay(Some(Duration::ZERO))
        .reordering_threshold(VarInt::from_u32(2));
    transport.ack_frequency_config(Some(ack));

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
    use std::time::Instant;

    /// A server with quinn's defaults, as the endpoint's ingress has.
    fn loopback_server() -> Endpoint {
        let (cert, key) = solana_tls_utils::new_dummy_x509_certificate(&Keypair::new());
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
        tls.alpn_protocols = vec![ALPN_TPU.to_vec()];
        let config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap(),
        ));
        Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap()
    }

    /// One transaction is one packet, and QUIC holds the acknowledgement of
    /// a lone packet for 25 ms unless the sender asks otherwise.
    #[tokio::test]
    async fn a_lone_stream_is_acknowledged_at_once() {
        let server = loopback_server();
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            let conn = server.accept().await.unwrap().await.unwrap();
            while let Ok(mut stream) = conn.accept_uni().await {
                let _ = stream.read_to_end(4096).await;
            }
        });
        let client = build_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &Keypair::new(),
            Duration::from_secs(1),
        )
        .unwrap();
        let conn = client.connect(addr, "apex").unwrap().await.unwrap();
        // The handshake's last packets settle, so each send below is alone.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The fastest of a few: a stall on a busy machine does not fail it,
        // and without the request every one of them waits out the timer.
        let mut fastest = Duration::MAX;
        for _ in 0..5 {
            let mut stream = conn.open_uni().await.unwrap();
            stream.write_all(&[7u8; 300]).await.unwrap();
            stream.finish().unwrap();
            let sent = Instant::now();
            stream.stopped().await.unwrap();
            fastest = fastest.min(sent.elapsed());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            fastest < Duration::from_millis(10),
            "acknowledged after {fastest:?}"
        );
    }

    /// Pinned against the server's `derived_keypair_vector` test.
    #[test]
    fn derivation_matches_the_server() {
        assert_eq!(
            derive_client_keypair("test-api-key").pubkey().to_string(),
            "2ovF9aU8fszHXpZwxaC1S9NF2m5VVDRBUvDUVQvXs4Tv"
        );
    }
}
