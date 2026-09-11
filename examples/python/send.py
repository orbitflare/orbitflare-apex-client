"""Send a tipped transaction to an Apex endpoint over plain HTTP from Python.

    pip install solders requests
    APEX_API_KEY=... KEYPAIR_PATH=payer.json python send.py

Two routes are shown: /send-bin takes the raw transaction bytes (fastest
HTTP path), and JSON-RPC sendTransaction takes base64. Every transaction
needs one SystemProgram transfer to a published tip account at or above
your tier's floor; the tip is inside the transaction, so it is only paid if
the transaction lands.
"""

import json
import os
import random
import time

import requests
from solders.hash import Hash
from solders.instruction import Instruction, AccountMeta
from solders.keypair import Keypair
from solders.message import Message
from solders.pubkey import Pubkey
from solders.system_program import transfer, TransferParams
from solders.transaction import Transaction

APEX_RPC = os.environ.get("APEX_RPC", "http://fra.sender.orbitflare.com:7000")
API_KEY = os.environ["APEX_API_KEY"]
SOLANA_RPC = os.environ.get("SOLANA_RPC_URL", "https://api.mainnet-beta.solana.com")
TIP_LAMPORTS = int(os.environ.get("TIP_LAMPORTS", "1000000"))
MEMO_PROGRAM = Pubkey.from_string("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr")


def apex_rpc(method, params):
    r = requests.post(APEX_RPC, headers={"x-api-key": API_KEY},
                      json={"jsonrpc": "2.0", "id": 1, "method": method, "params": params}, timeout=5)
    body = r.json()
    if "error" in body:
        raise RuntimeError(f"{method} failed {body['error']['code']}: {body['error']['message']}")
    return body["result"]


def solana_rpc(method, params):
    r = requests.post(SOLANA_RPC, json={"jsonrpc": "2.0", "id": 1, "method": method, "params": params}, timeout=10)
    return r.json()["result"]


def build_tx(payer: Keypair, tip_account: Pubkey) -> Transaction:
    blockhash = Hash.from_string(solana_rpc("getLatestBlockhash", [{"commitment": "confirmed"}])["value"]["blockhash"])
    memo = Instruction(MEMO_PROGRAM, f"apex python {time.time_ns()}".encode(),
                       [AccountMeta(payer.pubkey(), is_signer=True, is_writable=True)])
    tip = transfer(TransferParams(from_pubkey=payer.pubkey(), to_pubkey=tip_account, lamports=TIP_LAMPORTS))
    msg = Message.new_with_blockhash([memo, tip], payer.pubkey(), blockhash)
    return Transaction([payer], msg, blockhash)


def confirm(signature: str, timeout_s: float = 30.0):
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        st = solana_rpc("getSignatureStatuses", [[signature]])["value"][0]
        if st and st.get("confirmationStatus") in ("confirmed", "finalized"):
            return st["slot"]
        time.sleep(0.5)
    return None


def main():
    with open(os.environ.get("KEYPAIR_PATH", "payer.json")) as f:
        payer = Keypair.from_bytes(bytes(json.load(f)))
    tip_accounts = apex_rpc("getTipAccounts", [])
    tip_account = Pubkey.from_string(random.choice(tip_accounts))

    # Binary route: raw bytes, no encoding.
    tx = build_tx(payer, tip_account)
    started = time.perf_counter()
    r = requests.post(f"{APEX_RPC}/send-bin", headers={"x-api-key": API_KEY, "content-type": "application/octet-stream"},
                      data=bytes(tx), timeout=5)
    body = r.json()
    if "signature" not in body:
        raise RuntimeError(f"rejected: {body}")
    print(f"/send-bin accepted in {(time.perf_counter() - started) * 1000:.1f} ms: {body['signature']}")
    print("landed in slot", confirm(body["signature"]))

    # JSON-RPC route: the standard sendTransaction shape plus the key header.
    tx = build_tx(payer, tip_account)
    import base64
    sig = apex_rpc("sendTransaction", [base64.b64encode(bytes(tx)).decode(), {"encoding": "base64"}, False])
    print("sendTransaction accepted:", sig)
    print("landed in slot", confirm(sig))


if __name__ == "__main__":
    main()
