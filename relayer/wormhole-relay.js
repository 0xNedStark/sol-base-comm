#!/usr/bin/env node
// Step 4: carry a Wormhole message from Solana to the destination chain.
//
// Wormhole gives no automatic delivery on this route, and that is a feature
// here: a VAA is a self-contained, guardian-signed artifact that anyone can
// submit. If every relayer in the world disappeared, this script -- or a
// person with curl and a wallet -- still lands the message. That is a
// different liveness profile from a transport where delivery depends on a
// designated executor, and it is why Wormhole is the first transport to try.
//
// The relayer cannot forge anything. A compromised one can delay or drop, not
// fabricate, so this needs monitoring for liveness and not for integrity.
//
//   SOLANA_EMITTER=<base58 emitter PDA> SEQUENCE=0 \
//   WORMHOLE_API=https://api.testnet.wormholescan.io \
//   RPC_URL=<destination rpc> PRIVATE_KEY=0x... \
//   WORMHOLE_ADAPTER=0x... \
//   node relayer/wormhole-relay.js
const { ethers } = require("ethers");
const fs = require("fs");
const path = require("path");

const SOLANA_CHAIN_ID = 1; // Wormhole's own id for Solana, verified
const artifacts = JSON.parse(
  fs.readFileSync(path.join(__dirname, "..", "evm", "test", "out", "artifacts.json"), "utf8")
);

function need(n) {
  const v = process.env[n];
  if (!v) { console.error(`missing ${n}`); process.exit(2); }
  return v;
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Poll for the signed VAA. It does not exist until enough guardians have
 * observed the message at the finalized commitment, so 404 is the normal
 * first answer, not an error.
 */
async function fetchVaa(api, emitter, sequence, { tries = 40, waitMs = 15000 } = {}) {
  const url = `${api}/v1/signed_vaa/${SOLANA_CHAIN_ID}/${emitter}/${sequence}`;
  for (let i = 1; i <= tries; i++) {
    const res = await fetch(url);
    if (res.ok) {
      const body = await res.json();
      return Buffer.from(body.vaaBytes, "base64");
    }
    if (res.status !== 404) {
      throw new Error(`guardian API ${res.status}: ${(await res.text()).slice(0, 200)}`);
    }
    process.stdout.write(`\r  waiting for guardians (${i}/${tries}) ...`);
    await sleep(waitMs);
  }
  throw new Error(
    "\nno VAA after the full wait. Either the message was published below " +
    "finalized commitment, or the emitter/sequence is wrong."
  );
}

async function main() {
  const api = process.env.WORMHOLE_API || "https://api.testnet.wormholescan.io";
  const emitter = need("SOLANA_EMITTER");
  const sequence = need("SEQUENCE");

  console.log(`emitter   ${emitter}`);
  console.log(`sequence  ${sequence}`);
  const vaa = await fetchVaa(api, emitter, sequence);
  console.log(`\n  got VAA, ${vaa.length} bytes`);

  const provider = new ethers.JsonRpcProvider(need("RPC_URL"));
  const wallet = new ethers.Wallet(need("PRIVATE_KEY"), provider);
  const adapter = new ethers.Contract(
    need("WORMHOLE_ADAPTER"), artifacts.WormholeAdapter.abi, wallet);

  // Simulate first. A revert here names the reason -- wrong emitter,
  // insufficient consistency level, already consumed -- while a blind send
  // just burns gas and leaves you reading a trace.
  try {
    await adapter.receiveMessage.staticCall("0x" + vaa.toString("hex"));
  } catch (e) {
    console.error(`\nthe adapter would reject this VAA: ${e.shortMessage || e.message}`);
    console.error("  WrongEmitter          the adapter pins a different Solana peer");
    console.error("  InsufficientConsistency  published below finalized");
    console.error("  AlreadyConsumed       already delivered; nothing to do");
    process.exit(1);
  }

  const tx = await adapter.receiveMessage("0x" + vaa.toString("hex"));
  console.log(`  submitted ${tx.hash}`);
  const receipt = await tx.wait();
  console.log(`  mined in block ${receipt.blockNumber}, gas ${receipt.gasUsed}`);

  const gateway = new ethers.Interface(artifacts.SolanaGateway.abi);
  for (const log of receipt.logs) {
    try {
      const p = gateway.parseLog({ topics: log.topics, data: log.data });
      if (!p) continue;
      if (p.name === "CallExecuted") console.log(`\n  EXECUTED  messageId ${p.args.messageId}`);
      if (p.name === "CallFailed") console.log(`\n  PARKED    reason ${p.args.reason} -- retryable`);
      if (p.name === "MessageRejected") console.log(`\n  REJECTED  reason ${p.args.reason} -- terminal`);
      if (p.name === "MessageDelivered") console.log(`  delivered, ${p.args.confirmations} confirmation(s)`);
    } catch {}
  }
}

main().catch((e) => { console.error(e.message || e); process.exit(1); });
