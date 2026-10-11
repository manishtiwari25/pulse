//! Single-dash command-line flags: a `FlagSet` whose `parse` stops at the
//! first non-flag argument, fixed error texts (`flag provided but not
//! defined: -x`, `flag needs an argument: -x`, `invalid value "v" for flag
//! -x: parse error`), the `print_defaults` layout, and a `Usage of NAME:`
//! message on stderr unless the output is discarded or captured. Command
//! help and error output are built from this text, so it stays as it is.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write};
use std::time::Duration;

/// Why `parse` stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagError {
    /// `-h`/`-help` was seen and no such flag is defined; the usage has
    /// already been printed.
    Help,
    Message(String),
}

impl fmt::Display for FlagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Help => write!(f, "flag: help requested"),
            Self::Message(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for FlagError {}

/// Where a `FlagSet` prints parse errors and its usage: stderr by default,
/// `Discard` silences it, `Capture` collects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Output {
    #[default]
    Stderr,
    Discard,
    /// Collect what `parse` prints and let the caller forward it with
    /// [`FlagSet::take_output`].
    Capture,
}

struct CaptureWriter<'a>(&'a std::cell::RefCell<Vec<u8>>);

impl Write for CaptureWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const ERR_PARSE: &str = "parse error";
const ERR_RANGE: &str = "value out of range";

#[derive(Debug, Clone, PartialEq)]
pub enum FlagValue {
    String(String),
    Bool(bool),
    Int(i32),
    I64(i64),
    F64(f64),
    Duration(Duration),
}

