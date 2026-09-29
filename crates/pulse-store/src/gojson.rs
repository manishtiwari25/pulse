//! JSON encoding with Go's `encoding/json` number and string formatting.
//!
//! serde_json prints every f64 with a fractional part (`0.0`, `1.0`) and
//! exponents as `1e21`; Go prints the shortest round-trip form (`0`, `1`,
//! `2.5`), switches to exponent form only below 1e-6 or from 1e21 (`1e+21`,
//! `1e-7`), and rejects NaN/Inf. Go also escapes `<`, `>` and `&` as
//! `\u003c`, `\u003e`, `\u0026` (json.Marshal; an Encoder with
//! `SetEscapeHTML(false)` does not), always escapes U+2028/U+2029, and writes
//! backspace and form feed as `\u0008`/`\u000c` rather than `\b`/`\f`.
//! Everything PULSE stores or prints as JSON went through Go's encoder, so the
//! Rust side must match byte for byte.

use serde::Serialize;
use serde_json::ser::{CharEscape, Formatter, PrettyFormatter};
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

/// Go's encodeState.string for the bytes serde_json leaves unescaped:
/// `<`, `>`, `&` when escapeHTML, and U+2028/U+2029 always.
fn write_go_fragment<W: ?Sized + Write>(
    writer: &mut W,
    fragment: &str,
    escape_html: bool,
) -> io::Result<()> {
    let bytes = fragment.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let esc: Option<&str> = match b {
            b'<' if escape_html => Some("\\u003c"),
            b'>' if escape_html => Some("\\u003e"),
            b'&' if escape_html => Some("\\u0026"),
            // U+2028 = E2 80 A8, U+2029 = E2 80 A9
            0xE2 if i + 2 < bytes.len() && bytes[i + 1] == 0x80 && bytes[i + 2] == 0xA8 => {
                Some("\\u2028")
            }
            0xE2 if i + 2 < bytes.len() && bytes[i + 1] == 0x80 && bytes[i + 2] == 0xA9 => {
                Some("\\u2029")
            }
            _ => None,
        };
        let Some(esc) = esc else {
            i += 1;
            continue;
        };
        writer.write_all(&bytes[start..i])?;
        writer.write_all(esc.as_bytes())?;
        i += if b == 0xE2 { 3 } else { 1 };
        start = i;
    }
    writer.write_all(&bytes[start..])
}

/// Go writes `\u0008` and `\u000c` where serde_json writes `\b` and `\f`.
fn write_go_char_escape<W: ?Sized + Write>(
    writer: &mut W,
    char_escape: CharEscape,
) -> io::Result<()> {
    match char_escape {
        CharEscape::Backspace => writer.write_all(b"\\u0008"),
        CharEscape::FormFeed => writer.write_all(b"\\u000c"),
        other => {
            let mut f = serde_json::ser::CompactFormatter;
            f.write_char_escape(writer, other)
        }
    }
}

/// Compact output, like json.Marshal (`escape_html: true`) or
/// json.NewEncoder + SetEscapeHTML(false) (`escape_html: false`).
#[derive(Clone, Debug)]
pub struct GoCompact {
    pub escape_html: bool,
}

impl Default for GoCompact {
    fn default() -> Self {
        GoCompact { escape_html: true }
    }
}

impl Formatter for GoCompact {
    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        write_go_float(writer, value as f64)
    }

    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        write_go_float(writer, value)
    }

    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        write_go_fragment(writer, fragment, self.escape_html)
    }

    fn write_char_escape<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        char_escape: CharEscape,
    ) -> io::Result<()> {
        write_go_char_escape(writer, char_escape)
    }
}

/// Indented output, like json.MarshalIndent(v, "", "  ") (`escape_html:
/// true`) or json.NewEncoder + SetIndent("", "  ") + SetEscapeHTML(false)
/// (`escape_html: false`).
#[derive(Clone, Debug)]
pub struct GoPretty<'a> {
    inner: PrettyFormatter<'a>,
    pub escape_html: bool,
}

