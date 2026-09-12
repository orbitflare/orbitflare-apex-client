// Plain JavaScript (Node 18+), no build step:
//
//   npm install @solana/web3.js
//   APEX_API_KEY=... KEYPAIR_PATH=payer.json node send.mjs
//
// Sends one tipped memo over the raw binary route, then two more as a
// batch. The tip is an instruction inside each transaction, so it is only
// paid when that transaction lands.

import {
  ComputeBudgetProgram,
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import { readFileSync } from "node:fs";

const APEX = process.env.APEX_RPC ?? "http://fra.apex.orbitflare.com";
const API_KEY = process.env.APEX_API_KEY;
const SOLANA_RPC = process.env.SOLANA_RPC_URL ?? "https://api.mainnet-beta.solana.com";
const TIP_LAMPORTS = Number(process.env.TIP_LAMPORTS ?? 1_000_000);
const MEMO = new PublicKey("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
if (!API_KEY) throw new Error("APEX_API_KEY is required");

const payer = Keypair.fromSecretKey(
  Uint8Array.from(JSON.parse(readFileSync(process.env.KEYPAIR_PATH ?? "payer.json", "utf8"))),
);
const connection = new Connection(SOLANA_RPC, "confirmed");

async function tipAccounts() {
  const res = await fetch(APEX, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getTipAccounts", params: [] }),
  });
  return (await res.json()).result.map((a) => new PublicKey(a));
}

function tippedMemo(label, blockhash, tipAccount) {
  const message = new TransactionMessage({
    payerKey: payer.publicKey,
    recentBlockhash: blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitLimit({ units: 100_000 }),
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 10_000 }),
      new TransactionInstruction({
        programId: MEMO,
        keys: [{ pubkey: payer.publicKey, isSigner: true, isWritable: true }],
        data: Buffer.from(`${label} ${Date.now()}`),
      }),
      SystemProgram.transfer({ fromPubkey: payer.publicKey, toPubkey: tipAccount, lamports: TIP_LAMPORTS }),
    ],
  }).compileToV0Message();
  const tx = new VersionedTransaction(message);
  tx.sign([payer]);
  return tx.serialize();
}

async function post(path, bytes) {
  const res = await fetch(`${APEX}${path}`, {
    method: "POST",
    headers: { "x-api-key": API_KEY, "content-type": "application/octet-stream" },
    body: bytes,
  });
  const body = await res.json();
  if (!res.ok) throw new Error(`${path} ${res.status}: ${body.error}: ${body.message}`);
  return body;
}

function frames(transactions) {
  const parts = [];
  for (const tx of transactions) {
    const len = Buffer.alloc(2);
    len.writeUInt16BE(tx.length);
    parts.push(len, Buffer.from(tx));
  }
  return Buffer.concat(parts);
}

const accounts = await tipAccounts();
const pick = () => accounts[Math.floor(Math.random() * accounts.length)];
const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash("confirmed");

const started = Date.now();
const { signature } = await post("/send-bin", tippedMemo("apex js", blockhash, pick()));
console.log(`/send-bin accepted in ${Date.now() - started} ms: ${signature}`);
const result = await connection.confirmTransaction({ signature, blockhash, lastValidBlockHeight }, "confirmed");
console.log(result.value.err ? `failed on chain: ${JSON.stringify(result.value.err)}` : "confirmed");

const batch = await post(
  "/send-batch",
  frames([tippedMemo("apex js batch 1", blockhash, pick()), tippedMemo("apex js batch 2", blockhash, pick())]),
);
console.log(`/send-batch: ${batch.accepted}/${batch.attempted} accepted`);
for (const r of batch.results) console.log(" ", r.signature ?? `${r.error}: ${r.message}`);
