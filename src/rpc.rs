//! Optional JSON-RPC helpers (feature `rpc`): the endpoint's own JSON-RPC
//! (`sendTransaction`, `getTipAccounts`) and the two calls every sender
//! needs from a Solana RPC: a recent blockhash and confirmation.

use std::time::Duration;

use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use serde_json::{Value, json};
use solana_pubkey::Pubkey;

use crate::Region;

#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("rpc error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("bad response")]
    BadResponse,
}

pub struct RpcClient {
    http: reqwest::Client,
    url: String,
    api_key: String,
}

impl RpcClient {
    pub fn new(region: Region, api_key: &str) -> Self {
        Self::with_url(region.rpc_url(), api_key)
    }

    pub fn with_url(url: impl Into<String>, api_key: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("reqwest client"),
            url: url.into(),
            api_key: api_key.to_owned(),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let resp: Value = self
            .http
            .post(&self.url)
            .header("x-api-key", &self.api_key)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        if let Some(err) = resp.get("error") {
            return Err(RpcError::Rpc {
                code: err.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            });
        }
        resp.get("result").cloned().ok_or(RpcError::BadResponse)
    }

    /// The tip accounts this Apex endpoint accepts.
    pub async fn get_tip_accounts(&self) -> Result<Vec<Pubkey>, RpcError> {
        let v = self.call("getTipAccounts", json!([])).await?;
        v.as_array()
            .ok_or(RpcError::BadResponse)?
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.parse().map_err(|_| RpcError::BadResponse))
            .collect()
    }

    /// `sendTransaction` over HTTP. Returns the signature string.
    pub async fn send_transaction(
        &self,
        wire: &[u8],
        mev_protect: bool,
        max_retries: Option<u16>,
    ) -> Result<String, RpcError> {
        let mut config = json!({ "encoding": "base64" });
        if let Some(n) = max_retries {
            config["maxRetries"] = json!(n);
        }
        let v = self
            .call(
                "sendTransaction",
                json!([BASE64_STANDARD.encode(wire), config, mev_protect]),
            )
            .await?;
        v.as_str().map(str::to_owned).ok_or(RpcError::BadResponse)
    }
}

