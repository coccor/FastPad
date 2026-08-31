use crate::{FastPadError, Result};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedText {
    pub text: String,
    pub encoding: Encoding,
}

pub fn decode(bytes: &[u8]) -> Result<DecodedText> {
    let (text, encoding) = if let Some(body) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        (
            std::str::from_utf8(body)
                .map_err(|_| FastPadError::UnsupportedEncoding)?
                .to_owned(),
            Encoding::Utf8Bom,
        )
    } else if let Some(body) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        (decode_utf16(body, u16::from_le_bytes)?, Encoding::Utf16Le)
    } else if let Some(body) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        (decode_utf16(body, u16::from_be_bytes)?, Encoding::Utf16Be)
    } else {
        (
            std::str::from_utf8(bytes)
                .map_err(|_| FastPadError::UnsupportedEncoding)?
                .to_owned(),
            Encoding::Utf8,
        )
    };

    Ok(DecodedText { text, encoding })
}

pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => {
            let mut bytes = vec![0xEF, 0xBB, 0xBF];
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }
        Encoding::Utf16Le => encode_utf16(text, u16::to_le_bytes, [0xFF, 0xFE]),
        Encoding::Utf16Be => encode_utf16(text, u16::to_be_bytes, [0xFE, 0xFF]),
    }
}

fn decode_utf16(bytes: &[u8], read: fn([u8; 2]) -> u16) -> Result<String> {
    if bytes.len() % 2 != 0 {
        return Err(FastPadError::UnsupportedEncoding);
    }

    let units = bytes
        .chunks_exact(2)
        .map(|chunk| read([chunk[0], chunk[1]]));
    char::decode_utf16(units)
        .collect::<core::result::Result<String, _>>()
        .map_err(|_| FastPadError::UnsupportedEncoding)
}

fn encode_utf16(text: &str, write: fn(u16) -> [u8; 2], bom: [u8; 2]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(2 + text.len() * 2);
    bytes.extend_from_slice(&bom);
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&write(unit));
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::{Encoding, decode, encode};
    use crate::FastPadError;

    #[test]
    fn detects_all_supported_encodings() {
        assert_eq!(decode(b"hello").unwrap().encoding, Encoding::Utf8);
        assert_eq!(
            decode(b"\xEF\xBB\xBFhello").unwrap().encoding,
            Encoding::Utf8Bom
        );
        assert_eq!(
            decode(b"\xFF\xFEh\0i\0").unwrap().encoding,
            Encoding::Utf16Le
        );
        assert_eq!(
            decode(b"\xFE\xFF\0h\0i").unwrap().encoding,
            Encoding::Utf16Be
        );
    }

    #[test]
    fn every_supported_encoding_round_trips_non_ascii_text() {
        for encoding in [
            Encoding::Utf8,
            Encoding::Utf8Bom,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
        ] {
            let bytes = encode("zăpadă 🦀", encoding);
            let decoded = decode(&bytes).unwrap();
            assert_eq!(decoded.text, "zăpadă 🦀");
            assert_eq!(decoded.encoding, encoding);
        }
    }

    #[test]
    fn rejects_invalid_utf8_without_a_bom() {
        assert!(matches!(
            decode(&[0x80]),
            Err(FastPadError::UnsupportedEncoding)
        ));
    }
}
