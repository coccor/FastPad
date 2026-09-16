use crate::{FastPadError, Result};
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

pub const FRAME_MAGIC: [u8; 4] = *b"FPI1";
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const HEADER_BYTES: usize = FRAME_MAGIC.len() + 1 + 4;

const COMMAND_OPEN: u8 = 1;
const COMMAND_NEW: u8 = 2;
const COMMAND_ACTIVATE: u8 = 3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IpcRequest {
    Open(PathBuf),
    New,
    Activate,
}

pub fn encode_frame(request: &IpcRequest) -> Result<Vec<u8>> {
    let (command, payload) = match request {
        IpcRequest::Open(path) => (COMMAND_OPEN, encode_path(path)?),
        IpcRequest::New => (COMMAND_NEW, Vec::new()),
        IpcRequest::Activate => (COMMAND_ACTIVATE, Vec::new()),
    };
    if HEADER_BYTES + payload.len() > MAX_FRAME_BYTES {
        return Err(FastPadError::Ipc("frame exceeds 64 KiB"));
    }
    let mut frame = Vec::with_capacity(HEADER_BYTES + payload.len());
    frame.extend_from_slice(&FRAME_MAGIC);
    frame.push(command);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_frame(bytes: &[u8]) -> Result<IpcRequest> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(FastPadError::Ipc("frame exceeds 64 KiB"));
    }
    if bytes.len() < HEADER_BYTES {
        return Err(FastPadError::Ipc("frame header is truncated"));
    }
    if bytes[..FRAME_MAGIC.len()] != FRAME_MAGIC {
        return Err(FastPadError::Ipc("frame magic is not FPI1"));
    }
    let command = bytes[FRAME_MAGIC.len()];
    let mut length = [0_u8; 4];
    length.copy_from_slice(&bytes[FRAME_MAGIC.len() + 1..HEADER_BYTES]);
    let payload = &bytes[HEADER_BYTES..];
    if payload.len() != u32::from_le_bytes(length) as usize {
        return Err(FastPadError::Ipc("frame length does not match its payload"));
    }
    match command {
        COMMAND_OPEN => decode_path(payload).map(IpcRequest::Open),
        COMMAND_NEW | COMMAND_ACTIVATE if !payload.is_empty() => {
            Err(FastPadError::Ipc("command does not accept a payload"))
        }
        COMMAND_NEW => Ok(IpcRequest::New),
        COMMAND_ACTIVATE => Ok(IpcRequest::Activate),
        _ => Err(FastPadError::Ipc("unknown command")),
    }
}

fn encode_path(path: &std::path::Path) -> Result<Vec<u8>> {
    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    validate_path_units(&units)?;
    Ok(units.iter().flat_map(|unit| unit.to_le_bytes()).collect())
}

fn decode_path(payload: &[u8]) -> Result<PathBuf> {
    if !payload.len().is_multiple_of(2) {
        return Err(FastPadError::Ipc("path payload has an odd byte length"));
    }
    let units = payload
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();
    validate_path_units(&units)?;
    Ok(PathBuf::from(OsString::from_wide(&units)))
}

fn validate_path_units(units: &[u16]) -> Result<()> {
    if units.is_empty() {
        return Err(FastPadError::Ipc("path payload is empty"));
    }
    if units.contains(&0) {
        return Err(FastPadError::Ipc("path payload contains NUL"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn request_round_trips_utf16_path() {
        let request = IpcRequest::Open(PathBuf::from(r"C:\notes\zăpadă.md"));
        assert_eq!(
            decode_frame(&encode_frame(&request).unwrap()).unwrap(),
            request
        );
    }

    #[test]
    fn oversized_and_unknown_frames_are_rejected() {
        assert!(decode_frame(&vec![0; 65_537]).is_err());
        assert!(decode_frame(b"FPI1\xFF\0\0\0").is_err());
    }

    fn frame(command: u8, declared: u32, payload: &[u8]) -> Vec<u8> {
        let mut bytes = b"FPI1".to_vec();
        bytes.push(command);
        bytes.extend_from_slice(&declared.to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn rejection(bytes: &[u8]) -> &'static str {
        match decode_frame(bytes) {
            Err(FastPadError::Ipc(reason)) => reason,
            other => panic!("expected an IPC rejection, got {other:?}"),
        }
    }

    #[test]
    fn frame_layout_is_magic_command_little_endian_length_and_utf16le_path() {
        // Break caught: a big-endian length or UTF-8 path silently breaks older/newer secondaries.
        let bytes = encode_frame(&IpcRequest::Open(PathBuf::from("a\u{0103}"))).unwrap();
        assert_eq!(bytes, frame(1, 4, &[b'a', 0, 0x03, 0x01]));
        assert_eq!(encode_frame(&IpcRequest::New).unwrap(), frame(2, 0, &[]));
        assert_eq!(
            encode_frame(&IpcRequest::Activate).unwrap(),
            frame(3, 0, &[])
        );
    }

    #[test]
    fn new_and_activate_round_trip_without_payload() {
        for request in [IpcRequest::New, IpcRequest::Activate] {
            assert_eq!(
                decode_frame(&encode_frame(&request).unwrap()).unwrap(),
                request
            );
        }
    }

    #[test]
    fn strict_decoder_rejects_each_malformed_shape_for_its_own_reason() {
        // Break caught: a lenient decoder that truncates, ignores trailing bytes, or maps unknown
        // commands onto a default request mutates the primary from a malformed client.
        assert_eq!(rejection(&frame(0xFF, 0, &[])), "unknown command");
        assert_eq!(
            rejection(&frame(1, 3, &[b'a', 0, b'b'])),
            "path payload has an odd byte length"
        );
        assert_eq!(
            rejection(&frame(1, 4, &[b'a', 0, 0, 0])),
            "path payload contains NUL"
        );
        assert_eq!(rejection(&frame(1, 0, &[])), "path payload is empty");
        assert_eq!(
            rejection(&frame(1, 2, &[b'a', 0, b'b', 0])),
            "frame length does not match its payload"
        );
        assert_eq!(
            rejection(&frame(1, 4, &[b'a', 0])),
            "frame length does not match its payload"
        );
        assert_eq!(
            rejection(&frame(2, 2, &[b'a', 0])),
            "command does not accept a payload"
        );
        assert_eq!(rejection(b"FPI2\x02\0\0\0\0"), "frame magic is not FPI1");
        assert_eq!(rejection(b"FPI1\x02\0\0\0"), "frame header is truncated");
        assert_eq!(
            rejection(&vec![0; MAX_FRAME_BYTES + 1]),
            "frame exceeds 64 KiB"
        );
    }

    #[test]
    fn largest_path_that_fits_is_accepted_and_one_more_unit_is_refused() {
        // Break caught: an off-by-one limit either rejects valid long paths or emits frames the
        // primary must refuse.
        let units = (MAX_FRAME_BYTES - HEADER_BYTES) / 2;
        let fits = PathBuf::from("a".repeat(units));
        let frame = encode_frame(&IpcRequest::Open(fits.clone())).unwrap();
        assert!(frame.len() <= MAX_FRAME_BYTES);
        assert_eq!(decode_frame(&frame).unwrap(), IpcRequest::Open(fits));
        assert!(encode_frame(&IpcRequest::Open(PathBuf::from("a".repeat(units + 1)))).is_err());
    }

    #[test]
    fn encoder_refuses_paths_the_decoder_would_reject() {
        assert!(encode_frame(&IpcRequest::Open(PathBuf::new())).is_err());
        assert!(encode_frame(&IpcRequest::Open(PathBuf::from("a\0b"))).is_err());
    }
}
