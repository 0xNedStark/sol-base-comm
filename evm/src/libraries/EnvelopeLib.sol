// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title EnvelopeLib
/// @notice Canonical Solana -> Base message envelope. Layout is specified in
///         docs/02-message-format.md and must stay byte-identical to the Rust
///         encoder in solana/programs/base-caller/src/envelope.rs.
///
/// Packed big-endian, 101-byte fixed header followed by calldata:
///
///   off size field
///     0    1 version
///     1    1 msgType
///     2    2 srcChainId
///     4   32 sender
///    36    8 nonce
///    44   20 target
///    64   16 value
///    80    8 gasLimit
///    88    8 expiry
///    96    1 mode
///    97    4 calldataLen
///   101    n calldata
library EnvelopeLib {
    uint256 internal constant HEADER_SIZE = 101;

    uint8 internal constant VERSION = 1;
    uint8 internal constant MSG_TYPE_CALL = 0;

    uint8 internal constant MODE_DIRECT = 0;
    uint8 internal constant MODE_ACCOUNT = 1;

    /// @notice Internal chain id for Solana. Deliberately not a transport's id;
    ///         adapters translate at the boundary. See docs/02-message-format.md.
    uint16 internal constant CHAIN_ID_SOLANA = 1;

    struct Envelope {
        uint8 version;
        uint8 msgType;
        uint16 srcChainId;
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

    /// @notice Decode a calldata envelope. Reverts unless the length exactly
    ///         matches the declared calldata length, so trailing bytes can never
    ///         ride along unnoticed inside a message that still hashes cleanly.
    function decode(bytes calldata env) internal pure returns (Envelope memory e) {
        if (env.length < HEADER_SIZE) revert MalformedEnvelope();

        uint256 p;
        assembly {
            p := env.offset
        }

        uint256 w;

        // bytes 0..31: version | msgType | srcChainId | (start of sender)
        assembly {
            w := calldataload(p)
        }
        e.version = uint8(w >> 248);
        e.msgType = uint8(w >> 240);
        e.srcChainId = uint16(w >> 224);

        // bytes 4..35: sender (exactly one word)
        assembly {
            w := calldataload(add(p, 4))
        }
        e.sender = bytes32(w);

        // bytes 36..67: nonce(8) | target(20) | (start of value)
        assembly {
            w := calldataload(add(p, 36))
        }
        e.nonce = uint64(w >> 192);
        e.target = address(uint160(w >> 32));

        // bytes 64..95: value(16) | gasLimit(8) | expiry(8)
        assembly {
            w := calldataload(add(p, 64))
        }
        e.value = uint128(w >> 128);
        e.gasLimit = uint64(w >> 64);
        e.expiry = uint64(w);

        // bytes 96..100: mode(1) | calldataLen(4)
        assembly {
            w := calldataload(add(p, 96))
        }
        e.mode = uint8(w >> 248);
        uint256 len = uint32(w >> 216);

        if (env.length != HEADER_SIZE + len) revert MalformedEnvelope();
        e.callData = env[HEADER_SIZE:];
    }

    /// @notice Mirror of the Rust encoder. Present so tests can assert
    ///         cross-language parity on a fixed vector rather than trusting that
    ///         two hand-written codecs agree.
    function encode(Envelope memory e) internal pure returns (bytes memory) {
        return abi.encodePacked(
            e.version,
            e.msgType,
            e.srcChainId,
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