impl FlagValue {
    /// The type name shown in usage when the flag's usage text has no
    /// back-quoted value name (`""` for booleans, which take no argument).
    fn type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "string",
            Self::Int(_) => "int",
            Self::I64(_) => "int",
            Self::F64(_) => "float",
            Self::Duration(_) => "duration",
            Self::Bool(_) => "",
        }
    }

    /// The value as `print_defaults` shows a default.
    fn default_text(&self) -> String {
        match self {
            Self::String(v) => v.clone(),
            Self::Bool(v) => v.to_string(),
            Self::Int(v) => v.to_string(),
            Self::I64(v) => v.to_string(),
            Self::F64(v) => format_float_g(*v),
            Self::Duration(v) => format_duration(*v),
        }
    }

    /// Parses `value` as this value's type; the error is `parse error` or
    /// `value out of range`.
    fn set_from(&self, value: &str) -> Result<FlagValue, &'static str> {
        Ok(match self {
            Self::String(_) => Self::String(value.to_string()),
            Self::Bool(_) => Self::Bool(parse_bool(value).map_err(|()| ERR_PARSE)?),
            Self::Int(_) => {
                let v = parse_go_int(value, 64)?;
                Self::Int(i32::try_from(v).map_err(|_| ERR_RANGE)?)
            }
            Self::I64(_) => Self::I64(parse_go_int(value, 64)?),
            Self::F64(_) => Self::F64(parse_go_float(value)?),
            Self::Duration(_) => Self::Duration(parse_duration(value).map_err(|_| ERR_PARSE)?),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Flag {
    pub name: String,
    pub usage: String,
    pub default: FlagValue,
    pub value: FlagValue,
    pub set: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FlagSet {
    name: String,
    formal: BTreeMap<String, Flag>,
    actual: BTreeMap<String, ()>,
    args: Vec<String>,
    output: Output,
    captured: std::cell::RefCell<Vec<u8>>,
}

impl FlagSet {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Selects where parse errors and usage go; `Output::Discard` silences
    /// them.
    pub fn set_output(&mut self, output: Output) {
        self.output = output;
    }

    pub fn string_var(&mut self, name: &str, default: &str, usage: &str) {
        self.define(name, FlagValue::String(default.to_string()), usage);
    }
    pub fn bool_var(&mut self, name: &str, default: bool, usage: &str) {
        self.define(name, FlagValue::Bool(default), usage);
    }
    pub fn int_var(&mut self, name: &str, default: i32, usage: &str) {
        self.define(name, FlagValue::Int(default), usage);
    }
    pub fn i64_var(&mut self, name: &str, default: i64, usage: &str) {
        self.define(name, FlagValue::I64(default), usage);
    }
    pub fn float64_var(&mut self, name: &str, default: f64, usage: &str) {
        self.define(name, FlagValue::F64(default), usage);
    }
    pub fn duration_var(&mut self, name: &str, default: Duration, usage: &str) {
        self.define(name, FlagValue::Duration(default), usage);
    }

    fn define(&mut self, name: &str, default: FlagValue, usage: &str) {
        assert!(
            !name.starts_with('-') && !name.contains('='),
            "flag \"{name}\" begins with - or contains ="
        );
        assert!(
            !self.formal.contains_key(name),
            "{} flag redefined: {name}",
            if self.name.is_empty() {
                "flag".to_string()
            } else {
                self.name.clone()
            }
        );
        self.formal.insert(
            name.to_string(),
            Flag {
                name: name.to_string(),
                usage: usage.to_string(),
                value: default.clone(),
                default,
                set: false,
            },
        );
    }

    fn out(&self) -> Option<Box<dyn Write + '_>> {
        match self.output {
            Output::Stderr => Some(Box::new(io::stderr())),
            Output::Discard => None,
            Output::Capture => Some(Box::new(CaptureWriter(&self.captured))),
        }
    }

    /// Everything Parse wrote under [`Output::Capture`] so far (and clears it).
    pub fn take_output(&self) -> Vec<u8> {
        std::mem::take(&mut *self.captured.borrow_mut())
    }

    /// Prints the message on the output, then the usage, and returns the
    /// message as the error.
    fn failf(&self, msg: String) -> FlagError {
        if let Some(mut w) = self.out() {
            let _ = writeln!(w, "{msg}");
        }
        self.usage();
        FlagError::Message(msg)
    }

    /// Prints `Usage of NAME:` (or `Usage:` when unnamed) and the defaults.
    fn usage(&self) {
        let Some(mut w) = self.out() else {
            return;
        };
        if self.name.is_empty() {
            let _ = writeln!(w, "Usage:");
        } else {
            let _ = writeln!(w, "Usage of {}:", self.name);
        }
        let _ = self.print_defaults(&mut w);
    }

    /// Parses flags until the first non-flag argument or `--` (the rest is
    /// left in `args()`). `-h`/`-help`, when not defined, prints the usage
    /// and returns [`FlagError::Help`]; any other failure prints its message
    /// and the usage and returns it as [`FlagError::Message`].
    pub fn parse(&mut self, args: &[String]) -> Result<(), FlagError> {
        self.args = args.to_vec();
        loop {
            let seen = self.parse_one()?;
            if !seen {
                return Ok(());
            }
        }
    }

    fn parse_one(&mut self) -> Result<bool, FlagError> {
        let Some(s) = self.args.first().cloned() else {
            return Ok(false);
        };
        let b = s.as_bytes();
        if b.len() < 2 || b[0] != b'-' {
            return Ok(false);
        }
        let mut num_minuses = 1;
        if b[1] == b'-' {
            num_minuses += 1;
            if b.len() == 2 {
                // "--" terminates the flags
                self.args.remove(0);
                return Ok(false);
            }
        }
        let mut name = &s[num_minuses..];
        if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
            return Err(self.failf(format!("bad flag syntax: {s}")));
        }
        // it's a flag. does it have an argument?
        self.args.remove(0);
        let mut has_value = false;
        let mut value = String::new();
        if let Some(i) = name[1..].find('=') {
            // equals cannot be first
            value = name[i + 2..].to_string();
            has_value = true;
            name = &name[..i + 1];
        }
        let name = name.to_string();
        let Some(flag) = self.formal.get(&name) else {
            if name == "help" || name == "h" {
                // special case for nice help message.
                self.usage();
                return Err(FlagError::Help);
            }
            return Err(self.failf(format!("flag provided but not defined: -{name}")));
        };
        let new_value = if matches!(flag.value, FlagValue::Bool(_)) {
            // special case: doesn't need an arg
            if has_value {
                match flag.value.set_from(&value) {
                    Ok(v) => v,
                    Err(e) => {
                        return Err(self.failf(format!(
                            "invalid boolean value {} for -{name}: {e}",
                            go_quote(&value)
                        )));
                    }
                }
            } else {
                FlagValue::Bool(true)
            }
        } else {
            // It must have a value, which might be the next argument.
            if !has_value && !self.args.is_empty() {
                has_value = true;
                value = self.args.remove(0);
            }
            if !has_value {
                return Err(self.failf(format!("flag needs an argument: -{name}")));
            }
            match flag.value.set_from(&value) {
                Ok(v) => v,
                Err(e) => {
                    return Err(self.failf(format!(
                        "invalid value {} for flag -{name}: {e}",
                        go_quote(&value)
                    )));
                }
            }
        };
        let flag = self.formal.get_mut(&name).expect("looked up above");
        flag.value = new_value;
        flag.set = true;
        self.actual.insert(name, ());
        Ok(true)
    }

    /// Sets a defined flag from text without touching the output; the error
    /// is `no such flag -NAME` or the value's own (`parse error`, `value out
    /// of range`).
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), FlagError> {
        let Some(flag) = self.formal.get_mut(name) else {
            return Err(FlagError::Message(format!("no such flag -{name}")));
        };
        flag.value = flag
            .value
            .set_from(value)
            .map_err(|e| FlagError::Message(e.to_string()))?;
        flag.set = true;
        self.actual.insert(name.to_string(), ());
        Ok(())
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }
    pub fn arg(&self, i: usize) -> &str {
        self.args.get(i).map_or("", String::as_str)
    }
    pub fn narg(&self) -> usize {
        self.args.len()
    }
    pub fn nflag(&self) -> usize {
        self.actual.len()
    }
    pub fn lookup(&self, name: &str) -> Option<&Flag> {
        self.formal.get(name)
    }
    pub fn is_set(&self, name: &str) -> bool {
        self.actual.contains_key(name)
    }
    pub fn visit<F: FnMut(&Flag)>(&self, mut f: F) {
        for name in self.actual.keys() {
            if let Some(flag) = self.formal.get(name) {
                f(flag);
            }
        }
    }
    pub fn visit_all<F: FnMut(&Flag)>(&self, mut f: F) {
        for flag in self.formal.values() {
            f(flag);
        }
    }
    pub fn get_string(&self, name: &str) -> String {
        match &self.formal[name].value {
            FlagValue::String(v) => v.clone(),
            _ => String::new(),
        }
    }
    pub fn get_bool(&self, name: &str) -> bool {
        matches!(self.formal[name].value, FlagValue::Bool(true))
    }
    pub fn get_int(&self, name: &str) -> i32 {
        match self.formal[name].value {
            FlagValue::Int(v) => v,
            _ => 0,
        }
    }
    pub fn get_i64(&self, name: &str) -> i64 {
        match self.formal[name].value {
            FlagValue::I64(v) => v,
            _ => 0,
        }
    }
    pub fn get_f64(&self, name: &str) -> f64 {
        match self.formal[name].value {
            FlagValue::F64(v) => v,
            _ => 0.0,
        }
    }
    pub fn get_duration(&self, name: &str) -> Duration {
        match self.formal[name].value {
            FlagValue::Duration(v) => v,
            _ => Duration::ZERO,
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Writes one entry per flag in name order: `  -name VALUE` (VALUE from
    /// [`unquote_usage`]), the usage on the same line for one-letter booleans
    /// and on an indented line otherwise, and ` (default X)` appended when
    /// the default is non-zero (strings quoted).
    pub fn print_defaults(&self, w: &mut dyn Write) -> io::Result<()> {
        for flag in self.formal.values() {
            let mut b = format!("  -{}", flag.name);
            let (name, usage) = unquote_usage(flag);
            if !name.is_empty() {
                b.push(' ');
                b.push_str(&name);
            }
            // Boolean flags of one ASCII letter are so common we
            // treat them specially, putting their usage on the same line.
            if b.len() <= 4 {
                // space, space, '-', 'x'.
                b.push('\t');
            } else {
                // Four spaces before the tab triggers good alignment
                // for both 4- and 8-space tab stops.
                b.push_str("\n    \t");
            }
            b.push_str(&usage.replace('\n', "\n    \t"));
            let default = flag.default.default_text();
            let is_zero = match &flag.default {
                FlagValue::String(v) => v.is_empty(),
                FlagValue::Bool(v) => !*v,
                FlagValue::Int(v) => *v == 0,
                FlagValue::I64(v) => *v == 0,
                FlagValue::F64(_) => default == "0",
                FlagValue::Duration(v) => v.is_zero(),
            };
            if !is_zero {
                match flag.default {
                    FlagValue::String(_) => {
                        b.push_str(&format!(" (default {})", go_quote(&default)))
                    }
                    _ => b.push_str(&format!(" (default {default})")),
                }
            }
            writeln!(w, "{b}")?;
        }
        Ok(())
    }
}

/// The value name and usage text for a flag: a back-quoted word in the
/// usage names the value and loses its quotes; otherwise the type names it.
pub fn unquote_usage(flag: &Flag) -> (String, String) {
    let usage = &flag.usage;
    if let Some(i) = usage.find('`')
        && let Some(j) = usage[i + 1..].find('`')
    {
        let name = usage[i + 1..i + 1 + j].to_string();
        let unquoted = format!("{}{}{}", &usage[..i], name, &usage[i + 2 + j..]);
        return (name, unquoted);
    }
    (flag.default.type_name().to_string(), usage.clone())
}

/// A double-quoted literal for the plain strings flags carry: `\"`, `\\`,
/// `\n`, `\r`, `\t`, `\a`, `\b`, `\f`, `\v`, and `\xHH` for any other
/// control character.
fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Shortest round-trip digits; exponent form with a signed two-digit
/// exponent when the decimal exponent is below -4 or at least 21 (`1e-05`,
/// `1e+21`); `NaN`, `+Inf`, `-Inf` and `-0` by name.
fn format_float_g(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "+Inf".into()
        } else {
            "-Inf".into()
        };
    }
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    let exp = v.abs().log10().floor() as i32;
    if !(-4..21).contains(&exp) {
        let s = format!("{v:e}");
        let (mant, e) = s.split_once('e').unwrap_or((&s, "0"));
        let e: i32 = e.parse().unwrap_or(0);
        let sign = if e < 0 { '-' } else { '+' };
        return format!("{mant}e{sign}{:02}", e.abs());
    }
    v.to_string()
}

/// An integer with optional sign and base by prefix (`0x`, `0o`, `0b`,
/// leading `0` = octal, else decimal), underscores allowed between digits;
/// `value out of range` beyond `bit_size` signed bits.
fn parse_go_int(s: &str, bit_size: u32) -> Result<i64, &'static str> {
    if s.is_empty() {
        return Err(ERR_PARSE);
    }
    let (neg, body) = match s.as_bytes()[0] {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    let un = parse_go_uint(body)?;
    let cutoff: u64 = 1u64 << (bit_size - 1);
    if !neg && un >= cutoff {
        return Err(ERR_RANGE);
    }
    if neg && un > cutoff {
        return Err(ERR_RANGE);
    }
    Ok(if neg {
        (un as i64).wrapping_neg()
    } else {
        un as i64
    })
}

fn parse_go_uint(s0: &str) -> Result<u64, &'static str> {
    if s0.is_empty() {
        return Err(ERR_PARSE);
    }
    let b = s0.as_bytes();
    let (base, s): (u64, &str) = if b[0] == b'0' {
        match b.get(1).map(u8::to_ascii_lowercase) {
            Some(b'b') if b.len() >= 3 => (2, &s0[2..]),
            Some(b'o') if b.len() >= 3 => (8, &s0[2..]),
            Some(b'x') if b.len() >= 3 => (16, &s0[2..]),
            _ => (8, &s0[1..]),
        }
    } else {
        (10, s0)
    };
    let mut underscores = false;
    let mut n: u64 = 0;
    for &c in s.as_bytes() {
        let d: u64 = match c {
            b'_' => {
                underscores = true;
                continue;
            }
            b'0'..=b'9' => u64::from(c - b'0'),
            b'a'..=b'z' => u64::from(c - b'a') + 10,
            b'A'..=b'Z' => u64::from(c - b'A') + 10,
            _ => return Err(ERR_PARSE),
        };
        if d >= base {
            return Err(ERR_PARSE);
        }
        n = n.checked_mul(base).ok_or(ERR_RANGE)?;
        n = n.checked_add(d).ok_or(ERR_RANGE)?;
    }
    if underscores && !underscore_ok(s0) {
        return Err(ERR_PARSE);
    }
    Ok(n)
}

