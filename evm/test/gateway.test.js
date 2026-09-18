// SolanaGateway behaviour tests on an in-process EVM (Cancun, so tstore/tload
// work). Run: node test/build.js && node test/gateway.test.js
//
// Every row of the failure taxonomy gets a case, plus replay, quorum, the gas
// floor, retry, the config-version pattern end to end, and ACCOUNT mode.
const { VM } = require('@ethereumjs/vm');
const { Common, Chain, Hardfork } = require('@ethereumjs/common');
const { Block } = require('@ethereumjs/block');
const { Address, Account, hexToBytes, bytesToHex } = require('@ethereumjs/util');
const { Interface, AbiCoder, keccak256, concat, zeroPadValue, toBeHex } = require('ethers');
const fs = require('fs');
const path = require('path');

const artifacts = JSON.parse(fs.readFileSync(path.join(__dirname, 'out', 'artifacts.json'), 'utf8'));

// ------------------------------------------------------------------ helpers

const CHAIN_SOLANA = 1, CHAIN_BASE = 3;
const MODE_DIRECT = 0, MODE_ACCOUNT = 1;
const Status = { None: 0, Pending: 1, Executed: 2, Failed: 3, Rejected: 4 };
const Reason = {
  None: 0, Malformed: 1, BadVersion: 2, BadMessageType: 3, BadSourceChain: 4,
  BadDestinationChain: 5, BadMode: 6, Expired: 7, ValueNotSupported: 8,
  ForbiddenTarget: 9, AccountModeDisabled: 10, NotAllowed: 11, InnerCallReverted: 12,
};

const OWNER = Address.fromString('0x1000000000000000000000000000000000000001');
const ADAPTER_A = Address.fromString('0x2000000000000000000000000000000000000002');
const ADAPTER_B = Address.fromString('0x2000000000000000000000000000000000000003');
const STRANGER = Address.fromString('0x3000000000000000000000000000000000000004');

const CONFIGURER = '0x' + '11'.repeat(32);
const INVOKER = '0x' + '22'.repeat(32);
const NOBODY = '0x' + '33'.repeat(32);

const NOW = 1_700_000_000n;

// Mirrors docs/02-message-format.md exactly. Cross-checked against the Rust
// encoder's golden vector by test/envelope-parity.js.
function encodeEnvelope(o) {
  const be = (v, bytes) => zeroPadValue(toBeHex(BigInt(v)), bytes);
  const calldata = o.calldata ?? '0x';
  const len = (calldata.length - 2) / 2;
  return concat([
    be(o.version ?? 1, 1),
    be(o.msgType ?? 0, 1),
    be(o.srcChainId ?? CHAIN_SOLANA, 2),
    be(o.dstChainId ?? CHAIN_BASE, 2),
    o.sender,
    be(o.nonce, 8),
    o.target,
    be(o.value ?? 0, 16),
    be(o.gasLimit ?? 300_000, 8),
    be(o.expiry ?? 0, 8),
    be(o.mode ?? MODE_DIRECT, 1),
    be(len, 4),
    calldata,
  ]);
}

const allAbis = new Interface(
  Object.values(artifacts).flatMap((a) => a.abi)
    .filter((f) => f.type !== 'constructor')
    // dedupe by signature so Interface does not complain
    .filter((f, i, arr) => arr.findIndex((g) => JSON.stringify(g) === JSON.stringify(f)) === i)
);

