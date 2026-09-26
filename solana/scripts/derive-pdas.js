#!/usr/bin/env node
// Derives every cross-chain identity, in the hex form the destination
// contracts want.
//
// These are the constants that unit tests structurally cannot check: they are
// an agreement between two chains. Getting one wrong in one direction means
// messages are silently rejected, which is the good failure. Pinning a peer
// you do not control means they are silently accepted.
const { PublicKey } = require("@solana/web3.js");
const fs = require("fs");
const path = require("path");

function programId(name) {
  const env = process.env[name.toUpperCase() + "_ID"];
  if (env) return new PublicKey(env);
  // The dispatcher builds in its own workspace, so look there too.
  const candidates = [
    path.join(__dirname, "..", "target", "deploy", `${name}-keypair.json`),
    path.join(__dirname, "..", "layerzero-dispatcher", "target", "deploy", `${name}-keypair.json`),
  ];
  const kp = candidates.find(fs.existsSync);
  if (!kp) {
    throw new Error(
      `no ${name} program id.\n` +
      `  Build first (npm run build:all), or pass ${name.toUpperCase()}_ID=<pubkey>.\n` +
      `  Before a real deploy, give each program a keypair you control:\n` +
      `    solana-keygen new -o target/deploy/${name}-keypair.json\n` +
      `  then sync the declare_id! in its source to match.`
    );
  }
  const secret = Uint8Array.from(JSON.parse(fs.readFileSync(kp, "utf8")));
  // last 32 bytes of an ed25519 keypair are the public key
  return new PublicKey(secret.slice(32));
}

const pda = (seeds, program) => PublicKey.findProgramAddressSync(seeds, program)[0];
const toHex32 = (pk) => "0x" + Buffer.from(pk.toBytes()).toString("hex");

const outbox = programId("base_caller");
const dispatcher = programId("lz_dispatcher");

const emitter = pda([Buffer.from("emitter")], outbox);
const oapp = pda([Buffer.from("oapp")], dispatcher);
const dispatcherAuthority = pda([Buffer.from("dispatcher")], dispatcher);

console.log(`  base_caller program   ${outbox.toBase58()}`);
console.log(`  lz_dispatcher program ${dispatcher.toBase58()}`);
console.log();
console.log("  For the destination deploy (script/deploy.js):");
console.log(`    SOLANA_EMITTER=${toHex32(emitter)}`);
console.log(`      (${emitter.toBase58()} -- WormholeAdapter pins this)`);
console.log(`    SOLANA_OAPP=${toHex32(oapp)}`);
console.log(`      (${oapp.toBase58()} -- LayerZeroAdapter pins this)`);
console.log();
console.log("  Registered inside base_caller (set_transport):");
console.log(`    dispatcher = ${dispatcher.toBase58()}`);
console.log(`    its authority PDA = ${dispatcherAuthority.toBase58()}`);
console.log();
console.log("  Still to choose: the two Solana authorities the target trusts.");
console.log("  Use PDAs of YOUR program, not wallet keys -- a PDA has no");
console.log("  private key to steal. Separate ones per privilege level:");
console.log("    CONFIGURER=0x<32 bytes>   rare, privileged");
console.log("    INVOKER=0x<32 bytes>      routine");
