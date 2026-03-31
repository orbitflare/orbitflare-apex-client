//! The wire format, written by hand so this crate has no dependency on the
//! sender's internals. See the Apex docs at https://docs.orbitflare.com/apex.

use solana_signature::Signature;

/// Solana's packet limit.
pub const MAX_TRANSACTION_SIZE: usize = 1232;
pub const MAX_ADMISSION_FRAME: usize = 1 + 2 + 512;

/// `u64 LE length` before the transaction bytes; `mev_protect` byte and
/// `Option<u16>` (`0`, or `1` + `u16 LE`) after them.
pub fn frame_parts(len: usize, mev_protect: bool, max_retries: Option<u16>) -> ([u8; 8], Vec<u8>) {
    let header = (len as u64).to_le_bytes();
    let mut trailer = Vec::with_capacity(4);
    trailer.push(u8::from(mev_protect));
    match max_retries {
        None => trailer.push(0),
        Some(n) => {
            trailer.push(1);
            trailer.extend_from_slice(&n.to_le_bytes());
        }
    }
    (header, trailer)
}

/// One contiguous packet, for callers that want a single buffer.
pub fn encode_packet(wire: &[u8], mev_protect: bool, max_retries: Option<u16>) -> Vec<u8> {
    let (header, trailer) = frame_parts(wire.len(), mev_protect, max_retries);
    let mut out = Vec::with_capacity(8 + wire.len() + trailer.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(wire);
    out.extend_from_slice(&trailer);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AdmissionCode {
    Ok = 0,
    Unauthorized = 1,
    RateLimited = 2,
    Invalid = 3,
    NoTip = 4,
    BelowFloor = 5,
    Busy = 6,
    MalformedPacket = 7,
    Unknown = 255,
}

impl AdmissionCode {
    pub const fn from_u8(b: u8) -> Self {
        match b {
            0 => Self::Ok,
            1 => Self::Unauthorized,
            2 => Self::RateLimited,
            3 => Self::Invalid,
            4 => Self::NoTip,
            5 => Self::BelowFloor,
            6 => Self::Busy,
            7 => Self::MalformedPacket,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    Accepted(Signature),
    Rejected { code: AdmissionCode, message: String },
}

impl Admission {
    pub const fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted(_))
    }
}

pub fn decode_admission(frame: &[u8]) -> Option<Admission> {
    let (&code, rest) = frame.split_first()?;
    if code == 0 {
        let sig: [u8; 64] = rest.get(..64)?.try_into().ok()?;
        return Some(Admission::Accepted(Signature::from(sig)));
    }
    let len = u16::from_le_bytes(rest.get(..2)?.try_into().ok()?) as usize;
    let message = String::from_utf8_lossy(rest.get(2..2 + len)?).into_owned();
    Some(Admission::Rejected { code: AdmissionCode::from_u8(code), message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_layout_is_bincode_of_transaction_packet() {
        // u64 LE len | bytes | mev | Option<u16>
        let p = encode_packet(&[9, 9, 9], true, Some(7));
        assert_eq!(p, vec![3, 0, 0, 0, 0, 0, 0, 0, 9, 9, 9, 1, 1, 7, 0]);
        let p = encode_packet(&[1], false, None);
        assert_eq!(p, vec![1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0]);
    }

    #[test]
    fn admission_round_trip() {
        let mut ok = vec![0u8];
        ok.extend_from_slice(&[5u8; 64]);
        assert_eq!(decode_admission(&ok), Some(Admission::Accepted(Signature::from([5u8; 64]))));
        let mut rej = vec![5u8, 3, 0];
        rej.extend_from_slice(b"low");
        assert_eq!(
            decode_admission(&rej),
            Some(Admission::Rejected { code: AdmissionCode::BelowFloor, message: "low".into() })
        );
        assert_eq!(decode_admission(&[]), None);
    }
}
