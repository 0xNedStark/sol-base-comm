// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title ITransportAdapter
/// @notice One implementation per GMP protocol (Wormhole, LayerZero, ...).
///
/// An adapter has exactly one job, and the security of the whole system rests
/// on it doing that job: prove that the bytes it hands to the gateway were
/// published by the registered Solana peer program, then forward them verbatim.
///
/// An adapter MUST:
///   - verify the message against its transport's own proof system;
///   - check the source chain is Solana in that transport's id namespace;
///   - check the emitter/sender equals the registered peer, and nothing else;
///   - forward the envelope bytes unmodified.
///
/// An adapter MUST NOT interpret the envelope. Parsing, replay protection,
/// expiry, quorum and authorization all live in the gateway, so that adding a
/// transport cannot weaken any of them.
interface ITransportAdapter {
    event PeerRegistered(bytes32 indexed peer);
    event MessageForwarded(bytes32 indexed messageId, bytes32 indexed transportMessageId);

    /// @notice Human-readable transport name, e.g. "wormhole-v1".
    function transportId() external view returns (string memory);

    /// @notice The registered Solana peer this adapter accepts messages from,
    ///         as a 32-byte pubkey.
    function solanaPeer() external view returns (bytes32);
}
