// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {SolanaCallable} from "../SolanaCallable.sol";

/// @title ConfigInvokeTarget
/// @notice The phase-one target shape: a configuration that is set rarely by
///         one Solana authority, and an action invoked routinely by another.
///
/// Two things this contract demonstrates that matter more than its body:
///
/// 1. **Two authorities.** `CONFIGURER` and `INVOKER` are different Solana
///    PDAs. Compromising the routine one cannot rewrite the configuration.
///
/// 2. **A version guard instead of message ordering.** Cross-chain delivery
///    is unordered. An invoke that lands before the config change it depends
///    on would run against stale configuration. Rather than have the target
///    enforce nonce order -- which lets one stuck config message wedge every
///    invoke behind it -- each invoke names the config version it expects and
///    reverts on mismatch. The gateway parks that revert as Failed; once the
///    config lands, anyone retries it. Nothing is wedged, nothing runs against
///    the wrong config.
contract ConfigInvokeTarget is SolanaCallable {
    struct Config {
        address recipient;
        uint256 limit;
        bool enabled;
    }

    bytes32 public immutable CONFIGURER;
    bytes32 public immutable INVOKER;

    Config public config;
    uint64 public configVersion;

    uint256 public invocations;
    bytes32 public lastArgsHash;

    event ConfigSet(uint64 indexed version, address recipient, uint256 limit, bool enabled);
    event Invoked(uint64 indexed configVersion, uint64 indexed solanaNonce, bytes32 argsHash);

    error StaleConfig(uint64 current, uint64 expected);
    error Disabled();

    constructor(address gateway_, bytes32 configurer_, bytes32 invoker_) SolanaCallable(gateway_) {
        CONFIGURER = configurer_;
        INVOKER = invoker_;
    }

    function setConfig(Config calldata c) external onlySolanaSender(CONFIGURER) {
        config = c;
        configVersion += 1;
        emit ConfigSet(configVersion, c.recipient, c.limit, c.enabled);
    }

    function invoke(uint64 expectedVersion, bytes calldata args) external onlySolanaSender(INVOKER) {
        if (configVersion != expectedVersion) revert StaleConfig(configVersion, expectedVersion);
        if (!config.enabled) revert Disabled();

        invocations += 1;
        lastArgsHash = keccak256(args);
        emit Invoked(configVersion, _solanaNonce(), lastArgsHash);
    }
}
