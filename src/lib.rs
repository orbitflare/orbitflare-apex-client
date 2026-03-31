//! `apex-sender-client` submits Solana transactions to OrbitFlare's
//! apex-sender over QUIC.
//!
//! It keeps one persistent connection per PoP, authenticates with a client
//! certificate derived from your API key, and sends one serialized
//! transaction per unidirectional stream. A bidirectional variant returns a
//! compact admission response when you want to know the transaction was
//! accepted before it is raced to the leaders.
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

mod tls;
pub mod wire;
pub mod tip;
#[cfg(feature = "rpc")]
pub mod rpc;

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

/// Default QUIC ingress per PoP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Frankfurt,
    NewYork,
}

impl Region {
    pub const fn host(self) -> &'static str {
        match self {
            Region::Frankfurt => "fra.sender.orbitflare.com",
            Region::NewYork => "ny.sender.orbitflare.com",
        }
    }

    pub const QUIC_PORT: u16 = 7001;
    pub const RPC_PORT: u16 = 7000;

    pub fn quic_endpoint(self) -> String {
        format!("{}:{}", self.host(), Self::QUIC_PORT)
    }

    pub fn rpc_url(self) -> String {
        format!("http://{}:{}", self.host(), Self::RPC_PORT)
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
    #[error("transaction too large: {0} bytes (max 1232)")]
    TooLarge(usize),
    #[error("serialize transaction: {0}")]
    Serialize(String),
    #[error("malformed admission response")]
    BadAdmission,
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
    /// Retry budget handed to Apex boxes; `None` uses their default.
    pub max_retries: Option<u16>,
    /// Local UDP bind address; defaults to an ephemeral port.
    pub bind_addr: Option<SocketAddr>,
    pub connect_timeout: Duration,
    pub send_timeout: Duration,
    pub keep_alive: Duration,
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
        Self::connect_with_options(ClientOptions { endpoint: Some(region.quic_endpoint()), ..Default::default() }, api_key)
            .await
    }

    /// Connect to `options.endpoint` (`host:port`) with the certificate
    /// derived from `api_key`.
    pub async fn connect_with_options(options: ClientOptions, api_key: &str) -> Result<Self, Error> {
        let target = options.endpoint.clone().unwrap_or_else(|| Region::Frankfurt.quic_endpoint());
        let remote = tokio::net::lookup_host(&target)
            .await
            .map_err(|e| Error::Resolve(e.to_string()))?
            .next()
            .ok_or_else(|| Error::Resolve(format!("{target}: no address")))?;
        let server_name = target.rsplit_once(':').map_or(target.as_str(), |(h, _)| h).to_owned();
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
            Ok(g) if g.as_ref().is_some_and(|c| c.close_reason().is_none()) => ConnectionHealth::Healthy,
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
        let signature = tx.signatures.first().copied().ok_or_else(|| Error::Serialize("no signature".into()))?;
        let wire = bincode::serialize(tx).map_err(|e| Error::Serialize(e.to_string()))?;
        self.send_transaction_bytes(Bytes::from(wire)).await?;
        Ok(signature)
    }

    /// Send pre-serialized transaction bytes. Zero-copy: the bytes are
    /// written after an 8-byte length prefix and before a 3-byte trailer.
    pub async fn send_transaction_bytes(&self, wire: Bytes) -> Result<(), Error> {
        if wire.len() > wire::MAX_TRANSACTION_SIZE {
            return Err(Error::TooLarge(wire.len()));
        }
        let (header, trailer) = wire::frame_parts(wire.len(), self.options.mev_protect, self.options.max_retries);
        match self.write_uni(&header, &wire, &trailer).await {
            Ok(()) => Ok(()),
            Err(_) => {
                self.reconnect().await?;
                self.write_uni(&header, &wire, &trailer).await
            }
        }
    }

    async fn write_uni(&self, header: &[u8], wire: &[u8], trailer: &[u8]) -> Result<(), Error> {
        let conn = self.get_or_connect().await?;
        tokio::time::timeout(self.options.send_timeout, async {
            let mut stream = conn.open_uni().await?;
            stream.write_all_chunks(&mut [Bytes::copy_from_slice(header), Bytes::copy_from_slice(wire), Bytes::copy_from_slice(trailer)]).await?;
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
        let (header, trailer) = wire::frame_parts(wire.len(), self.options.mev_protect, self.options.max_retries);
        let conn = self.get_or_connect().await?;
        tokio::time::timeout(self.options.send_timeout, async {
            let (mut send, mut recv) = conn.open_bi().await?;
            send.write_all_chunks(&mut [Bytes::copy_from_slice(&header), wire, Bytes::copy_from_slice(&trailer)]).await?;
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

/// The ed25519 public key this API key's client certificate carries. What
/// the server stores at issuance.
pub fn client_pubkey(api_key: &str) -> solana_pubkey::Pubkey {
    use solana_signer::Signer;
    tls::derive_client_keypair(api_key).pubkey()
}
