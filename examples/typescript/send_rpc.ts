// JSON-RPC sendTransaction to an Apex endpoint from TypeScript.
//
//   npm install @solana/web3.js
//   APEX_API_KEY=... KEYPAIR_PATH=payer.json npx tsx send_rpc.ts
//
// The endpoint's JSON-RPC is the standard Solana sendTransaction shape plus
// an x-api-key header, so any existing sender code moves over by changing
// the URL. Every transaction needs one SystemProgram transfer to a published
// tip account at or above your tier's floor.

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

const APEX_RPC = process.env.APEX_RPC ?? "http://fra.apex.orbitflare.com";
const API_KEY = process.env.APEX_API_KEY ?? "";
const SOLANA_RPC = process.env.SOLANA_RPC_URL ?? "https://api.mainnet-beta.solana.com";
const TIP_LAMPORTS = Number(process.env.TIP_LAMPORTS ?? 1_000_000);
const MEMO_PROGRAM = new PublicKey("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

async function apexRpc(method: string, params: unknown[]): Promise<any> {
  const res = await fetch(APEX_RPC, {
    method: "POST",
    headers: { "content-type": "application/json", "x-api-key": API_KEY },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
  });
  const body = await res.json();
  if (body.error) throw new Error(`${method} failed ${body.error.code}: ${body.error.message}`);
  return body.result;
}

async function main() {
  if (!API_KEY) throw new Error("APEX_API_KEY is required");
  const payer = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(readFileSync(process.env.KEYPAIR_PATH ?? "payer.json", "utf8"))),
  );
  const connection = new Connection(SOLANA_RPC, "confirmed");

  const tipAccounts: string[] = await apexRpc("getTipAccounts", []);
  const tipAccount = new PublicKey(tipAccounts[Math.floor(Math.random() * tipAccounts.length)]);

  const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash("confirmed");
  const message = new TransactionMessage({
    payerKey: payer.publicKey,
    recentBlockhash: blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitLimit({ units: 100_000 }),
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 10_000 }),
      new TransactionInstruction({
        programId: MEMO_PROGRAM,
        keys: [{ pubkey: payer.publicKey, isSigner: true, isWritable: true }],
        data: Buffer.from(`apex typescript ${Date.now()}`),
      }),
      SystemProgram.transfer({ fromPubkey: payer.publicKey, toPubkey: tipAccount, lamports: TIP_LAMPORTS }),
    ],
  }).compileToV0Message();
  const tx = new VersionedTransaction(message);
  tx.sign([payer]);

  const started = Date.now();
  // params: [base64 tx, { encoding, maxRetries? }, mevProtect]
  const signature: string = await apexRpc("sendTransaction", [
    Buffer.from(tx.serialize()).toString("base64"),
    { encoding: "base64" },
    false,
  ]);
  console.log(`accepted in ${Date.now() - started} ms: ${signature}`);

  const result = await connection.confirmTransaction({ signature, blockhash, lastValidBlockHeight }, "confirmed");
  if (result.value.err) throw new Error(`failed on chain: ${JSON.stringify(result.value.err)}`);
  console.log(`confirmed after ${Date.now() - started} ms`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
