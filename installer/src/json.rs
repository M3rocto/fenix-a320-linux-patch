// SPDX-License-Identifier: MIT
//! JSON as the Python installer read and wrote it.
//!
//! State files are shared with Flightdeck and with earlier releases, so output
//! is byte-identical to `json.dumps`: key order kept, `", "` and `": "`
//! separators, and every character outside printable ASCII as `\uXXXX`.
use crate::error::{Error, Result, repr};
use serde::Serialize;
use serde_json::ser::{CharEscape, Formatter, Serializer};
use std::io::{self, Write};

pub use serde_json::{Map, Value, json};

/// `json.loads`.
pub fn loads(text: &str) -> Result<Value> {
    serde_json::from_str(text).map_err(|error| Error::Value(error.to_string()))
}

/// `json.dumps(value)` or, with `indent`, `json.dumps(value, indent=indent)`.
pub fn dumps(value: &Value, indent: Option<usize>) -> String {
    let mut out = Vec::new();
    let formatter = Python {
        indent,
        depth: 0,
        filled: false,
    };
    let mut serializer = Serializer::with_formatter(&mut out, formatter);
    // Writing into a vector cannot fail, and a `Value` always serialises.
    if value.serialize(&mut serializer).is_err() {
        out.clear();
    }
    String::from_utf8(out).unwrap_or_default()
}

struct Python {
    indent: Option<usize>,
    depth: usize,
    filled: bool,
}

impl Python {
    fn line<W: ?Sized + Write>(&self, writer: &mut W) -> io::Result<()> {
        if let Some(width) = self.indent {
            writer.write_all(b"\n")?;
            for _ in 0..self.depth * width {
                writer.write_all(b" ")?;
            }
        }
        Ok(())
    }

    fn open<W: ?Sized + Write>(&mut self, writer: &mut W, bracket: &[u8]) -> io::Result<()> {
        self.depth += 1;
        self.filled = false;
        writer.write_all(bracket)
    }

    fn close<W: ?Sized + Write>(&mut self, writer: &mut W, bracket: &[u8]) -> io::Result<()> {
        self.depth -= 1;
        if self.filled {
            self.line(writer)?;
        }
        writer.write_all(bracket)
    }

    fn item<W: ?Sized + Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
        if !first {
            writer.write_all(if self.indent.is_some() { b"," } else { b", " })?;
        }
        self.line(writer)
    }
}

impl Formatter for Python {
    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(float_repr(value).as_bytes())
    }

    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        writer.write_all(float_repr(f64::from(value)).as_bytes())
    }

    fn write_number_str<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        value: &str,
    ) -> io::Result<()> {
        writer.write_all(number_repr(value).as_bytes())
    }

    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        let mut units = [0u16; 2];
        for c in fragment.chars() {
            if matches!(c, ' '..='~') {
                writer.write_all(&[c as u8])?;
            } else {
                for unit in c.encode_utf16(&mut units) {
                    write!(writer, "\\u{unit:04x}")?;
                }
            }
        }
        Ok(())
    }

    fn write_char_escape<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        escape: CharEscape,
    ) -> io::Result<()> {
        match escape {
            CharEscape::Quote => writer.write_all(b"\\\""),
            CharEscape::ReverseSolidus => writer.write_all(b"\\\\"),
            CharEscape::Solidus => writer.write_all(b"/"),
            CharEscape::Backspace => writer.write_all(b"\\b"),
            CharEscape::FormFeed => writer.write_all(b"\\f"),
            CharEscape::LineFeed => writer.write_all(b"\\n"),
            CharEscape::CarriageReturn => writer.write_all(b"\\r"),
            CharEscape::Tab => writer.write_all(b"\\t"),
            CharEscape::AsciiControl(byte) => write!(writer, "\\u{byte:04x}"),
        }
    }

    fn begin_array<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.open(writer, b"[")
    }

    fn end_array<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.close(writer, b"]")
    }

    fn begin_array_value<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.item(writer, first)
    }

    fn end_array_value<W: ?Sized + Write>(&mut self, _writer: &mut W) -> io::Result<()> {
        self.filled = true;
        Ok(())
    }

    fn begin_object<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.open(writer, b"{")
    }

    fn end_object<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.close(writer, b"}")
    }

    fn begin_object_key<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.item(writer, first)
    }

    fn begin_object_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        writer.write_all(b": ")
    }

    fn end_object_value<W: ?Sized + Write>(&mut self, _writer: &mut W) -> io::Result<()> {
        self.filled = true;
        Ok(())
    }
}

