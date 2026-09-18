// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title ISolanaAccount
/// @notice A minimal smart account on Base controlled by a Solana pubkey, used
///         in ACCOUNT execution mode. One deterministic address per sender.
interface ISolanaAccount {
    function gateway() external view returns (address);
    function sender() external view returns (bytes32);

    function initialize(bytes32 sender_) external;

    /// @dev Gateway-only. Returns rather than reverts so the gateway can park a
    ///      failed message for retry instead of losing it.
    function execute(address target, uint256 value, bytes calldata data, uint256 gasLimit)
        external
        payable
        returns (bool ok, bytes memory ret);
}
