#!/usr/bin/env node
// Execute the deployment plan against a real JSON-RPC endpoint.
//
//   RPC_URL=... PRIVATE_KEY=0x... \
//   WORMHOLE_CORE=0x... LZ_ENDPOINT=0x... \
//   SOLANA_EMITTER=0x<32 bytes> SOLANA_OAPP=0x<32 bytes> \
//   CONFIGURER=0x<32 bytes> INVOKER=0x<32 bytes> \
//   SOLANA_EID=40168 INTERNAL_CHAIN_ID=3 \
//   node script/deploy.js
//
// Writes deployments/<chainId>.json. The plan itself lives in plan.js and is
// executed against the in-process EVM by test/deploy.test.js, so the order and
// the constructor arguments are checked before they meet a real chain.
const fs = require("fs");
const path = require("path");
const { ethers } = require("ethers");
const { plan } = require("./plan");

const artifacts = JSON.parse(
  fs.readFileSync(path.join(__dirname, "..", "test", "out", "artifacts.json"), "utf8")
);

function need(name) {
  const v = process.env[name];
  if (!v) {
    console.error(`missing required environment variable: ${name}`);
    process.exit(2);
  }
  return v;
}

function resolve(value, deployed) {
  if (typeof value === "string" && value.startsWith("$")) {
    const addr = deployed[value.slice(1)];
    if (!addr) throw new Error(`plan refers to ${value} before it is deployed`);
    return addr;
  }
  return value;
}

async function main() {
  const provider = new ethers.JsonRpcProvider(need("RPC_URL"));
  const wallet = new ethers.Wallet(need("PRIVATE_KEY"), provider);
  const net = await provider.getNetwork();
  const balance = await provider.getBalance(wallet.address);

  console.log(`chain     ${net.chainId}`);
  console.log(`deployer  ${wallet.address}`);
  console.log(`balance   ${ethers.formatEther(balance)} ETH`);
  if (balance === 0n) {
    console.error("deployer has no balance; fund it before deploying");
    process.exit(2);
  }

  const p = plan({
    owner: process.env.OWNER || wallet.address,
    wormholeCore: need("WORMHOLE_CORE"),
    lzEndpoint: need("LZ_ENDPOINT"),
    solanaEmitter: need("SOLANA_EMITTER"),
    solanaOApp: need("SOLANA_OAPP"),
    configurer: need("CONFIGURER"),
    invoker: need("INVOKER"),
    internalChainId: Number(process.env.INTERNAL_CHAIN_ID || 3),
    solanaEid: Number(need("SOLANA_EID")),
  });

  const deployed = {};
  for (const step of p.deploy) {
    const { abi, bytecode } = artifacts[step.name];
    const args = step.args.map((a) => resolve(a, deployed));
    const factory = new ethers.ContractFactory(abi, bytecode, wallet);
    const c = await factory.deploy(...args);
    await c.waitForDeployment();
    deployed[step.name] = await c.getAddress();
    console.log(`deployed  ${step.name.padEnd(20)} ${deployed[step.name]}`);
  }

  for (const call of p.calls) {
    const c = new ethers.Contract(deployed[call.on], artifacts[call.on].abi, wallet);
    const args = call.args.map((a) => resolve(a, deployed));
    const tx = await c[call.fn](...args);
    await tx.wait();
    console.log(`called    ${call.on}.${call.fn}(${args.join(", ")})`);
  }

  const out = path.join(__dirname, "..", "deployments");
  fs.mkdirSync(out, { recursive: true });
  const file = path.join(out, `${net.chainId}.json`);
  fs.writeFileSync(file, JSON.stringify({ chainId: Number(net.chainId), deployed }, null, 2) + "\n");
  console.log(`\nwrote ${file}`);
  console.log("\nNEXT: register these on Solana with set_transport, using");
  console.log("  wormhole  peer = WormholeAdapter   left-padded to 32 bytes");
  console.log("  layerzero peer = LayerZeroAdapter  left-padded to 32 bytes");
}

main().catch((e) => {
  console.error(e.message || e);
  process.exit(1);
});
