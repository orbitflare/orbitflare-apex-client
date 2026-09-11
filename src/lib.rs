//! `apex-sender-client` submits Solana transactions to OrbitFlare's Apex
//! endpoints.
//!
//! Three transports, one tip rule:
//!
//! | Transport | Call | Returns | Use when |
//! |---|---|---|---|
//! | QUIC, unidirectional stream | [`ApexSenderClient::send_transaction`] | the signature, no acknowledgement | lowest latency, you track landing yourself |
//! | QUIC, bidirectional stream | [`ApexSenderClient::send_transaction_with_response`] | accepted or a rejection code | you want the rejection reason inline |
//! | JSON-RPC over HTTP | [`rpc::RpcClient::send_transaction`] (feature `rpc`) | the signature or a JSON-RPC error | drop-in for existing `sendTransaction` code, other languages |
//!
//! The client keeps one persistent QUIC connection per endpoint, authenticates
//! with a client certificate derived from your API key (no key on the wire),
//! sends one serialized transaction per stream and reconnects with 0-RTT when
//! the connection drops.
//!
//! Every transaction must carry one top-level SystemProgram transfer to one of
//! the published tip accounts, at or above your tier's floor
//! ([`MIN_TIP_LAMPORTS`] for the standard tier). [`tip_instruction`] builds
//! it; [`rpc::RpcClient::get_tip_accounts`] or [`rpc::fetch_vaults`] lists
//! the accounts.
//!
//! ```no_run
//! # async fn run() -> Result<(), apex_sender_client::Error> {
//! use apex_sender_client::{ApexSenderClient, Region, tip_instruction};
//! let client = ApexSenderClient::connect(Region::Frankfurt, "your-api-key").await?;
//! // build a VersionedTransaction that includes tip_instruction(payer, tip_account, lamports)
//! # let tx: solana_transaction::versioned::VersionedTransaction = unimplemented!();
//! let signature = client.send_transaction(&tx).await?;
//! # Ok(()) }
//! ```
//!
//! The `examples/` directory covers each transport, raw bytes, a throughput
//! loop and a TypeScript JSON-RPC sender; the Apex docs at https://docs.orbitflare.com/apex specifies the wire
//! format for other languages.

#[cfg(feature = "rpc")]
pub mod rpc;
pub mod tip;
mod tls;
pub mod wire;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use quinn::{Connection, Endpoint};
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;
use tokio::sync::Mutex;

pub use tip::{MIN_TIP_LAMPORTS, tip_instruction};
pub use wire::{Admission, AdmissionCode};

/// The Apex endpoints. Pick the one nearest to you; each routes to every
/// validator client, Jito and the leader TPUs on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Frankfurt,
    Amsterdam,
    London,
    NewYork,
    SaltLakeCity,
    Singapore,
    Tokyo,
    Siauliai,
    /// Geolocated: resolves to the nearest endpoint. Use a named region when
    /// you want a fixed host, for a firewall rule or a pinned round trip.
    Global,
}

impl Region {
    pub const ALL: [Region; 9] = [
        Region::Frankfurt,
        Region::Amsterdam,
        Region::London,
        Region::NewYork,
        Region::SaltLakeCity,
        Region::Singapore,
        Region::Tokyo,
        Region::Siauliai,
        Region::Global,
    ];

    /// `<code>.apex.orbitflare.com`.
    pub fn host(self) -> String {
        format!("{}.apex.orbitflare.com", self.code())
    }

