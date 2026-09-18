# Threat model

Ordered by how much damage the threat does, not by how likely it is. The first
item dominates everything below it, and it is worth being blunt about that: most
of the controls in this system are only meaningful *conditional on the transport
being honest*.

## T1. Transport validator set compromise

**Impact: total.** A colluding or compromised guardian set / DVN set can forge a
VAA or an endpoint delivery for any Solana sender. The adapter verifies the
signatures, the signatures are valid, the gateway executes. Every other check in
this document passes cleanly. This is not a bug in the design; it is the trust
assumption you accept by using a bridge at all.

**Mitigation, and there is only one that works:** require two independent
transports for high-value targets.

```solidity
gateway.setRequiredConfirmations(address(treasury), 2);
```

Because `messageId` is the hash of the transport-independent envelope, both
adapters produce the same id and the gateway executes on the second distinct
adapter. Forging now requires compromising Wormhole's guardians *and* your
LayerZero DVN set at the same time. Choose DVNs that are not operationally
related to the guardian set, or you have bought less independence than you think.

**Secondary mitigation:** value caps per unit time on the target contract, so a
successful forgery is bounded rather than fatal. A rate limit that survives a
total transport compromise is worth more than several controls that don't.

## T2. Solana reorg

A message attested at `confirmed` rather than `finalized` can be published on a
slot that is subsequently rolled back. The Solana state that justified the call
never happened, but the attestation is real and the call executes on Base.

This is the most likely way to get quietly wrecked, because the unsafe setting is
also the fast one, it is a single constant on each side, and everything works
perfectly in testing right up until the one reorg that matters.

**Mitigation:** publish at finalized (`CONSISTENCY_FINALIZED` in
`transports/wormhole.rs`) *and* enforce it on receipt
(`WormholeAdapter.minConsistencyLevel`). Two independent places, because a
mistake in one is then still caught by the other. Add this pair to the deploy
checklist explicitly; do not rely on remembering.

## T3. Compromise of a Solana authority key or PDA

Whoever controls the authority that signs `send_call` can do on Base exactly what
that authority is authorized to do — no more, which is the point of authorizing
per-sender rather than per-gateway.

**Mitigations:**
- Prefer program PDAs over wallet keys. A PDA has no private key to steal; an
  attacker must compromise the program's logic or its upgrade authority.
- One authority per privilege level. Do not let the PDA that authorizes a
  parameter tweak also authorize treasury payouts.
- Put the program's upgrade authority behind a multisig, or make the program
  immutable. An upgradeable program's upgrade authority *is* the authority over
  every Base contract that trusts its PDAs — it is easy to forget that the
  ultimate root of trust is a key on Solana, not anything on Base.
- Set `expiry` so a stolen key's queued messages go stale.

## T4. Target contract checks the gateway but not the sender

A contract that checks only `msg.sender == address(gateway)` is callable by
**every Solana account in existence**, because the gateway delivers messages from
anyone who pays. This costs nothing to get wrong and gives no symptom: your own
calls keep working, and so does everyone else's.

**Mitigation:** inherit `SolanaCallable` and use `onlySolanaSender(EXPECTED)`.
Make "does every cross-domain entry point check the origin sender, not just the
caller?" a standing review question rather than something an auditor has to
rediscover each time. Consider `strictMode` on high-value targets as a second
net that catches this class from the gateway side.

## T5. Replay

Envelope delivered twice, by the same adapter or a different one.

**Mitigation:** `messageId = keccak256(envelope)` is marked `Executed` before the
inner call, and duplicates return early. Note the deliberate choice *not* to
revert on a duplicate: some transports queue a reverting receive for
redelivery, so reverting would convert one duplicate into an unbounded retry
loop. Adapters also keep their own local dedupe (`consumedVaa`).

## T6. Griefing via gas

A relayer submits a delivery with just barely too little gas. The inner call
receives 63/64 of not-enough, runs out, and the message is consumed as failed —
cheap for the attacker, and on a target whose action is time-sensitive, a failed
delivery can be as good as a forged one.

**Mitigation:** the gateway requires `gasleft() >= gasLimit * 64 / 63 +
GAS_RESERVE` before the call. Failed messages are also retryable by anyone with a
raised gas limit, so the attack costs the attacker a transaction and buys nothing
durable.

## T7. Stuck or poisoned messages

A target that reverts — transient dependency failure, bad parameter, insufficient
balance — must not wedge the channel behind it.

**Mitigation:** failures are caught, parked as `Failed`, and retryable
permissionlessly. Delivery is unordered by choice (`nextNonce` returns 0);
applications that need sequencing enforce it themselves against the envelope
nonce, which converts a global outage into one application's problem.

## T8. Privileged key compromise on Base

The gateway owner can register a malicious adapter, which is equivalent to
forging any message.

**Mitigations:**
- Owner is a multisig behind a timelock. `setAdapter` and `setSolanaPeer` are the
  two calls that matter — both are equivalent to replacing the other chain.
- Two-step ownership transfer (`Auth`), so a mistyped address cannot orphan it.
- Monitor `AdapterSet` and `PeerRegistered` events with alerting. An adapter
  registration you did not plan is an incident, not a notification.

## T9. Fund contamination across senders

In ACCOUNT mode each sender's assets live in its own `SolanaAccount`, so a bug
affecting one sender's interactions cannot reach another's. The gateway itself
holds only the prefunded `value` balances, tracked per sender. This is most of
the reason ACCOUNT mode exists — see D3 in `01-architecture.md`.

## T10. Message format drift

The Rust encoder and the Solidity decoder are two hand-written codecs over one
byte layout. Drift between them is silent on the sending side and shows up as a
decode failure or, worse, a misparse on the receiving side.

**Mitigation:** a frozen golden vector on both sides —
`envelope.rs::golden_vector_is_stable` and `evm/test/envelope-parity.js`, which
checks the Solidity offsets and shifts against the exact bytes the Rust encoder
produces. Any layout change is a `version` bump, and the gateway rejects unknown
versions outright rather than attempting a best-effort parse.

---

## Pre-deployment checklist

- [ ] Gateway owner is a multisig behind a timelock.
- [ ] Solana program upgrade authority is a multisig, or the program is immutable.
- [ ] Consistency/finality level set to finalized on **both** the Solana
      publisher and the Base adapter, and confirmed to match.
- [ ] Transport constants (endpoint addresses, chain/endpoint ids, program ids)
      verified against live docs — several in this repo are marked unverified.
- [ ] Every target inherits `SolanaCallable` and checks the origin sender.
- [ ] `requiredConfirmations = 2` on every target that moves meaningful value.
- [ ] Rate limits on high-value targets, sized to survive T1.
- [ ] Alerting on `AdapterSet`, `PeerRegistered`, `CallFailed`, `PausedSet`.
- [ ] Pause runbook rehearsed on both chains (they pause independently, and
      pausing Solana does not stop messages already in flight).
- [ ] Golden-vector tests green on both sides.
- [ ] External audit covering the gateway, the account clone factory, and both
      adapters.
