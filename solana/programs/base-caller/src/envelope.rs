//! Canonical envelope encoder.
//!
//! This is the source of truth for the Solana side of the wire format specified
//! in docs/02-message-format.md. It must stay byte-identical to
//! `evm/src/libraries/EnvelopeLib.sol`. Deliberately free of Anchor and Solana
//! types so it can be unit-tested on its own, which is the only cheap way to
//! catch a layout drift before it becomes a decode failure on Base.

pub const VERSION: u8 = 1;
pub const MSG_TYPE_CALL: u8 = 0;

/// Internal chain ids, not a transport's. One registry, shared by both sides.
/// See docs/02-message-format.md.
pub const CHAIN_SOLANA: u16 = 1;
pub const CHAIN_ETHEREUM: u16 = 2;
pub const CHAIN_BASE: u16 = 3;

/// Kept for callers that only ever send from Solana.
pub const SRC_CHAIN_SOLANA: u16 = CHAIN_SOLANA;

pub const MODE_DIRECT: u8 = 0;
pub const MODE_ACCOUNT: u8 = 1;

pub const HEADER_SIZE: usize = 103;

/// Upper bound on calldata, so a single message cannot be sized to exceed the
/// transport's payload limit or blow the Base-side gas budget.
pub const MAX_CALLDATA: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallParams {
    /// Internal id of the chain this call executes on. Every gateway checks
    /// it against its own id, so an envelope meant for one destination cannot
    /// be replayed on another.
    pub dst_chain_id: u16,
    /// Contract to call on the destination, 20 bytes.
    pub target: [u8; 20],
    /// Wei of native ETH to attach, drawn from the sender's gateway balance.
    pub value: u128,
    /// Gas for the inner call on Base.
    pub gas_limit: u64,
    /// Unix seconds after which the gateway refuses to execute. 0 = never.
    pub expiry: u64,
    /// MODE_DIRECT or MODE_ACCOUNT.
    pub mode: u8,
    /// ABI-encoded call, selector first.
    pub calldata: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    CalldataTooLarge,
    InvalidMode,
    InvalidDestination,
}

