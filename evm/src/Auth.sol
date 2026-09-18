// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @notice Minimal two-step ownership + pause + reentrancy guard.
/// @dev Deliberately dependency-free so this reference tree compiles with no
///      package installs. For production, swap for the audited OpenZeppelin
///      equivalents (Ownable2Step, Pausable, ReentrancyGuard) rather than
///      shipping hand-rolled access control.
abstract contract Auth {
    address public owner;
    address public pendingOwner;
    bool public paused;

    uint256 private _entered;

    event OwnershipTransferStarted(address indexed from, address indexed to);
    event OwnershipTransferred(address indexed from, address indexed to);
    event PausedSet(bool paused);

    error NotOwner();
    error NotPendingOwner();
    error Paused();
    error Reentrancy();
    error ZeroAddress();

    constructor(address owner_) {
        if (owner_ == address(0)) revert ZeroAddress();
        owner = owner_;
        emit OwnershipTransferred(address(0), owner_);
    }

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    modifier whenNotPaused() {
        if (paused) revert Paused();
        _;
    }

    modifier nonReentrant() {
        if (_entered == 1) revert Reentrancy();
        _entered = 1;
        _;
        _entered = 0;
    }

    /// @dev Two-step so a typo in the new owner cannot brick the gateway.
    function transferOwnership(address newOwner) external onlyOwner {
        pendingOwner = newOwner;
        emit OwnershipTransferStarted(owner, newOwner);
    }

    function acceptOwnership() external {
        if (msg.sender != pendingOwner) revert NotPendingOwner();
        address old = owner;
        owner = pendingOwner;
        pendingOwner = address(0);
        emit OwnershipTransferred(old, owner);
    }

    function setPaused(bool p) external onlyOwner {
        paused = p;
        emit PausedSet(p);
    }
}