/// Underscores are allowed only between digits (or right after a base
/// prefix): never leading, trailing or doubled.
fn underscore_ok(s: &str) -> bool {
    let b = s.as_bytes();
    let mut saw = b'^';
    let mut i = 0;
    let mut hex = false;
    if b.len() >= 2 && b[0] == b'0' && matches!(b[1].to_ascii_lowercase(), b'b' | b'o' | b'x') {
        i = 2;
        saw = b'0';
        hex = b[1].eq_ignore_ascii_case(&b'x');
    }
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || (hex && matches!(c.to_ascii_lowercase(), b'a'..=b'f')) {
            saw = b'0';
            i += 1;
            continue;
        }
        if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            i += 1;
            continue;
        }
        if saw == b'_' {
            return false;
        }
        saw = b'!';
        i += 1;
    }
    saw != b'_'
}

/// A decimal float with optional sign, exponent and underscores between
/// digits, or `inf`/`infinity`/`nan` by name (any case); a finite literal
/// too large for an f64 is `value out of range`.
fn parse_go_float(s: &str) -> Result<f64, &'static str> {
    let cleaned = if s.contains('_') && underscore_ok(s.trim_start_matches(['+', '-'])) {
        s.replace('_', "")
    } else {
        s.to_string()
    };
    let v: f64 = cleaned.parse().map_err(|_| ERR_PARSE)?;
    let lower = cleaned.trim_start_matches(['+', '-']).to_ascii_lowercase();
    if v.is_infinite() && lower != "inf" && lower != "infinity" {
        return Err(ERR_RANGE);
    }
    Ok(v)
}

