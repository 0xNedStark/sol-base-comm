//! LayerZero executor options, built in-program from the envelope.
//!
//! The gas the executor is paid for on the destination must match the
//! `gasLimit` the envelope promises. Taking options as a client argument would
//! let the two disagree: a client could pay for 50k while the envelope claims
//! 300k, and the call would run out of gas on arrival for reasons invisible
//! from Solana. So the options are derived here from the envelope's own bytes.
//!
//! Type-3 option layout, one executor lzReceive entry:
//!
//! ```text
//!   0  2  u16 BE  options type, always 3
//!   2  1  u8      worker id, 1 = executor
//!   3  2  u16 BE  option length, 1 + params
//!   5  1  u8      option type, 1 = LZRECEIVE
//!   6 16  u128 BE gas
//! ```
//!
//! Checked against LayerZero's own TypeScript builder
//! (`lz-v2-utilities`, `Options.newOptions().addExecutorLzReceiveOption(gas)`)
//! -- see the golden vectors in the tests below.

pub const TYPE_3: u16 = 3;
pub const WORKER_EXECUTOR: u8 = 1;
pub const OPTION_LZRECEIVE: u8 = 1;

/// Encode a single executor lzReceive option carrying `gas`.
pub fn executor_lz_receive(gas: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(22);
    out.extend_from_slice(&TYPE_3.to_be_bytes());
    out.push(WORKER_EXECUTOR);
    // length covers the option type byte plus the 16-byte gas parameter
    out.extend_from_slice(&(1u16 + 16).to_be_bytes());
    out.push(OPTION_LZRECEIVE);
    out.extend_from_slice(&(gas as u128).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{:02x}", x)).collect()
    }

    /// Byte-for-byte output of
    /// `Options.newOptions().addExecutorLzReceiveOption(gas, 0).toHex()`.
    #[test]
    fn matches_layerzeros_own_builder() {
        assert_eq!(
            hex(&executor_lz_receive(200_000)),
            "00030100110100000000000000000000000000030d40"
        );
        assert_eq!(
            hex(&executor_lz_receive(300_000)),
            "000301001101000000000000000000000000000493e0"
        );
    }

    #[test]
    fn shape_is_a_single_type_3_executor_option() {
        let o = executor_lz_receive(1);
        assert_eq!(o.len(), 22);
        assert_eq!(u16::from_be_bytes([o[0], o[1]]), TYPE_3);
        assert_eq!(o[2], WORKER_EXECUTOR);
        assert_eq!(u16::from_be_bytes([o[3], o[4]]), 17);
        assert_eq!(o[5], OPTION_LZRECEIVE);
        assert_eq!(u128::from_be_bytes(o[6..22].try_into().unwrap()), 1);
    }
}