class Chain_ {
  static async create() {
    const c = new Chain_();
    c.common = new Common({ chain: Chain.Mainnet, hardfork: Hardfork.Cancun });
    c.vm = await VM.create({ common: c.common });
    for (const a of [OWNER, ADAPTER_A, ADAPTER_B, STRANGER]) {
      await c.vm.stateManager.putAccount(a, new Account(0n, 10n ** 24n));
    }
    c.timestamp = NOW;
    return c;
  }
  block() {
    return Block.fromBlockData({ header: { timestamp: this.timestamp, gasLimit: 30_000_000n, number: 1n } }, { common: this.common });
  }
  async deploy(name, args = [], from = OWNER) {
    const { abi, bytecode } = artifacts[name];
    const iface = new Interface(abi);
    const data = bytecode + (args.length ? iface.encodeDeploy(args).slice(2) : '');
    const r = await this.vm.evm.runCall({ caller: from, data: hexToBytes(data), gasLimit: 20_000_000n, block: this.block() });
    if (r.execResult.exceptionError) throw new Error(`deploy ${name}: ${r.execResult.exceptionError.error}`);
    return { address: r.createdAddress, iface };
  }
  async call(to, iface, fn, args = [], { from = OWNER, gasLimit = 10_000_000n } = {}) {
    const data = hexToBytes(iface.encodeFunctionData(fn, args));
    const r = await this.vm.evm.runCall({ caller: from, to, data, gasLimit, block: this.block() });
    const ret = bytesToHex(r.execResult.returnValue);
    const logs = r.execResult.logs.map(([addr, topics, d]) => {
      try {
        return allAbis.parseLog({ topics: topics.map(bytesToHex), data: bytesToHex(d) });
      } catch { return null; }
    }).filter(Boolean);
    let error = null;
    if (r.execResult.exceptionError) {
      let decoded = null;
      try { decoded = allAbis.parseError(ret); } catch {}
      error = { kind: r.execResult.exceptionError.error, name: decoded?.name ?? null, args: decoded?.args ?? null };
    }
    let result = null;
    if (!error) {
      try { result = iface.decodeFunctionResult(fn, ret); } catch {}
    }
    return { ok: !error, error, logs, result, gasUsed: r.execResult.executionGasUsed };
  }
}

// ------------------------------------------------------------- tiny runner

const results = [];
async function test(name, fn) {
  try { await fn(); results.push({ name, ok: true }); console.log(`  ok   ${name}`); }
  catch (e) { results.push({ name, ok: false, e }); console.log(`  FAIL ${name}\n       ${e.message}`); }
}
function eq(a, b, what) {
  const sa = typeof a === 'bigint' ? a.toString() : JSON.stringify(a);
  const sb = typeof b === 'bigint' ? b.toString() : JSON.stringify(b);
  if (sa !== sb) throw new Error(`${what ?? 'assert'}: got ${sa}, want ${sb}`);
}
function has(logs, name, pred = () => true) {
  const l = logs.find((x) => x.name === name && pred(x.args));
  if (!l) throw new Error(`expected event ${name}; got [${logs.map((x) => x.name).join(', ')}]`);
  return l;
}

// ------------------------------------------------------------------ fixture

async function fixture() {
  const c = await Chain_.create();
  const gw = await c.deploy('SolanaGateway', [OWNER.toString(), CHAIN_BASE]);
  const tgt = await c.deploy('ConfigInvokeTarget', [gw.address.toString(), CONFIGURER, INVOKER]);
  const counter = await c.deploy('Counter');
  for (const a of [ADAPTER_A, ADAPTER_B]) {
    const r = await c.call(gw.address, gw.iface, 'setAdapter', [a.toString(), true]);
    if (!r.ok) throw new Error('setAdapter failed');
  }
  const status = async (env) => (await c.call(gw.address, gw.iface, 'statusOf', [keccak256(env)])).result[0];
  const deliver = (env, from = ADAPTER_A, gasLimit) => c.call(gw.address, gw.iface, 'deliver', [env], { from, gasLimit });
  const retry = (env, override = 0, from = STRANGER) => c.call(gw.address, gw.iface, 'retry', [env, override], { from });
  const configCalldata = (recipient, limit, enabled) =>
    tgt.iface.encodeFunctionData('setConfig', [{ recipient, limit, enabled }]);
  const invokeCalldata = (version, args) => tgt.iface.encodeFunctionData('invoke', [version, args]);
  let nonce = 0n;
  const env = (o) => encodeEnvelope({ nonce: ++nonce, target: tgt.address.toString(), ...o });
  return { c, gw, tgt, counter, status, deliver, retry, configCalldata, invokeCalldata, env };
}

// -------------------------------------------------------------------- tests

