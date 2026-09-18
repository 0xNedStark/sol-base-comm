# Architecture: invoking Base contract functions from Solana

## The problem

You want a Solana transaction to cause a function call on a Base contract.

Base is an OP-Stack L2. Its *native* bridge (`L1CrossDomainMessenger` /
`L2CrossDomainMessenger`) only connects Base to Ethereum L1. There is no native
Solana <-> Base path. Any Solana -> Base call therefore has to ride a
third-party general message passing (GMP) protocol, and the entire security of
your system reduces to that protocol's validator set plus the correctness of the
code on either end of it.

That framing drives the whole design: **the transport is the risk, so the
transport must be swappable, and the authorization must not live in the
transport.**

## Three layers

```
  SOLANA                          TRANSPORT                       BASE
  ------------------------------  ----------------------------    ---------------------------------

  your program                                                    Treasury / Vault / whatever
        | CPI (invoke_signed)                                                ^
        v                                                                    | call
  base_caller program                                             SolanaGateway
    - authorize                                                     - dedupe (messageId)
    - assign nonce                                                  - check expiry
    - build Envelope  ------->  Wormhole core / LayerZero  ----->   - set xDomainMessageSender
    - CPI to transport          endpoint / Hyperlane mailbox        - execute (DIRECT or ACCOUNT)
    - emit CallDispatched            |                              - on revert: park for retry
                                     |                                       ^
                                guardians / DVNs                             | deliver()
                                verify + relay                       *Adapter (Wormhole / LZ / ...)
                                                                       - verify provenance
                                                                       - check emitter == our program
```

1. **Intent layer (Solana).** The `base_caller` Anchor program is the only thing
   that mints outbound envelopes. Callers are either user wallets or other
   Solana programs that CPI in with `invoke_signed`, so the identity that lands
   on Base is a PDA the calling program controls.

   The program splits *prepare* from *dispatch*: `prepare` builds the envelope
   once, assigns the nonce and stores the bytes in a PDA; `dispatch_via_*`
   forwards those bytes over one transport; `finalize` reclaims the rent. This
   is not ceremony -- it is the only way two transports can carry byte-identical
   envelopes, which the dual-transport quorum in D-transport depends on. A
   design with one send instruction per transport gives the same call two
   nonces, two hashes, and a quorum that is never met.

2. **Transport layer.** A pluggable GMP. Solana-side transport module plus a
   Base-side adapter contract. Each side only ever trusts a *registered peer* on
   the other side.

3. **Execution layer (Base).** `SolanaGateway` verifies the envelope arrived via
   an authorized adapter, replay-protects it, and executes the call while
   exposing the originating Solana pubkey to the target.

## Why this shape

**It mirrors the OP Stack messenger semantics Base developers already know.**
The target-side authorization check is:

```solidity
require(msg.sender == address(gateway));
require(gateway.xDomainMessageSender() == EXPECTED_SOLANA_AUTHORITY);
```

which is exactly the `L2CrossDomainMessenger.xDomainMessageSender()` pattern.
Nobody has to learn a new mental model, and the check is one line to audit.

**The transport is a runtime detail, not an architectural commitment.** App
contracts never import a Wormhole or LayerZero type. You can switch transports,
run two in parallel, or require agreement from two independent transports for
high-value calls, without touching a single application contract.

**Authorization lives next to the money.** The gateway is deliberately not the
place where "may Solana key X call function Y" is decided. See D1 below.

---

## Design decisions

### D1. Where does authorization live?

Two candidate models:

| | (a) Gateway allowlist | (b) Target-side check |
|---|---|---|
| Rule | gateway holds `(sender, target, selector) -> bool` | each target checks `xDomainMessageSender()` |
| Adding an integration | gateway governance action | deploy your contract, done |
| Blast radius of gateway owner compromise | every integrated contract | still every contract that trusted the gateway |
| Where an auditor looks | one big central mapping | the contract holding the funds |

**Decision: (b) is mandatory, (a) is available as opt-in hardening.**

A gateway whose owner can authorize arbitrary `(target, selector)` pairs is a
honeypot — one key compromise reaches every integrator at once, and integrators
have no way to defend themselves. With target-side checks, each contract states
its own trust assumption in its own source, and adding an integration needs no
privileged action from anyone.

`SolanaGateway` still offers `setStrictMode(target, true)` plus a per-target
selector allowlist for contracts that want belt-and-braces. It is opt-in and
**enforced by the gateway on top of**, never instead of, the target's own check.

The corresponding failure mode to avoid: a target that checks only
`msg.sender == gateway` and not the origin sender. That contract is callable by
*any* Solana account. The `SolanaCallable` base contract exists so nobody writes
that check by hand.

### D2. Typed intents or raw calldata?

Raw `bytes` forwarded to `target.call(data)` is maximally flexible; typed intents
(an enum plus args, mapped on the Base side to a specific function) are safer but
need a coordinated upgrade on both chains for every new action.

**Decision: raw calldata, with an opt-in per-target selector allowlist.**

Typed intents buy you very little here, because the thing they protect against —
a compromised Solana authority calling something unintended — is already bounded
by the target's own `xDomainMessageSender()` check. Meanwhile they impose a
two-chain upgrade on every new feature, which is a real and recurring cost.
Targets that want the extra bound can register their selector set and get it.

