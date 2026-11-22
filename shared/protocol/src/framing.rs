//! Bounded length-prefixed UTF-8 frames for Protocol v1.

pub const HEADER_BYTES: usize = 4;
/// Recommended v1 payload cap, excluding the header. Endpoints enforce their own cap.
pub const DEFAULT_MAX_PAYLOAD_BYTES: u32 = 8_388_608;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    Empty,
    Oversize,
    InvalidUtf8,
}

pub fn encode_header(
    payload_len: usize,
    max_payload: u32,
) -> Result<[u8; HEADER_BYTES], FrameError> {
    let len = u32::try_from(payload_len).map_err(|_| FrameError::Oversize)?;
    validate_len(len, max_payload)?;
    Ok(len.to_be_bytes())
}

/// Validate the declared payload length before reading or allocating the body.
pub fn decode_header(header: [u8; HEADER_BYTES], max_payload: u32) -> Result<u32, FrameError> {
    let len = u32::from_be_bytes(header);
    validate_len(len, max_payload)?;
    Ok(len)
}

/// Borrow exactly one UTF-8 payload and the unconsumed suffix, without parsing JSON.
/// `Ok(None)` means more bytes are needed; `Err` ends framing for this stream.
pub fn split_frame(bytes: &[u8], max_payload: u32) -> Result<Option<(&str, &[u8])>, FrameError> {
    let Some((header, body_and_rest)) = bytes.split_first_chunk::<HEADER_BYTES>() else {
        return Ok(None);
    };
    let len =
        usize::try_from(decode_header(*header, max_payload)?).map_err(|_| FrameError::Oversize)?;
    let Some((payload, rest)) = body_and_rest.split_at_checked(len) else {
        return Ok(None);
    };
    let text = std::str::from_utf8(payload).map_err(|_| FrameError::InvalidUtf8)?;
    Ok(Some((text, rest)))
}

fn validate_len(len: u32, max_payload: u32) -> Result<(), FrameError> {
    if len == 0 {
        return Err(FrameError::Empty);
    }
    if len > max_payload {
        return Err(FrameError::Oversize);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_big_endian_and_limits_are_inclusive() {
        assert_eq!(encode_header(258, 258), Ok([0, 0, 1, 2]));
        assert_eq!(decode_header([0, 0, 1, 2], 258), Ok(258));
        assert_eq!(
            encode_header(8_388_608, DEFAULT_MAX_PAYLOAD_BYTES),
            Ok([0, 128, 0, 0])
        );
        assert_eq!(
            encode_header(8_388_609, DEFAULT_MAX_PAYLOAD_BYTES),
            Err(FrameError::Oversize)
        );
        assert_eq!(encode_header(9, 8), Err(FrameError::Oversize));
        #[cfg(target_pointer_width = "64")]
        assert_eq!(
            encode_header(u32::MAX as usize + 1, u32::MAX),
            Err(FrameError::Oversize)
        );
    }

    #[test]
    fn empty_and_oversize_headers_fail_before_payload() {
        assert_eq!(
            split_frame(&[0; HEADER_BYTES], DEFAULT_MAX_PAYLOAD_BYTES),
            Err(FrameError::Empty)
        );
        assert_eq!(
            split_frame(&[255; HEADER_BYTES], DEFAULT_MAX_PAYLOAD_BYTES),
            Err(FrameError::Oversize)
        );
        assert_eq!(encode_header(0, 8), Err(FrameError::Empty));
    }

    #[test]
    fn frame_prefixes_do_not_consume_partial_header_or_body() {
        let frame = b"\0\0\0\x05false";
        for len in 0..frame.len() {
            assert_eq!(
                split_frame(&frame[..len], DEFAULT_MAX_PAYLOAD_BYTES),
                Ok(None)
            );
        }
        assert_eq!(
            split_frame(frame, DEFAULT_MAX_PAYLOAD_BYTES),
            Ok(Some(("false", &[][..])))
        );
    }

    #[test]
    fn split_returns_borrowed_exact_frame_and_rejects_invalid_utf8() {
        let frames = b"\0\0\0\x04null\0\0\0\x04\"\xc3\xa9\"";
        let (first, remaining) = split_frame(frames, DEFAULT_MAX_PAYLOAD_BYTES)
            .unwrap()
            .unwrap();
        assert_eq!(first, "null");
        assert_eq!(first.as_ptr(), frames[HEADER_BYTES..].as_ptr());
        assert_eq!(remaining, &frames[8..]);
        assert_eq!(remaining.as_ptr(), frames[8..].as_ptr());
        assert_eq!(
            split_frame(&remaining[..6], DEFAULT_MAX_PAYLOAD_BYTES),
            Ok(None)
        );
        assert_eq!(
            split_frame(remaining, DEFAULT_MAX_PAYLOAD_BYTES),
            Ok(Some(("\"é\"", &[][..])))
        );
        assert_eq!(
            split_frame(b"\0\0\0\x01\xff", DEFAULT_MAX_PAYLOAD_BYTES),
            Err(FrameError::InvalidUtf8)
        );
    }
}
