//! Optional JSON-RPC helpers (feature `rpc`): fetch the published tip
//! accounts and send over HTTP when QUIC is not an option.

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
