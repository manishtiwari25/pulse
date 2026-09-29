use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagError {
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
    fn type_name(&self) -> Option<&'static str> {
        match self {
            Self::String(_) => Some("string"),
            Self::Int(_) => Some("int"),
            Self::I64(_) => Some("int"),
            Self::F64(_) => Some("float"),
            Self::Duration(_) => Some("duration"),
            Self::Bool(_) => None,
        }
    }

    fn default_text(&self) -> String {
        match self {
            Self::String(v) => v.clone(),
            Self::Bool(v) => v.to_string(),
            Self::Int(v) => v.to_string(),
            Self::I64(v) => v.to_string(),
            Self::F64(v) => v.to_string(),
            Self::Duration(v) => format_duration(*v),
        }
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
}

impl FlagSet {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
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

    pub fn parse(&mut self, args: &[String]) -> Result<(), FlagError> {
        self.args.clear();
        let mut i = 0;
        while i < args.len() {
            let s = &args[i];
            if s == "--" {
                self.args.extend_from_slice(&args[i + 1..]);
                return Ok(());
            }
            if !s.starts_with('-') || s == "-" {
                self.args.extend_from_slice(&args[i..]);
                return Ok(());
            }
            let trimmed = s.trim_start_matches('-');
            if trimmed == "h" || trimmed == "help" {
                return Err(FlagError::Help);
            }
            let (name, value_inline) = match trimmed.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (trimmed, None),
            };
            let Some(flag) = self.formal.get(name) else {
                return Err(FlagError::Message(format!(
                    "flag provided but not defined: -{name}"
                )));
            };
            let value = if matches!(flag.value, FlagValue::Bool(_)) {
                value_inline.unwrap_or("true").to_string()
            } else if let Some(value) = value_inline {
                value.to_string()
            } else {
                i += 1;
                if i >= args.len() {
                    return Err(FlagError::Message(format!(
                        "flag needs an argument: -{name}"
                    )));
                }
                args[i].clone()
            };
            self.set(name, &value)?;
            i += 1;
        }
        Ok(())
    }

    pub fn set(&mut self, name: &str, value: &str) -> Result<(), FlagError> {
        let Some(flag) = self.formal.get_mut(name) else {
            return Err(FlagError::Message(format!("no such flag -{name}")));
        };
        let parsed = match &flag.default {
            FlagValue::String(_) => FlagValue::String(value.to_string()),
            FlagValue::Bool(_) => FlagValue::Bool(parse_bool(value).map_err(|_| {
                FlagError::Message(format!("invalid boolean value \"{value}\" for -{name}"))
            })?),
            FlagValue::Int(_) => FlagValue::Int(value.parse::<i32>().map_err(|e| {
                FlagError::Message(format!("invalid value \"{value}\" for flag -{name}: {e}"))
            })?),
            FlagValue::I64(_) => FlagValue::I64(value.parse::<i64>().map_err(|e| {
                FlagError::Message(format!("invalid value \"{value}\" for flag -{name}: {e}"))
            })?),
            FlagValue::F64(_) => FlagValue::F64(value.parse::<f64>().map_err(|e| {
                FlagError::Message(format!("invalid value \"{value}\" for flag -{name}: {e}"))
            })?),
            FlagValue::Duration(_) => FlagValue::Duration(parse_duration(value).map_err(|e| {
                FlagError::Message(format!("invalid value \"{value}\" for flag -{name}: {e}"))
            })?),
        };
        flag.value = parsed;
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

    pub fn print_defaults(&self, w: &mut dyn Write) -> io::Result<()> {
        for flag in self.formal.values() {
            write!(w, "  -{}", flag.name)?;
            if let Some(t) = flag.default.type_name() {
                write!(w, " {t}")?;
            }
            let default = flag.default.default_text();
            let show_default = match &flag.default {
                FlagValue::String(v) => !v.is_empty(),
                FlagValue::Bool(v) => *v,
                FlagValue::Int(v) => *v != 0,
                FlagValue::I64(v) => *v != 0,
                FlagValue::F64(v) => *v != 0.0,
                FlagValue::Duration(v) => !v.is_zero(),
            };
            writeln!(w)?;
            write!(w, "    \t{}", flag.usage)?;
            if show_default {
                match flag.default {
                    FlagValue::String(_) => write!(w, " (default {default:?})")?,
                    _ => write!(w, " (default {default})")?,
                }
            }
            writeln!(w)?;
        }
        Ok(())
    }
}

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
            Err(FlagError::Message(
                "invalid boolean value \"maybe\" for -b".into()
            ))
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