impl Default for GoPretty<'_> {
    fn default() -> Self {
        GoPretty {
            inner: PrettyFormatter::new(),
            escape_html: true,
        }
    }
}

impl GoPretty<'_> {
    pub fn new(escape_html: bool) -> Self {
        GoPretty {
            inner: PrettyFormatter::new(),
            escape_html,
        }
    }
}

impl Formatter for GoPretty<'_> {
    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        write_go_float(writer, value as f64)
    }

    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        write_go_float(writer, value)
    }

    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        write_go_fragment(writer, fragment, self.escape_html)
    }

    fn write_char_escape<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        char_escape: CharEscape,
    ) -> io::Result<()> {
        write_go_char_escape(writer, char_escape)
    }

    fn begin_array<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.inner.begin_array(writer)
    }

    fn end_array<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.inner.end_array(writer)
    }

    fn begin_array_value<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.inner.begin_array_value(writer, first)
    }

    fn end_array_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.inner.end_array_value(writer)
    }

    fn begin_object<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.inner.begin_object(writer)
    }

    fn end_object<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.inner.end_object(writer)
    }

    fn begin_object_key<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.inner.begin_object_key(writer, first)
    }

    fn begin_object_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.inner.begin_object_value(writer)
    }

    fn end_object_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.inner.end_object_value(writer)
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

/// json.NewEncoder(w).Encode(v) minus the trailing newline (escapes HTML).
pub fn to_writer<W: Write, T: ?Sized + Serialize>(writer: W, value: &T) -> serde_json::Result<()> {
    let mut ser = serde_json::Serializer::with_formatter(writer, GoCompact::default());
    value.serialize(&mut ser)
}

/// json.MarshalIndent(v, "", "  ") to a writer (escapes HTML).
pub fn to_writer_pretty<W: Write, T: ?Sized + Serialize>(
    writer: W,
    value: &T,
) -> serde_json::Result<()> {
    let mut ser = serde_json::Serializer::with_formatter(writer, GoPretty::default());
    value.serialize(&mut ser)
}

/// json.NewEncoder + SetEscapeHTML(false), compact, no trailing newline.
pub fn to_vec_no_html<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(128);
    let mut ser =
        serde_json::Serializer::with_formatter(&mut out, GoCompact { escape_html: false });
    value.serialize(&mut ser)?;
    Ok(out)
}

/// json.NewEncoder + SetEscapeHTML(false), compact, as a String.
pub fn to_string_no_html<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<String> {
    let v = to_vec_no_html(value)?;
    Ok(unsafe { String::from_utf8_unchecked(v) })
}

/// json.NewEncoder + SetIndent("", "  ") + SetEscapeHTML(false), no trailing
/// newline.
pub fn to_writer_pretty_no_html<W: Write, T: ?Sized + Serialize>(
    writer: W,
    value: &T,
) -> serde_json::Result<()> {
    let mut ser = serde_json::Serializer::with_formatter(writer, GoPretty::new(false));
    value.serialize(&mut ser)
}

/// json.NewEncoder + SetIndent("", "  ") + SetEscapeHTML(false) as bytes.
pub fn to_vec_pretty_no_html<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(128);
    to_writer_pretty_no_html(&mut out, value)?;
    Ok(out)
}

/// json.NewEncoder + SetIndent("", "  ") + SetEscapeHTML(false) as a String.
pub fn to_string_pretty_no_html<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<String> {
    let v = to_vec_pretty_no_html(value)?;
    Ok(unsafe { String::from_utf8_unchecked(v) })
}

/// Go `map[string]any{...}` whose values may be structs: keys sort bytewise
/// like Go's map encoding while each value keeps its own field order (a
/// `serde_json::json!` literal would convert struct values into sorted-key
/// maps). Build one with [`gomap!`](crate::gomap).
pub struct GoMap<'a>(pub Vec<(&'a str, &'a dyn erased_serde::Serialize)>);

