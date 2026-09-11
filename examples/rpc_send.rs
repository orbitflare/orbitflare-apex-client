//! JSON-RPC `sendTransaction` to the endpoint over HTTP. Same tip rule and
//! same routing as QUIC; a few hundred microseconds slower per send. Works
//! from any language and drops into code that already calls
//! `sendTransaction` on an RPC: swap the URL and add the `x-api-key` header.
//!
//! cargo run --example rpc_send --features rpc

#[path = "common/mod.rs"]
mod common;

use std::time::Instant;

use apex_sender_client::rpc::RpcClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let s = common::setup().await?;
    let rpc = RpcClient::with_url(s.rpc_url.clone(), &s.api_key);

    let tx = s.tipped_memo("apex rpc_send").await?;
    let wire = apex_sender_client::serialize_transaction(&tx)?;
    let sent_at = Instant::now();
    // mev_protect = false, max_retries = None (the endpoint's default).
    let signature = rpc.send_transaction(&wire, false, None).await?;
    println!("accepted in {} us", sent_at.elapsed().as_micros());
    s.report(&signature, sent_at).await
}
