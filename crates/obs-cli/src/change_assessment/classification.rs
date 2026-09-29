//! obs.py:390-404 change classes. The effective class of a prompt never drops: it is the
//! higher of the declared/planned class and the class computed from actual counts.

use crate::failure::{CliFailure, CliResult};

const ROUTINE_MAX_LINES: i64 = 15;
const ROUTINE_MAX_FILES: i64 = 1;
const MODERATE_MAX_LINES: i64 = 50;
const MODERATE_MAX_FILES: i64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeClass {
    NonCode,
    Routine,
    Moderate,
    Substantial,
    HighRisk,
}

impl ChangeClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NonCode => "non-code",
            Self::Routine => "routine",
            Self::Moderate => "moderate",
            Self::Substantial => "substantial",
            Self::HighRisk => "high-risk",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "non-code" => Some(Self::NonCode),
            "routine" => Some(Self::Routine),
            "moderate" => Some(Self::Moderate),
            "substantial" => Some(Self::Substantial),
            "high-risk" => Some(Self::HighRisk),
            _ => None,
        }
    }

    /// obs.py `CHANGE_CLASS_RANK`.
    #[inline]
    fn rank(self) -> u8 {
        self as u8
    }

    /// A class stored in the spool; an unknown value is a corrupted row (obs.py KeyError).
    pub fn parse_stored(text: &str) -> CliResult<Self> {
        Self::parse(text).ok_or_else(|| {
            CliFailure::Internal(anyhow::anyhow!(
                "unknown change class stored in the spool: {text}"
            ))
        })
    }
}

/// obs.py:393 `classify_change`.
pub fn classify_change(
    lines_changed: i64,
    files_changed: i64,
    is_high_risk: bool,
) -> CliResult<ChangeClass> {
    if lines_changed < 0 || files_changed < 0 {
        return Err(CliFailure::refusal(
            "Changed-line and file counts cannot be negative.",
        ));
    }

    let class = if is_high_risk {
        ChangeClass::HighRisk
    } else if lines_changed == 0 && files_changed == 0 {
        ChangeClass::NonCode
    } else if lines_changed <= ROUTINE_MAX_LINES && files_changed <= ROUTINE_MAX_FILES {
        ChangeClass::Routine
    } else if lines_changed <= MODERATE_MAX_LINES && files_changed <= MODERATE_MAX_FILES {
        ChangeClass::Moderate
    } else {
        ChangeClass::Substantial
    };
    Ok(class)
}

/// Python `max((first, second), key=rank)`: the first wins a tie.
#[inline]
pub fn higher_class(first: ChangeClass, second: ChangeClass) -> ChangeClass {
    if second.rank() > first.rank() {
        second
    } else {
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_lines_files_and_risk() {
        assert_eq!(classify_change(0, 0, false).unwrap(), ChangeClass::NonCode);
        assert_eq!(classify_change(15, 1, false).unwrap(), ChangeClass::Routine);
        assert_eq!(
            classify_change(16, 1, false).unwrap(),
            ChangeClass::Moderate
        );
        assert_eq!(
            classify_change(10, 4, false).unwrap(),
            ChangeClass::Substantial
        );
        assert_eq!(classify_change(1, 1, true).unwrap(), ChangeClass::HighRisk);
        assert!(classify_change(-1, 0, false).is_err());
    }

    #[test]
    fn higher_class_never_downgrades() {
        assert_eq!(
            higher_class(ChangeClass::Moderate, ChangeClass::Routine),
            ChangeClass::Moderate
        );
        assert_eq!(
            higher_class(ChangeClass::Routine, ChangeClass::HighRisk),
            ChangeClass::HighRisk
        );
    }

    #[test]
    fn stored_text_round_trips() {
        for class in [
            ChangeClass::NonCode,
            ChangeClass::Routine,
            ChangeClass::Moderate,
            ChangeClass::Substantial,
            ChangeClass::HighRisk,
        ] {
            assert_eq!(ChangeClass::parse(class.as_str()), Some(class));
        }
        assert!(ChangeClass::parse_stored("huge").is_err());
    }
}
