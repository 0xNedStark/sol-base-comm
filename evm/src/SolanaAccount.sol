// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ISolanaAccount} from "./interfaces/ISolanaAccount.sol";

/// @title SolanaAccount
/// @notice A smart account on Base whose sole controller is a Solana pubkey,
///         reachable only through the gateway. Deployed as an EIP-1167 clone at
///         a CREATE2 address derived from the sender, so the address is known
///         before first use and can be funded in advance.
///
/// Why this exists (see D3 in docs/01-architecture.md): without it, every Solana
/// sender interacting with a third-party Base contract either shares one pot of
/// funds held by the gateway -- where a single bug drains everyone -- or cannot
/// hold assets on Base at all. A per-sender account gives each Solana caller a
/// durable Base identity that can hold ETH, tokens and approvals scoped to
/// itself, and makes the target see an ordinary caller with no special
/// semantics to learn.
///
/// @dev `gateway` is immutable and therefore lives in the implementation's
///      runtime code, shared by every clone via delegatecall -- correct here
///      because all accounts answer to the same gateway. Only `sender` needs
///      per-clone storage.
contract SolanaAccount is ISolanaAccount {
    address public immutable gateway;

    bytes32 public sender;
    bool private _initialized;

    error NotGateway();
    error AlreadyInitialized();

    /// @param gateway_ the one gateway allowed to drive every clone.
    constructor(address gateway_) {
        gateway = gateway_;
        // Lock the implementation itself so it can never be initialized and
        // driven directly.
        _initialized = true;
    }

    /// @dev Only the gateway can deploy a clone at a given CREATE2 address (the
    ///      address commits to the gateway as deployer), so there is no window
    ///      in which someone else can front-run initialization.
    function initialize(bytes32 sender_) external {
        if (msg.sender != gateway) revert NotGateway();
        if (_initialized) revert AlreadyInitialized();
        _initialized = true;
        sender = sender_;
    }

    /// @inheritdoc ISolanaAccount
    function execute(address target, uint256 value, bytes calldata data, uint256 gasLimit)
        external
        payable
        returns (bool ok, bytes memory ret)
    {
        if (msg.sender != gateway) revert NotGateway();
        // Bubbling nothing: the gateway needs the failure as a value so it can
        // park the message for retry rather than reverting the whole delivery.
        (ok, ret) = target.call{value: value, gas: gasLimit}(data);
    }

    receive() external payable {}
}
