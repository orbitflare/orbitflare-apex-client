//! Binary path: you already hold serialized transaction bytes (from your own
//! signer, another process, or a file) and want them on the wire with no
//! re-serialization. `send_transaction_bytes` writes the bytes between an
//! 8-byte length prefix and a 3-byte trailer, nothing else touches them.
//!
//! Also prints the exact packet bytes so an implementation in another
//! language can be checked against the Apex docs at https://docs.orbitflare.com/apex.
//!
//! cargo run --example raw_bytes --features rpc

#[path = "common/mod.rs"]
mod common;

use std::time::Instant;

use bytes::Bytes;
use orbitflare_apex::{ApexSenderClient, ClientOptions, wire};

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

    let tx = s.tipped_memo("apex raw_bytes").await?;
    let signature = tx.signatures[0];
    // Wherever your bytes come from: this is bincode of a VersionedTransaction,
    // the same encoding `solana-transaction` and every wallet produce.
    let wire_bytes: Bytes = orbitflare_apex::serialize_transaction(&tx)?.into();

    let packet = wire::encode_packet(&wire_bytes, false, None);
    println!(
        "packet: {} bytes = 8 (len) + {} (tx) + 3 (flags)",
        packet.len(),
        wire_bytes.len()
    );
    println!("first 16 bytes: {:02x?}", &packet[..16]);

    let sent_at = Instant::now();
    client.send_transaction_bytes(wire_bytes).await?;
    println!("sent in {} us", sent_at.elapsed().as_micros());
    s.report(&signature.to_string(), sent_at).await
}