/// Flags and positional arguments may mix; `--` ends flag parsing and
/// everything after it is positional.
pub fn parse_interleaved(fs: &mut FlagSet, mut args: &[String]) -> Result<Vec<String>, FlagError> {
    let mut positional = Vec::new();
    while !args.is_empty() {
        let a = &args[0];
        if a == "--" {
            positional.extend_from_slice(&args[1..]);
            return Ok(positional);
        }
        if !a.starts_with('-') || a == "-" {
            positional.push(a.clone());
            args = &args[1..];
            continue;
        }
        fs.parse(args)?;
        let rest = fs.args();
        let consumed = args.len() - rest.len();
        if consumed > 0 && args[consumed - 1] == "--" {
            positional.extend_from_slice(rest);
            return Ok(positional);
        }
        args = &args[consumed..];
    }
    Ok(positional)
}

fn parse_bool(s: &str) -> Result<bool, ()> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err(()),
    }
}

pub fn parse_duration(s: &str) -> Result<Duration, String> {
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    if s.is_empty() {
        return Err("time: invalid duration \"\"".to_string());
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut neg = false;
    if bytes[i] == b'+' || bytes[i] == b'-' {
        neg = bytes[i] == b'-';
        i += 1;
    }
    if neg || i >= bytes.len() {
        return Err(format!("time: invalid duration {s:?}"));
    }
    let mut total: u128 = 0;
    let mut saw = false;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let whole = &s[start..i];
        let mut frac = "";
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            let fstart = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            frac = &s[fstart..i];
        }
        if whole.is_empty() && frac.is_empty() {
            return Err(format!("time: invalid duration {s:?}"));
        }
        let ustart = i;
        while i < bytes.len() {
            let ch = s[i..]
                .chars()
                .next()
                .ok_or_else(|| format!("time: invalid duration {s:?}"))?;
            if !(ch.is_ascii_alphabetic() || ch == 'µ' || ch == 'μ') {
                break;
            }
            i += ch.len_utf8();
        }
        let unit = &s[ustart..i];
        let mult = match unit {
            "ns" => 1u128,
            "us" | "µs" | "μs" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60 * 1_000_000_000,
            "h" => 60 * 60 * 1_000_000_000,
            "" => return Err(format!("time: missing unit in duration {s:?}")),
            _ => return Err(format!("time: unknown unit {unit:?} in duration {s:?}")),
        };
        let whole_n: u128 = if whole.is_empty() {
            0
        } else {
            whole
                .parse()
                .map_err(|_| format!("time: invalid duration {s:?}"))?
        };
        let mut add = whole_n
            .checked_mul(mult)
            .ok_or_else(|| format!("time: invalid duration {s:?}"))?;
        if !frac.is_empty() {
            let mut scale = 1u128;
            let mut frac_n = 0u128;
            for ch in frac.bytes() {
                frac_n = frac_n * 10 + u128::from(ch - b'0');
                scale *= 10;
            }
            add += frac_n * mult / scale;
        }
        total = total
            .checked_add(add)
            .ok_or_else(|| format!("time: invalid duration {s:?}"))?;
        saw = true;
    }
    if !saw || total > u64::MAX as u128 {
        return Err(format!("time: invalid duration {s:?}"));
    }
    Ok(Duration::from_nanos(total as u64))
}

