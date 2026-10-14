//! Independently authored offline codec for the observed legacy DiracIO framing.
//! No networking, calibration, filter upload, or implicit message ID selection.
use anyhow::{bail, Result};

pub const MARKER: [u8; 3] = [0x21, 0x01, 0xff];
pub const MAX_PAYLOAD: usize = 1024 * 1024;
/// Installed proxy's parser requires marker + 12-byte header + payload < 32768.
pub const MAX_LEGACY_PAYLOAD: usize = 32752;
const HEADER_SIZE: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub message_id: u32,
    pub payload: Vec<u8>,
}

pub fn payload_hash(payload: &[u8]) -> u32 {
    payload.iter().fold(0u32, |h, &byte| {
        h.wrapping_mul(31).wrapping_add(byte as u32)
    })
}

/// Encode a frame within the observed installed legacy parser's size limit.
/// Caller must establish the message ID separately; no transport is invoked.
pub fn encode_frame(message_id: u32, payload: &[u8]) -> Result<Vec<u8>> {
    if payload.len() > MAX_LEGACY_PAYLOAD {
        bail!("Payload exceeds the observed legacy frame limit ({MAX_LEGACY_PAYLOAD} bytes)");
    }
    let mut bytes = Vec::with_capacity(HEADER_SIZE + payload.len());
    bytes.extend_from_slice(&MARKER);
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&message_id.to_be_bytes());
    bytes.extend_from_slice(&payload_hash(payload).to_be_bytes());
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

/// Decode exactly one frame from the beginning of a byte slice.
/// Returns bytes consumed so callers can handle coalesced frames. A short input
/// is an error; a stream caller must retain it and wait for more bytes itself.
/// A hard 1 MiB allocation ceiling also bounds capture-file decoding.
pub fn decode_frame(bytes: &[u8]) -> Result<(Frame, usize)> {
    if bytes.len() < HEADER_SIZE {
        bail!("Truncated DiracIO frame header");
    }
    if bytes[..3] != MARKER {
        bail!("Invalid DiracIO frame marker");
    }
    // Lengths are checked before any payload allocation.
    let length = u32::from_be_bytes(bytes[3..7].try_into()?) as usize;
    if length > MAX_PAYLOAD {
        bail!("DiracIO payload exceeds the 1 MiB decoding limit");
    }
    let consumed = HEADER_SIZE
        .checked_add(length)
        .ok_or_else(|| anyhow::anyhow!("Frame length overflow"))?;
    if bytes.len() < consumed {
        bail!("Truncated DiracIO frame payload");
    }
    let message_id = u32::from_be_bytes(bytes[7..11].try_into()?);
    let expected_hash = u32::from_be_bytes(bytes[11..15].try_into()?);
    let payload = &bytes[HEADER_SIZE..consumed];
    if payload_hash(payload) != expected_hash {
        bail!("DiracIO payload hash mismatch");
    }
    Ok((
        Frame {
            message_id,
            payload: payload.to_vec(),
        },
        consumed,
    ))
}

/// Read-only request kinds observed in the embedded diracio.proto descriptor.
/// Values do not establish device support or the outer frame's message ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RequestType {
    GetUnit = 0,
    GetVolume = 1,
    GetChannelAttenuation = 2,
    GetActiveFilterSlot = 3,
    GetStimulusState = 4,
    GetFilteringState = 5,
    GetTopologyInfo = 6,
    GetStimulusInfo = 7,
    GetProtocolInfo = 8,
}

fn varint(mut value: u32, bytes: &mut Vec<u8>) {
    while value >= 0x80 {
        bytes.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
}

/// Produce a protobuf Request payload from established field numbers only.
/// Required requestType=1 and deviceIndex=2 are always emitted, including zero.
/// Optional channel=3 is emitted only when explicitly supplied.
pub fn encode_read_request(kind: RequestType, device_index: u32, channel: Option<u32>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(14);
    bytes.push(0x08);
    varint(kind as u32, &mut bytes);
    bytes.push(0x10);
    varint(device_index, &mut bytes);
    if let Some(channel) = channel {
        bytes.push(0x18);
        varint(channel, &mut bytes);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_vectors_include_unsigned_bytes_and_wrapping() {
        assert_eq!(payload_hash(&[]), 0);
        assert_eq!(payload_hash(b"abc"), 96_354);
        assert_eq!(payload_hash(&[0xff]), 255);
        // 255 * (31^8 - 1) / 30, reduced modulo 2^32.
        assert_eq!(payload_hash(&[0xff; 8]), 3_963_989_888);
    }

    #[test]
    fn wire_layout_roundtrip_and_coalesced_frames() {
        let first = encode_frame(0x01020304, b"abc").unwrap();
        assert_eq!(&first[..11], &[0x21, 0x01, 0xff, 0, 0, 0, 3, 1, 2, 3, 4]);
        assert_eq!(&first[11..15], &96_354u32.to_be_bytes());
        let second = encode_frame(0, &[]).unwrap();
        let both = [first.clone(), second].concat();
        let (decoded, consumed) = decode_frame(&both).unwrap();
        assert_eq!(
            decoded,
            Frame {
                message_id: 0x01020304,
                payload: b"abc".to_vec()
            }
        );
        assert_eq!(consumed, first.len());
        assert_eq!(
            decode_frame(&both[consumed..]).unwrap().0.payload,
            Vec::<u8>::new()
        );
    }

    #[test]
    fn rejects_every_truncation_and_hash_corruption() {
        let bytes = encode_frame(1, b"measurement").unwrap();
        for length in 0..bytes.len() {
            assert!(decode_frame(&bytes[..length]).is_err());
        }
        let mut bad = bytes.clone();
        bad[15] ^= 1;
        assert!(decode_frame(&bad).is_err());
        bad = bytes;
        bad[0] = 0;
        assert!(decode_frame(&bad).is_err());
    }

    #[test]
    fn checks_length_before_allocating_or_waiting() {
        let mut bytes = encode_frame(0, &[]).unwrap();
        bytes[3..7].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(decode_frame(&bytes).is_err());
        assert!(encode_frame(0, &vec![0; MAX_LEGACY_PAYLOAD + 1]).is_err());
        assert!(encode_frame(0, &vec![0; MAX_LEGACY_PAYLOAD]).is_ok());
    }

    #[test]
    fn required_zero_fields_and_optional_channel_are_preserved() {
        assert_eq!(
            encode_read_request(RequestType::GetUnit, 0, None),
            vec![8, 0, 16, 0]
        );
        assert_eq!(
            encode_read_request(RequestType::GetProtocolInfo, 300, Some(128)),
            vec![8, 8, 16, 0xac, 2, 24, 0x80, 1]
        );
        assert_eq!(
            encode_read_request(RequestType::GetVolume, u32::MAX, None),
            vec![8, 1, 16, 0xff, 0xff, 0xff, 0xff, 0x0f]
        );
    }
}
