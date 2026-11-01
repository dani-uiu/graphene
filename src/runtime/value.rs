//! Runtime values and their JSON forms.
//!
//! Depends on: `ir::graph` (for `Type`), `rust_decimal`, `serde_json`.
//!
//! Invariants:
//! - `dec` values are exact; they travel through JSON as strings, never
//!   floats;
//! - `to_json` is exact (effect log); `to_output_json` rescales every
//!   `dec` to 2 places, rounding half away from zero (D-002);
//! - `from_json` is driven by the declared type, so a host or fixture can
//!   never smuggle in a value of the wrong shape.

use std::collections::BTreeMap;

use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{json, Map, Value as Json};

use crate::ir::graph::{Scale, Type};

pub type Record = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    U64(u64),
    I64(i64),
    Dec(Decimal),
    Bool(bool),
    Str(String),
    Record(Record),
    /// v0.3 list.
    List(Vec<Value>),
    /// The variant held by an error node.
    Err(String),
    /// Marker held by the root and by sinks; carries no data.
    Unit,
}

/// Output scale for `dec` (D-002).
pub const OUTPUT_SCALE: u32 = 2;

pub fn rescale(d: Decimal) -> Decimal {
    rescale_to(d, OUTPUT_SCALE)
}

/// Round half away from zero to `places` and pad to exactly that scale.
pub fn rescale_to(d: Decimal, places: u32) -> Decimal {
    let mut r = d.round_dp_with_strategy(places, RoundingStrategy::MidpointAwayFromZero);
    r.rescale(places);
    r
}

impl Value {
    /// Exact JSON form, used in the effect log.
    pub fn to_json(&self) -> Json {
        self.json_with(&|d| d)
    }

    /// JSON form for the program result: `dec` rescaled to 2 places.
    pub fn to_output_json(&self) -> Json {
        self.json_with(&rescale)
    }

    /// JSON form for the program result with per-field output scales
    /// from `dec(N)` (D-052); unspecified places default to 2.
    pub fn to_output_json_scaled(&self, scale: &Scale) -> Json {
        match (self, scale) {
            (Value::Dec(d), Scale::Places(n)) => json!(rescale_to(*d, *n).to_string()),
            (Value::Record(r), Scale::Record(fields)) => {
                let m: Map<String, Json> = r
                    .iter()
                    .map(|(k, v)| {
                        let s = fields.get(k).cloned().unwrap_or_default();
                        (k.clone(), v.to_output_json_scaled(&s))
                    })
                    .collect();
                Json::Object(m)
            }
            (Value::List(items), Scale::List(inner)) => Json::Array(
                items
                    .iter()
                    .map(|v| v.to_output_json_scaled(inner))
                    .collect(),
            ),
            _ => self.to_output_json(),
        }
    }

    fn json_with(&self, dec: &dyn Fn(Decimal) -> Decimal) -> Json {
        match self {
            Value::U64(n) => json!(n),
            Value::I64(n) => json!(n),
            Value::Dec(d) => json!(dec(*d).to_string()),
            Value::Bool(b) => json!(b),
            Value::Str(s) => json!(s),
            Value::Err(e) => json!({ "err": e }),
            Value::Unit => Json::Null,
            Value::Record(r) => {
                let m: Map<String, Json> = r
                    .iter()
                    .map(|(k, v)| (k.clone(), v.json_with(dec)))
                    .collect();
                Json::Object(m)
            }
            Value::List(items) => Json::Array(items.iter().map(|v| v.json_with(dec)).collect()),
        }
    }

