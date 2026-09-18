# Choosing a transport

Base has no native Solana bridge, so a third-party GMP protocol carries the
message and its validator set becomes your trust root. This document is the
reasoning behind the default choice and what it would take to change it.

> **Verify before deploying.** Endpoint addresses, chain/endpoint ids, and
> program ids below are from documentation that was not reachable from this
> environment at the time of writing. Confirm every constant against the live
> docs and against the on-chain deployment before you send anything with value.
> The ones marked *(verified)* were confirmed during this design work.

## Candidates

### LayerZero v2 — **recommended default**

Solana OApp program on the source side, `lzReceive` on the Base side. The
Executor delivers and *calls your destination contract automatically*, so you do
not have to run a relayer to get a working system.

The reason it wins for this use case is the DVN (Decentralized Verifier Network)
configuration: the security stack is yours to set. You choose which verifiers
must sign, how many, and how many block confirmations to wait — and you can
require two independent DVNs so that no single verifier operator can forge a
message. That is a per-application security knob, not a protocol-wide constant
you inherit.

- Solana mainnet endpoint id: `30168` *(verified)*
- Base mainnet endpoint id: `30184` — **unverified, confirm**
- Cost: protocol fee plus DVN fees plus executor fee, quoted on the source side.
- Lock-in: message library and DVN config are LayerZero-specific, but it is
  confined to the adapter.

### Wormhole — **recommended second adapter**

The most travelled Solana <-> EVM path. The Solana core bridge `post_message`
instruction emits a message, the 19-guardian network signs a VAA, and the VAA is
submitted to Base where `parseAndVerifyVM` checks the signatures *(verified)*.

Two properties make it valuable as the *second* transport specifically. VAAs are
portable, self-contained, permissionlessly submittable artifacts — if every
relayer in the world disappears, you can still fetch the VAA and land it
yourself, which is a genuinely different liveness profile from a protocol where
delivery depends on a designated executor. And its guardian set is entirely
disjoint from any DVN set you configure on LayerZero, so requiring both gives you
real independence rather than two views of the same validators.

- Wormhole chain id: Solana `1`, Base `30` *(verified)*
- Solana->EVM delivery generally means running your own relayer process, or
  using Wormhole's newer execution service. Budget for the ops either way.
- Cost: small SOL message fee; you pay Base gas yourself if self-relaying.

### Hyperlane

Permissionless deployment with a fully custom Interchain Security Module — you
can run your own validator set and own the trust assumption outright, rather than
renting someone else's. That is the maximum-sovereignty option and the right
answer if you are large enough that being your own validator set is cheaper and
safer than trusting a third party's.

The cost is that you are now operating a validator set and a relayer, with the
monitoring, key management, and on-call that implies. SVM support is less
travelled than its EVM support. Reasonable phase-3 choice; heavy for phase 1.

### Chainlink CCIP

Strong operational risk controls — an independent Risk Management Network and
per-lane rate limits built into the protocol rather than bolted on. Attractive if
your compliance story needs a named, accountable operator. Solana support is
newer than its EVM support, onboarding is more permissioned, and lanes are
configured per pair rather than freely. Worth revisiting if institutional
counterparties are in the picture.

### Axelar

General message passing with a familiar `callContract` / `_execute` shape and a
gas service that handles destination execution, so no relayer to run. Solana
arrives via the Amplifier stack. A perfectly reasonable alternative default; it
loses to LayerZero here mainly on the granularity of per-application security
configuration.

## Comparison

| | LayerZero v2 | Wormhole | Hyperlane | CCIP | Axelar |
|---|---|---|---|---|---|
| Auto-execution on Base | yes (Executor) | run your own / exec service | run your own | yes | yes (gas service) |
| Trust root | your DVN set | 19 guardians | your ISM | DON + RMN | validator set |
| Configurable per-app security | **yes** | no | **yes** | limited | limited |
| Self-serve deployment | yes | yes | yes | permissioned | yes |
| Ops burden | low | medium | high | low | low |
| Solana maturity | good | **highest** | moderate | newer | newer |
| Message artifact | in-protocol | **portable VAA** | in-protocol | in-protocol | in-protocol |

## Decision

**Ship on LayerZero v2. Build the Wormhole adapter in the same phase.**

LayerZero first because auto-execution removes an entire operational component
from the critical path, and the DVN stack means "add a second independent
verifier" is a config change rather than a redeployment.

Wormhole in the same phase, not later, because the whole point of
`ITransportAdapter` is unproven until a second implementation exists. Building
the second adapter is what surfaces the places where transport-specific
assumptions leaked into the gateway, and that is much cheaper to find before
launch than after. It also buys the two things above: a disjoint validator set
for the dual-transport mode below, and a self-relayable fallback if LayerZero
execution degrades.

## Dual-transport mode

For high-value calls, require the same envelope through two transports before
executing:

```
                    +--> LayerZeroAdapter --+
  Solana envelope --+                        +--> SolanaGateway: execute only
                    +--> WormholeAdapter  --+     when quorum for messageId is met
```

Because `messageId` is `keccak256(envelope)` and is transport-independent
(`02-message-format.md`), both adapters naturally produce the same id for the
same message. The gateway counts distinct adapters that have delivered a given
id and executes on the Nth.

This is the only mitigation that actually addresses transport compromise — every
other control in this system assumes the transport is honest. Forging a call now
requires compromising two disjoint validator sets simultaneously.

The costs are real: you pay both transports, you wait for the slower one, and
liveness now depends on both being up rather than either. So it is a per-target
setting (`setRequiredConfirmations(target, 2)`), not a global mode. Turn it on
for the treasury, leave it off for telemetry.

## Switching transports later

The adapter boundary is what makes this cheap, and it is worth being precise
about what "cheap" means:

1. Write the new `ITransportAdapter` implementation.
2. Write the matching Solana transport module behind the `Transport` trait.
3. `gateway.setAdapter(newAdapter, true)` and register peers both ways.
4. Move traffic by changing the default transport in the Solana `Config`.
5. Keep the old adapter enabled until every in-flight message has landed or
   expired, then disable it.

No application contract changes. No envelope format change. No migration of the
dedupe map — `messageId` does not depend on the transport, so a message in flight
on the old transport and a retry on the new one still deduplicate against each
other correctly.
