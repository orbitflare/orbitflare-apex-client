//! Same QUIC connection, but on a bidirectional stream: the endpoint answers
//! with accepted or a rejection code (no tip, tip below your floor, rate
//! limited, invalid) before racing the transaction. A few hundred
//! microseconds slower than the unidirectional path; use it while
//! integrating, or when you need the rejection reason inline.
//!
//! cargo run --example quic_send_with_response --features rpc

#[path = "common/mod.rs"]
mod common;

use std::time::Instant;

use orbitflare_apex::{ApexSenderClient, ClientOptions, Error};

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

    let tx = s.tipped_memo("apex quic_send_with_response").await?;
    let sent_at = Instant::now();
    match client.send_transaction_with_response(&tx).await {
        Ok(signature) => {
            println!("accepted in {} us", sent_at.elapsed().as_micros());
            s.report(&signature.to_string(), sent_at).await?;
        }
        Err(Error::Rejected { code, message }) => {
            // Fix the transaction, not the client: the message names the
            // floor when the tip is short.
            println!("rejected: {code:?}: {message}");
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
