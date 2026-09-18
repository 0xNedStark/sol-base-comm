# sol-base-comm

Invoke Base contract functions from Solana.

A Solana transaction calls a program; a function call happens on a Base contract.
Base's native bridge only reaches Ethereum L1, so a third-party GMP protocol
carries the message — and because that protocol's validator set becomes your
trust root, the design keeps it swappable and keeps authorization out of it.

```
  SOLANA                       TRANSPORT                    BASE
  your program                                              Treasury / Vault / ...
      | CPI                                                        ^
  base_caller  ---- envelope ---->  Wormhole / LayerZero  --->  SolanaGateway
   nonce, encode                    guardians / DVNs             verify, dedupe, execute
                                                                 exposes xDomainMessageSender()
```

## What integration costs

One immutable and one modifier:

```solidity
contract Treasury is SolanaCallable {
    bytes32 public immutable TREASURER; // a Solana PDA

    function pay(address token, address to, uint256 amount)
        external
        onlySolanaSender(TREASURER)
    { ... }
}
```

`onlySolanaSender` checks both that the caller is the gateway *and* that the
Solana pubkey behind the call is the one you expect. A contract that checks only
the first is callable by every Solana account in existence — which is why the
base contract exists rather than leaving it to each integrator.

From Solana, a program CPIs in with its own PDA as the authority:

```rust
// 1. Build the envelope once; pick which transports will carry it.
base_caller::cpi::prepare(
    CpiContext::new_with_signer(program, accounts, &[&[b"treasurer", &[bump]]]),
    SendParams {
        dst_chain_id: CHAIN_BASE,
        target: TREASURY_ADDR,
        value: 0,
        gas_limit: 200_000,
        expiry: now + 3600,
        mode: MODE_DIRECT,
        calldata: abi_encode_pay(token, to, amount),
    },
    MASK_WORMHOLE | MASK_LAYERZERO,
)?;

// 2. Dispatch the same bytes over each transport (permissionless; any payer).
base_caller::cpi::dispatch_via_wormhole(ctx_wh, nonce, 0)?;
base_caller::cpi::dispatch_via_layerzero(ctx_lz, nonce, native_fee)?;

// 3. Reclaim rent once every expected transport has carried it.
base_caller::cpi::finalize(ctx_fin, nonce)?;
```

The identity that arrives on Base is a PDA no human holds. Because both
transports carry byte-identical envelopes, the gateway derives one message id
for both, which is what makes `requiredConfirmations = 2` reachable.

## Docs

| | |
|---|---|
| [`docs/DESIGN.md`](docs/DESIGN.md) | **The generalised design doc** (Solana → any EVM chain): status, architecture, envelope, decisions, transports, threat model, interfaces, adoption path |
| [`docs/01-architecture.md`](docs/01-architecture.md) | The design, the decisions behind it, and what it does **not** protect against |
| [`docs/02-message-format.md`](docs/02-message-format.md) | Envelope wire format |
| [`docs/03-transport-comparison.md`](docs/03-transport-comparison.md) | Wormhole vs LayerZero vs Hyperlane vs CCIP vs Axelar, and the recommendation |
| [`docs/04-security.md`](docs/04-security.md) | Threat model and pre-deployment checklist |
| [`docs/05-operations.md`](docs/05-operations.md) | Deploy order, relayer, monitoring, incident response |

## Layout

```
evm/src/
  SolanaGateway.sol         entry point: classify every delivery (duplicate / terminal /
                            parked / transient / execute), dedupe, quorum, version set
  SolanaAccount.sol         per-sender CREATE2 smart account (ACCOUNT mode)
  SolanaCallable.sol        inherit this in target contracts
  Auth.sol                  minimal ownership / pause / reentrancy
  libraries/EnvelopeLib.sol envelope decoder
  adapters/                 WormholeAdapter, LayerZeroAdapter
  examples/ConfigInvokeTarget.sol
                            the phase-one target: two Solana authorities, config-version
                            guard instead of message ordering
evm/test/
  gateway.test.js           28 behaviour tests on an in-process Cancun EVM
  envelope-parity.js        Solidity offsets vs. the Rust encoder's actual bytes
  build.js                  solc-js build to test/out/artifacts.json

solana/programs/base-caller/src/
  lib.rs                    prepare / dispatch_via_* / finalize, accounts, validation
  envelope.rs               canonical encoder (unit-tested, golden vector)
  state.rs                  Config, SenderState, TransportConfig, PreparedMessage
  transports/               wormhole.rs (skeleton), layerzero.rs (stub)
solana/programs/mock-transport/
                            localnet stand-in for a bridge: accepts any instruction
solana/program-tests/       BanksClient tests against the real .so (own workspace)
solana/tests/outbox.ts      TypeScript end-to-end through the Anchor client
```

## Design decisions at a glance

Full reasoning in `docs/01-architecture.md`.

- **Authorization lives in the target, not the gateway.** A gateway that can
  authorize arbitrary `(target, selector)` pairs is a honeypot — one key
  compromise reaches every integrator, and integrators cannot defend themselves.
  Optional `strictMode` stacks on top, never instead.
