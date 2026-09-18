# Operations

## Deployment order

The two sides reference each other, so there is a bootstrapping order:

1. **Base: gateway.** Deploy `SolanaGateway(owner)`. It deploys the
   `SolanaAccount` implementation in its constructor. Record the address.
2. **Solana: program.** Deploy `base_caller`, run `initialize`. Derive the
   emitter PDA (`["emitter"]`) and/or OApp PDA (`["oapp"]`) — these are the
   identities Base will pin.
3. **Base: adapters.** Deploy `WormholeAdapter` / `LayerZeroAdapter` with
   `solanaPeer` set to the PDA from step 2. Then
   `gateway.setAdapter(adapter, true)`.
4. **Solana: transports.** `set_transport` with the Base adapter address,
   left-padded to 32 bytes, as `peer`.
5. **Verify on testnet.** Send one message end to end on Sepolia Base + Solana
   devnet before anything else. Do not skip to step 6 on the strength of unit
   tests; every constant in step 3 and 4 is a cross-chain identity that unit
   tests cannot check.
6. **Base: targets.** Deploy your `SolanaCallable` contracts with the expected
   Solana sender baked in as an immutable.
7. **Harden.** `setRequiredConfirmations(target, 2)` on anything valuable, then
   hand ownership to the timelock.

Steps 3 and 4 are the mutual pinning. Get them wrong and messages are silently
rejected, which is the good failure. Get them wrong in the other direction —
pinning a peer you do not control — and they are silently accepted.

## Running a Wormhole relayer

LayerZero's Executor delivers automatically. Wormhole, used as the second
transport, generally needs a relayer. It is a small stateless loop:

1. Watch for `CallDispatched` events from the Solana program, or poll the
   emitter's sequence account.
2. Fetch the signed VAA from a guardian RPC for `(chain=1, emitter, sequence)`.
   Retry with backoff — a VAA is not available until enough guardians have
   observed and signed at the finalized commitment.
3. Submit `WormholeAdapter.receiveMessage(vaa)` on Base.
4. Confirm the `MessageDelivered` / `CallExecuted` event.

Properties worth designing for, because they change how careful the loop has to
be:

- **It is stateless and permissionless.** VAAs are self-contained. Losing relayer
  state loses nothing — re-derive from on-chain sequence numbers. Running two
  relayers is safe; the second one's duplicate settles quietly at the gateway.
- **It cannot forge.** A compromised relayer can delay or drop, not fabricate.
  Liveness needs monitoring; integrity does not.
- **Dropping is the realistic failure.** Alert on messages dispatched on Solana
  with no corresponding `CallExecuted` on Base after a threshold, and keep a
  manual submit path — anyone holding the VAA can land it from a terminal.

## Monitoring

| Signal | Source | Meaning |
|---|---|---|
| `CallDispatched` without `CallExecuted` | both chains | relayer down, or stuck in transit |
| `CallFailed` | Base | target reverted; inspect `returnData` and retry |
| `MessageDelivered` with `confirmations < required` | Base | one transport landed, waiting on the other |
| `MessageDuplicate` | Base | benign; spikes mean a relayer is looping |
| `AdapterSet` / `PeerRegistered` | Base | **page someone** — unplanned means compromise |
| `PausedSet` | either | kill switch used |
| gateway ETH balance | Base | `value`-carrying calls will start failing when low |

Track dispatched-to-executed latency as a distribution, not an average. The tail
is what tells you a transport is degrading, and it degrades before it stops.

## Incident response

**Suspected transport compromise.** `setPaused(true)` on the gateway first — it
stops execution of everything already in flight, which pausing Solana does not.
Then pause the Solana program to stop new sends. Then
`setAdapter(suspect, false)`. Paused gateway plus disabled adapter is the
complete stop; either alone is not.

**Target contract bug.** `setStrictMode(target, true)` with no allowlisted calls
blocks that one target without stopping the rest of the system.

**Stuck message.** Check `statusOf(messageId)`. `Pending` means it is waiting on a
second adapter — deliver via the other transport. `Failed` means retry with a
higher gas limit, after fixing whatever the `returnData` says went wrong.

**Gateway out of ETH.** `value`-carrying calls park as `Failed` with
`InsufficientBalance` rather than reverting, deliberately: top up with
`deposit(sender)` and retry each one. Nothing is lost, the balance is refunded on
failure, but the calls do not self-heal — you have to retry them.

## Upgrades

The contracts here are non-upgradeable on purpose: an upgradeable gateway is a
key that can rewrite the rules for every integrated contract at once, which is a
larger risk than the migration cost it saves.

To migrate, run both gateways in parallel:

1. Deploy the new gateway and adapters.
2. Register the new adapters on Solana as an additional transport.
3. Move traffic by changing which transport clients send over.
4. Drain: leave the old gateway live until every in-flight message has landed or
   expired. `expiry` gives this a bounded end.
5. Withdraw remaining balances from the old gateway via a DIRECT message from
   each sender, then pause it.

Targets that hold funds should accept messages from both gateways during the
overlap, or you will need a second migration for them.
