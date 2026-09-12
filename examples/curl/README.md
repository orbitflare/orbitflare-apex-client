# cURL

Every route with nothing but cURL. `TX_B64` is a signed, base64-encoded
transaction that includes the tip transfer; `tx.bin` is the same transaction
as raw bytes.

```bash
export APEX=http://fra.apex.orbitflare.com
export APEX_API_KEY=...

# Tip accounts (no key needed)
curl -s $APEX -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getTipAccounts","params":[]}'

# JSON-RPC sendTransaction: [base64, config, mevProtect]
curl -s $APEX -H "x-api-key: $APEX_API_KEY" -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"sendTransaction","params":["'"$TX_B64"'",{"encoding":"base64"},false]}'

# Plain JSON
curl -s $APEX/send -H "x-api-key: $APEX_API_KEY" -H 'content-type: application/json' \
  -d '{"transaction":"'"$TX_B64"'"}'

# Raw bytes: the fastest HTTP route
curl -s "$APEX/send-bin" -H "x-api-key: $APEX_API_KEY" \
  -H 'content-type: application/octet-stream' --data-binary @tx.bin

# Batch: frames of big-endian u16 length + bytes, up to 16
( for f in tx1.bin tx2.bin; do
    printf "%04x" "$(wc -c < "$f")" | xxd -r -p; cat "$f"
  done ) > batch.bin
curl -s "$APEX/send-batch" -H "x-api-key: $APEX_API_KEY" \
  -H 'content-type: application/octet-stream' --data-binary @batch.bin

# Atomic bundle: same framing, 1 to 4 transactions, one of them tipped
curl -s "$APEX/send-bundle" -H "x-api-key: $APEX_API_KEY" \
  -H 'content-type: application/octet-stream' --data-binary @batch.bin

# Bundle status
curl -s $APEX -H "x-api-key: $APEX_API_KEY" -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getInflightBundleStatuses","params":[["<bundle id>"]]}'

# Keep the connection warm
curl -s $APEX/ping
```

Replies: `{"signature":"..."}` on success; `{"error":"<label>","message":"..."}`
with 401, 429, 400, 413 or 503 otherwise. JSON-RPC uses the standard error
object with codes -32001, -32029, -32602 and -32603.
