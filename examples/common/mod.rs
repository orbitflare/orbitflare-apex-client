//! Shared setup for the examples: configuration from the environment and a
//! tipped, compute-budgeted memo transaction.
//!
//! ```
//! APEX_API_KEY=...            your key
//! APEX_REGION=fra|nyc         which endpoint (default fra)
//! APEX_QUIC=host:port         override the QUIC address (optional)
//! APEX_RPC=http://host:port   override the endpoint's JSON-RPC (optional)
//! KEYPAIR_PATH=payer.json     fee payer and tip funder
//! SOLANA_RPC_URL=...          any Solana RPC for blockhash and confirmation
//! TIP_LAMPORTS=1000000        default: the standard floor
//! APEX_TX_VERSION=legacy|v1   message format (default legacy)
//! ```
#![allow(dead_code)]

use std::str::FromStr;
use std::time::Duration;

use apex_sender_client::rpc::{RpcClient, SolanaRpc};
use apex_sender_client::{MIN_TIP_LAMPORTS, Region, tip, tip_instruction};
use solana_hash::Hash;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::{Keypair, read_keypair_file};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction::versioned::VersionedTransaction;

pub const MEMO_PROGRAM: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";

pub struct Setup {
    pub api_key: String,
    pub region: Region,
    pub quic: String,
    pub rpc_url: String,
    pub payer: Keypair,
    pub solana: SolanaRpc,
    pub tip_lamports: u64,
    pub tip_accounts: Vec<Pubkey>,
    pub v1: bool,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub async fn setup() -> Result<Setup, Box<dyn std::error::Error>> {
    let api_key = env("APEX_API_KEY").ok_or("APEX_API_KEY is required")?;
    let region = env("APEX_REGION")
        .map(|r| Region::parse(&r).ok_or(format!("unknown APEX_REGION {r}")))
        .transpose()?
        .unwrap_or(Region::Frankfurt);
    let quic = env("APEX_QUIC").unwrap_or_else(|| region.quic_endpoint());
    let rpc_url = env("APEX_RPC").unwrap_or_else(|| region.rpc_url());
    let payer = read_keypair_file(env("KEYPAIR_PATH").unwrap_or_else(|| "payer.json".into()))?;
    let solana = SolanaRpc::new(
        env("SOLANA_RPC_URL").unwrap_or_else(|| "https://api.mainnet-beta.solana.com".into()),
    );
    let tip_lamports = env("TIP_LAMPORTS")
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(MIN_TIP_LAMPORTS);
    let tip_accounts = RpcClient::with_url(rpc_url.clone(), &api_key)
        .get_tip_accounts()
        .await?;
    if tip_accounts.is_empty() {
        return Err("the endpoint published no tip accounts".into());
    }
    let v1 = env("APEX_TX_VERSION").is_some_and(|v| v.eq_ignore_ascii_case("v1"));
    Ok(Setup {
        api_key,
        region,
        quic,
        rpc_url,
        payer,
        solana,
        tip_lamports,
        tip_accounts,
        v1,
    })
}

impl Setup {
    /// A memo transaction with a compute budget and the tip, signed by the
    /// payer, unique per call so repeated runs never share a signature.
    pub async fn tipped_memo(
        &self,
        label: &str,
    ) -> Result<VersionedTransaction, Box<dyn std::error::Error>> {
        let blockhash = self.solana.latest_blockhash().await?;
        self.tipped_memo_with(label, blockhash)
    }

    pub fn tipped_memo_with(
        &self,
        label: &str,
        blockhash: Hash,
    ) -> Result<VersionedTransaction, Box<dyn std::error::Error>> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let memo = Instruction::new_with_bytes(
            Pubkey::from_str(MEMO_PROGRAM)?,
            format!("{label} {nonce}").as_bytes(),
            vec![AccountMeta::new(self.payer.pubkey(), true)],
        );
        let tip_account = tip::pick_tip_account(&self.tip_accounts).ok_or("no tip accounts")?;
        let ixs = [
            solana_compute_budget_interface::ComputeBudgetInstruction::set_compute_unit_limit(
                100_000,
            ),
            solana_compute_budget_interface::ComputeBudgetInstruction::set_compute_unit_price(
                tip::DEFAULT_COMPUTE_UNIT_PRICE_MICRO_LAMPORTS,
            ),
            memo,
            tip_instruction(&self.payer.pubkey(), &tip_account, self.tip_lamports),
        ];
        let tx = Transaction::new_signed_with_payer(
            &ixs,
            Some(&self.payer.pubkey()),
            &[&self.payer],
            blockhash,
        );
        Ok(VersionedTransaction::from(tx))
    }

    /// Wait for the signature and print where it landed.
    pub async fn report(
        &self,
        signature: &str,
        sent_at: std::time::Instant,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match self
            .solana
            .confirm(signature, Duration::from_secs(30))
            .await?
        {
            Some(slot) => println!(
                "landed in slot {slot} after {} ms: {signature}",
                sent_at.elapsed().as_millis()
            ),
            None => println!("not confirmed within 30 s: {signature}"),
        }
        Ok(())
    }
}
