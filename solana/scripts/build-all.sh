#!/usr/bin/env bash
# Build every program in the repo, including the ones that live in their own
# workspace because their SDKs cannot share a binary with the outbox.
#
# The dispatcher's .so is copied into the outbox's target/deploy so that
# solana-program-test (SBF_OUT_DIR) and solana-test-validator can load all
# three programs together -- which they can, because the version conflict is
# a property of a binary, not of a validator.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> outbox workspace (anchor 0.30.1 / solana 1.18)"
cargo build-sbf --manifest-path programs/base-caller/Cargo.toml
cargo build-sbf --manifest-path programs/mock-transport/Cargo.toml

echo "==> layerzero dispatcher (anchor 0.29 / solana 1.17.31)"
(cd layerzero-dispatcher && cargo build-sbf --manifest-path programs/lz-dispatcher/Cargo.toml)
cp layerzero-dispatcher/target/deploy/lz_dispatcher.so target/deploy/

ls -la target/deploy/*.so
