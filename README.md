# apex-sender-client

`apex-sender-client` submits Solana transactions to OrbitFlare's apex-sender
over QUIC. One persistent connection per PoP, authenticated by a client
certificate derived from your API key, one serialized transaction per
stream, 0-RTT resumption, and an optional admission response.

## Installation

```toml
[dependencies]
apex-sender-client = "0.1"
bytes = "1"          # for send_transaction_bytes / send_with_response
```

Enable the `rpc` feature for `getTipAccounts` and an HTTP fallback.

## API at a glance

| API | Purpose |
|---|---|
| `ApexSenderClient::connect(region, api_key)` | Connect to a PoP with an ephemeral local port |
| `ApexSenderClient::connect_with_options(opts, api_key)` | Custom endpoint, bind address, timeouts, `mev_protect`, `max_retries` |
| `client.send_transaction(&tx)` | Serialize a `VersionedTransaction` and send it; returns its signature |
| `client.send_transaction_bytes(bytes)` | Send pre-serialized bytes, no allocation of the payload |
| `client.send_with_response(bytes)` | Send on a bidi stream and read the admission response |
| `client.health()`, `client.reconnects_total()` | Connection state |
| `client.reconnect()`, `client.close()` | Lifecycle |
| `tip_instruction(payer, tip_account, lamports)` | The SystemProgram transfer apex-sender requires |
| `rpc::fetch_tip_accounts(region, api_key)` | The PoP's tip accounts (`rpc` feature) |
| `client_pubkey(api_key)` | The certificate key your API key derives to |

## Sending a transaction

```rust
use apex_sender_client::{ApexSenderClient, Region, MIN_TIP_LAMPORTS, tip_instruction};

let client = ApexSenderClient::connect(Region::Frankfurt, &api_key).await?;
let tip_accounts = apex_sender_client::rpc::fetch_tip_accounts(Region::Frankfurt, &api_key).await?;
let tip = tip_instruction(&payer.pubkey(), &tip_accounts[0], MIN_TIP_LAMPORTS);
// ... build and sign a transaction that includes `tip` as a top-level instruction
let signature = client.send_transaction(&tx).await?;
```

Every transaction needs one top-level SystemProgram transfer to one of the
published tip accounts, at or above your tier's floor (`MIN_TIP_LAMPORTS`
for the standard tier). Put the tip account in the static account keys, not
in a lookup table.

## What it does not do

- build or sign transactions for you
- simulate or preflight
- wait for confirmation (use `getSignatureStatuses` on any RPC)

the Apex docs at https://docs.orbitflare.com/apex describes the wire format for other languages.

## Example

```
cargo run --example send_memo --features rpc -- fra.sender.orbitflare.com:7001 http://fra.sender.orbitflare.com:7000 <api key> payer.json https://api.mainnet-beta.solana.com
```

## License

Apache-2.0