(async () => {
  console.log('SolanaGateway');

  await test('rejects deliver from a non-adapter (revert, not consume)', async () => {
    const f = await fixture();
    const e = f.env({ sender: CONFIGURER, calldata: f.configCalldata(STRANGER.toString(), 1, true) });
    const r = await f.deliver(e, STRANGER);
    eq(r.ok, false); eq(r.error.name, 'NotAdapter');
    eq(await f.status(e), Status.None, 'untouched');
  });

  await test('DIRECT happy path: setConfig lands, xDomainMessageSender seen by target', async () => {
    const f = await fixture();
    const e = f.env({ sender: CONFIGURER, calldata: f.configCalldata(STRANGER.toString(), 42, true) });
    const r = await f.deliver(e);
    eq(r.ok, true, r.error && JSON.stringify(r.error));
    has(r.logs, 'CallExecuted');
    has(r.logs, 'ConfigSet', (a) => a.version === 1n && a.limit === 42n);
    eq(await f.status(e), Status.Executed);
    const v = await f.c.call(f.tgt.address, f.tgt.iface, 'configVersion');
    eq(v.result[0], 1n);
    // transient storage cleared after execution
    const x = await f.c.call(f.gw.address, f.gw.iface, 'xDomainMessageSender');
    eq(x.result[0], '0x' + '00'.repeat(32), 'xdomain cleared');
  });

  await test('duplicate delivery is a no-op, not a revert, and does not re-execute', async () => {
    const f = await fixture();
    const e = f.env({ sender: CONFIGURER, calldata: f.configCalldata(STRANGER.toString(), 1, true) });
    await f.deliver(e);
    const r = await f.deliver(e);
    eq(r.ok, true); has(r.logs, 'MessageDuplicate');
    const v = await f.c.call(f.tgt.address, f.tgt.iface, 'configVersion');
    eq(v.result[0], 1n, 'not re-executed');
    // and the same envelope through a second adapter after execution is also a duplicate
    const r2 = await f.deliver(e, ADAPTER_B);
    eq(r2.ok, true); has(r2.logs, 'MessageDuplicate');
  });

  // ---- terminal rejections: consumed, never revert, never retryable
  const terminal = [
    ['malformed (too short)', (f) => '0x0100', Reason.Malformed],
    ['malformed (length mismatch)', (f) => f.env({ sender: CONFIGURER, calldata: '0xdeadbeef' }) + 'ff', Reason.Malformed],
    ['unknown version', (f) => f.env({ version: 9, sender: CONFIGURER, calldata: '0x' }), Reason.BadVersion],
    ['unknown msgType', (f) => f.env({ msgType: 7, sender: CONFIGURER, calldata: '0x' }), Reason.BadMessageType],
    ['wrong source chain', (f) => f.env({ srcChainId: 2, sender: CONFIGURER, calldata: '0x' }), Reason.BadSourceChain],
    ['wrong destination chain', (f) => f.env({ dstChainId: 2, sender: CONFIGURER, calldata: '0x' }), Reason.BadDestinationChain],
    ['unknown mode', (f) => f.env({ mode: 5, sender: CONFIGURER, calldata: '0x' }), Reason.BadMode],
    ['expired', (f) => f.env({ expiry: NOW - 1n, sender: CONFIGURER, calldata: '0x' }), Reason.Expired],
    ['non-zero value (phase one)', (f) => f.env({ value: 1, sender: CONFIGURER, calldata: '0x' }), Reason.ValueNotSupported],
    ['forbidden target: gateway itself', (f) => f.env({ sender: CONFIGURER, target: f.gw.address.toString(), calldata: '0x' }), Reason.ForbiddenTarget],
    ['forbidden target: an adapter', (f) => f.env({ sender: CONFIGURER, target: ADAPTER_B.toString(), calldata: '0x' }), Reason.ForbiddenTarget],
    ['forbidden target: zero address', (f) => f.env({ sender: CONFIGURER, target: '0x' + '00'.repeat(20), calldata: '0x' }), Reason.ForbiddenTarget],
  ];
  for (const [name, mk, reason] of terminal) {
    await test(`terminal: ${name} -> Rejected(${reason}), no revert`, async () => {
      const f = await fixture();
      const e = mk(f);
      const r = await f.deliver(e);
      eq(r.ok, true, r.error && JSON.stringify(r.error));
      has(r.logs, 'MessageRejected', (a) => Number(a.reason) === reason);
      eq(await f.status(e), Status.Rejected);
      const rr = await f.retry(e);
      eq(rr.ok, false); eq(rr.error.name, 'NotFailed', 'terminal is not retryable');
    });
  }

  await test('expiry is enforced at quorum time and on retry, not just first delivery', async () => {
    const f = await fixture();
    await f.c.call(f.gw.address, f.gw.iface, 'setRequiredConfirmations', [f.tgt.address.toString(), 2]);
    const e = f.env({ sender: CONFIGURER, expiry: NOW + 100n, calldata: f.configCalldata(STRANGER.toString(), 1, true) });
    const r1 = await f.deliver(e, ADAPTER_A);
    eq(r1.ok, true); eq(await f.status(e), Status.Pending);
    f.c.timestamp = NOW + 101n;
    const r2 = await f.deliver(e, ADAPTER_B);
    eq(r2.ok, true); has(r2.logs, 'MessageRejected', (a) => Number(a.reason) === Reason.Expired);
    eq(await f.status(e), Status.Rejected);
  });

  // ---- parked failures: consumed, retryable
  await test('inner revert (wrong Solana sender) -> Failed with revert data, retryable', async () => {
    const f = await fixture();
    const e = f.env({ sender: NOBODY, calldata: f.configCalldata(STRANGER.toString(), 1, true) });
    const r = await f.deliver(e);
    eq(r.ok, true);
    const ev = has(r.logs, 'CallFailed', (a) => Number(a.reason) === Reason.InnerCallReverted);
    const inner = allAbis.parseError(ev.args.returnData);
    eq(inner.name, 'UnauthorizedSolanaSender');
    eq(await f.status(e), Status.Failed);
  });

  await test('config-version pattern end to end: stale invoke parks, config lands, retry succeeds', async () => {
    const f = await fixture();
    // invoke expecting version 1 arrives BEFORE the config that creates version 1
    const inv = f.env({ sender: INVOKER, calldata: f.invokeCalldata(1, '0xabcdef') });
    const r1 = await f.deliver(inv);
    eq(r1.ok, true);
    const ev = has(r1.logs, 'CallFailed', (a) => Number(a.reason) === Reason.InnerCallReverted);
    eq(allAbis.parseError(ev.args.returnData).name, 'StaleConfig');
    eq(await f.status(inv), Status.Failed);
    // now the config arrives (out of order, as transports allow)
    const cfg = f.env({ sender: CONFIGURER, calldata: f.configCalldata(STRANGER.toString(), 7, true) });
    eq((await f.deliver(cfg)).ok, true);
    // anyone retries the parked invoke
    const r2 = await f.retry(inv);
    eq(r2.ok, true, r2.error && JSON.stringify(r2.error));
    has(r2.logs, 'Invoked', (a) => a.configVersion === 1n);
    eq(await f.status(inv), Status.Executed);
    const n = await f.c.call(f.tgt.address, f.tgt.iface, 'invocations');
    eq(n.result[0], 1n);
    // and a second retry of an executed message is refused
    const r3 = await f.retry(inv);
    eq(r3.ok, false); eq(r3.error.name, 'NotFailed');
  });

  await test('retry with a lower gas override than the envelope is refused', async () => {
    const f = await fixture();
    const inv = f.env({ sender: INVOKER, gasLimit: 300_000, calldata: f.invokeCalldata(1, '0x') });
    await f.deliver(inv);
    const r = await f.retry(inv, 100_000);
    eq(r.ok, false); eq(r.error.name, 'GasLimitTooLow');
  });

  await test('strict mode: un-allowlisted call parks as NotAllowed; allowing it makes retry succeed', async () => {
    const f = await fixture();
    await f.c.call(f.gw.address, f.gw.iface, 'setStrictMode', [f.tgt.address.toString(), true]);
    const e = f.env({ sender: CONFIGURER, calldata: f.configCalldata(STRANGER.toString(), 1, true) });
    const r1 = await f.deliver(e);
    eq(r1.ok, true); has(r1.logs, 'CallFailed', (a) => Number(a.reason) === Reason.NotAllowed);
    eq(await f.status(e), Status.Failed);
    const sel = f.tgt.iface.getFunction('setConfig').selector;
    await f.c.call(f.gw.address, f.gw.iface, 'setAllowedCall', [f.tgt.address.toString(), CONFIGURER, sel, true]);
    const r2 = await f.retry(e);
    eq(r2.ok, true); has(r2.logs, 'CallExecuted');
  });

  // ---- transient: revert so the transport redelivers
  await test('under-gassed delivery reverts InsufficientGas and leaves the message untouched', async () => {
    const f = await fixture();
    const e = f.env({ sender: CONFIGURER, gasLimit: 300_000, calldata: f.configCalldata(STRANGER.toString(), 1, true) });
    const r = await f.deliver(e, ADAPTER_A, 200_000n);
    eq(r.ok, false);
    eq(r.error.name, 'InsufficientGas');
    eq(await f.status(e), Status.None, 'nothing recorded; redelivery can succeed');
    const r2 = await f.deliver(e);
    eq(r2.ok, true); has(r2.logs, 'CallExecuted');
  });

  await test('paused gateway reverts (transient), and resumes', async () => {
    const f = await fixture();
    await f.c.call(f.gw.address, f.gw.iface, 'setPaused', [true]);
    const e = f.env({ sender: CONFIGURER, calldata: f.configCalldata(STRANGER.toString(), 1, true) });
    const r = await f.deliver(e);
    eq(r.ok, false); eq(r.error.name, 'Paused');
    await f.c.call(f.gw.address, f.gw.iface, 'setPaused', [false]);
    eq((await f.deliver(e)).ok, true);
  });

  // ---- quorum
  await test('quorum of 2: first adapter -> Pending, same adapter again -> duplicate, second adapter -> Executed', async () => {
    const f = await fixture();
    await f.c.call(f.gw.address, f.gw.iface, 'setRequiredConfirmations', [f.tgt.address.toString(), 2]);
    const e = f.env({ sender: CONFIGURER, calldata: f.configCalldata(STRANGER.toString(), 1, true) });
    const r1 = await f.deliver(e, ADAPTER_A);
    eq(r1.ok, true); has(r1.logs, 'MessageDelivered', (a) => a.confirmations === 1n);
    eq(await f.status(e), Status.Pending);
    const r1b = await f.deliver(e, ADAPTER_A);
    eq(r1b.ok, true); has(r1b.logs, 'MessageDuplicate');
    eq(await f.status(e), Status.Pending, 'same adapter cannot vote twice');
    const r2 = await f.deliver(e, ADAPTER_B);
    eq(r2.ok, true); has(r2.logs, 'CallExecuted');
    eq(await f.status(e), Status.Executed);
    const v = await f.c.call(f.tgt.address, f.tgt.iface, 'configVersion');
    eq(v.result[0], 1n, 'executed exactly once');
  });

  // ---- ACCOUNT mode
  await test('ACCOUNT mode disabled -> parked; enabled -> retry deploys account at accountFor() and executes', async () => {
    const f = await fixture();
    const inc = f.counter.iface.encodeFunctionData('increment', []);
    const e = f.env({ sender: INVOKER, mode: MODE_ACCOUNT, target: f.counter.address.toString(), calldata: inc });
    const r1 = await f.deliver(e);
    eq(r1.ok, true); has(r1.logs, 'CallFailed', (a) => Number(a.reason) === Reason.AccountModeDisabled);
    await f.c.call(f.gw.address, f.gw.iface, 'setAccountModeEnabled', [true]);
    const predicted = (await f.c.call(f.gw.address, f.gw.iface, 'accountFor', [INVOKER])).result[0];
    const r2 = await f.retry(e);
    eq(r2.ok, true, r2.error && JSON.stringify(r2.error));
    has(r2.logs, 'AccountDeployed', (a) => a.account.toLowerCase() === predicted.toLowerCase());
    has(r2.logs, 'CallExecuted');
    const lc = await f.c.call(f.counter.address, f.counter.iface, 'lastCaller');
    eq(lc.result[0].toLowerCase(), predicted.toLowerCase(), 'target saw the account as msg.sender');
    // second message from the same sender reuses the account (no second deploy)
    const e2 = f.env({ sender: INVOKER, mode: MODE_ACCOUNT, target: f.counter.address.toString(), calldata: inc });
    const r3 = await f.deliver(e2);
    eq(r3.ok, true); has(r3.logs, 'CallExecuted');
    eq(r3.logs.some((l) => l.name === 'AccountDeployed'), false, 'no redeploy');
    const cnt = await f.c.call(f.counter.address, f.counter.iface, 'count');
    eq(cnt.result[0], 2n);
  });

  await test('a deployed SolanaAccount is a forbidden DIRECT target (T11)', async () => {
    const f = await fixture();
    await f.c.call(f.gw.address, f.gw.iface, 'setAccountModeEnabled', [true]);
    const inc = f.counter.iface.encodeFunctionData('increment', []);
    await f.deliver(f.env({ sender: INVOKER, mode: MODE_ACCOUNT, target: f.counter.address.toString(), calldata: inc }));
    const victim = (await f.c.call(f.gw.address, f.gw.iface, 'accountFor', [INVOKER])).result[0];
    // an attacker-controlled Solana sender tries to drive the victim's account in DIRECT mode
    const attack = f.env({
      sender: NOBODY, mode: MODE_DIRECT, target: victim,
      calldata: new Interface(artifacts.SolanaAccount.abi).encodeFunctionData('execute', [f.counter.address.toString(), 0, inc, 100_000]),
    });
    const r = await f.deliver(attack);
    eq(r.ok, true); has(r.logs, 'MessageRejected', (a) => Number(a.reason) === Reason.ForbiddenTarget);
  });

  await test('SolanaCallable target in ACCOUNT mode correctly refuses (caller is the account, not the gateway)', async () => {
    const f = await fixture();
    await f.c.call(f.gw.address, f.gw.iface, 'setAccountModeEnabled', [true]);
    const e = f.env({ sender: CONFIGURER, mode: MODE_ACCOUNT, calldata: f.configCalldata(STRANGER.toString(), 1, true) });
    const r = await f.deliver(e);
    eq(r.ok, true);
    const ev = has(r.logs, 'CallFailed', (a) => Number(a.reason) === Reason.InnerCallReverted);
    eq(allAbis.parseError(ev.args.returnData).name, 'NotSolanaGateway');
  });

  // ---- versions
  await test('version set: v2 rejected until accepted', async () => {
    const f = await fixture();
    const e = f.env({ version: 2, sender: CONFIGURER, calldata: f.configCalldata(STRANGER.toString(), 1, true) });
    const r1 = await f.deliver(e);
    has(r1.logs, 'MessageRejected', (a) => Number(a.reason) === Reason.BadVersion);
    await f.c.call(f.gw.address, f.gw.iface, 'setAcceptedVersion', [2, true]);
    // a *new* v2 envelope (the rejected one is terminal by design)
    const e2 = f.env({ version: 2, sender: CONFIGURER, calldata: f.configCalldata(STRANGER.toString(), 1, true) });
    const r2 = await f.deliver(e2);
    eq(r2.ok, true); has(r2.logs, 'CallExecuted');
  });

  await test('non-owner cannot register adapters or change settings', async () => {
    const f = await fixture();
    const r = await f.c.call(f.gw.address, f.gw.iface, 'setAdapter', [STRANGER.toString(), true], { from: STRANGER });
    eq(r.ok, false); eq(r.error.name, 'NotOwner');
  });

  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} passed`);
  process.exit(failed ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(2); });
