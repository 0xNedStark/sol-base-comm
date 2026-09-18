// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title ISolanaGateway
/// @notice The Base-side entry point for calls originating on Solana.
interface ISolanaGateway {
    enum Status {
        None,
        Pending, // seen by >=1 adapter, still short of the required quorum
        Executed,
        Failed // delivered, inner call reverted, retryable
    }

    event MessageDelivered(bytes32 indexed messageId, address indexed adapter, uint256 confirmations);
    event CallExecuted(bytes32 indexed messageId, bytes32 indexed sender, address indexed target, uint64 nonce);
    event CallFailed(bytes32 indexed messageId, address indexed target, bytes returnData);
    event MessageDuplicate(bytes32 indexed messageId, address indexed adapter);
    event AccountDeployed(bytes32 indexed sender, address indexed account);
    event Deposited(bytes32 indexed sender, address indexed from, uint256 amount);

    /// @notice Called by a registered adapter once it has verified that
    ///         `envelope` genuinely originated from the registered Solana peer.
    /// @dev    MUST NOT revert on a duplicate: several transports respond to a
    ///         reverting receive by queueing the message for redelivery, which
    ///         would turn one duplicate into an unbounded retry loop.
    function deliver(bytes calldata envelope) external;

    /// @notice Re-run a message whose inner call reverted. Permissionless.
    /// @param gasLimitOverride 0 to reuse the envelope's limit, otherwise a
    ///        value greater than it (a reverted call is often just underfunded).
    function retry(bytes calldata envelope, uint64 gasLimitOverride) external;

    /// @notice The Solana pubkey that authorized the call currently executing.
    ///         Zero outside of a delivery. This is the check a target contract
    ///         must make; see SolanaCallable.
    function xDomainMessageSender() external view returns (bytes32);

    /// @notice Per-sender nonce of the call currently executing.
    function xDomainMessageNonce() external view returns (uint64);

    /// @notice Deterministic Base address for a Solana sender under ACCOUNT
    ///         mode. Returns the address whether or not it is deployed yet, so
    ///         it can be funded ahead of first use.
    function accountFor(bytes32 sender) external view returns (address);

    /// @notice Fund a sender's balance, drawn on to pay `value` on its calls.
    function deposit(bytes32 sender) external payable;

    function statusOf(bytes32 messageId) external view returns (Status);
    function balanceOf(bytes32 sender) external view returns (uint256);
}
