// The deployment as data, so the same sequence can be executed against a real
// RPC and against the in-process EVM the tests use. A deploy script that has
// only ever run on a testnet is a script that gets debugged on a testnet.
//
// Order matters and is the one thing unit tests cannot check: steps 3 and 4
// are the mutual pinning between chains. Getting them wrong in one direction
// means messages are silently rejected (the good failure); pinning a peer you
// do not control means they are silently accepted.

const CHAIN = {
  // Internal chain ids from EnvelopeLib / envelope.rs. Not a transport's.
  base: 3,
  ethereum: 2,
};

// Verified against wormhole-foundation/sdk-base.
const WORMHOLE_CHAIN_ID = { base: 30, baseSepolia: 10004 };

// Verified against layerzerolabs/lz-definitions.
const LZ_EID = { base: 30184, baseSepolia: 40245, solana: 30168, solanaDevnet: 40168 };

/**
 * @param {object} cfg
 * @param {string} cfg.owner            - address that will own the gateway
 * @param {string} cfg.wormholeCore     - deployed Wormhole core bridge on this chain
 * @param {string} cfg.lzEndpoint       - deployed LayerZero endpoint on this chain
 * @param {string} cfg.solanaEmitter    - 32-byte hex: the outbox's ["emitter"] PDA
 * @param {string} cfg.solanaOApp       - 32-byte hex: the dispatcher's ["oapp"] PDA
 * @param {string} cfg.configurer       - 32-byte hex: Solana PDA allowed to set config
 * @param {string} cfg.invoker          - 32-byte hex: Solana PDA allowed to invoke
 * @param {number} cfg.internalChainId  - this chain's id in our own registry
 * @param {number} cfg.solanaEid        - LayerZero eid of the SOURCE chain
 */
function plan(cfg) {
  return {
    deploy: [
      {
        name: "SolanaGateway",
        args: [cfg.owner, cfg.internalChainId],
        note: "holds the replay map and quorum state; non-upgradeable by design",
      },
      {
        name: "WormholeAdapter",
        // owner, wormhole core, gateway, solana peer, min consistency level.
        // 1 = Finalized, per wormhole-anchor-sdk's Finality enum. Never 0.
        args: [cfg.owner, cfg.wormholeCore, "$SolanaGateway", cfg.solanaEmitter, 1],
        note: "verifies guardian-signed VAAs from the outbox's emitter PDA",
      },
      {
        name: "LayerZeroAdapter",
        args: [cfg.owner, cfg.lzEndpoint, "$SolanaGateway", cfg.solanaOApp, cfg.solanaEid],
        note: "receives Executor deliveries from the dispatcher's OApp PDA",
      },
      {
        name: "ConfigInvokeTarget",
        args: ["$SolanaGateway", cfg.configurer, cfg.invoker],
        note: "two Solana authorities; config-version guard instead of ordering",
      },
    ],
    // Post-deploy wiring, executed in order against the deployed addresses.
    calls: [
      {
        on: "SolanaGateway",
        fn: "setAdapter",
        args: ["$WormholeAdapter", true],
        note: "the gateway accepts deliveries only from registered adapters",
      },
      {
        on: "SolanaGateway",
        fn: "setAdapter",
        args: ["$LayerZeroAdapter", true],
      },
      // Deliberately NOT set here: requiredConfirmations = 2 on the target.
      // Turn it on once both transports have each landed a message alone,
      // otherwise a single broken transport looks like a broken system.
    ],
  };
}

module.exports = { plan, CHAIN, WORMHOLE_CHAIN_ID, LZ_EID };