/// A JSON number literal as Python prints it after reading it.
fn number_repr(literal: &str) -> String {
    if literal.contains(['.', 'e', 'E']) {
        literal
            .parse()
            .map_or_else(|_| literal.to_string(), float_repr)
    } else if literal
        .trim_start_matches('-')
        .bytes()
        .all(|byte| byte == b'0')
    {
        "0".to_string()
    } else {
        literal.to_string()
    }
}

/// `repr(float)`: the shortest digits that round-trip, exponent form outside 1e-4..1e16.
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-Infinity" } else { "Infinity" }.to_string();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let point = exponent + 1;
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if (-3..=16).contains(&point) {
        if point <= 0 {
            out.push_str("0.");
            out.extend(std::iter::repeat_n('0', point.unsigned_abs() as usize));
            out.push_str(&digits);
        } else {
            let point = point as usize;
            if digits.len() <= point {
                out.push_str(&digits);
                out.extend(std::iter::repeat_n('0', point - digits.len()));
                out.push_str(".0");
            } else {
                out.push_str(&digits[..point]);
                out.push('.');
                out.push_str(&digits[point..]);
            }
        }
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push_str(&format!(
            "e{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        ));
    }
    out
}

fn number_value(number: &serde_json::Number) -> (Option<i128>, f64) {
    let literal = number.to_string();
    let integer = if literal.contains(['.', 'e', 'E']) {
        None
    } else {
        literal.parse().ok()
    };
    (integer, literal.parse().unwrap_or(f64::NAN))
}

/// Python's `==` between two decoded JSON documents: key order is irrelevant,
/// `1 == 1.0` and `True == 1`.
pub fn equal(left: &Value, right: &Value) -> bool {
    let numeric = |value: &Value| match value {
        Value::Bool(flag) => Some((Some(i128::from(*flag)), f64::from(u8::from(*flag)))),
        Value::Number(number) => Some(number_value(number)),
        _ => None,
    };
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| equal(value, other)))
        }
        _ => match (numeric(left), numeric(right)) {
            (Some((Some(a), _)), Some((Some(b), _))) => a == b,
            (Some((_, a)), Some((_, b))) => a == b,
            _ => false,
        },
    }
}

/// `value == text`.
pub fn is_text(value: &Value, text: &str) -> bool {
    value.as_str() == Some(text)
}

/// Python truthiness of a decoded JSON value.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number_value(number).1 != 0.0,
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(items) => !items.is_empty(),
    }
}

/// Python's `str(value)` for a decoded JSON value.
pub fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => python_repr(other),
    }
}

