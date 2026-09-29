//! `adapters.base.count`: a non-negative token count from an int or numeric string (`int(value)`,
//! anything unparseable or negative is 0).

use obs_cli::python_compat::parse_python_int;
use serde_json::{Number, Value};

pub fn count(value: Option<&Value>) -> u64 {
    let parsed = match value {
        Some(Value::Number(number)) => python_int_of_number(number),
        Some(Value::Bool(flag)) => Some(i128::from(*flag)),
        Some(Value::String(text)) => parse_python_int(text).map(i128::from),
        _ => None,
    };
    parsed
        .filter(|number| *number >= 0)
        .map(|number| u64::try_from(number).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// `int(number)`: integers as-is, floats truncated toward zero, NaN/inf rejected.
#[inline]
fn python_int_of_number(number: &Number) -> Option<i128> {
    if let Some(integer) = number.as_i64() {
        return Some(i128::from(integer));
    }
    if let Some(unsigned) = number.as_u64() {
        return Some(i128::from(unsigned));
    }
    number
        .as_f64()
        .filter(|float| float.is_finite())
        .map(|float| float.trunc() as i128)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mirrors_python_int_with_non_negative_floor() {
        assert_eq!(count(Some(&json!(12))), 12);
        assert_eq!(count(Some(&json!("34"))), 34);
        assert_eq!(count(Some(&json!(" 5 "))), 5);
        assert_eq!(count(Some(&json!(7.9))), 7);
        assert_eq!(count(Some(&json!(true))), 1);
        assert_eq!(count(Some(&json!(-3))), 0);
        assert_eq!(count(Some(&json!("1.5"))), 0);
        assert_eq!(count(Some(&json!(null))), 0);
        assert_eq!(count(None), 0);
    }
}
