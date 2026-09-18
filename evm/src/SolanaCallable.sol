// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ISolanaGateway} from "./interfaces/ISolanaGateway.sol";

/// @title SolanaCallable
/// @notice Inherit this in any Base contract that should be callable from
///         Solana in DIRECT mode.
///
/// The check that matters is `onlySolanaSender`, and it has two halves:
///
///   1. the caller is the gateway, and
///   2. the Solana pubkey behind this call is the one you expect.
///
/// Half 2 is the one that gets forgotten. A contract that checks only
/// `msg.sender == gateway` is callable by *every* Solana account in existence,
/// because the gateway will happily deliver a message from anyone. That is an
/// open door, not a bug you notice in testing -- your own messages keep
/// working. This base contract exists so nobody writes that check by hand.
abstract contract SolanaCallable {
    ISolanaGateway public immutable solanaGateway;

    error NotSolanaGateway(address caller);
    error UnauthorizedSolanaSender(bytes32 actual);

    constructor(address gateway_) {
        solanaGateway = ISolanaGateway(gateway_);
    }

    modifier onlySolanaSender(bytes32 expected) {
        if (msg.sender != address(solanaGateway)) revert NotSolanaGateway(msg.sender);
        bytes32 actual = solanaGateway.xDomainMessageSender();
        if (actual != expected) revert UnauthorizedSolanaSender(actual);
        _;
    }

    /// @notice Any Solana origin, for contracts that authorize per-sender
    ///         internally rather than against a single constant.
    modifier onlySolana() {
        if (msg.sender != address(solanaGateway)) revert NotSolanaGateway(msg.sender);
        if (solanaGateway.xDomainMessageSender() == bytes32(0)) revert UnauthorizedSolanaSender(bytes32(0));
        _;
    }

    function _solanaSender() internal view returns (bytes32) {
        return solanaGateway.xDomainMessageSender();
    }

    function _solanaNonce() internal view returns (uint64) {
        return solanaGateway.xDomainMessageNonce();
    }
}