    /// Decode JSON as a value of type `ty`.
    pub fn from_json(j: &Json, ty: &Type) -> Result<Value, String> {
        let bad = || format!("expected {ty}, found {j}");
        match ty {
            Type::U64 => j.as_u64().map(Value::U64).ok_or_else(bad),
            Type::I64 => j.as_i64().map(Value::I64).ok_or_else(bad),
            Type::Bool => j.as_bool().map(Value::Bool).ok_or_else(bad),
            Type::Str => j.as_str().map(|s| Value::Str(s.into())).ok_or_else(bad),
            Type::Dec => {
                let text = match j {
                    Json::String(s) => s.clone(),
                    Json::Number(n) => n.to_string(),
                    _ => return Err(bad()),
                };
                Decimal::from_str_exact(&text)
                    .map(Value::Dec)
                    .map_err(|_| bad())
            }
            Type::Record(fields) => {
                let obj = j.as_object().ok_or_else(bad)?;
                if obj.len() != fields.len() || obj.keys().any(|k| !fields.contains_key(k)) {
                    return Err(bad());
                }
                let mut out = Record::new();
                for (k, t) in fields {
                    let v = obj.get(k).ok_or_else(bad)?;
                    out.insert(k.clone(), Value::from_json(v, t)?);
                }
                Ok(Value::Record(out))
            }
            Type::List(inner) => {
                let items = j.as_array().ok_or_else(bad)?;
                items
                    .iter()
                    .map(|x| Value::from_json(x, inner))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::List)
            }
            Type::Err(_) => Err(bad()),
        }
    }

    /// Parse a command-line argument (`--arg name=text`) as type `ty`.
    pub fn parse_arg(text: &str, ty: &Type) -> Result<Value, String> {
        let bad = |_| format!("`{text}` is not a valid {ty}");
        match ty {
            Type::U64 => text.parse().map(Value::U64).map_err(|_| bad(())),
            Type::I64 => text.parse().map(Value::I64).map_err(|_| bad(())),
            Type::Bool => text.parse().map(Value::Bool).map_err(|_| bad(())),
            Type::Str => Ok(Value::Str(text.to_string())),
            Type::Dec => Decimal::from_str_exact(text)
                .map(Value::Dec)
                .map_err(|_| bad(())),
            // Lists are written as JSON on the command line (D-044).
            Type::List(_) => {
                let j: Json = serde_json::from_str(text)
                    .map_err(|_| format!("`{text}` is not valid JSON for {ty}"))?;
                Value::from_json(&j, ty)
            }
            Type::Record(_) | Type::Err(_) => Err(format!("inputs of type {ty} are not supported")),
        }
    }

    /// True if `j` denotes this value (used to match mock fixture args).
    /// Decimals compare numerically, so `"1.0"` matches `1.00`.
    pub fn matches_json(&self, j: &Json) -> bool {
        match self {
            Value::U64(n) => j.as_u64() == Some(*n),
            Value::I64(n) => j.as_i64() == Some(*n),
            Value::Bool(b) => j.as_bool() == Some(*b),
            Value::Str(s) => j.as_str() == Some(s.as_str()),
            Value::Dec(d) => match j {
                Json::String(s) => Decimal::from_str_exact(s).is_ok_and(|x| x == *d),
                Json::Number(n) => Decimal::from_str_exact(&n.to_string()).is_ok_and(|x| x == *d),
                _ => false,
            },
            Value::Record(r) => j.as_object().is_some_and(|o| {
                o.len() == r.len()
                    && r.iter()
                        .all(|(k, v)| o.get(k).is_some_and(|x| v.matches_json(x)))
            }),
            Value::List(items) => j.as_array().is_some_and(|a| {
                a.len() == items.len() && items.iter().zip(a).all(|(v, x)| v.matches_json(x))
            }),
            Value::Err(_) | Value::Unit => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str_exact(s).expect("test literal")
    }

    #[test]
    fn rescale_pads_and_rounds_half_away_from_zero() {
        assert_eq!(rescale(dec("80.0000")).to_string(), "80.00");
        assert_eq!(rescale(dec("1260")).to_string(), "1260.00");
        assert_eq!(rescale(dec("0.125")).to_string(), "0.13");
        assert_eq!(rescale(dec("-0.125")).to_string(), "-0.13");
        assert_eq!(rescale(dec("2.345")).to_string(), "2.35");
    }

    #[test]
    fn json_roundtrip_by_type() {
        let ty = Type::Record(BTreeMap::from([
            ("bal".to_string(), Type::Dec),
            ("role".to_string(), Type::Str),
        ]));
        let j = json!({"role": "prem", "bal": "100.00"});
        let v = Value::from_json(&j, &ty).expect("valid");
        assert_eq!(v.to_json(), j);
        assert!(Value::from_json(&json!({"role": "x"}), &ty).is_err());
        assert!(Value::from_json(&json!(1.5), &Type::U64).is_err());
    }

    #[test]
    fn decimals_match_numerically() {
        assert!(Value::Dec(dec("1.00")).matches_json(&json!("1.0")));
        assert!(!Value::Dec(dec("1.00")).matches_json(&json!("1.01")));
    }
}
