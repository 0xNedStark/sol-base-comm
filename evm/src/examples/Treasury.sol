// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {SolanaCallable} from "../SolanaCallable.sol";

interface IERC20 {
    function transfer(address to, uint256 amount) external returns (bool);
}

/// @title Treasury
/// @notice Worked example: a Base treasury whose payouts are authorized by a
///         Solana PDA. Shows what an integration actually costs -- one
///         immutable and one modifier.
///
/// On the Solana side, `TREASURER` is a PDA of your governance program, and
/// that program CPIs into `base_caller::send_call` with `invoke_signed`. The
/// pubkey that lands in `xDomainMessageSender()` is therefore a key no human
/// holds and that only your program's logic can produce.
contract Treasury is SolanaCallable {
    /// @notice The Solana PDA allowed to move funds. Immutable on purpose: a
    ///         settable authority is a second privileged key to protect, and
    ///         the whole point is that this one cannot be stolen.
    bytes32 public immutable TREASURER;

    /// @notice Guards against out-of-order execution. Transports deliver
    ///         unordered (D5), so a contract that cares must say so itself.
    uint64 public lastNonce;

    event Paid(address indexed token, address indexed to, uint256 amount, uint64 nonce);

    error StaleNonce(uint64 got, uint64 last);

    constructor(address gateway_, bytes32 treasurer_) SolanaCallable(gateway_) {
        TREASURER = treasurer_;
    }

    function pay(address token, address to, uint256 amount) external onlySolanaSender(TREASURER) {
        uint64 nonce = _solanaNonce();
        if (nonce <= lastNonce) revert StaleNonce(nonce, lastNonce);
        lastNonce = nonce;

        if (token == address(0)) {
            (bool ok,) = to.call{value: amount}("");
            require(ok, "eth transfer failed");
        } else {
            require(IERC20(token).transfer(to, amount), "erc20 transfer failed");
        }

        emit Paid(token, to, amount, nonce);
    }

    receive() external payable {}
}
