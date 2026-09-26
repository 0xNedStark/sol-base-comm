// Runs the real deployment plan against an in-process EVM.
//
// script/deploy.js only transports the plan to a live RPC; the plan itself --
// what gets deployed, in what order, with which constructor arguments, and
// what gets wired afterwards -- is what this checks. A deploy script whose
// first execution is against a testnet is a script debugged on a testnet,
// and the mistakes that matter there (a peer pinned to the wrong key, a
// consistency level of 0) are silent rather than loud.
const { VM } = require("@ethereumjs/vm");
const { Common, Chain, Hardfork } = require("@ethereumjs/common");
const { Block } = require("@ethereumjs/block");
const { Address, Account, hexToBytes, bytesToHex } = require("@ethereumjs/util");
const { Interface } = require("ethers");
const fs = require("fs");
const path = require("path");
const { plan } = require("../script/plan");

const artifacts = JSON.parse(
  fs.readFileSync(path.join(__dirname, "out", "artifacts.json"), "utf8")
);

const OWNER = Address.fromString("0x1000000000000000000000000000000000000001");
const CORE = "0x00000000000000000000000000000000000000c0";
const ENDPOINT = "0x00000000000000000000000000000000000000e0";
const EMITTER = "0x" + "aa".repeat(32);
const OAPP = "0x" + "bb".repeat(32);
const CONFIGURER = "0x" + "11".repeat(32);
const INVOKER = "0x" + "22".repeat(32);
const SOLANA_DEVNET_EID = 40168;
const BASE_INTERNAL_ID = 3;

const results = [];
async function test(name, fn) {
  try {
    await fn();
    results.push(true);
    console.log(`  ok   ${name}`);
  } catch (e) {
    results.push(false);
    console.log(`  FAIL ${name}\n       ${e.message}`);
  }
}
function eq(a, b, what) {
  const s = (x) => (typeof x === "bigint" ? x.toString() : String(x));
  if (s(a).toLowerCase() !== s(b).toLowerCase()) {
    throw new Error(`${what}: got ${s(a)}, want ${s(b)}`);
  }
}

async function run() {
  const common = new Common({ chain: Chain.Mainnet, hardfork: Hardfork.Cancun });
  const vm = await VM.create({ common });
  await vm.stateManager.putAccount(OWNER, new Account(0n, 10n ** 24n));
  const block = Block.fromBlockData(
    { header: { timestamp: 1700000000n, gasLimit: 30000000n, number: 1n } },
    { common }
  );

  const resolve = (v, deployed) =>
    typeof v === "string" && v.startsWith("$") ? deployed[v.slice(1)] : v;

  const p = plan({
    owner: OWNER.toString(),
    wormholeCore: CORE,
    lzEndpoint: ENDPOINT,
    solanaEmitter: EMITTER,
    solanaOApp: OAPP,
    configurer: CONFIGURER,
    invoker: INVOKER,
    internalChainId: BASE_INTERNAL_ID,
    solanaEid: SOLANA_DEVNET_EID,
  });

  // --- execute the plan, exactly as deploy.js would
  const deployed = {};
  const ifaces = {};
  for (const step of p.deploy) {
    const { abi, bytecode } = artifacts[step.name];
    const iface = new Interface(abi);
    ifaces[step.name] = iface;
    const args = step.args.map((a) => resolve(a, deployed));
    const data = bytecode + iface.encodeDeploy(args).slice(2);
    const r = await vm.evm.runCall({
      caller: OWNER,
      data: hexToBytes(data),
      gasLimit: 20000000n,
      block,
    });
    if (r.execResult.exceptionError) {
      throw new Error(`${step.name} deploy reverted: ${r.execResult.exceptionError.error}`);
    }
    deployed[step.name] = r.createdAddress.toString();
  }
  for (const call of p.calls) {
    const iface = ifaces[call.on];
    const args = call.args.map((a) => resolve(a, deployed));
    const r = await vm.evm.runCall({
      caller: OWNER,
      to: Address.fromString(deployed[call.on]),
      data: hexToBytes(iface.encodeFunctionData(call.fn, args)),
      gasLimit: 10000000n,
      block,
    });
    if (r.execResult.exceptionError) {
      throw new Error(`${call.on}.${call.fn} reverted: ${r.execResult.exceptionError.error}`);
    }
  }

  const read = async (name, fn, args = []) => {
    const iface = ifaces[name];
    const r = await vm.evm.runCall({
      caller: OWNER,
      to: Address.fromString(deployed[name]),
      data: hexToBytes(iface.encodeFunctionData(fn, args)),
      gasLimit: 10000000n,
      block,
    });
    if (r.execResult.exceptionError) throw new Error(`${name}.${fn} reverted`);
    return iface.decodeFunctionResult(fn, bytesToHex(r.execResult.returnValue))[0];
  };

  console.log("deployment plan");

  await test("every contract in the plan deploys", async () => {
    for (const step of p.deploy) {
      if (!deployed[step.name]) throw new Error(`${step.name} missing`);
    }
    eq(Object.keys(deployed).length, 4, "contract count");
  });

  await test("both adapters are registered on the gateway", async () => {
    eq(await read("SolanaGateway", "isAdapter", [deployed.WormholeAdapter]), true, "wormhole");
    eq(await read("SolanaGateway", "isAdapter", [deployed.LayerZeroAdapter]), true, "layerzero");
  });

  await test("gateway knows its own chain id, so another chain's message is rejected", async () => {
    eq(await read("SolanaGateway", "chainId"), BASE_INTERNAL_ID, "chainId");
  });

  await test("adapters pin the Solana peers they were given", async () => {
    eq(await read("WormholeAdapter", "solanaPeer"), EMITTER, "wormhole peer");
    eq(await read("LayerZeroAdapter", "solanaPeer"), OAPP, "layerzero peer");
  });

  await test("wormhole adapter demands finalized, never confirmed", async () => {
    const level = await read("WormholeAdapter", "minConsistencyLevel");
    eq(level, 1n, "minConsistencyLevel");
    if (level === 0n) throw new Error("0 is 'confirmed' -- a reorg forges calls");
  });

  await test("layerzero adapter is pointed at the source chain's endpoint id", async () => {
    eq(await read("LayerZeroAdapter", "SOLANA_EID"), BigInt(SOLANA_DEVNET_EID), "eid");
  });

  await test("target trusts the gateway and two distinct Solana authorities", async () => {
    eq(await read("ConfigInvokeTarget", "solanaGateway"), deployed.SolanaGateway, "gateway");
    eq(await read("ConfigInvokeTarget", "CONFIGURER"), CONFIGURER, "configurer");
    eq(await read("ConfigInvokeTarget", "INVOKER"), INVOKER, "invoker");
    if (CONFIGURER === INVOKER) throw new Error("one key for both privilege levels");
  });

  await test("quorum is left at 1, to be raised once each transport has landed alone", async () => {
    eq(await read("SolanaGateway", "requiredConfirmations", [deployed.ConfigInvokeTarget]), 0n, "default");
  });

  const failed = results.filter((r) => !r).length;
  console.log(`\n${results.length - failed}/${results.length} passed`);
  process.exit(failed ? 1 : 0);
}

run().catch((e) => {
  console.error(e);
  process.exit(2);
});
