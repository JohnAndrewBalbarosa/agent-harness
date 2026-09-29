//! Identifier rules that Postgres enforced implicitly (`uuid` type, `bytea` lengths).

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdentifierError {
    #[error("invalid input syntax for type uuid: \"{0}\"")]
    Uuid(String),
    #[error("expected {0}-byte hex value")]
    HexLength(usize),
}

/// Accepts any textual UUID Postgres accepted and returns the lowercase hyphenated form it printed.
pub fn normalize_uuid(raw: &str) -> Result<String, IdentifierError> {
    Uuid::parse_str(raw.trim())
        .map(|parsed| parsed.hyphenated().to_string())
        .map_err(|_| IdentifierError::Uuid(raw.chars().take(64).collect()))
}

/// Mirrors `bytes(hex, expected)` in server.mjs: exactly `expected` bytes of hex, any case.
pub fn decode_hex_bytes(raw: &str, expected: usize) -> Result<Vec<u8>, IdentifierError> {
    if !is_hex_of_length(raw, expected * 2) {
        return Err(IdentifierError::HexLength(expected));
    }
    hex::decode(raw).map_err(|_| IdentifierError::HexLength(expected))
}

#[inline]
fn is_hex_of_length(raw: &str, digits: usize) -> bool {
    raw.len() == digits && raw.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_uuid() {
        assert_eq!(
            normalize_uuid("01A0D736-8ED8-76DC-9C20-C342229F9A82").unwrap(),
            "01a0d736-8ed8-76dc-9c20-c342229f9a82"
        );
    }

    #[test]
    fn rejects_bad_uuid() {
        assert!(normalize_uuid("not-a-uuid").is_err());
    }

    #[test]
    fn decodes_exact_length_hex_only() {
        assert_eq!(decode_hex_bytes("0aFF", 2).unwrap(), vec![0x0a, 0xff]);
        assert_eq!(
            decode_hex_bytes("0a", 2),
            Err(IdentifierError::HexLength(2))
        );
        assert_eq!(
            decode_hex_bytes("zz00", 2),
            Err(IdentifierError::HexLength(2))
        );
    }
}
