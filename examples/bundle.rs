//! An atomic bundle: two transactions that land in order, both or neither.
//! The first is a plain memo, the second carries the tip for the bundle.
//! Bundles use the block-engine path only, so expect them to land on
//! Jito-enabled leaders.
//!
//! cargo run --example bundle --features rpc

#[path = "common/mod.rs"]
mod common;

use std::str::FromStr;
use std::time::{Duration, Instant};

use apex_sender_client::prelude::*;
use apex_sender_client::rpc::{BundleState, RpcClient};
use apex_sender_client::serialize_transaction;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let s = common::setup().await?;
    let rpc = RpcClient::with_url(s.rpc_url.clone(), &s.api_key);
    let blockhash = s.solana.latest_blockhash().await?;

    // First member: no tip.
    let memo = Instruction::new_with_bytes(
        Pubkey::from_str(common::MEMO_PROGRAM)?,
        format!("apex bundle first {}", std::process::id()).as_bytes(),
        vec![AccountMeta::new(s.payer.pubkey(), true)],
    );
    let first = VersionedTransaction::from(Transaction::new_signed_with_payer(
        &[memo],
        Some(&s.payer.pubkey()),
        &[&s.payer],
        blockhash.clone(),
    ));
    // Second member: a memo plus the bundle's tip.
    let second = s.tipped_memo_with("apex bundle second", blockhash)?;

    let wires = [
        serialize_transaction(&first)?,
        serialize_transaction(&second)?,
    ];
    let started = Instant::now();
    let accepted = rpc.send_bundle(&[&wires[0], &wires[1]]).await?;
    println!(
        "bundle {} accepted in {} us",
        accepted.bundle_id,
        started.elapsed().as_micros()
    );

    for _ in 0..60 {
        let status = rpc.bundle_statuses(&[&accepted.bundle_id]).await?;
        match status.first().map(|st| (st.state, st.landed_slot)) {
            Some((BundleState::Landed, slot)) => {
                println!(
                    "landed in slot {slot:?} after {} ms",
                    started.elapsed().as_millis()
                );
                for sig in &accepted.signatures {
                    println!("  {sig}");
                }
                return Ok(());
            }
            Some((BundleState::Failed | BundleState::Invalid, _)) => {
                println!("bundle did not land (no Jito leader before the blockhash expired)");
                return Ok(());
            }
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    println!("still pending after 30 s");
    Ok(())
}