fn python_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(number) => number_repr(&number.to_string()),
        Value::String(text) => repr(text),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(python_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|(key, value)| format!("{}: {}", repr(key), python_repr(value)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// The name Python uses for a value's type in its own error messages.
pub fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number_value(number).0.is_some() => "int",
        Value::Number(_) => "float",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// A decoded JSON object, as the dictionaries of the Python installer.
pub trait Dict {
    /// `value.get(key)`; `None` also stands for JSON `null`.
    fn item(&self, key: &str) -> Option<&Value>;
    /// `value[key]`, a `KeyError` when missing.
    fn need(&self, key: &str) -> Result<&Value>;
    /// `value[key]` where the installer then uses it as text.
    fn need_str(&self, key: &str) -> Result<&str>;
    /// `value.get(key)` as text; anything else counts as absent.
    fn str(&self, key: &str) -> Option<&str>;
    /// `value.get(key) == text`.
    fn is(&self, key: &str, text: &str) -> bool {
        self.str(key) == Some(text)
    }
    /// `value.get(key, {})` for nested tables.
    fn table(&self, key: &str) -> Result<Map<String, Value>>;
    /// `value[key]` for a nested table that must be there.
    fn need_table(&self, key: &str) -> Result<Map<String, Value>>;
    /// `value.get(key)` where a missing key and `null` are the same to the caller.
    fn field(&self, key: &str) -> &Value;
}

impl Dict for Value {
    fn item(&self, key: &str) -> Option<&Value> {
        self.as_object()
            .and_then(|items| items.get(key))
            .filter(|value| !value.is_null())
    }

    fn need(&self, key: &str) -> Result<&Value> {
        match self {
            Value::Object(items) => items.get(key).ok_or_else(|| Error::Key(key.to_string())),
            other => Err(Error::other(format!(
                "Expected a JSON object, found {}",
                type_name(other)
            ))),
        }
    }

    fn need_str(&self, key: &str) -> Result<&str> {
        let value = self.need(key)?;
        value.as_str().ok_or_else(|| {
            Error::other(format!(
                "Expected text for {}, found {}",
                repr(key),
                type_name(value)
            ))
        })
    }

    fn str(&self, key: &str) -> Option<&str> {
        self.item(key).and_then(Value::as_str)
    }

    fn table(&self, key: &str) -> Result<Map<String, Value>> {
        match self.as_object().and_then(|items| items.get(key)) {
            None => Ok(Map::new()),
            Some(Value::Object(items)) => Ok(items.clone()),
            Some(other) => Err(Error::other(format!(
                "Expected a JSON object for {}, found {}",
                repr(key),
                type_name(other)
            ))),
        }
    }

    fn need_table(&self, key: &str) -> Result<Map<String, Value>> {
        match self.need(key)? {
            Value::Object(items) => Ok(items.clone()),
            other => Err(Error::other(format!(
                "Expected a JSON object for {}, found {}",
                repr(key),
                type_name(other)
            ))),
        }
    }

    fn field(&self, key: &str) -> &Value {
        const NULL: &Value = &Value::Null;
        self.as_object()
            .and_then(|items| items.get(key))
            .unwrap_or(NULL)
    }
}

/// `{**left, **right}`: later keys replace earlier ones in place.
pub fn merged(left: &Map<String, Value>, right: &Map<String, Value>) -> Map<String, Value> {
    let mut out = left.clone();
    for (key, value) in right {
        out.insert(key.clone(), value.clone());
    }
    out
}

/// The object behind a state file. Python failed with a traceback on anything else.
pub fn object(value: Value, what: &str) -> Result<Map<String, Value>> {
    match value {
        Value::Object(items) => Ok(items),
        other => Err(Error::other(format!(
            "{what} is not a JSON object but {}",
            type_name(&other)
        ))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn output_matches_python() {
        let value = json!({"b": 1, "a": [1, 2.5, "x\u{2026}\u{7f}\n\u{1f600}"], "empty": {}, "list": [], "n": null, "t": true});
        assert_eq!(
            dumps(&value, None),
            "{\"b\": 1, \"a\": [1, 2.5, \"x\\u2026\\u007f\\n\\ud83d\\ude00\"], \"empty\": {}, \"list\": [], \"n\": null, \"t\": true}"
        );
        assert_eq!(
            dumps(&value, Some(2)),
            "{\n  \"b\": 1,\n  \"a\": [\n    1,\n    2.5,\n    \"x\\u2026\\u007f\\n\\ud83d\\ude00\"\n  ],\n  \"empty\": {},\n  \"list\": [],\n  \"n\": null,\n  \"t\": true\n}"
        );
    }

    #[test]
    fn numbers_match_python() {
        for (input, expected) in [
            ("1e16", "1e+16"),
            ("1e15", "1000000000000000.0"),
            ("0.0001", "0.0001"),
            ("0.00001", "1e-05"),
            ("1.50", "1.5"),
            ("-0", "0"),
            ("-0.0", "-0.0"),
            (
                "123456789012345678901234567890",
                "123456789012345678901234567890",
            ),
            ("1E2", "100.0"),
            ("2.5e-7", "2.5e-07"),
            ("1.7976931348623157e308", "1.7976931348623157e+308"),
            ("18446744073709551615", "18446744073709551615"),
        ] {
            assert_eq!(dumps(&loads(input).unwrap(), None), expected, "{input}");
        }
    }

    #[test]
    fn comparison_and_access_follow_python() {
        assert!(equal(
            &loads(r#"{"a": 1, "b": [true, 2.0]}"#).unwrap(),
            &loads(r#"{"b": [1, 2], "a": 1.0}"#).unwrap()
        ));
        assert!(!equal(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
        assert!(!equal(&json!("1"), &json!(1)));
        let state = json!({"state": "installed", "configured": null});
        assert!(state.is("state", "installed"));
        assert!(state.item("configured").is_none());
        assert_eq!(state.need("missing").unwrap_err().to_string(), "'missing'");
        assert_eq!(
            text(&json!([1, "a", null, {"k": false}])),
            "[1, 'a', None, {'k': False}]"
        );
        let merged = merged(
            json!({"a": 1, "b": 2}).as_object().unwrap(),
            json!({"b": 3, "c": 4}).as_object().unwrap(),
        );
        assert_eq!(
            dumps(&Value::Object(merged), None),
            r#"{"a": 1, "b": 3, "c": 4}"#
        );
    }
}
