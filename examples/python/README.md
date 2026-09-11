# Python

No client library needed: `send.py` builds a tipped memo with `solders`,
sends it over the raw binary route (`POST /send-bin`) and over JSON-RPC,
and confirms both.

```
pip install solders requests
APEX_API_KEY=... KEYPAIR_PATH=payer.json python send.py
```

Variables: `APEX_RPC` (default `http://fra.sender.orbitflare.com:7000`),
`SOLANA_RPC_URL`, `TIP_LAMPORTS` (default 1,000,000).
