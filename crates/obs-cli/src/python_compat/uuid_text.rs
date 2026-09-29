//! obs.py identifier text: `uuid7()`, `uuid.uuid5(NAMESPACE_URL, name)`, and
//! `str(uuid.UUID(raw))` normalization of user-supplied ids.

use uuid::Uuid;

const PYTHON_UUID_HEX_DIGITS: usize = 32;

/// obs.py:48 `uuid7()`.
pub fn uuid7_text() -> String {
    Uuid::now_v7().hyphenated().to_string()
}

/// `str(uuid.uuid5(uuid.NAMESPACE_URL, name))`.
pub fn uuid5_url_text(name: &str) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes())
        .hyphenated()
        .to_string()
}

/// `str(uuid.UUID(raw))`: strips `urn:`/`uuid:`, surrounding braces, and every hyphen, then
/// requires exactly 32 hex digits. Returns the lowercase hyphenated form.
pub fn python_uuid_text(raw: &str) -> Option<String> {
    let without_prefixes = raw.replace("urn:", "").replace("uuid:", "");
    let digits: String = without_prefixes
        .trim_matches(|character| character == '{' || character == '}')
        .chars()
        .filter(|character| *character != '-')
        .collect();

    if !is_uuid_hex(&digits) {
        return None;
    }
    Uuid::parse_str(&digits)
        .ok()
        .map(|parsed| parsed.hyphenated().to_string())
}

#[inline]
fn is_uuid_hex(digits: &str) -> bool {
    digits.len() == PYTHON_UUID_HEX_DIGITS && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid5_matches_python_namespace_url() {
        // python -c "import uuid;print(uuid.uuid5(uuid.NAMESPACE_URL,'a|b|c'))"
        assert_eq!(
            uuid5_url_text("a|b|c"),
            "9d8a4c30-e985-57b6-9e24-40213e123949"
        );
    }

    #[test]
    fn normalizes_python_accepted_forms() {
        let expected = Some("01a0d736-8ed8-76dc-9c20-c342229f9a82".to_owned());
        assert_eq!(
            python_uuid_text("01A0D736-8ED8-76DC-9C20-C342229F9A82"),
            expected
        );
        assert_eq!(
            python_uuid_text("{01a0d7368ed876dc9c20c342229f9a82}"),
            expected
        );
        assert_eq!(
            python_uuid_text("urn:uuid:01a0d736-8ed8-76dc-9c20-c342229f9a82"),
            expected
        );
        assert_eq!(python_uuid_text("p1"), None);
    }

    #[test]
    fn uuid7_has_version_seven() {
        assert_eq!(Uuid::parse_str(&uuid7_text()).unwrap().get_version_num(), 7);
    }
}
