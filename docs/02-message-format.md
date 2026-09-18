# Envelope wire format

One canonical byte layout, produced by the Solana program and parsed by
`SolanaGateway`. It is packed big-endian, chosen so that Rust writes it with
plain byte appends and Solidity parses it with cheap `calldataload`s — no ABI
decoder on one side and Borsh on the other, which is where format drift usually
creeps in.

The format is **transport-neutral**: adapters wrap these bytes in their own
payload and unwrap them again. Nothing in the envelope names a transport.

## Layout

| Offset | Size | Field | Notes |
|--:|--:|---|---|
| 0 | 1 | `version` | currently `1`; gateway rejects anything else |
| 1 | 1 | `msgType` | `0 = CALL`. Reserved for future types |
| 2 | 2 | `srcChainId` | internal chain id, **not** the transport's. `1 = Solana` |
| 4 | 2 | `dstChainId` | internal id of the chain that executes this. `3 = Base` |
| 6 | 32 | `sender` | Solana pubkey (wallet or program PDA) that authorized this |
| 38 | 8 | `nonce` | per-`sender` counter from `SenderState` |
| 46 | 20 | `target` | contract address to call on the destination |
| 66 | 16 | `value` | wei of native token to attach (`uint128`) |
| 82 | 8 | `gasLimit` | gas for the inner call |
| 90 | 8 | `expiry` | unix seconds; `0` = never expires |
| 98 | 1 | `mode` | `0 = DIRECT`, `1 = ACCOUNT` |
| 99 | 4 | `calldataLen` | length in bytes (`uint32`) |
| 103 | n | `calldata` | ABI-encoded call, selector first |

Fixed header is 103 bytes. Minimum valid envelope with a bare selector is 107.

Internal chain registry, shared by both codecs: `1 = Solana`, `2 = Ethereum`,
`3 = Base`. Add to the end; never renumber.

**`dstChainId` is what stops cross-destination replay.** Every gateway is
deployed with its own id and rejects an envelope naming any other. Without
it, a message meant for Base could be delivered, valid signatures and all, to
a gateway on a second chain that trusts the same Solana program.

## Message id

```
messageId = keccak256(envelope_bytes)
```

Over the **whole** envelope, not over `(sender, nonce)`. Hashing only the
identity triple would let a transport deliver a body that differs from the one
the sender authorized while still matching a dedupe key the gateway considers
canonical. Hashing everything makes any mutation a different message, which then
fails the adapter's provenance check. Uniqueness is still guaranteed because
`nonce` is monotonic per sender and is inside the hash.

## Field notes

**`srcChainId` is ours, not the transport's.** Wormhole calls Solana `1` and Base
`30`; LayerZero calls Solana `30168`; those are transport namespaces and they
disagree with each other. Adapters translate at the boundary. If the envelope
carried a transport-specific id, swapping transports would change the message
hash for identical messages and break every stored dedupe key.

**`sender` is 32 bytes and stays 32 bytes.** Do not truncate a Solana pubkey to
20 bytes to make it look like an address. Truncation creates collisions between
distinct Solana accounts, and a collision here is an authorization bypass.

**`value`.** ETH attached to the inner call, drawn from the gateway's prefunded
balance for that sender or from the transport's native drop. `uint128` rather
than `uint256` — 16 bytes saved on every message, and no plausible transfer needs
more than 3.4e20 ETH.

**`expiry`.** Cross-chain latency is unbounded when a transport degrades. A
price-sensitive or time-sensitive call that lands six hours late can be worse
than one that never lands. Set it for anything economically meaningful; leave it
`0` for idempotent administrative calls.

**`mode`.** See D3 in `01-architecture.md`. `DIRECT` calls the target from the
gateway; `ACCOUNT` routes through the sender's deterministic `SolanaAccount`.

## Encoding reference

Rust (source of truth: `solana/programs/base-caller/src/envelope.rs`):

```rust
let mut buf = Vec::with_capacity(103 + calldata.len());
buf.push(VERSION);                       // 1
buf.push(MSG_TYPE_CALL);                 // 1
buf.extend_from_slice(&SRC_CHAIN_SOLANA.to_be_bytes());   // 2
buf.extend_from_slice(&dst_chain_id.to_be_bytes());       // 2
buf.extend_from_slice(sender.as_ref());  // 32
buf.extend_from_slice(&nonce.to_be_bytes());              // 8
buf.extend_from_slice(&target);          // 20
buf.extend_from_slice(&value.to_be_bytes());              // 16
buf.extend_from_slice(&gas_limit.to_be_bytes());          // 8
buf.extend_from_slice(&expiry.to_be_bytes());             // 8
buf.push(mode);                          // 1
buf.extend_from_slice(&(calldata.len() as u32).to_be_bytes()); // 4
buf.extend_from_slice(calldata);
```

Solidity (source of truth: `evm/src/libraries/EnvelopeLib.sol`) reads the same
offsets. Any change to this table is a `version` bump on both sides — never a
silent reinterpretation of existing bytes.
