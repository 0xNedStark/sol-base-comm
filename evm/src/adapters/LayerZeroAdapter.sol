// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Auth} from "../Auth.sol";
import {ISolanaGateway} from "../interfaces/ISolanaGateway.sol";
import {ITransportAdapter} from "../interfaces/ITransportAdapter.sol";

/// @dev Minimal subset of LayerZero v2's receive-side interface. Replace with
///      the official `@layerzerolabs/oapp-evm` types when wiring for real.
struct Origin {
    uint32 srcEid;
    bytes32 sender;
    uint64 nonce;
}

/// @title LayerZeroAdapter
/// @notice Receives messages delivered by the LayerZero v2 Executor and
///         forwards the payload to the gateway.
///
/// LayerZero is the recommended default transport because the Executor calls
/// this contract automatically -- no relayer to operate -- and because the DVN
/// set that must sign a message is configured per application rather than
/// inherited from the protocol. Requiring two independent DVNs is a config
/// change here, which is a materially better security story than a fixed
/// validator set. See docs/03-transport-comparison.md.
contract LayerZeroAdapter is ITransportAdapter, Auth {
    /// @notice LayerZero v2 endpoint id for Solana mainnet. Verified: 30168.
    ///         (Base mainnet is believed to be 30184 -- confirm before deploy.)
    uint32 public constant SOLANA_EID = 30168;

    address public immutable endpoint;
    ISolanaGateway public immutable gateway;

    /// @notice Our Solana OApp's address, as a 32-byte pubkey.
    bytes32 public solanaPeer;

    error NotEndpoint();
    error WrongSourceEid(uint32 eid);
    error WrongPeer(bytes32 peer);

    constructor(address owner_, address endpoint_, address gateway_, bytes32 solanaPeer_) Auth(owner_) {
        endpoint = endpoint_;
        gateway = ISolanaGateway(gateway_);
        solanaPeer = solanaPeer_;
        emit PeerRegistered(solanaPeer_);
    }

    function transportId() external pure returns (string memory) {
        return "layerzero-v2";
    }

    /// @notice Called by the LayerZero endpoint on delivery.
    /// @dev Reverting here causes the endpoint to store the message for retry,
    ///      which is the behaviour we want for a genuinely bad delivery. The
    ///      gateway is careful NOT to revert on a mere duplicate, so a
    ///      double-delivery settles quietly instead of looping.
    function lzReceive(
        Origin calldata origin,
        bytes32 guid,
        bytes calldata message,
        address, /* executor */
        bytes calldata /* extraData */
    ) external payable whenNotPaused {
        if (msg.sender != endpoint) revert NotEndpoint();
        if (origin.srcEid != SOLANA_EID) revert WrongSourceEid(origin.srcEid);
        if (origin.sender != solanaPeer) revert WrongPeer(origin.sender);

        emit MessageForwarded(keccak256(message), guid);

        gateway.deliver(message);
    }

    /// @dev Endpoint calls this before initializing a new path.
    function allowInitializePath(Origin calldata origin) external view returns (bool) {
        return origin.srcEid == SOLANA_EID && origin.sender == solanaPeer;
    }

    /// @dev 0 selects unordered delivery. Deliberate: ordered delivery turns one
    ///      stuck message into a full outage. Applications that need sequencing
    ///      read the nonce out of the envelope. See D5 in docs/01-architecture.md.
    function nextNonce(uint32, bytes32) external pure returns (uint64) {
        return 0;
    }

    /// @dev Should be held by a timelock.
    function setSolanaPeer(bytes32 peer) external onlyOwner {
        solanaPeer = peer;
        emit PeerRegistered(peer);
    }
}