impl Serialize for GoMap<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut entries: Vec<&(&str, &dyn erased_serde::Serialize)> = self.0.iter().collect();
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let mut m = s.serialize_map(Some(entries.len()))?;
        for (k, v) in entries {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// `gomap!{"project": slug, "goal": goal}` = Go `map[string]any{"project":
/// slug, "goal": goal}`: sorted keys, struct values in declaration order.
/// Values are borrowed for the duration of the enclosing statement, so use it
/// inline (`write_json(out, &gomap!{...})`) or bind the values first.
#[macro_export]
macro_rules! gomap {
    ($($k:literal : $v:expr),* $(,)?) => {
        $crate::gojson::GoMap(vec![$(($k, &$v as &dyn $crate::erased_serde::Serialize)),*])
    };
}

/// A Go slice whose nil-ness reaches JSON: `var out []T` (nil, encodes as
/// `null`) versus `[]T{}` / `make([]T, 0)` (empty, encodes as `[]`). Use it
/// for fields that can carry either on some code path; fields that are nil
/// whenever empty can stay `Vec<T>` with [`nil_slice`]. It derefs to
/// `Vec<T>`, so reads and pushes look like any vector; only an *empty* nil
/// slice encodes as `null`, exactly like Go.
#[derive(Clone, Debug)]
pub struct GoSlice<T> {
    vec: Vec<T>,
    nil: bool,
}

impl<T> GoSlice<T> {
    /// `var out []T`.
    pub const fn nil() -> Self {
        Self {
            vec: Vec::new(),
            nil: true,
        }
    }

    /// `[]T{}` / `make([]T, 0)`.
    pub const fn empty() -> Self {
        Self {
            vec: Vec::new(),
            nil: false,
        }
    }

    /// The result of `append`-building onto `var out []T`: nil when nothing
    /// was appended.
    pub fn appended(vec: Vec<T>) -> Self {
        Self { vec, nil: true }
    }

    pub fn is_nil(&self) -> bool {
        self.nil && self.vec.is_empty()
    }

    pub fn into_vec(self) -> Vec<T> {
        self.vec
    }
}

impl<T> Default for GoSlice<T> {
    fn default() -> Self {
        Self::nil()
    }
}

impl<T> From<Vec<T>> for GoSlice<T> {
    fn from(vec: Vec<T>) -> Self {
        Self { vec, nil: false }
    }
}

impl<T> From<GoSlice<T>> for Vec<T> {
    fn from(s: GoSlice<T>) -> Self {
        s.vec
    }
}

impl<T> FromIterator<T> for GoSlice<T> {
    /// Collecting mirrors an `append` loop over `var out []T`.
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::appended(iter.into_iter().collect())
    }
}

impl<T> std::ops::Deref for GoSlice<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Vec<T> {
        &self.vec
    }
}

impl<T> std::ops::DerefMut for GoSlice<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.vec
    }
}

impl<T> AsRef<[T]> for GoSlice<T> {
    fn as_ref(&self) -> &[T] {
        &self.vec
    }
}

impl<T: PartialEq> PartialEq for GoSlice<T> {
    fn eq(&self, other: &Self) -> bool {
        self.vec == other.vec
    }
}

impl<T: PartialEq> PartialEq<Vec<T>> for GoSlice<T> {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.vec == *other
    }
}

impl<T: PartialEq> PartialEq<[T]> for GoSlice<T> {
    fn eq(&self, other: &[T]) -> bool {
        self.vec == other
    }
}

impl<T> IntoIterator for GoSlice<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.vec.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a GoSlice<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.vec.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut GoSlice<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.vec.iter_mut()
    }
}

impl<T: Serialize> Serialize for GoSlice<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.is_nil() {
            s.serialize_none()
        } else {
            self.vec.serialize(s)
        }
    }
}

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for GoSlice<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match Option::<Vec<T>>::deserialize(d)? {
            Some(vec) => Self::from(vec),
            None => Self::nil(),
        })
    }
}