/// Encode one envelope. Packed big-endian, 103-byte header then calldata.
pub fn encode(sender: &[u8; 32], nonce: u64, p: &CallParams) -> Result<Vec<u8>, EncodeError> {
    if p.calldata.len() > MAX_CALLDATA {
        return Err(EncodeError::CalldataTooLarge);
    }
    if p.mode > MODE_ACCOUNT {
        return Err(EncodeError::InvalidMode);
    }
    if p.dst_chain_id == 0 || p.dst_chain_id == SRC_CHAIN_SOLANA {
        return Err(EncodeError::InvalidDestination);
    }

    let mut buf = Vec::with_capacity(HEADER_SIZE + p.calldata.len());
    buf.push(VERSION); //                                     off 0,  len 1
    buf.push(MSG_TYPE_CALL); //                               off 1,  len 1
    buf.extend_from_slice(&SRC_CHAIN_SOLANA.to_be_bytes()); // off 2,  len 2
    buf.extend_from_slice(&p.dst_chain_id.to_be_bytes()); //   off 4,  len 2
    buf.extend_from_slice(sender); //                         off 6,  len 32
    buf.extend_from_slice(&nonce.to_be_bytes()); //            off 38, len 8
    buf.extend_from_slice(&p.target); //                       off 46, len 20
    buf.extend_from_slice(&p.value.to_be_bytes()); //          off 66, len 16
    buf.extend_from_slice(&p.gas_limit.to_be_bytes()); //      off 82, len 8
    buf.extend_from_slice(&p.expiry.to_be_bytes()); //         off 90, len 8
    buf.push(p.mode); //                                       off 98, len 1
    buf.extend_from_slice(&(p.calldata.len() as u32).to_be_bytes()); // off 99, len 4
    buf.extend_from_slice(&p.calldata); //                     off 103

    debug_assert_eq!(buf.len(), HEADER_SIZE + p.calldata.len());
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector() -> (([u8; 32], u64, CallParams), Vec<u8>) {
        let sender = [0x11u8; 32];
        let nonce = 7u64;
        let p = CallParams {
            dst_chain_id: CHAIN_BASE,
            target: [0xAB; 20],
            value: 1_000_000_000_000_000_000u128, // 1 ETH
            gas_limit: 300_000,
            expiry: 1_700_000_000,
            mode: MODE_DIRECT,
            calldata: vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01],
        };
        let bytes = encode(&sender, nonce, &p).unwrap();
        ((sender, nonce, p), bytes)
    }

    #[test]
    fn field_offsets_match_the_spec() {
        let ((sender, nonce, p), b) = vector();

        assert_eq!(b.len(), HEADER_SIZE + p.calldata.len());
        assert_eq!(b[0], VERSION);
        assert_eq!(b[1], MSG_TYPE_CALL);
        assert_eq!(
            u16::from_be_bytes(b[2..4].try_into().unwrap()),
            SRC_CHAIN_SOLANA
        );
        assert_eq!(
            u16::from_be_bytes(b[4..6].try_into().unwrap()),
            p.dst_chain_id
        );
        assert_eq!(&b[6..38], &sender[..]);
        assert_eq!(u64::from_be_bytes(b[38..46].try_into().unwrap()), nonce);
        assert_eq!(&b[46..66], &p.target[..]);
        assert_eq!(u128::from_be_bytes(b[66..82].try_into().unwrap()), p.value);
        assert_eq!(
            u64::from_be_bytes(b[82..90].try_into().unwrap()),
            p.gas_limit
        );
        assert_eq!(u64::from_be_bytes(b[90..98].try_into().unwrap()), p.expiry);
        assert_eq!(b[98], p.mode);
        assert_eq!(
            u32::from_be_bytes(b[99..103].try_into().unwrap()) as usize,
            p.calldata.len()
        );
        assert_eq!(&b[103..], &p.calldata[..]);
    }

    /// Frozen hex for the vector above. If this changes, the format changed, and
    /// the Solidity decoder plus the `version` byte must change with it.
    #[test]
    fn golden_vector_is_stable() {
        let (_, b) = vector();
        let hex: String = b.iter().map(|x| format!("{:02x}", x)).collect();
        assert_eq!(
            hex,
            concat!(
                "01",                                                               // version
                "00",                                                               // msgType
                "0001",                                                             // srcChainId
                "0003", // dstChainId (Base)
                "1111111111111111111111111111111111111111111111111111111111111111", // sender
                "0000000000000007", // nonce
                "abababababababababababababababababababab", // target
                "00000000000000000de0b6b3a7640000", // value (u128 BE)
                "00000000000493e0", // gasLimit
                "000000006553f100", // expiry
                "00",   // mode
                "00000005", // calldataLen
                "deadbeef01"  // calldata
            )
        );
    }

    #[test]
    fn rejects_oversized_calldata() {
        let p = CallParams {
            dst_chain_id: CHAIN_BASE,
            target: [0; 20],
            value: 0,
            gas_limit: 100_000,
            expiry: 0,
            mode: MODE_DIRECT,
            calldata: vec![0u8; MAX_CALLDATA + 1],
        };
        assert_eq!(encode(&[0; 32], 0, &p), Err(EncodeError::CalldataTooLarge));
    }

    #[test]
    fn rejects_unknown_mode() {
        let p = CallParams {
            dst_chain_id: CHAIN_BASE,
            target: [0; 20],
            value: 0,
            gas_limit: 100_000,
            expiry: 0,
            mode: 9,
            calldata: vec![],
        };
        assert_eq!(encode(&[0; 32], 0, &p), Err(EncodeError::InvalidMode));
    }

    #[test]
    fn rejects_bad_destination() {
        let mut p = CallParams {
            dst_chain_id: 0,
            target: [0; 20],
            value: 0,
            gas_limit: 100_000,
            expiry: 0,
            mode: MODE_DIRECT,
            calldata: vec![],
        };
        assert_eq!(
            encode(&[0; 32], 0, &p),
            Err(EncodeError::InvalidDestination)
        );
        p.dst_chain_id = SRC_CHAIN_SOLANA;
        assert_eq!(
            encode(&[0; 32], 0, &p),
            Err(EncodeError::InvalidDestination)
        );
    }
}
