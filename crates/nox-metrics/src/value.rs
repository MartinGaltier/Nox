//! Owned, plain-Rust image of a CoreFoundation property-list value.
//!
//! IOKit registry properties and IOPowerSources descriptions are converted to
//! this type once in `sys::cf`, so every decoder downstream is safe code that
//! unit tests can feed with synthetic fixtures.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Data(Vec<u8>),
    Array(Vec<Value>),
    Dict(BTreeMap<String, Value>),
    /// A CF type with no plist equivalent (kept so keys are not silently lost).
    Other,
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.get(key),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Float(v) if v.is_finite() => Some(*v as i64),
            Value::Bool(b) => Some(i64::from(*b)),
            Value::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Int(v) => Some(*v != 0),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    /// `key` as an integer, looked up at the top level.
    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(Value::as_i64)
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::as_bool)
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }
}

/// Builds a dictionary fixture: `dict([("a", Value::Int(1))])`.
#[cfg(test)]
pub fn dict<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Dict(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_accessors() {
        let v = dict([
            ("n", Value::Int(-199)),
            ("f", Value::Float(1.5)),
            ("b", Value::Bool(true)),
            ("s", Value::Str("42".into())),
        ]);
        assert_eq!(v.int("n"), Some(-199));
        assert_eq!(v.int("f"), Some(1));
        assert_eq!(v.bool("b"), Some(true));
        assert_eq!(v.int("s"), Some(42));
        assert_eq!(v.int("missing"), None);
        assert_eq!(Value::Int(3).get("x"), None);
    }
}
