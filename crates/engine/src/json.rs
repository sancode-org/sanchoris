//! Conversions between `serde_json` values and the JSON values of the Lean engine.

use leanfran::{Int, Nat};
use serde_json::{Map, Number, Value};

use crate::generated::sanchoris_engine::{Json, Num};

/// The Lean value of a JSON value.
pub fn to_lean(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool { b: *b },
        Value::Number(n) => Json::Num {
            n: number_to_lean(n),
        },
        Value::String(s) => Json::Str { s: s.clone() },
        Value::Array(items) => Json::Arr {
            items: items.iter().map(to_lean).collect(),
        },
        Value::Object(fields) => Json::Obj {
            fields: fields
                .iter()
                .map(|(k, v)| (k.clone(), to_lean(v)))
                .collect(),
        },
    }
}

/// The JSON value of a Lean value.
pub fn from_lean(json: &Json) -> Value {
    match json {
        Json::Null => Value::Null,
        Json::Bool { b } => Value::Bool(*b),
        Json::Num { n } => Value::Number(number_from_lean(n)),
        Json::Str { s } => Value::String(s.clone()),
        Json::Arr { items } => Value::Array(items.iter().map(from_lean).collect()),
        Json::Obj { fields } => {
            let mut map = Map::new();
            for (k, v) in fields {
                map.insert(k.clone(), from_lean(v));
            }
            Value::Object(map)
        }
    }
}

/// The number `mantissa * 10 ^ exponent` of a decimal text, with no trailing zeros in the
/// mantissa.
fn decimal(text: &str) -> (Int, i64) {
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (significand, exponent) = match text.find(['e', 'E']) {
        Some(i) => (
            &text[..i],
            text[i + 1..].parse::<i64>().expect("a JSON exponent"),
        ),
        None => (text, 0),
    };
    let (whole, fraction) = significand.split_once('.').unwrap_or((significand, ""));
    let mut digits = format!("{whole}{fraction}");
    let mut exponent = exponent - fraction.len() as i64;
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
        exponent += 1;
    }
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return (Int::from(0i64), 0);
    }
    let sign = if negative { "-" } else { "" };
    let mantissa: Int = format!("{sign}{digits}").parse().expect("decimal digits");
    (mantissa, exponent)
}

fn number_to_lean(n: &Number) -> Num {
    let (mantissa, exponent) = decimal(&n.to_string());
    Num {
        mantissa,
        exponent: Int::from(exponent),
    }
}

fn number_from_lean(n: &Num) -> Number {
    let exponent = i64::try_from(&n.exponent).expect("an exponent that fits in i64");
    let mantissa = n.mantissa.to_string();
    if exponent >= 0 {
        let text = format!("{mantissa}{}", "0".repeat(exponent as usize));
        if let Ok(i) = text.parse::<i64>() {
            return Number::from(i);
        }
        if let Ok(u) = text.parse::<u64>() {
            return Number::from(u);
        }
    }
    let f: f64 = format!("{mantissa}e{exponent}")
        .parse()
        .expect("a decimal number");
    Number::from_f64(f).unwrap_or_else(|| Number::from(0))
}

/// A `u64` as a Lean natural number.
pub fn nat(n: u64) -> Nat {
    Nat::from(n)
}

/// A Lean natural number as a `u64`.
pub fn u64_of(n: &Nat) -> u64 {
    u64::try_from(n).expect("a natural number that fits in u64")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trip() {
        let value = json!({
            "a": [1, -2, 2.5, 1e300, 0, 120, 0.001],
            "b": {"c": null, "d": true, "e": "text"},
            "big": 18446744073709551615u64
        });
        assert_eq!(from_lean(&to_lean(&value)), value);
    }

    #[test]
    fn numbers_have_no_trailing_zeros() {
        let Json::Num { n } = to_lean(&json!(1200)) else {
            panic!("a number")
        };
        assert_eq!(n.mantissa.to_string(), "12");
        assert_eq!(n.exponent.to_string(), "2");
    }
}
