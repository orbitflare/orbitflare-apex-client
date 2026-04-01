//! Send one tipped memo transaction through apex-sender.
//!
//! cargo run --example send_memo --features rpc -- <quic host:port> <rpc url> <api key> <keypair.json> <solana rpc url>

use std::str::FromStr;

use apex_sender_client::rpc::RpcClient;
use apex_sender_client::{ApexSenderClient, ClientOptions, MIN_TIP_LAMPORTS, tip, tip_instruction};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::read_keypair_file;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction::versioned::VersionedTransaction;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, quic, rpc_url, api_key, keypair, solana_rpc] = args.as_slice() else {
        eprintln!(
            "usage: send_memo <quic host:port> <sender rpc url> <api key> <keypair.json> <solana rpc url>"
        );
        std::process::exit(2);
    };
    let payer = read_keypair_file(keypair)?;

    let tip_accounts = RpcClient::with_url(rpc_url.clone(), api_key)
        .get_tip_accounts()
        .await?;
    let tip_account = tip::pick_tip_account(&tip_accounts).ok_or("no tip accounts")?;

    let blockhash = fetch_blockhash(solana_rpc).await?;
    // Unique memo, so two runs inside one blockhash do not share a signature.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let memo = Instruction::new_with_bytes(
        Pubkey::from_str("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr")?,
        format!("apex-sender-client {nonce}").as_bytes(),
        vec![AccountMeta::new(payer.pubkey(), true)],
    );
    let tx = Transaction::new_signed_with_payer(
        &[
            memo,
            tip_instruction(&payer.pubkey(), &tip_account, MIN_TIP_LAMPORTS),
        ],
        Some(&payer.pubkey()),
        &[&payer],
        blockhash,
    );
    let tx = VersionedTransaction::from(tx);

    let client = ApexSenderClient::connect_with_options(
        ClientOptions {
            endpoint: Some(quic.clone()),
            ..Default::default()
        },
        api_key,
    )
    .await?;
    let wire = bincode::serialize(&tx)?;
    let admission = client.send_with_response(wire.into()).await?;
    println!("{admission:?}");
    Ok(())
}

async fn fetch_blockhash(url: &str) -> Result<solana_hash::Hash, Box<dyn std::error::Error>> {
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"getLatestBlockhash","params":[{"commitment":"confirmed"}]});
    let v: serde_json::Value = reqwest::Client::new()
        .post(url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;
    let s = v["result"]["value"]["blockhash"]
        .as_str()
        .ok_or("no blockhash")?;
    Ok(solana_hash::Hash::from_str(s)?)
}
