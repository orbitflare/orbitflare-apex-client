# apex-sender-client

Submit Solana transactions to OrbitFlare's Apex endpoints. One tip, three
transports, and the endpoint races every transaction to the leaders through
stake-weighted validator clients, Jito bundles and the leader TPUs at once.

- **QUIC**: one persistent connection per endpoint, a client certificate
  derived from your API key (the key itself never crosses the wire), one
  serialized transaction per stream, 0-RTT reconnects. Fire-and-forget on a
  unidirectional stream, or a bidirectional stream for an inline
  accepted/rejected answer.
- **JSON-RPC**: Solana's `sendTransaction` with an `x-api-key` header, for
  existing code and other languages.
- **Plain HTTP**: `POST /send-bin` with the raw transaction bytes (no
  base64, no JSON), `POST /send-batch` for up to 16 at once, `GET /ping`
  to keep a connection warm, CORS on everything for browsers.
- **Raw bytes**: hand the client pre-serialized transaction bytes and nothing
  re-encodes them.

The tip is an instruction inside your transaction, so it is only paid when
the transaction lands. No landing, no cost.

## Quick start

```toml
[dependencies]
apex-sender-client = { version = "0.1", features = ["rpc"] }
```

```rust
use apex_sender_client::{ApexSenderClient, Region, MIN_TIP_LAMPORTS, tip, tip_instruction};
use apex_sender_client::rpc::{RpcClient, SolanaRpc};

let client = ApexSenderClient::connect(Region::Frankfurt, &api_key).await?;
let tip_accounts = RpcClient::new(Region::Frankfurt, &api_key).get_tip_accounts().await?;
let tip_account = tip::pick_tip_account(&tip_accounts).unwrap();

// Your instructions, plus the tip as a top-level instruction.
let ixs = [
    your_instruction,
    tip_instruction(&payer.pubkey(), &tip_account, MIN_TIP_LAMPORTS),
];
let blockhash = SolanaRpc::new(solana_rpc_url).latest_blockhash().await?;
let tx = VersionedTransaction::from(Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[&payer], blockhash));

let signature = client.send_transaction(&tx).await?;          // microseconds, no ack
let slot = SolanaRpc::new(solana_rpc_url).confirm(&signature.to_string(), Duration::from_secs(30)).await?;
```

The `rpc` feature adds the HTTP helpers (`getTipAccounts`, JSON-RPC
`sendTransaction`, blockhash and confirmation against any Solana RPC). The
QUIC path alone has no HTTP dependency.

## Endpoints

| Region | Code | Host |
|---|---|---|
| 🇩🇪 Frankfurt | `fra` | `fra.apex.orbitflare.com` |
| 🇳🇱 Amsterdam | `ams` | `ams.apex.orbitflare.com` |
| 🇬🇧 London | `lon` | `lon.apex.orbitflare.com` |
| 🇺🇸 New York | `nyc` | `nyc.apex.orbitflare.com` |
| 🇺🇸 Salt Lake City | `slc` | `slc.apex.orbitflare.com` |
| 🇸🇬 Singapore | `sgp` | `sgp.apex.orbitflare.com` |
| 🇯🇵 Tokyo | `tyo` | `tyo.apex.orbitflare.com` |
| 🇱🇹 Siauliai | `sqq` | `sqq.apex.orbitflare.com` |
| 🌐 Global (nearest) | `global` | `global.apex.orbitflare.com` |

JSON-RPC and the plain HTTP routes are on port 80; QUIC is UDP 7001.

Pick the endpoint nearest to you, or `Region::Global`, which resolves to
the nearest one. Every endpoint knows the full leader schedule and routes
each transaction to the validator clients nearest the upcoming leaders, so
the choice affects your round trip, not the landing path. Use a named
region when you need a fixed host, for a firewall rule or a pinned round
trip. `Region::parse("fra")` and `Region::code()` map to and from the short
codes.

## Transports

| | QUIC uni | QUIC bidi | HTTP binary | JSON-RPC |
|---|---|---|---|---|
| Call | `send_transaction`, `send_transaction_bytes` | `send_transaction_with_response`, `send_with_response` | `rpc::RpcClient::send_transaction_binary`, `send_batch` | `rpc::RpcClient::send_transaction` |
| Returns | the signature; nothing is read back | accepted, or a rejection code and message | the signature, or a JSON error with a label | the signature, or a JSON-RPC error |
| Cost per send | one stream on a warm connection | one stream plus one round trip | an HTTP request, raw bytes, no encoding | an HTTP request, base64 in JSON |
| Best for | bots on a persistent connection | integration and debugging, batch tools that want the reason inline | any language over HTTP, batches, browsers | drop-in for `sendTransaction` code |

The transport does not change priority or routing; the tip does.

A send that fails because the connection is gone reconnects (0-RTT when a
session ticket is cached) and retries once; set
`ClientOptions::auto_reconnect` to `false` to handle it yourself.
`health()` and `reconnects_total()` expose the state for your metrics.

## Tips

Every transaction carries exactly one top-level SystemProgram transfer to
one of the published tip accounts, funded by a signer, with the tip account
in the static account keys (not through a lookup table). The floor depends
on your key's tier; the standard tier is `MIN_TIP_LAMPORTS` (0.001 SOL). A
transaction without a tip, or below your floor, is rejected before it is
sent anywhere; the bidirectional stream and JSON-RPC tell you which.

The tip accounts are vaults of OrbitFlare's on-chain tip program
(`rpc::TIP_PROGRAM_ID`); only the program can debit them. When the Jito
path is the one that lands, the tip minus the base fee is bid to Jito in
the same bundle as your transaction; when a stake path lands first, the
tip stays with OrbitFlare and pays the validators whose stake carried it.
`rpc::fetch_vaults(solana_rpc_url)` reads the vault list from any Solana RPC
without calling the endpoint.

