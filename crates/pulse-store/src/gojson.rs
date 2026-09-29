//! JSON encoding with Go's `encoding/json` number formatting.
//!
//! serde_json prints every f64 with a fractional part (`0.0`, `1.0`) and
//! exponents as `1e21`; Go prints the shortest round-trip form (`0`, `1`,
//! `2.5`), switches to exponent form only below 1e-6 or from 1e21 (`1e+21`,
//! `1e-7`), and rejects NaN/Inf. Everything PULSE stores or prints as JSON
//! went through Go's encoder, so the Rust side must match byte for byte.

use serde::Serialize;
use serde_json::ser::{Formatter, PrettyFormatter};
use std::io::{self, Write};

/// Go's strconv.AppendFloat as used by encoding/json (bits = 64).
pub fn format_f64(f: f64) -> Option<String> {
    if f.is_nan() || f.is_infinite() {
        return None;
    }
    let abs = f.abs();
    if abs != 0.0 && !(1e-6..1e21).contains(&abs) {
        let s = format!("{f:e}");
        let (m, e) = s.split_once('e')?;
        return Some(if e.starts_with('-') {
            format!("{m}e{e}")
        } else {
            format!("{m}e+{e}")
        });
    }
    Some(format!("{f}"))
}

fn write_go_float<W: ?Sized + Write>(writer: &mut W, value: f64) -> io::Result<()> {
    match format_f64(value) {
        Some(s) => writer.write_all(s.as_bytes()),
        None => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("json: unsupported value: {value}"),
        )),
    }
}

/// Compact output, like json.Marshal.
#[derive(Clone, Debug, Default)]
pub struct GoCompact;

impl Formatter for GoCompact {
    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        write_go_float(writer, value as f64)
    }

    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        write_go_float(writer, value)
    }
}

/// Indented output, like json.NewEncoder + SetIndent("", "  ").
#[derive(Clone, Debug)]
pub struct GoPretty<'a>(PrettyFormatter<'a>);

impl Default for GoPretty<'_> {
    fn default() -> Self {
        GoPretty(PrettyFormatter::new())
    }
}

impl Formatter for GoPretty<'_> {
    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        write_go_float(writer, value as f64)
    }

    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        write_go_float(writer, value)
    }

    fn begin_array<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_array(writer)
    }

    fn end_array<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array(writer)
    }

    fn begin_array_value<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_array_value(writer, first)
    }

    fn end_array_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array_value(writer)
    }

    fn begin_object<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object(writer)
    }

    fn end_object<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object(writer)
    }

    fn begin_object_key<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_object_key(writer, first)
    }

    fn begin_object_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object_value(writer)
    }

    fn end_object_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object_value(writer)
    }
}

/// json.Marshal
pub fn to_vec<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(128);
    to_writer(&mut out, value)?;
    Ok(out)
}

/// json.Marshal as a String.
pub fn to_string<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<String> {
    let v = to_vec(value)?;
    // serde_json only ever writes valid UTF-8.
    Ok(unsafe { String::from_utf8_unchecked(v) })
}

/// json.MarshalIndent(v, "", "  ")
pub fn to_vec_pretty<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(128);
    to_writer_pretty(&mut out, value)?;
    Ok(out)
}

/// json.MarshalIndent(v, "", "  ") as a String.
pub fn to_string_pretty<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<String> {
    let v = to_vec_pretty(value)?;
    Ok(unsafe { String::from_utf8_unchecked(v) })
}

pub fn to_writer<W: Write, T: ?Sized + Serialize>(writer: W, value: &T) -> serde_json::Result<()> {
    let mut ser = serde_json::Serializer::with_formatter(writer, GoCompact);
    value.serialize(&mut ser)
}

pub fn to_writer_pretty<W: Write, T: ?Sized + Serialize>(
    writer: W,
    value: &T,
) -> serde_json::Result<()> {
    let mut ser = serde_json::Serializer::with_formatter(writer, GoPretty::default());
    value.serialize(&mut ser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_format_like_go() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (-0.0, "-0"),
            (1.0, "1"),
            (2.5, "2.5"),
            (0.1, "0.1"),
            (123456789.0, "123456789"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (1.5e22, "1.5e+22"),
            (1e100, "1e+100"),
            (0.000001, "0.000001"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (1e-10, "1e-10"),
            (3.0000000000000004, "3.0000000000000004"),
            (100.0 / 3.0, "33.333333333333336"),
        ];
        for (f, want) in cases {
            assert_eq!(format_f64(*f).as_deref(), Some(*want), "{f}");
        }
        assert_eq!(format_f64(f64::NAN), None);
        assert_eq!(format_f64(f64::INFINITY), None);
    }

    #[test]
    fn marshal_and_indent_match_go() {
        let v = json!({"hours": 0.0, "ratio": 0.25, "n": 3, "s": "a\"b", "list": [1.0, 2.5]});
        assert_eq!(
            to_string(&v).unwrap(),
            r#"{"hours":0,"list":[1,2.5],"n":3,"ratio":0.25,"s":"a\"b"}"#
        );
        assert_eq!(
            to_string_pretty(&v).unwrap(),
            "{\n  \"hours\": 0,\n  \"list\": [\n    1,\n    2.5\n  ],\n  \"n\": 3,\n  \"ratio\": 0.25,\n  \"s\": \"a\\\"b\"\n}"
        );
        assert_eq!(to_string_pretty(&json!([])).unwrap(), "[]");
        assert_eq!(to_string_pretty(&json!({})).unwrap(), "{}");
        assert!(
            to_string(&json!(f64::NAN)).is_err() || to_string(&json!(f64::NAN)).unwrap() == "null"
        );
    }
}
