//! A warm connection under load: N transactions sent concurrently over one
//! QUIC connection, per-send latency, then confirmation of each. Shows what
//! a bot loop looks like: share one client, spawn a task per send, never
//! reconnect by hand.
//!
//! cargo run --example throughput --features rpc -- 20

#[path = "common/mod.rs"]
mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use orbitflare_apex::{ApexSenderClient, ClientOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let count: usize = std::env::args()
        .nth(1)
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(10);
    let s = Arc::new(common::setup().await?);
    let client = Arc::new(
        ApexSenderClient::connect_with_options(
            ClientOptions {
                endpoint: Some(s.quic.clone()),
                ..Default::default()
            },
            &s.api_key,
        )
        .await?,
    );

    // One blockhash for the batch; build everything before the clock starts.
    let blockhash = s.solana.latest_blockhash().await?;
    let txs: Vec<_> = (0..count)
        .map(|i| s.tipped_memo_with(&format!("apex throughput {i}"), blockhash.clone()))
        .collect::<Result<_, _>>()?;

    let started = Instant::now();
    let sends = txs.iter().map(|tx| {
        let client = Arc::clone(&client);
        let tx = tx.clone();
        tokio::spawn(async move {
            let t = Instant::now();
            let r = client.send_transaction(&tx).await;
            (r, t.elapsed())
        })
    });
    let mut signatures = Vec::with_capacity(count);
    let mut latencies = Vec::with_capacity(count);
    for h in sends {
        let (r, elapsed) = h.await?;
        signatures.push(r?);
        latencies.push(elapsed.as_micros() as u64);
    }
    latencies.sort_unstable();
    println!(
        "{count} sends in {} ms: per-send p50 {} us, p99 {} us, reconnects {}",
        started.elapsed().as_millis(),
        latencies[count / 2],
        latencies[(count * 99 / 100).min(count - 1)],
        client.reconnects_total()
    );

    let mut landed = 0;
    for sig in &signatures {
        if s.solana
            .confirm(&sig.to_string(), Duration::from_secs(30))
            .await?
            .is_some()
        {
            landed += 1;
        }
    }
    println!("landed {landed}/{count} within 30 s");
    Ok(())
}
