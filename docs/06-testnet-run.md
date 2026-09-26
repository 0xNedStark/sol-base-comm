# Running the first message on testnets

Four steps. Each one prints what the next needs, so nothing has to be
copied out of a terminal by hand twice.

The environment this was developed in cannot reach any chain RPC (the egress
policy denies `api.devnet.solana.com`, `sepolia.base.org` and the public
alternatives), so these scripts have been verified as far as that allows:
the deployment plan is executed against an in-process EVM by
`evm/test/deploy.test.js`, and PDA derivation runs against the real build.
What has not happened is any of it touching a network.

## What you need

| | |
|---|---|
| Solana devnet keypair | ~3 SOL. Program deploys are the expensive part; `solana airdrop 2` a few times, or the web faucet |
| Destination deployer key | ~0.05 Base Sepolia ETH, from a faucet |
| RPC endpoints | Public ones work; a paid endpoint is steadier for the guardian polling in step 4 |
| Two Solana authority PDAs | For the target's `CONFIGURER` and `INVOKER`. PDAs of your program, not wallet keys, and different from each other |
| Transport addresses | Wormhole core bridge and LayerZero endpoint, on both chains |

Everything else the scripts derive.

## Step 1 — Solana

```bash
cd solana
SOLANA_RPC=https://api.devnet.solana.com \
KEYPAIR=~/.config/solana/id.json \
bash scripts/deploy-devnet.sh
```

Builds all three programs, deploys the outbox and the dispatcher, then prints
`SOLANA_EMITTER` and `SOLANA_OAPP` — the two PDAs the destination adapters
pin in their constructors.

**Before a real deploy**, give each program a keypair you control and sync
its `declare_id!`; the committed ids are placeholders.

## Step 2 — destination chain

```bash
cd evm
RPC_URL=https://sepolia.base.org PRIVATE_KEY=0x... \
WORMHOLE_CORE=0x... LZ_ENDPOINT=0x... \
SOLANA_EMITTER=0x...  SOLANA_OAPP=0x...   # from step 1
CONFIGURER=0x... INVOKER=0x... \
SOLANA_EID=40168 INTERNAL_CHAIN_ID=3 \
npm run deploy
```

Deploys the gateway, both adapters and the example target, registers the
adapters, and writes `deployments/<chainId>.json`.

The plan lives in `script/plan.js` as data, and `test/deploy.test.js` runs
that same plan against an in-process EVM — so the order, the constructor
arguments and the wiring are checked before they meet a chain. It asserts,
among other things, that the Wormhole adapter demands finalized rather than
confirmed.

## Step 3 — point Solana back at the destination

```bash
cd solana
ANCHOR_PROVIDER_URL=https://api.devnet.solana.com \
ANCHOR_WALLET=~/.config/solana/id.json \
WORMHOLE_ADAPTER=0x... LAYERZERO_ADAPTER=0x... \  # from step 2
WORMHOLE_CORE=<solana core bridge> LZ_ENDPOINT=<solana endpoint> \
LZ_DISPATCHER=<dispatcher program id> \
DEST_WORMHOLE_CHAIN=10004 DEST_LZ_EID=40245 \
node scripts/register-transports.js
```

Steps 2 and 3 are the mutual pinning, and they are the part no unit test can
check — they are an agreement between two chains. Wrong in one direction and
messages are silently rejected, which is the good failure. Pin a peer you do
not control and they are silently accepted.

## Step 4 — send one, and carry it

Send with `prepare` + `dispatch_via_wormhole`, then:

```bash
cd relayer && npm install
SOLANA_EMITTER=<base58 emitter> SEQUENCE=<from the dispatch tx> \
RPC_URL=https://sepolia.base.org PRIVATE_KEY=0x... \
WORMHOLE_ADAPTER=0x... \
npm run relay
```

Polls the guardians for the signed VAA, simulates the delivery (a revert
names the reason rather than burning gas), submits it, and reports whether
the gateway executed, parked or rejected the message.

Wormhole first because its attestations are portable: no relayer is
privileged, and if this script breaks, anyone holding the VAA can land it.

## Expect it to fail a few times

Nothing local exercises the mutual pinning, real attestation, or the live
endpoint's account list, so the first attempts will find things the 46
passing tests could not. The likely ones:

| Symptom | Cause |
|---|---|
| `WrongEmitter` | The adapter pins a different Solana PDA than the one that sent |
| `InsufficientConsistency` | Published below finalized. Check both the publisher and the adapter's floor |
| No VAA after the full wait | Same, or the emitter/sequence is wrong |
| `BadDestinationChain` | `INTERNAL_CHAIN_ID` at deploy disagrees with `dst_chain_id` in the envelope |
| Parked as `InnerCallReverted` | Reached the target and it refused — read the revert data; often a stale config version |
| Out of gas on the destination | `gasLimit` in the envelope too low for the real target |

A parked message is retryable and nothing is lost; a rejected one is
terminal. `statusOf(messageId)` tells you which.

## Only after a message lands

Raise `requiredConfirmations` to 2 on anything valuable — but only once each
transport has landed a message on its own. A two-transport quorum with one
broken transport looks exactly like a broken system, and you will debug the
wrong thing.
