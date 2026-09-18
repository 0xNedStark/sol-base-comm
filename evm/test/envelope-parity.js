// Simulate EnvelopeLib.decode's calldataload+shift arithmetic against the
// byte vector the Rust encoder actually produced.
const hex =
  "01" + "00" + "0001" + "0003" +
  "1111111111111111111111111111111111111111111111111111111111111111" +
  "0000000000000007" +
  "abababababababababababababababababababab" +
  "00000000000000000de0b6b3a7640000" +
  "00000000000493e0" +
  "000000006553f100" +
  "00" + "00000005" + "deadbeef01";
const buf = Buffer.from(hex, "hex");

// calldataload: 32 bytes at offset, zero-padded past the end
function load(off) {
  const w = Buffer.alloc(32);
  buf.copy(w, 0, Math.min(off, buf.length), Math.min(off + 32, buf.length));
  return BigInt("0x" + w.toString("hex"));
}
const M = (n) => (1n << BigInt(n)) - 1n;

let w = load(0);
const version    = (w >> 248n) & M(8);
const msgType    = (w >> 240n) & M(8);
const srcChainId = (w >> 224n) & M(16);
const dstChainId = (w >> 208n) & M(16);

w = load(6);
const sender = "0x" + w.toString(16).padStart(64, "0");

w = load(38);
const nonce  = (w >> 192n) & M(64);
const target = "0x" + (((w >> 32n) & M(160))).toString(16).padStart(40, "0");

w = load(66);
const value    = (w >> 128n) & M(128);
const gasLimit = (w >> 64n) & M(64);
const expiry   = w & M(64);

w = load(98);
const mode        = (w >> 248n) & M(8);
const calldataLen = (w >> 216n) & M(32);

const HEADER = 103;
const callData = "0x" + buf.subarray(HEADER).toString("hex");

const got = { version, msgType, srcChainId, dstChainId, sender, nonce, target, value, gasLimit, expiry, mode, calldataLen, callData, totalLen: buf.length };
const want = {
  version: 1n, msgType: 0n, srcChainId: 1n, dstChainId: 3n,
  sender: "0x" + "11".repeat(32),
  nonce: 7n,
  target: "0x" + "ab".repeat(20),
  value: 1000000000000000000n,
  gasLimit: 300000n,
  expiry: 1700000000n,
  mode: 0n,
  calldataLen: 5n,
  callData: "0xdeadbeef01",
  totalLen: HEADER + 5,
};

let fail = 0;
for (const k of Object.keys(want)) {
  const ok = got[k] === want[k];
  if (!ok) fail++;
  console.log(`${ok ? "ok  " : "FAIL"} ${k.padEnd(12)} got=${got[k]} want=${want[k]}`);
}
console.log(fail === 0 ? "\nPARITY OK: Solidity offsets/shifts match the Rust encoder"
                       : `\nPARITY FAILED: ${fail} field(s)`);
process.exit(fail ? 1 : 0);
