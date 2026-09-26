#!/usr/bin/env node
// Step 3: point the Solana side at the destination adapters.
//
// This and the adapter constructors are the mutual pinning. Unit tests cannot
// check either -- they are an agreement between two chains, and the only way
// to find a mistake is to send a message.
//
//   ANCHOR_PROVIDER_URL=https://api.devnet.solana.com \
//   ANCHOR_WALLET=~/.config/solana/id.json \
//   WORMHOLE_ADAPTER=0x... LAYERZERO_ADAPTER=0x... \
//   WORMHOLE_CORE=<solana core bridge pubkey> \
//   LZ_ENDPOINT=<solana endpoint pubkey> \
//   LZ_DISPATCHER=<dispatcher program pubkey> \
//   DEST_WORMHOLE_CHAIN=10004 DEST_LZ_EID=40245 \
//   node scripts/register-transports.js
const anchor = require("@coral-xyz/anchor");
const { PublicKey, SystemProgram } = require("@solana/web3.js");

const TRANSPORT_WORMHOLE = 1;
const TRANSPORT_LAYERZERO = 2;

function need(n) {
  const v = process.env[n];
  if (!v) { console.error(`missing ${n}`); process.exit(2); }
  return v;
}

/** An EVM address, left-padded to the 32 bytes the envelope and config use. */
function evmPeer(addr) {
  const hex = addr.replace(/^0x/, "").toLowerCase();
  if (hex.length !== 40) { console.error(`not an EVM address: ${addr}`); process.exit(2); }
  return Array.from(Buffer.concat([Buffer.alloc(12), Buffer.from(hex, "hex")]));
}

async function main() {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.BaseCaller;
  const admin = provider.wallet.publicKey;

  const [config] = PublicKey.findProgramAddressSync([Buffer.from("config")], program.programId);
  const transport = (id) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("transport"), Buffer.from([id])], program.programId)[0];

  // Idempotent: initialize only if the config does not exist yet.
  if (await provider.connection.getAccountInfo(config)) {
    console.log(`config already initialized at ${config.toBase58()}`);
  } else {
    await program.methods.initialize(admin)
      .accountsPartial({ config, payer: admin, systemProgram: SystemProgram.programId })
      .rpc();
    console.log(`initialized config ${config.toBase58()}`);
  }

  const rows = [
    {
      id: TRANSPORT_WORMHOLE,
      label: "wormhole  (in-process)",
      programId: new PublicKey(need("WORMHOLE_CORE")),
      peer: evmPeer(need("WORMHOLE_ADAPTER")),
      destChain: Number(need("DEST_WORMHOLE_CHAIN")),
      // dispatched by an instruction of base_caller itself
      dispatcher: PublicKey.default,
    },
    {
      id: TRANSPORT_LAYERZERO,
      label: "layerzero (external program)",
      programId: new PublicKey(need("LZ_ENDPOINT")),
      peer: evmPeer(need("LAYERZERO_ADAPTER")),
      destChain: Number(need("DEST_LZ_EID")),
      // LayerZero's SDK cannot be linked into base_caller, so its dispatch
      // lives in its own program and calls back via mark_dispatched.
      dispatcher: new PublicKey(need("LZ_DISPATCHER")),
    },
  ];

  for (const r of rows) {
    await program.methods
      .setTransport(r.id, true, r.programId, r.peer, r.destChain, r.dispatcher)
      .accountsPartial({
        config, transport: transport(r.id), admin, systemProgram: SystemProgram.programId,
      })
      .rpc();
    console.log(`registered ${r.label}`);
    console.log(`    endpoint   ${r.programId.toBase58()}`);
    console.log(`    peer       0x${Buffer.from(r.peer.slice(12)).toString("hex")}`);
    console.log(`    destChain  ${r.destChain}`);
    console.log(`    dispatcher ${r.dispatcher.equals(PublicKey.default) ? "(in-process)" : r.dispatcher.toBase58()}`);
  }

  console.log("\nBoth sides now pin each other. Send one message before raising");
  console.log("requiredConfirmations on any target: a two-transport quorum with");
  console.log("one broken transport looks exactly like a broken system.");
}

main().catch((e) => { console.error(e.message || e); process.exit(1); });
