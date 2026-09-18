// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @dev Plain target with no Solana awareness, for ACCOUNT-mode tests where
///      the caller is the sender's smart account rather than the gateway.
contract Counter {
    uint256 public count;
    address public lastCaller;

    function increment() external {
        count += 1;
        lastCaller = msg.sender;
    }
}