Priority fee: the endpoint gets your transaction to the leader; the leader's
scheduler still orders by compute unit price. Set a compute unit limit near
what the transaction uses and a price that fits the market. The examples do
both.

## Examples

All examples read `APEX_API_KEY`, `KEYPAIR_PATH` (default `payer.json`),
`SOLANA_RPC_URL`, optional `APEX_REGION` (a code from the table above),
`APEX_QUIC`,
`APEX_RPC`, `TIP_LAMPORTS` and `APEX_TX_VERSION` (`legacy` or `v1`). Each
sends a tipped memo and reports the slot it landed in.

| Example | Shows |
|---|---|
| `quic_send` | Unidirectional stream: the fastest path, then confirmation from a Solana RPC |
| `quic_send_with_response` | Bidirectional stream: the accepted/rejected answer and how to read a rejection |
| `rpc_send` | JSON-RPC `sendTransaction` over HTTP with the same tip rule |
| `raw_bytes` | Pre-serialized bytes on the wire, and the exact packet layout for other languages |
| `throughput` | One warm connection, N concurrent sends, per-send p50 and p99, landing count |
| `client_pubkey` | The certificate key an API key derives to, to compare with your dashboard |
| `typescript/send_rpc.ts` | The JSON-RPC path from `@solana/web3.js`, no client library needed |
| `python/send.py` | The binary route and JSON-RPC from Python with `solders` |

```
APEX_API_KEY=... KEYPAIR_PATH=payer.json SOLANA_RPC_URL=https://... \
  cargo run --release --example quic_send --features rpc
```

Typical output on mainnet, from a client next to the endpoint:

```
sent in 16 us
landed in slot 447831391 after 745 ms: 5gwAmfVM...
```

The memo program is a poor guide for compute limits (it burns about 20,000
CU on a short memo); size the limit to your own instructions.

## API

| | |
|---|---|
| `ApexSenderClient::connect(region, api_key)` | Connect with an ephemeral local port and the defaults below |
| `ApexSenderClient::connect_with_options(opts, api_key)` | `endpoint` override, `bind_addr` for firewall allowlists, `connect_timeout` 3 s, `send_timeout` 2 s, `keep_alive` 1 s, `mev_protect`, `max_retries`, `auto_reconnect` |
| `send_transaction(&tx)` | Serialize (legacy, v0 or v1) and send on a unidirectional stream; returns the first signature |
| `send_transaction_bytes(bytes)` | The same with bytes you already hold |
| `send_transaction_with_response(&tx)` | Bidirectional stream; `Err(Error::Rejected { code, message })` on rejection |
| `send_with_response(bytes)` | The same with bytes; returns the raw `Admission` |
| `health()`, `reconnects_total()`, `remote_addr()` | Connection state |
| `reconnect()`, `close()` | Lifecycle |
| `tip_instruction(payer, tip_account, lamports)`, `tip::pick_tip_account(&accounts)` | The tip |
| `rpc::RpcClient` | The endpoint over HTTP: `get_tip_accounts`, `send_transaction` (JSON-RPC), `send_transaction_binary`, `send_batch`, `ping` |
| `rpc::SolanaRpc` | Any Solana RPC: `latest_blockhash`, `confirm(signature, timeout)` |
| `rpc::fetch_vaults(solana_rpc_url)` | Tip accounts straight from the tip program |
| `client_pubkey(api_key)` | The certificate key your API key derives to, as shown on your dashboard |
| `serialize_transaction(&tx)` | The canonical wire bytes; the same as bincode for legacy and v0, and correct for v1 |
| `wire::encode_packet`, `wire::decode_admission` | The wire format, for other languages |

## Limits and errors

| | |
|---|---|
| Transaction size | legacy and v0 up to 1232 bytes, v1 up to 4096; over 4096 is `Error::TooLarge` before anything is sent, an oversized legacy transaction is rejected by the endpoint |
| Packet size | at most 4160 bytes on the stream |
| Idle timeout | 10 s on the endpoint; the client pings every `keep_alive` (1 s) |
| Rate limit | per key and tier, `AdmissionCode::RateLimited` or JSON-RPC `-32029` |
| Connections | one client per process and endpoint is enough; streams multiplex on it |

Plain HTTP routes reply with JSON. Accepted: `{"signature": "..."}` and
200. Rejected: `{"error": "<label>", "message": "..."}` with 401
(unauthorized), 429 (rate limited), 400 (invalid transaction, tip or size)
or 503 (busy). `/send-batch` replies 200 with `attempted`, `accepted`,
`rejected` and one result per frame.

Admission codes on the bidirectional stream: `Ok`, `Unauthorized`,
`RateLimited`, `Invalid` (the message says what failed sanitization),
`NoTip`, `BelowFloor` (the message states your floor), `Busy`,
`MalformedPacket`. JSON-RPC errors: `-32001` unauthorized, `-32029` rate
limited, `-32602` invalid transaction or tip, `-32603` busy.

Accepted means the endpoint holds the transaction and is racing it to the
leaders until it lands or its blockhash expires. It does not mean it
landed: confirm with `rpc::SolanaRpc::confirm` or `getSignatureStatuses` on
any RPC.

## What it does not do

- build or sign transactions, or choose your priority fee
- simulate or preflight (nothing between you and the leader does)
- bundle transactions together; each is independent

## Other languages

the Apex docs at https://docs.orbitflare.com/apex specifies the QUIC transport, the client certificate
derivation (with a test vector) and the packet and admission frames; the
Rust crate is the reference implementation. The JSON-RPC path needs no
library at all, see `examples/typescript/`.

## License

Apache-2.0