- **Transport is a runtime detail.** No application contract imports a Wormhole
  or LayerZero type. Switching transports is a config change.
- **Two execution modes.** `DIRECT` for your own Solana-aware contracts;
  `ACCOUNT` routes through a per-sender CREATE2 smart account so each Solana
  caller has a durable Base identity that can hold funds and approvals without
  pooling everyone's assets in the gateway.
- **Failures park, they don't revert.** A reverting target is recorded as
  `Failed` and retried permissionlessly, so one bad message cannot wedge the
  channel.
- **Unordered delivery.** Ordering is the application's to enforce against the
  envelope nonce. Enforcing it globally turns any stuck message into an outage.
- **Dual-transport mode for high-value targets.** `requiredConfirmations = 2`
  means forging a call needs two disjoint validator sets at once. This is the
  only control that addresses transport compromise; everything else assumes the
  transport is honest.

## Status

This is a design plus a reference implementation, not audited production code.

| | |
|---|---|
| Solidity (12 files) | **compiles clean** — solc 0.8.28, cancun, optimizer on |
| Gateway tests | **28/28 pass** on an in-process EVM: every failure-taxonomy row, replay, quorum, gas floor, retry, strict mode, ACCOUNT mode, T11 account-drain attempt, config-version pattern end to end |
| `envelope.rs` | **5 unit tests pass**, incl. a frozen golden vector |
| Cross-language parity | **verified** — Solidity offsets/shifts checked against the bytes Rust actually emits |
| Anchor program | **builds for SBF and executes**: 5 program-tests through BanksClient on the real binary, and 4 TypeScript tests through the Anchor client on `solana-test-validator` |
| `transports/wormhole.rs` | skeleton; account ordering and the fee layout need checking against the deployed core bridge |
| `transports/layerzero.rs` | deliberate stub — wire to the official Solana OApp SDK rather than hand-encoding the endpoint CPI |
| Testnet end to end | **not done** — the one thing nothing above covers |

**Constants to verify before deploying anything with value.** The docs sites for
LayerZero and Wormhole were not reachable from the environment this was written
in. Verified: Wormhole chain ids (Solana 1, Base 30) and LayerZero's Solana
endpoint id (30168). Unverified and marked in-source: LayerZero's Base endpoint
id (believed 30184), Wormhole's numeric consistency-level encoding, core bridge
account ordering, and the bridge config fee offset.

## Running the checks

```
# EVM: build + 28 gateway tests + cross-language parity
cd evm && npm install && npm test

# Solana, no toolchain beyond rustup: type-check against real anchor-lang
cd solana && cargo check -p base-caller

# Solana, full: SBF build, IDL, program-tests, and the localnet end-to-end
cd solana && npm install
npm run build                                   # cargo build-sbf + IDL + TS types
(cd program-tests && SBF_OUT_DIR=../target/deploy cargo test)   # BanksClient, real .so
solana-test-validator -r \
  --bpf-program $(grep ^base_caller Anchor.toml | cut -d'"' -f2) target/deploy/base_caller.so \
  --bpf-program $(grep ^mock_transport Anchor.toml | cut -d'"' -f2) target/deploy/mock_transport.so &
npm run test:local                              # ts-mocha through the Anchor client
```

Toolchain the Solana side was verified with: Agave 1.18.26 (`solana`,
`cargo-build-sbf`, platform-tools v1.41), Anchor CLI 0.30.1, host Rust 1.85.0
for the workspace plus `nightly-2025-03-10` for Anchor's IDL step. Four things
about that combination are handled in-repo so they do not need rediscovering:

| Problem | Where it is handled |
|---|---|
| platform-tools' cargo cannot read v4 lockfiles | `solana/Cargo.lock` is committed as v3 |
| Host resolver locks crates the SBF toolchain cannot build | `rust-version = "1.75"` on the programs, `.cargo/config.toml` MSRV-aware resolution, `blake3` pinned to 1.8.2 |
| Anchor's IDL step runs `cargo +nightly` and needs a pre-April-2025 nightly | npm scripts set `RUSTUP_TOOLCHAIN=nightly-2025-03-10`; `proc-macro2` pinned to 1.0.94 |
| `solana-program-test` drags a modern dependency tree into the lockfile | `solana/program-tests` is its own workspace |

`anchor test` with its self-managed validator reported the RPC port busy in the
environment this was built in; `npm run test:local` against a validator you
start yourself is the path that was verified.

## Phase one scope, as built

Per the design doc's decisions: DIRECT mode only (ACCOUNT mode is present but
gated off by default), `value` reserved-zero, one internal chain registry
(`1 = Solana, 2 = Ethereum, 3 = Base`), two Solana authorities per target with
a config-version guard rather than message ordering, and the failure taxonomy
enforced in the gateway -- revert only when redelivery could succeed.

## Next steps

1. One message end to end on devnet -> Base Sepolia over one adapter. Needs
   funded keys and RPC endpoints.
2. Verify the transport constants and pin them.
3. Wire `transports/layerzero.rs` to the official SDK; add a `quote`
   instruction.
4. Choose the two independent DVN operators for the mainnet config path.
5. Key transport config by (transport, destination) before a second chain.
6. Audit before mainnet.