### D3. Whose `msg.sender` does the target see?

Two execution modes, chosen per message via the envelope's `mode` byte:

**`DIRECT` (mode 0).** The gateway calls the target itself. The target reads
`gateway.xDomainMessageSender()` to learn who on Solana asked. Cheapest path.
Correct for contracts you own and have written to be Solana-aware.

**`ACCOUNT` (mode 1).** The gateway routes the call through a `SolanaAccount` —
a minimal CREATE2 clone, one per `(srcChainId, srcSender)` pair, at an address
derived deterministically from the Solana pubkey. The target sees a normal
contract caller with a stable address and no special semantics.

`ACCOUNT` mode matters more than it looks. It gives each Solana sender a durable
Base identity that can hold ETH and tokens, receive NFTs, and hold ERC-20
approvals scoped to itself. Without it, interacting with any third-party contract
that wasn't written for this system means either the gateway holds everyone's
funds in one pot (cross-tenant contamination, and a gateway bug drains all of it)
or you can't hold funds on Base at all. It is effectively a Solana-key-controlled
smart account on Base, and it is the right default for anything touching
contracts you did not write.

Use `DIRECT` for your own Solana-aware contracts; use `ACCOUNT` for everything
else.

### D4. What happens when the inner call reverts?

If a delivery reverts wholesale, the transport will either retry it forever or
consider the message stuck — and with an ordered channel, one poisoned message
blocks every message behind it.

**Decision: catch the revert, park the message, allow permissionless retry.**

`SolanaGateway` executes the inner call in a `try`/low-level `call` and, on
failure, records `failedMessages[messageId] = keccak256(envelope)`, emits
`CallFailed` with the revert data, and returns successfully to the adapter. The
message is consumed from the transport's point of view but not lost: anyone can
call `retry(envelope)` later, optionally with a higher gas limit.

This is the difference between "a target that reverts on a transient condition
costs you a retry" and "a target that reverts wedges the channel."

The rule generalises into a taxonomy the gateway applies to every delivery,
and the wrong response to each class is a live failure mode:

| Class | Examples | Response |
|---|---|---|
| Duplicate | same id, any adapter | consume, emit, return |
| Terminal | malformed, unknown version, wrong chain, expired, forbidden target | mark `Rejected`, emit, return |
| Parkable | target reverted, strict-mode miss, ACCOUNT mode off | mark `Failed`, emit, return; anyone retries |
| Transient | paused, under-gassed | **revert**, so the transport redelivers |

The distinguishing question: *will retrying with no change on our side ever
succeed?* No -> consume. Yes because something external changes -> revert.
Yes because a human can act -> park.

### D5. Ordering and delivery guarantees

Assume **at-least-once, unordered** delivery, because that is the weakest
guarantee across the candidate transports and designing for it costs nothing.

- *Idempotency* is unconditional: `messageId = keccak256(envelope)` is marked
  consumed before execution. A duplicate delivery is a no-op, whichever adapter
  it arrives through.
- *Ordering*, if an application needs it, is the application's to enforce. The
  per-sender `nonce` is in the envelope and is passed to the target, so a
  contract that needs sequencing can keep a `lastNonce` and reject out-of-order
  messages itself.

Do not enforce global ordering in the gateway. It converts every stuck message
into a full outage, which is precisely what D4 is trying to avoid.

### D6. Fees

Execution on Base has to be paid for from Solana. The envelope carries a
`gasLimit` and a `value` (native ETH to attach, funded by the transport's native
drop or by a prefunded gateway balance). The Solana program exposes a `quote`
instruction that asks the transport what the send costs in lamports, so the
client can attach the right fee.

Gas accounting detail worth getting right: before the inner call, the gateway
requires `gasleft() >= gasLimit * 64 / 63 + RESERVE`. Without it, a relayer can
submit the delivery with just barely too little gas, the inner call gets 63/64ths
of not-enough, reverts out of gas, and the message is consumed as "failed" — a
cheap griefing vector that this one check closes.

### D7. Return path (Base -> Solana)

Out of scope for phase 1, symmetric when you want it: `SolanaGateway` emits an
envelope, the same adapter set carries it back, and a Solana `base_inbox` program
verifies and dispatches via CPI. The envelope format is already direction-neutral.
Design the phase-1 contracts so this is additive — in particular, do not assume
anywhere that `srcChainId` is always Solana.

---

## What this does *not* protect against

Stated plainly, because the temptation is to believe an abstraction layer buys
more than it does:

- **The transport's validator set.** If Wormhole's guardians or your configured
  LayerZero DVN set collude or are compromised, they can forge an envelope from
  any Solana sender, and every check in this design passes. The adapter
  abstraction lets you *require two independent transports* for high-value calls,
  which is the only real mitigation. Nothing else on this list matters as much.
- **A compromised Solana authority key.** If the key or PDA that signs
  `send_call` is compromised, it can do on Base whatever you authorized it to do.
  Scope each authority narrowly and use separate authorities per privilege level.
- **Reorgs on Solana.** Messages must be attested at a finalized commitment, not
  `confirmed`. A message attested on a slot that gets rolled back is a free
  mint of a fraudulent call. This is a transport configuration setting
  (Wormhole's consistency level, LayerZero's confirmations) and it is easy to get
  wrong in the direction of "faster."
