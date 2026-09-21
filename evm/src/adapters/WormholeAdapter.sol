// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Auth} from "../Auth.sol";
import {ISolanaGateway} from "../interfaces/ISolanaGateway.sol";
import {ITransportAdapter} from "../interfaces/ITransportAdapter.sol";

interface IWormhole {
    struct Signature {
        bytes32 r;
        bytes32 s;
        uint8 v;
        uint8 guardianIndex;
    }

    struct VM {
        uint8 version;
        uint32 timestamp;
        uint32 nonce;
        uint16 emitterChainId;
        bytes32 emitterAddress;
        uint64 sequence;
        uint8 consistencyLevel;
        bytes payload;
        uint32 guardianSetIndex;
        Signature[] signatures;
        bytes32 hash;
    }

    function parseAndVerifyVM(bytes calldata encodedVM)
        external
        view
        returns (VM memory vm, bool valid, string memory reason);
}

/// @title WormholeAdapter
/// @notice Verifies a Wormhole VAA carrying one of our envelopes and forwards
///         the payload to the gateway.
///
/// Delivery is permissionless: a VAA is a self-contained, guardian-signed
/// artifact, so anyone holding it can land it. That property is why Wormhole is
/// the recommended second transport -- if every relayer stops, messages are
/// still deliverable by hand. See docs/03-transport-comparison.md.
contract WormholeAdapter is ITransportAdapter, Auth {
    /// @notice Wormhole's own chain id for Solana.
    ///
    /// Verified against the vendor's own published npm package
    /// (wormhole-foundation/sdk-base): Solana 1, Ethereum 2, Base 30,
    /// Base Sepolia 10004.
    uint16 public constant WORMHOLE_CHAIN_ID_SOLANA = 1;

    /// @notice Consistency level meaning "finalized" on Solana.
    ///
    /// Verified against `wormhole-anchor-sdk`, whose `Finality` enum
    /// serialises `Confirmed` to 0 and `Finalized` to 1. The Solana side
    /// publishes at `Finality::Finalized`; this is the matching floor.
    uint8 public constant CONSISTENCY_FINALIZED = 1;

    IWormhole public immutable wormhole;
    ISolanaGateway public immutable gateway;

    /// @notice The Solana emitter (our base_caller program's emitter address)
    ///         whose messages this adapter will accept. Nothing else.
    bytes32 public solanaPeer;

    /// @notice Minimum consistency level the VAA must have been published at.
    ///
    /// This is the reorg guard and it is the setting most likely to be wrong in
    /// the unsafe direction, because the unsafe value is also the fast one: a
    /// message attested on a Solana slot that later gets rolled back is a free
    /// forged call. Pass `CONSISTENCY_FINALIZED`; never 0 (confirmed).
    uint8 public minConsistencyLevel;

    mapping(bytes32 => bool) public consumedVaa;

    event MinConsistencyLevelSet(uint8 level);

    error InvalidVaa(string reason);
    error WrongEmitterChain(uint16 chainId);
    error WrongEmitter(bytes32 emitter);
    error InsufficientConsistency(uint8 level);
    error AlreadyConsumed(bytes32 vaaHash);

    constructor(address owner_, address wormhole_, address gateway_, bytes32 solanaPeer_, uint8 minConsistencyLevel_)
        Auth(owner_)
    {
        wormhole = IWormhole(wormhole_);
        gateway = ISolanaGateway(gateway_);
        solanaPeer = solanaPeer_;
        minConsistencyLevel = minConsistencyLevel_;
        emit PeerRegistered(solanaPeer_);
        emit MinConsistencyLevelSet(minConsistencyLevel_);
    }

    function transportId() external pure returns (string memory) {
        return "wormhole-v1";
    }

    /// @notice Submit a guardian-signed VAA. Permissionless by design.
    function receiveMessage(bytes calldata encodedVaa) external whenNotPaused {
        (IWormhole.VM memory vm_, bool valid, string memory reason) = wormhole.parseAndVerifyVM(encodedVaa);
        if (!valid) revert InvalidVaa(reason);

        if (vm_.emitterChainId != WORMHOLE_CHAIN_ID_SOLANA) revert WrongEmitterChain(vm_.emitterChainId);
        if (vm_.emitterAddress != solanaPeer) revert WrongEmitter(vm_.emitterAddress);
        if (vm_.consistencyLevel < minConsistencyLevel) revert InsufficientConsistency(vm_.consistencyLevel);

        // The gateway dedupes on envelope hash too; this is the cheaper local
        // check and it keeps the adapter's event log honest.
        if (consumedVaa[vm_.hash]) revert AlreadyConsumed(vm_.hash);
        consumedVaa[vm_.hash] = true;

        emit MessageForwarded(keccak256(vm_.payload), vm_.hash);

        // Forward verbatim. The adapter never parses the envelope.
        gateway.deliver(vm_.payload);
    }

    /// @dev Should be held by a timelock: repointing the peer is equivalent to
    ///      replacing the Solana side of the system.
    function setSolanaPeer(bytes32 peer) external onlyOwner {
        solanaPeer = peer;
        emit PeerRegistered(peer);
    }

    function setMinConsistencyLevel(uint8 level) external onlyOwner {
        minConsistencyLevel = level;
        emit MinConsistencyLevelSet(level);
    }
}
