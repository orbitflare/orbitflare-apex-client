//! Print the certificate public key an API key derives to. It must match
//! the "client pubkey" shown next to the key on your dashboard; if it does
//! not, the key was copied wrong. Never prints the key itself.
//!
//! APEX_API_KEY=... cargo run --example client_pubkey

use orbitflare_apex::client_pubkey;

fn main() {
    let key = std::env::var("APEX_API_KEY").unwrap_or_default();
    if key.is_empty() {
        eprintln!("APEX_API_KEY is required");
        std::process::exit(2);
    }
    println!("{}", client_pubkey(&key));
}