    /// The short code the endpoint reports itself as; `global` for the
    /// geolocated host.
    pub const fn code(self) -> &'static str {
        match self {
            Region::Frankfurt => "fra",
            Region::Amsterdam => "ams",
            Region::London => "lon",
            Region::NewYork => "nyc",
            Region::SaltLakeCity => "slc",
            Region::Singapore => "sgp",
            Region::Tokyo => "tyo",
            Region::Siauliai => "sqq",
            Region::Global => "global",
        }
    }

    /// Parse a code or city name (case-insensitive).
    pub fn parse(code: &str) -> Option<Self> {
        match code.to_ascii_lowercase().as_str() {
            "fra" | "frankfurt" => Some(Region::Frankfurt),
            "ams" | "amsterdam" => Some(Region::Amsterdam),
            "lon" | "london" => Some(Region::London),
            "nyc" | "ny" | "newyork" | "new-york" => Some(Region::NewYork),
            "slc" | "saltlakecity" | "salt-lake-city" => Some(Region::SaltLakeCity),
            "sgp" | "sin" | "singapore" => Some(Region::Singapore),
            "tyo" | "tokyo" => Some(Region::Tokyo),
            "sqq" | "siauliai" => Some(Region::Siauliai),
            "global" | "auto" => Some(Region::Global),
            _ => None,
        }
    }

    pub const QUIC_PORT: u16 = 7001;
    pub const RPC_PORT: u16 = 80;

    pub fn quic_endpoint(self) -> String {
        format!("{}:{}", self.host(), Self::QUIC_PORT)
    }

    /// JSON-RPC and the plain HTTP routes, on port 80.
    pub fn rpc_url(self) -> String {
        format!("http://{}", self.host())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("resolve endpoint: {0}")]
    Resolve(String),
    #[error("tls config: {0}")]
    Tls(String),
    #[error("bind local socket: {0}")]
    Bind(#[source] std::io::Error),
    #[error("connect: {0}")]
    Connect(#[from] quinn::ConnectError),
    #[error("connection: {0}")]
    Connection(#[from] quinn::ConnectionError),
    #[error("write: {0}")]
    Write(#[from] quinn::WriteError),
    #[error("read: {0}")]
    Read(#[from] quinn::ReadToEndError),
    #[error("timed out")]
    Timeout,
    #[error("transaction too large: {0} bytes (max 4096; legacy and v0 max 1232)")]
    TooLarge(usize),
    #[error("serialize transaction: {0}")]
    Serialize(String),
    #[error("malformed admission response")]
    BadAdmission,
    #[error("rejected ({code:?}): {message}")]
    Rejected {
        code: AdmissionCode,
        message: String,
    },
    #[error("closed")]
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionHealth {
    Healthy,
    Closed,
}

#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// `host:port` of the QUIC ingress. Overrides the region default.
    pub endpoint: Option<String>,
    /// Ask Apex to skip Shield-blocklisted leaders.
    pub mev_protect: bool,
    /// Retry budget handed to validator clients; `None` uses their default.
    pub max_retries: Option<u16>,
    /// Local UDP bind address; defaults to an ephemeral port.
    pub bind_addr: Option<SocketAddr>,
    pub connect_timeout: Duration,
    pub send_timeout: Duration,
    /// QUIC PING interval; the endpoint's idle timeout is 10 s.
    pub keep_alive: Duration,
    /// When a send fails because the connection is gone, reconnect (0-RTT
    /// when a session ticket is cached) and send once more.
    pub auto_reconnect: bool,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            endpoint: None,
            mev_protect: false,
            max_retries: None,
            bind_addr: None,
            connect_timeout: Duration::from_secs(3),
            send_timeout: Duration::from_secs(2),
            keep_alive: Duration::from_secs(1),
            auto_reconnect: true,
        }
    }
}

pub struct ApexSenderClient {
    endpoint: Endpoint,
    remote: SocketAddr,
    server_name: String,
    options: ClientOptions,
    connection: Mutex<Option<Connection>>,
    reconnects: AtomicU64,
}

impl ApexSenderClient {
    pub async fn connect(region: Region, api_key: &str) -> Result<Self, Error> {
        Self::connect_with_options(
            ClientOptions {
                endpoint: Some(region.quic_endpoint()),
                ..Default::default()
            },
            api_key,
        )
        .await
    }

    /// Connect to `options.endpoint` (`host:port`) with the certificate
    /// derived from `api_key`.
    pub async fn connect_with_options(
        options: ClientOptions,
        api_key: &str,
    ) -> Result<Self, Error> {
        let target = options
            .endpoint
            .clone()
            .unwrap_or_else(|| Region::Frankfurt.quic_endpoint());
        let remote = tokio::net::lookup_host(&target)
            .await
            .map_err(|e| Error::Resolve(e.to_string()))?
            .next()
            .ok_or_else(|| Error::Resolve(format!("{target}: no address")))?;
        let server_name = target
            .rsplit_once(':')
            .map_or(target.as_str(), |(h, _)| h)
            .to_owned();
        let keypair = tls::derive_client_keypair(api_key);
        let endpoint = tls::build_endpoint(
            options.bind_addr.unwrap_or_else(|| match remote {
                SocketAddr::V4(_) => "0.0.0.0:0".parse().expect("addr"),
                SocketAddr::V6(_) => "[::]:0".parse().expect("addr"),
            }),
            &keypair,
            options.keep_alive,
        )?;
        let client = Self {
            endpoint,
            remote,
            server_name,
            options,
            connection: Mutex::new(None),
            reconnects: AtomicU64::new(0),
        };
        client.get_or_connect().await?;
        Ok(client)
    }

    async fn get_or_connect(&self) -> Result<Connection, Error> {
        let mut guard = self.connection.lock().await;
        if let Some(conn) = guard.as_ref()
            && conn.close_reason().is_none()
        {
            return Ok(conn.clone());
        }
        if guard.is_some() {
            self.reconnects.fetch_add(1, Ordering::Relaxed);
        }
        let connecting = self.endpoint.connect(self.remote, &self.server_name)?;
        let conn = tokio::time::timeout(self.options.connect_timeout, connecting)
            .await
            .map_err(|_| Error::Timeout)??;
        *guard = Some(conn.clone());
        Ok(conn)
    }

    pub fn health(&self) -> ConnectionHealth {
        match self.connection.try_lock() {
            Ok(g) if g.as_ref().is_some_and(|c| c.close_reason().is_none()) => {
                ConnectionHealth::Healthy
            }
            _ => ConnectionHealth::Closed,
        }
    }

    pub fn reconnects_total(&self) -> u64 {
        self.reconnects.load(Ordering::Relaxed)
    }

    pub const fn remote_addr(&self) -> SocketAddr {
        self.remote
    }

    /// Drop the current connection and open a new one.
    pub async fn reconnect(&self) -> Result<(), Error> {
        *self.connection.lock().await = None;
        self.get_or_connect().await.map(|_| ())
    }

    /// Serialize and send. Returns the transaction's first signature. No
    /// acknowledgement is read; use [`send_with_response`] for one.
    ///
    /// [`send_with_response`]: ApexSenderClient::send_with_response
    pub async fn send_transaction(&self, tx: &VersionedTransaction) -> Result<Signature, Error> {
        let signature = tx
            .signatures
            .first()
            .copied()
            .ok_or_else(|| Error::Serialize("no signature".into()))?;
        let wire = serialize_transaction(tx)?;
        self.send_transaction_bytes(Bytes::from(wire)).await?;
        Ok(signature)
    }

    /// Send pre-serialized transaction bytes. Zero-copy: the bytes are
    /// written after an 8-byte length prefix and before a 3-byte trailer.
    pub async fn send_transaction_bytes(&self, wire: Bytes) -> Result<(), Error> {
        if wire.len() > wire::MAX_TRANSACTION_SIZE {
            return Err(Error::TooLarge(wire.len()));
        }
        let (header, trailer) = wire::frame_parts(
            wire.len(),
            self.options.mev_protect,
            self.options.max_retries,
        );
        match self.write_uni(&header, &wire, &trailer).await {
            Ok(()) => Ok(()),
            Err(e) if self.options.auto_reconnect => {
                let _ = e;
                self.reconnect().await?;
                self.write_uni(&header, &wire, &trailer).await
            }
            Err(e) => Err(e),
        }
    }

    /// Serialize, send on a bidirectional stream and turn a rejection into
    /// [`Error::Rejected`]. Accepted means the endpoint has the transaction
    /// and is racing it to the leaders, not that it landed.
    pub async fn send_transaction_with_response(
        &self,
        tx: &VersionedTransaction,
    ) -> Result<Signature, Error> {
        let wire = serialize_transaction(tx)?;
        match self.send_with_response(Bytes::from(wire)).await? {
            Admission::Accepted(signature) => Ok(signature),
            Admission::Rejected { code, message } => Err(Error::Rejected { code, message }),
        }
    }

    async fn write_uni(&self, header: &[u8], wire: &[u8], trailer: &[u8]) -> Result<(), Error> {
        let conn = self.get_or_connect().await?;
        tokio::time::timeout(self.options.send_timeout, async {
            let mut stream = conn.open_uni().await?;
            stream
                .write_all_chunks(&mut [
                    Bytes::copy_from_slice(header),
                    Bytes::copy_from_slice(wire),
                    Bytes::copy_from_slice(trailer),
                ])
                .await?;
            stream.finish().map_err(|_| Error::Closed)?;
            Ok::<(), Error>(())
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    /// Send on a bidirectional stream and read the admission response.
    pub async fn send_with_response(&self, wire: Bytes) -> Result<Admission, Error> {
        if wire.len() > wire::MAX_TRANSACTION_SIZE {
            return Err(Error::TooLarge(wire.len()));
        }
        let (header, trailer) = wire::frame_parts(
            wire.len(),
            self.options.mev_protect,
            self.options.max_retries,
        );
        match self.write_bi(&header, wire.clone(), &trailer).await {
            Ok(a) => Ok(a),
            Err(Error::Rejected { code, message }) => Err(Error::Rejected { code, message }),
            Err(_) if self.options.auto_reconnect => {
                self.reconnect().await?;
                self.write_bi(&header, wire, &trailer).await
            }
            Err(e) => Err(e),
        }
    }

    async fn write_bi(
        &self,
        header: &[u8],
        wire: Bytes,
        trailer: &[u8],
    ) -> Result<Admission, Error> {
        let conn = self.get_or_connect().await?;
        tokio::time::timeout(self.options.send_timeout, async {
            let (mut send, mut recv) = conn.open_bi().await?;
            send.write_all_chunks(&mut [
                Bytes::copy_from_slice(header),
                wire,
                Bytes::copy_from_slice(trailer),
            ])
            .await?;
            send.finish().map_err(|_| Error::Closed)?;
            let frame = recv.read_to_end(wire::MAX_ADMISSION_FRAME).await?;
            wire::decode_admission(&frame).ok_or(Error::BadAdmission)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    pub fn close(self) {
        self.endpoint.close(0u32.into(), b"client closed");
    }
}

/// Serialize a transaction to its wire bytes. Uses the canonical encoding,
/// which is byte-identical to bincode for legacy and v0 and the only correct
/// one for v1 (SIMD-0385), whose envelope starts with the version prefix.
pub fn serialize_transaction(tx: &VersionedTransaction) -> Result<Vec<u8>, Error> {
    wincode::serialize(tx).map_err(|e| Error::Serialize(e.to_string()))
}

/// The ed25519 public key this API key's client certificate carries. What
/// the server stores at issuance.
pub fn client_pubkey(api_key: &str) -> solana_pubkey::Pubkey {
    use solana_signer::Signer;
    tls::derive_client_keypair(api_key).pubkey()
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_instruction::Instruction;
    use solana_keypair::Keypair;
    use solana_message::{Message, VersionedMessage, v1};
    use solana_pubkey::Pubkey;
    use solana_signer::Signer;
    use solana_transaction::Transaction;

    #[test]
    fn legacy_serialization_matches_bincode() {
        let payer = Keypair::new();
        let ix = Instruction::new_with_bytes(Pubkey::new_unique(), &[1, 2, 3], vec![]);
        let tx = VersionedTransaction::from(Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[&payer],
            solana_hash::Hash::new_from_array([7u8; 32]),
        ));
        assert_eq!(
            serialize_transaction(&tx).unwrap(),
            bincode::serialize(&tx).unwrap()
        );
    }

    #[test]
    fn v1_serialization_starts_with_the_version_prefix_and_fits_4096() {
        let payer = Keypair::new();
        let big = Instruction::new_with_bytes(Pubkey::new_unique(), &vec![9u8; 3_000], vec![]);
        let msg = v1::Message::try_compile(
            &payer.pubkey(),
            &[big],
            solana_hash::Hash::new_from_array([7u8; 32]),
        )
        .unwrap();
        let tx = VersionedTransaction::try_new(VersionedMessage::V1(msg), &[&payer]).unwrap();
        let wire = serialize_transaction(&tx).unwrap();
        assert_eq!(wire[0], v1::V1_PREFIX);
        assert!(
            wire.len() > 1232 && wire.len() <= wire::MAX_TRANSACTION_SIZE,
            "{}",
            wire.len()
        );
        let _ = Message::new(&[], None);
    }
}
