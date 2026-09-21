# Plain JavaScript example

Node 18 or newer, no build step and no client library: `send.mjs` builds a
tipped memo with `@solana/web3.js`, sends one over the raw binary route
(`POST /send-bin`), then two more as one batch (`POST /send-batch`), and
waits for each to land.

```bash
npm install
APEX_API_KEY=... KEYPAIR_PATH=payer.json node send.mjs
```

| Variable | Default | Meaning |
|---|---|---|
| `APEX_API_KEY` | required | Your key, sent as the `x-api-key` header |
| `KEYPAIR_PATH` | `payer.json` | Fee payer, a Solana CLI keypair file. It pays the tip |
| `APEX_RPC` | `http://fra.apex.orbitflare.com` | The Apex endpoint nearest you |
| `SOLANA_RPC_URL` | public mainnet | Any Solana RPC, for the blockhash and confirmation |
| `TIP_LAMPORTS` | `1000000` | Must meet your tier's floor |

The tip is an instruction inside each transaction, so it is only paid when
that transaction lands. The HTTP routes are unencrypted; use the header form
of the key as this example does, and prefer QUIC (the Rust client) where you
can.

The batch body is up to 16 frames of a big-endian `u16` length followed by
the transaction bytes; the reply lists one result per frame, in order.
the Apex docs at https://docs.orbitflare.com/apex has the full contract.

The tip account comes from `getTipAccounts` at run time. On mainnet every published account starts with `APeX` (the ten are listed in the top-level README); if you ever see a different prefix, stop and check the endpoint you are talking to.
