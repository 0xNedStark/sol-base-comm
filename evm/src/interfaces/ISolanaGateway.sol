// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title ISolanaGateway
/// @notice The destination-chain entry point for calls originating on Solana.
interface ISolanaGateway {
    enum Status {
        None,
        Pending, // seen by >=1 adapter, still short of the required quorum
        Executed,
        Failed, // delivered, could not run yet; retryable
        Rejected // delivered, can never run; terminal
    }

    /// @dev Why a message was consumed without running. Terminal reasons mark
    ///      the message Rejected; parkable reasons mark it Failed.
    enum Reason {
        None,
        // -- terminal: retrying with no change on our side can never succeed
        Malformed,
        BadVersion,
        BadMessageType,
        BadSourceChain,
        BadDestinationChain,
        BadMode,
        Expired,
        ValueNotSupported,
        ForbiddenTarget,
        // -- parkable: a human can act, then anyone retries
        AccountModeDisabled,
        NotAllowed,
        InnerCallReverted
    }

    event MessageDelivered(bytes32 indexed messageId, address indexed adapter, uint256 confirmations);
    event CallExecuted(bytes32 indexed messageId, bytes32 indexed sender, address indexed target, uint64 nonce);
    event CallFailed(bytes32 indexed messageId, address indexed target, Reason reason, bytes returnData);
    event MessageRejected(bytes32 indexed messageId, Reason reason);
    event MessageDuplicate(bytes32 indexed messageId, address indexed adapter);
    event AccountDeployed(bytes32 indexed sender, address indexed account);

    /// @notice Called by a registered adapter once it has verified that
    ///         `envelope` genuinely originated from the registered Solana peer.
    /// @dev    Reverts ONLY for transient conditions (paused, under-gassed),
    ///         where a later redelivery can succeed. Everything else -- a
    ///         duplicate, a malformed or expired envelope, a reverting target --
    ///         is consumed and recorded, because several transports respond to
    ///         a reverting receive by queueing it for redelivery forever.
    function deliver(bytes calldata envelope) external;

    /// @notice Re-run a message parked as Failed. Permissionless.
    /// @param gasLimitOverride 0 to reuse the envelope's limit, otherwise a
    ///        value greater than it.
    function retry(bytes calldata envelope, uint64 gasLimitOverride) external;

    /// @notice The Solana pubkey that authorized the call currently executing.
    ///         Zero outside of a delivery. Meaningful ONLY when msg.sender is
    ///         this gateway; see SolanaCallable.
    function xDomainMessageSender() external view returns (bytes32);

    /// @notice Per-sender nonce of the call currently executing.
    function xDomainMessageNonce() external view returns (uint64);

    /// @notice Deterministic address for a Solana sender under ACCOUNT mode.
    function accountFor(bytes32 sender) external view returns (address);

    function statusOf(bytes32 messageId) external view returns (Status);
    function chainId() external view returns (uint16);
}
