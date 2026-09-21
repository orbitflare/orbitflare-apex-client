# Python

No client library needed: `send.py` builds a tipped memo with `solders`,
sends it over the raw binary route (`POST /send-bin`) and over JSON-RPC,
and confirms both.

```
pip install solders requests
APEX_API_KEY=... KEYPAIR_PATH=payer.json python send.py
```

Variables: `APEX_RPC` (default `http://fra.apex.orbitflare.com`),
`SOLANA_RPC_URL`, `TIP_LAMPORTS` (default 1,000,000).

The tip account comes from `getTipAccounts` at run time. On mainnet every published account starts with `APeX` (the ten are listed in the top-level README); if you ever see a different prefix, stop and check the endpoint you are talking to.
