# TypeScript

The JSON-RPC transport needs no client library: it is Solana's
`sendTransaction` with an `x-api-key` header. `send_rpc.ts` builds a tipped
memo with `@solana/web3.js`, sends it to an Apex endpoint and confirms it.

```
npm install
APEX_API_KEY=... KEYPAIR_PATH=payer.json npm run send
```

Variables: `APEX_RPC` (default `http://fra.apex.orbitflare.com`),
`SOLANA_RPC_URL`, `TIP_LAMPORTS` (default 1,000,000).

QUIC from Node needs a QUIC library and the client certificate derivation in
the Apex docs at https://docs.orbitflare.com/apex; the HTTP path is a few hundred microseconds slower and is the
one to start with.