pub fn format_duration(d: Duration) -> String {
    if d.is_zero() {
        return "0s".to_string();
    }
    let mut n = d.as_nanos();
    if n < 1_000_000_000 {
        if n < 1_000 {
            return format!("{n}ns");
        }
        if n < 1_000_000 {
            return decimal_unit(n, 1_000, "µs");
        }
        return decimal_unit(n, 1_000_000, "ms");
    }
    let mut out = String::new();
    let h = n / 3_600_000_000_000;
    if h > 0 {
        out.push_str(&format!("{h}h"));
        n -= h * 3_600_000_000_000;
    }
    let m = n / 60_000_000_000;
    if m > 0 {
        out.push_str(&format!("{m}m"));
        n -= m * 60_000_000_000;
    }
    let s = n / 1_000_000_000;
    n -= s * 1_000_000_000;
    if n == 0 {
        out.push_str(&format!("{s}s"));
    } else {
        let frac = format!("{n:09}");
        out.push_str(&format!("{s}.{}s", frac.trim_end_matches('0')));
    }
    out
}

fn decimal_unit(nanos: u128, unit: u128, suffix: &str) -> String {
    let whole = nanos / unit;
    let rem = nanos % unit;
    if rem == 0 {
        return format!("{whole}{suffix}");
    }
    let width = match unit {
        1_000 => 3,
        1_000_000 => 6,
        _ => 0,
    };
    let frac = format!("{rem:0width$}");
    format!("{whole}.{}{suffix}", frac.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parses_go_style_flags() {
        let _serial = crate::test_helpers::serial();
        let mut fs = FlagSet::new("x");
        fs.string_var("s", "", "string flag");
        fs.bool_var("b", false, "bool flag");
        fs.int_var("n", 0, "int flag");
        fs.i64_var("i64", 0, "i64 flag");
        fs.float64_var("f", 0.0, "float flag");
        fs.duration_var("d", Duration::ZERO, "duration flag");
        fs.parse(&sv(&[
            "-s=hi",
            "-b",
            "-n",
            "3",
            "--i64=4",
            "-f",
            "1.5",
            "-d",
            "1h30m",
            "arg",
            "--ignored",
        ]))
        .unwrap();
        assert_eq!(fs.get_string("s"), "hi");
        assert!(fs.get_bool("b"));
        assert_eq!(fs.get_int("n"), 3);
        assert_eq!(fs.get_i64("i64"), 4);
        assert_eq!(fs.get_f64("f"), 1.5);
        assert_eq!(fs.get_duration("d"), Duration::from_secs(90 * 60));
        assert_eq!(fs.args(), &sv(&["arg", "--ignored"]));
        assert_eq!(fs.arg(99), "");
        assert_eq!(fs.nflag(), 6);
    }

    #[test]
    fn flag_errors_and_interleaving() {
        let _serial = crate::test_helpers::serial();
        let mut fs = FlagSet::new("x");
        fs.bool_var("b", false, "");
        assert_eq!(fs.parse(&sv(&["-h"])), Err(FlagError::Help));
        assert_eq!(
            fs.parse(&sv(&["--bogus"])),
            Err(FlagError::Message(
                "flag provided but not defined: -bogus".into()
            ))
        );
        assert_eq!(
            fs.set("missing", "x"),
            Err(FlagError::Message("no such flag -missing".into()))
        );
        assert_eq!(
            fs.set("b", "maybe"),
            Err(FlagError::Message("parse error".into()))
        );
        assert_eq!(
            fs.parse(&sv(&["-b=maybe"])),
            Err(FlagError::Message(
                "invalid boolean value \"maybe\" for -b: parse error".into()
            ))
        );
        assert_eq!(
            fs.parse(&sv(&["---b"])),
            Err(FlagError::Message("bad flag syntax: ---b".into()))
        );

        let mut fs = FlagSet::new("x");
        fs.string_var("kind", "", "");
        let got = parse_interleaved(
            &mut fs,
            &sv(&["one", "--kind", "finding", "two", "--", "-x"]),
        )
        .unwrap();
        assert_eq!(got, sv(&["one", "two", "-x"]));
        assert_eq!(fs.get_string("kind"), "finding");
    }

    #[test]
    fn durations_match_go_forms() {
        let _serial = crate::test_helpers::serial();
        for (s, want) in [
            ("0", Duration::ZERO),
            ("500ms", Duration::from_millis(500)),
            ("10m", Duration::from_secs(600)),
            ("1h30m", Duration::from_secs(5400)),
            ("1.5s", Duration::from_millis(1500)),
            ("1s500ms", Duration::from_millis(1500)),
        ] {
            assert_eq!(parse_duration(s).unwrap(), want);
        }
        assert_eq!(format_duration(Duration::from_secs(5400)), "1h30m0s");
        assert_eq!(format_duration(Duration::from_micros(1500)), "1.5ms");
        assert!(parse_duration("1d").unwrap_err().contains("unknown unit"));
        assert!(parse_duration("-1s").is_err());
    }
}
