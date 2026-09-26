#!/usr/bin/env bash
# Step 1 of the testnet run: deploy the Solana side and print what the EVM
# side needs.
#
# Order matters. The destination adapters pin Solana PDAs in their
# constructors, so the programs must exist before the EVM deploy can name
# them. Registration then flows back the other way, in step 3.
#
#   SOLANA_RPC=https://api.devnet.solana.com \
#   KEYPAIR=~/.config/solana/id.json \
#   bash scripts/deploy-devnet.sh
set -euo pipefail
cd "$(dirname "$0")/.."

RPC="${SOLANA_RPC:-https://api.devnet.solana.com}"
KEYPAIR="${KEYPAIR:-$HOME/.config/solana/id.json}"

command -v solana >/dev/null || { echo "solana CLI not found"; exit 2; }
command -v anchor >/dev/null || { echo "anchor CLI not found"; exit 2; }
[ -f "$KEYPAIR" ] || { echo "no keypair at $KEYPAIR (solana-keygen new -o $KEYPAIR)"; exit 2; }

solana config set --url "$RPC" --keypair "$KEYPAIR" >/dev/null
PAYER=$(solana address)
BALANCE=$(solana balance | awk '{print $1}')
echo "rpc      $RPC"
echo "payer    $PAYER"
echo "balance  $BALANCE SOL"
# Program deploys are the expensive part: budget a few SOL, not a few lamports.
awk -v b="$BALANCE" 'BEGIN { if (b+0 < 3) { print "\nWARNING: under 3 SOL. Program deploys may fail part-way."; } }'

echo
echo "==> building (outbox + mock + dispatcher)"
bash scripts/build-all.sh >/dev/null
echo "    built"

echo
echo "==> deploying"
for prog in base_caller lz_dispatcher; do
  echo -n "    $prog ... "
  solana program deploy "target/deploy/${prog}.so" \
    --program-id "target/deploy/${prog}-keypair.json" --url "$RPC" >/dev/null
  echo "$(solana address -k "target/deploy/${prog}-keypair.json")"
done

echo
echo "==> addresses the destination-chain deploy needs"
node scripts/derive-pdas.js
