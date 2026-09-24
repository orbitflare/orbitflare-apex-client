//! Lowest latency: one persistent QUIC connection, one unidirectional stream
//! per transaction, no acknowledgement. Confirmation comes from a Solana RPC.
//!
//! cargo run --example quic_send --features rpc

#[path = "common/mod.rs"]
mod common;

use std::time::Instant;

use orbitflare_apex::{ApexSenderClient, ClientOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let s = common::setup().await?;
    let client = ApexSenderClient::connect_with_options(
        ClientOptions {
            endpoint: Some(s.quic.clone()),
            ..Default::default()
        },
        &s.api_key,
    )
    .await?;
    println!(
        "connected to {} ({})",
        client.remote_addr(),
        s.region.code()
    );

    let tx = s.tipped_memo("apex quic_send").await?;
    let sent_at = Instant::now();
    let signature = client.send_transaction(&tx).await?;
    println!("sent in {} us", sent_at.elapsed().as_micros());
    s.report(&signature.to_string(), sent_at).await
}
