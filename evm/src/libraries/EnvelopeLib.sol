// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title EnvelopeLib
/// @notice Canonical Solana -> Base message envelope. Layout is specified in
///         docs/02-message-format.md and must stay byte-identical to the Rust
///         encoder in solana/programs/base-caller/src/envelope.rs.
///
/// Packed big-endian, 103-byte fixed header followed by calldata:
///
///   off size field
///     0    1 version
///     1    1 msgType
///     2    2 srcChainId
///     4    2 dstChainId
///     6   32 sender
///    38    8 nonce
///    46   20 target
///    66   16 value
///    82    8 gasLimit
///    90    8 expiry
///    98    1 mode
///    99    4 calldataLen
///   103    n calldata
library EnvelopeLib {
    uint256 internal constant HEADER_SIZE = 103;

    uint8 internal constant VERSION = 1;
    uint8 internal constant MSG_TYPE_CALL = 0;

    uint8 internal constant MODE_DIRECT = 0;
    uint8 internal constant MODE_ACCOUNT = 1;

    /// @notice Internal chain ids. Deliberately not a transport's; adapters
    ///         translate at the boundary. One registry shared with the Rust
    ///         side. See docs/02-message-format.md.
    uint16 internal constant CHAIN_ID_SOLANA = 1;
    uint16 internal constant CHAIN_ID_ETHEREUM = 2;
    uint16 internal constant CHAIN_ID_BASE = 3;

    struct Envelope {
        uint8 version;
        uint8 msgType;
        uint16 srcChainId;
        uint16 dstChainId;
        bytes32 sender;
        uint64 nonce;
        address target;
        uint128 value;
        uint64 gasLimit;
        uint64 expiry;
        uint8 mode;
        bytes callData;
    }

    error MalformedEnvelope();

    /// @notice Decode a calldata envelope, reverting on a malformed one.
    function decode(bytes calldata env) internal pure returns (Envelope memory e) {
        bool ok;
        (ok, e) = tryDecode(env);
        if (!ok) revert MalformedEnvelope();
    }

    /// @notice Decode without reverting. The gateway needs the failure as a
    ///         value so it can record a malformed envelope as terminally
    ///         rejected instead of reverting the delivery back to the
    ///         transport. Requires the length to exactly match the declared
    ///         calldata length, so trailing bytes can never ride along
    ///         unnoticed inside a message that still hashes cleanly.
    function tryDecode(bytes calldata env) internal pure returns (bool ok, Envelope memory e) {
        if (env.length < HEADER_SIZE) return (false, e);

        uint256 p;
        assembly {
            p := env.offset
        }

        uint256 w;

        // bytes 0..31: version | msgType | srcChainId | dstChainId | (start of sender)
        assembly {
            w := calldataload(p)
        }
        e.version = uint8(w >> 248);
        e.msgType = uint8(w >> 240);
        e.srcChainId = uint16(w >> 224);
        e.dstChainId = uint16(w >> 208);

        // bytes 6..37: sender (exactly one word)
        assembly {
            w := calldataload(add(p, 6))
        }
        e.sender = bytes32(w);

        // bytes 38..69: nonce(8) | target(20) | (start of value)
        assembly {
            w := calldataload(add(p, 38))
        }
        e.nonce = uint64(w >> 192);
        e.target = address(uint160(w >> 32));

        // bytes 66..97: value(16) | gasLimit(8) | expiry(8)
        assembly {
            w := calldataload(add(p, 66))
        }
        e.value = uint128(w >> 128);
        e.gasLimit = uint64(w >> 64);
        e.expiry = uint64(w);

        // bytes 98..102: mode(1) | calldataLen(4)
        assembly {
            w := calldataload(add(p, 98))
        }
        e.mode = uint8(w >> 248);
        uint256 len = uint32(w >> 216);

        if (env.length != HEADER_SIZE + len) return (false, e);
        e.callData = env[HEADER_SIZE:];
        ok = true;
    }

    /// @notice Mirror of the Rust encoder. Present so tests can assert
    ///         cross-language parity on a fixed vector rather than trusting that
    ///         two hand-written codecs agree.
    function encode(Envelope memory e) internal pure returns (bytes memory) {
        return abi.encodePacked(
            e.version,
            e.msgType,
            e.srcChainId,
            e.dstChainId,
            e.sender,
            e.nonce,
            e.target,
            e.value,
            e.gasLimit,
            e.expiry,
            e.mode,
            uint32(e.callData.length),
            e.callData
        );
    }

    /// @notice Message id is the hash of the whole envelope, not of
    ///         (sender, nonce) -- see docs/02-message-format.md for why.
    function messageId(bytes calldata env) internal pure returns (bytes32) {
        return keccak256(env);
    }

    /// @notice First four bytes of calldata, or 0x00000000 for a bare call.
    function selector(Envelope memory e) internal pure returns (bytes4 s) {
        bytes memory d = e.callData;
        if (d.length < 4) return bytes4(0);
        assembly {
            s := mload(add(d, 32))
        }
    }
}