/// One frame of a `/send-batch` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchItem {
    Accepted(String),
    Rejected { error: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchResult {
    pub attempted: usize,
    pub accepted: usize,
    pub rejected: usize,
    pub results: Vec<BatchItem>,
}

/// Up to this many transactions per `send_batch`.
pub const MAX_BATCH: usize = 16;

impl RpcClient {
    fn plain_url(&self, route: &str, mev_protect: bool, max_retries: Option<u16>) -> String {
        let mut url = format!(
            "{}{route}?mev_protect={}",
            self.url.trim_end_matches('/'),
            u8::from(mev_protect)
        );
        if let Some(n) = max_retries {
            url.push_str(&format!("&max_retries={n}"));
        }
        url
    }

    fn plain_error(v: &Value) -> RpcError {
        RpcError::Rpc {
            code: 0,
            message: format!(
                "{}: {}",
                v["error"].as_str().unwrap_or("error"),
                v["message"].as_str().unwrap_or("")
            ),
        }
    }

    /// `POST /send-bin`: the raw transaction bytes, no base64 and no JSON on
    /// the way in. The cheapest HTTP path. Returns the signature.
    pub async fn send_transaction_binary(
        &self,
        wire: &[u8],
        mev_protect: bool,
        max_retries: Option<u16>,
    ) -> Result<String, RpcError> {
        let resp = self
            .http
            .post(self.plain_url("/send-bin", mev_protect, max_retries))
            .header("x-api-key", &self.api_key)
            .header("content-type", "application/octet-stream")
            .body(wire.to_vec())
            .send()
            .await?;
        let v: Value = resp.json().await?;
        match v["signature"].as_str() {
            Some(sig) => Ok(sig.to_owned()),
            None => Err(Self::plain_error(&v)),
        }
    }

    /// `POST /send-batch`: up to [`MAX_BATCH`] transactions in one request,
    /// each framed as a big-endian u16 length and the bytes. Every
    /// transaction is admitted on its own; the reply says which were.
    pub async fn send_batch(
        &self,
        wires: &[&[u8]],
        mev_protect: bool,
        max_retries: Option<u16>,
    ) -> Result<BatchResult, RpcError> {
        let body = encode_batch(wires).ok_or(RpcError::BadResponse)?;
        let resp = self
            .http
            .post(self.plain_url("/send-batch", mev_protect, max_retries))
            .header("x-api-key", &self.api_key)
            .header("content-type", "application/octet-stream")
            .body(body)
            .send()
            .await?;
        let v: Value = resp.json().await?;
        let Some(results) = v["results"].as_array() else {
            return Err(Self::plain_error(&v));
        };
        Ok(BatchResult {
            attempted: v["attempted"].as_u64().unwrap_or(0) as usize,
            accepted: v["accepted"].as_u64().unwrap_or(0) as usize,
            rejected: v["rejected"].as_u64().unwrap_or(0) as usize,
            results: results
                .iter()
                .map(|r| match r["signature"].as_str() {
                    Some(sig) => BatchItem::Accepted(sig.to_owned()),
                    None => BatchItem::Rejected {
                        error: r["error"].as_str().unwrap_or("error").to_owned(),
                        message: r["message"].as_str().unwrap_or("").to_owned(),
                    },
                })
                .collect(),
        })
    }

    /// `GET /ping`: warms the HTTP connection and proves the endpoint is up.
    /// Needs no key.
    pub async fn ping(&self) -> Result<(), RpcError> {
        let text = self
            .http
            .get(format!("{}/ping", self.url.trim_end_matches('/')))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        if text == "pong" {
            Ok(())
        } else {
            Err(RpcError::BadResponse)
        }
    }
}

/// What the endpoint returns for an accepted bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleAccepted {
    pub bundle_id: String,
    pub signatures: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleState {
    Pending,
    Landed,
    Failed,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleStatus {
    pub bundle_id: String,
    pub state: BundleState,
    pub landed_slot: Option<u64>,
}

/// Up to this many transactions per bundle, each at most 1232 bytes.
pub const MAX_BUNDLE: usize = 4;

impl RpcClient {
    /// `POST /send-bundle`: one to four transactions that land in order, all
    /// or nothing. Exactly one of them carries the tip. Bundles travel the
    /// block-engine path only, so they land on Jito-enabled leaders; the
    /// endpoint resubmits until the bundle lands or the first transaction's
    /// blockhash expires.
    pub async fn send_bundle(&self, wires: &[&[u8]]) -> Result<BundleAccepted, RpcError> {
        if wires.is_empty()
            || wires.len() > MAX_BUNDLE
            || wires.iter().any(|w| w.is_empty() || w.len() > 1232)
        {
            return Err(RpcError::BadResponse);
        }
        let mut body = Vec::with_capacity(wires.iter().map(|w| w.len() + 2).sum());
        for w in wires {
            body.extend_from_slice(&(w.len() as u16).to_be_bytes());
            body.extend_from_slice(w);
        }
        let resp = self
            .http
            .post(format!("{}/send-bundle", self.url.trim_end_matches('/')))
            .header("x-api-key", &self.api_key)
            .header("content-type", "application/octet-stream")
            .body(body)
            .send()
            .await?;
        let v: Value = resp.json().await?;
        let Some(bundle_id) = v["bundle_id"].as_str() else {
            return Err(Self::plain_error(&v));
        };
        Ok(BundleAccepted {
            bundle_id: bundle_id.to_owned(),
            signatures: v["signatures"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// `getInflightBundleStatuses` for ids returned by [`send_bundle`].
    ///
    /// [`send_bundle`]: RpcClient::send_bundle
    pub async fn bundle_statuses(
        &self,
        bundle_ids: &[&str],
    ) -> Result<Vec<BundleStatus>, RpcError> {
        let v = self
            .call("getInflightBundleStatuses", json!([bundle_ids]))
            .await?;
        Ok(v["value"]
            .as_array()
            .ok_or(RpcError::BadResponse)?
            .iter()
            .map(|s| BundleStatus {
                bundle_id: s["bundle_id"].as_str().unwrap_or("").to_owned(),
                state: match s["status"].as_str() {
                    Some("Landed") => BundleState::Landed,
                    Some("Failed") => BundleState::Failed,
                    Some("Pending") => BundleState::Pending,
                    _ => BundleState::Invalid,
                },
                landed_slot: s["landed_slot"].as_u64(),
            })
            .collect())
    }
}

/// Frame transactions for `/send-batch`: `u16 BE length + bytes` each.
/// `None` when empty, over [`MAX_BATCH`], or a transaction exceeds 4096 bytes.
pub fn encode_batch(wires: &[&[u8]]) -> Option<Vec<u8>> {
    if wires.is_empty() || wires.len() > MAX_BATCH {
        return None;
    }
    let mut out = Vec::with_capacity(wires.iter().map(|w| w.len() + 2).sum());
    for w in wires {
        if w.is_empty() || w.len() > crate::wire::MAX_TRANSACTION_SIZE {
            return None;
        }
        out.extend_from_slice(&(w.len() as u16).to_be_bytes());
        out.extend_from_slice(w);
    }
    Some(out)
}

/// Convenience: fetch the tip accounts for a region.
pub async fn fetch_tip_accounts(region: Region, api_key: &str) -> Result<Vec<Pubkey>, RpcError> {
    RpcClient::new(region, api_key).get_tip_accounts().await
}

/// The OrbitFlare tip vault program. Tip accounts are its vault accounts:
/// 41 bytes, tagged `APEXVLT1`.
pub const TIP_PROGRAM_ID: &str = "9ig7pd4gqe2m16ACGPbPo4HfMGD3ba38poDhXEayx7EF";
const VAULT_TAG_BASE64: &str = "QVBFWFZMVDE=";

/// Read the tip vaults straight from chain through any Solana RPC, for
/// callers who would rather not ask the sender. Same set as `getTipAccounts`.
pub async fn fetch_vaults(solana_rpc_url: &str) -> Result<Vec<Pubkey>, RpcError> {
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "getProgramAccounts",
        "params": [TIP_PROGRAM_ID, {
            "encoding": "base64",
            "filters": [{ "dataSize": 41 }, { "memcmp": { "offset": 0, "bytes": VAULT_TAG_BASE64, "encoding": "base64" } }]
        }]
    });
    let resp: Value = reqwest::Client::new()
        .post(solana_rpc_url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;
    if let Some(err) = resp.get("error") {
        return Err(RpcError::Rpc {
            code: err.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        });
    }
    let mut vaults: Vec<Pubkey> = resp["result"]
        .as_array()
        .ok_or(RpcError::BadResponse)?
        .iter()
        .filter_map(|row| row["pubkey"].as_str()?.parse().ok())
        .collect();
    vaults.sort();
    Ok(vaults)
}

/// Two calls to any Solana RPC that every sender needs: a blockhash to build
/// with and confirmation afterwards. Apex endpoints do not answer these; use
/// your own RPC or a public one.
pub struct SolanaRpc {
    http: reqwest::Client,
    url: String,
}

impl SolanaRpc {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
            url: url.into(),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let resp: Value = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        if let Some(err) = resp.get("error") {
            return Err(RpcError::Rpc {
                code: err.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            });
        }
        resp.get("result").cloned().ok_or(RpcError::BadResponse)
    }

    /// `getLatestBlockhash` at `confirmed`.
    pub async fn latest_blockhash(&self) -> Result<solana_hash::Hash, RpcError> {
        let v = self
            .call("getLatestBlockhash", json!([{ "commitment": "confirmed" }]))
            .await?;
        v["value"]["blockhash"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or(RpcError::BadResponse)
    }

    /// Poll `getSignatureStatuses` until the transaction is confirmed or
    /// `timeout` passes. Returns the slot it landed in, or `None` on timeout.
    /// An on-chain execution error is returned as [`RpcError::Rpc`].
    pub async fn confirm(
        &self,
        signature: &str,
        timeout: Duration,
    ) -> Result<Option<u64>, RpcError> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let v = self
                .call(
                    "getSignatureStatuses",
                    json!([[signature], { "searchTransactionHistory": false }]),
                )
                .await?;
            if let Some(status) = v["value"].get(0).filter(|s| !s.is_null()) {
                if let Some(err) = status.get("err").filter(|e| !e.is_null()) {
                    return Err(RpcError::Rpc {
                        code: 0,
                        message: format!("transaction failed on chain: {err}"),
                    });
                }
                let confirmed = status["confirmationStatus"]
                    .as_str()
                    .is_some_and(|c| c == "confirmed" || c == "finalized");
                if confirmed {
                    return Ok(status["slot"].as_u64());
                }
            }
            if std::time::Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_frames_are_length_prefixed_big_endian() {
        let b = encode_batch(&[&[1, 2, 3], &[9]]).unwrap();
        assert_eq!(b, vec![0, 3, 1, 2, 3, 0, 1, 9]);
        assert!(encode_batch(&[]).is_none());
        let too_many: Vec<&[u8]> = vec![&[1u8][..]; MAX_BATCH + 1];
        assert!(encode_batch(&too_many).is_none());
    }
}