/// Go slices that are only ever `append`ed to (or scanned from rows) stay
/// nil when empty and marshal as `null`, never `[]`. Mark such fields with
/// `#[serde(with = "pulse_store::gojson::nil_slice")]`: an empty Vec writes
/// `null`, and `null` (as Go wrote it) reads back as an empty Vec.
pub mod nil_slice {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer>(v: &[T], s: S) -> Result<S::Ok, S::Error> {
        if v.is_empty() {
            s.serialize_none()
        } else {
            v.serialize(s)
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Vec<T>, D::Error> {
        Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gomap_sorts_keys_but_keeps_struct_field_order() {
        let _serial = crate::test_helpers::serial();
        #[derive(Serialize)]
        struct Goal {
            text: &'static str,
            source: &'static str,
        }
        let goal = Goal {
            text: "t",
            source: "s",
        };
        let items = vec![1, 2];
        let out = to_string(
            &crate::gomap! {"project": "demo", "goal": goal, "items": items, "agent": "a"},
        )
        .unwrap();
        assert_eq!(
            out,
            r#"{"agent":"a","goal":{"text":"t","source":"s"},"items":[1,2],"project":"demo"}"#
        );
        let pretty =
            to_string_pretty(&crate::gomap! {"b": 1, "a": Goal { text: "x", source: "y" }})
                .unwrap();
        assert_eq!(
            pretty,
            "{\n  \"a\": {\n    \"text\": \"x\",\n    \"source\": \"y\"\n  },\n  \"b\": 1\n}"
        );
    }

    #[test]
    fn go_slice_encodes_nil_and_empty_like_go() {
        let _serial = crate::test_helpers::serial();
        let mut nil: GoSlice<i32> = GoSlice::nil();
        assert_eq!(to_string(&nil).unwrap(), "null");
        assert_eq!(to_string(&GoSlice::<i32>::empty()).unwrap(), "[]");
        assert_eq!(to_string(&GoSlice::from(vec![1])).unwrap(), "[1]");
        assert_eq!(
            to_string(&GoSlice::<i32>::appended(Vec::new())).unwrap(),
            "null"
        );
        assert_eq!(
            to_string(&Vec::<i32>::new().into_iter().collect::<GoSlice<i32>>()).unwrap(),
            "null"
        );
        nil.push(2);
        assert_eq!(to_string(&nil).unwrap(), "[2]");
        assert!(nil == vec![2]);
        assert_eq!(nil.len(), 1);
        let back: GoSlice<i32> = serde_json::from_str("null").unwrap();
        assert!(back.is_nil());
        let back: GoSlice<i32> = serde_json::from_str("[]").unwrap();
        assert!(!back.is_nil() && back.is_empty());
    }

    #[test]
    fn nil_slice_round_trips_null() {
        let _serial = crate::test_helpers::serial();
        #[derive(Serialize, serde::Deserialize, PartialEq, Debug)]
        struct R {
            #[serde(with = "nil_slice")]
            items: Vec<i32>,
            #[serde(with = "nil_slice", default)]
            more: Vec<String>,
        }
        let r = R {
            items: vec![],
            more: vec!["a".into()],
        };
        assert_eq!(to_string(&r).unwrap(), r#"{"items":null,"more":["a"]}"#);
        let back: R = serde_json::from_str(r#"{"items":null,"more":null}"#).unwrap();
        assert_eq!(
            back,
            R {
                items: vec![],
                more: vec![]
            }
        );
        let back: R = serde_json::from_str(r#"{"items":[1],"more":["a"]}"#).unwrap();
        assert_eq!(
            back,
            R {
                items: vec![1],
                more: vec!["a".into()]
            }
        );
    }

    #[test]
    fn floats_format_like_go() {
        let _serial = crate::test_helpers::serial();
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
    fn strings_escape_like_go() {
        let _serial = crate::test_helpers::serial();
        let v = json!({"s": "<a href=\"x\">&amp;</a>\u{2028}\u{8}\u{c}\n\ttail"});
        assert_eq!(
            to_string(&v).unwrap(),
            r#"{"s":"\u003ca href=\"x\"\u003e\u0026amp;\u003c/a\u003e\u2028\u0008\u000c\n\ttail"}"#
        );
        assert_eq!(
            to_string_no_html(&v).unwrap(),
            r#"{"s":"<a href=\"x\">&amp;</a>\u2028\u0008\u000c\n\ttail"}"#
        );
        assert_eq!(
            to_string_pretty_no_html(&json!({"k": "<>&"})).unwrap(),
            "{\n  \"k\": \"<>&\"\n}"
        );
        assert_eq!(
            to_string_pretty(&json!({"k": "<>&"})).unwrap(),
            "{\n  \"k\": \"\\u003c\\u003e\\u0026\"\n}"
        );
    }

    #[test]
    fn marshal_and_indent_match_go() {
        let _serial = crate::test_helpers::serial();
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

/// Go: fmt.Sprint(v) for a value decoded from JSON into `any` (nil, bool,
/// float64, string, []any, map[string]any): `<nil>`, `true`, `%g`-shortest
/// floats, the bare string, `[a b]` and `map[k:v]` with sorted keys.
pub fn sprint(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Null => "<nil>".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => sprint_float(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => s.clone(),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(sprint).collect::<Vec<_>>().join(" ")
        ),
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            format!(
                "map[{}]",
                keys.iter()
                    .map(|k| format!("{k}:{}", sprint(&fields[*k])))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        }
    }
}

/// Go: fmt.Sprint(f) for a float64 - %v is %g with the shortest digits, in
/// exponent form when the decimal exponent is below -4 or at least 6
/// (strconv's eprec for the shortest precision), e.g. 1e+06 but 100000.
fn sprint_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "+Inf" } else { "-Inf" }.to_string();
    }
    let s = format!("{f:e}");
    let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = e.parse().unwrap_or(0);
    if f != 0.0 && !(-4..6).contains(&exp) {
        let (sign, digits) = match e.strip_prefix('-') {
            Some(d) => ("-", d),
            None => ("+", e),
        };
        return format!("{m}e{sign}{digits:0>2}");
    }
    format!("{f}")
}

#[cfg(test)]
mod sprint_tests {
    use super::sprint;
    use serde_json::json;

    #[test]
    fn sprint_matches_go_fmt() {
        assert_eq!(sprint(&json!(null)), "<nil>");
        assert_eq!(sprint(&json!(true)), "true");
        assert_eq!(sprint(&json!(3.0)), "3");
        assert_eq!(sprint(&json!(1.5)), "1.5");
        assert_eq!(sprint(&json!(1e21)), "1e+21");
        assert_eq!(sprint(&json!(1e20)), "1e+20");
        assert_eq!(sprint(&json!(1000000.0)), "1e+06");
        assert_eq!(sprint(&json!(100000.0)), "100000");
        assert_eq!(sprint(&json!(0.00001)), "1e-05");
        assert_eq!(sprint(&json!(0.0001)), "0.0001");
        assert_eq!(sprint(&json!(123456789.0)), "1.23456789e+08");
        assert_eq!(sprint(&json!(-2.5)), "-2.5");
        assert_eq!(sprint(&json!(0.0)), "0");
        assert_eq!(sprint(&json!("s")), "s");
        assert_eq!(sprint(&json!(["a", 1.0, null])), "[a 1 <nil>]");
        assert_eq!(
            sprint(&json!({"b": 1, "a": "x", "c": ["a", 2.0, null], "d": {"z": true}})),
            "map[a:x b:1 c:[a 2 <nil>] d:map[z:true]]"
        );
    }
}
