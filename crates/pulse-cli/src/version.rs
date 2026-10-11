#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Module {
    pub path: String,
    pub version: String,
    pub sum: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildSetting {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildInfo {
    pub path: String,
    pub main: Module,
    pub deps: Vec<Module>,
    pub settings: Vec<BuildSetting>,
}

include!(concat!(env!("OUT_DIR"), "/build_info_const.rs"));

/// Reads back a quoted build setting: a double-quoted literal with the `\a \b \f \n \r \t \v`,
/// `\\`, `\"`, `\'`, `\xHH`, `\uHHHH`, `\UHHHHHHHH` and three-digit octal escapes, or a
/// back-quoted raw string (no backtick inside; CRs dropped). None when the literal is malformed.
fn unquote(raw: &str) -> Option<String> {
    let mut chars = raw.chars();
    let quote = chars.next()?;
    let body = raw.get(1..raw.len().checked_sub(1)?)?;
    if !raw.ends_with(quote) || raw.len() < 2 {
        return None;
    }
    if quote == '`' {
        return (!body.contains('`')).then(|| body.replace('\r', ""));
    }
    if quote != '"' {
        return None;
    }
    let mut out = String::new();
    let mut it = body.chars();
    while let Some(ch) = it.next() {
        if ch == '"' || ch == '\n' {
            return None;
        }
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match it.next()? {
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            'f' => out.push('\x0c'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\x0b'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            'x' => out.push(char::from_u32(hex(&mut it, 2)?)?),
            'u' => out.push(char::from_u32(hex(&mut it, 4)?)?),
            'U' => out.push(char::from_u32(hex(&mut it, 8)?)?),
            c @ '0'..='7' => {
                let mut v = c.to_digit(8)?;
                for _ in 0..2 {
                    v = v * 8 + it.next()?.to_digit(8)?;
                }
                out.push(char::from_u32(v)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

fn hex(it: &mut std::str::Chars<'_>, n: usize) -> Option<u32> {
    let mut v = 0u32;
    for _ in 0..n {
        v = v * 16 + it.next()?.to_digit(16)?;
    }
    Some(v)
}

fn quote_key(key: &str) -> bool {
    key.is_empty() || key.contains(['=', ' ', '\t', '\r', '\n', '"', '`'])
}

fn quote_value(value: &str) -> bool {
    value.contains([' ', '\t', '\r', '\n', '"', '`'])
}

fn quoted_prefix(s: &str) -> Option<&str> {
    let quote = s.chars().next()?;
    if quote == '`' {
        let end = s[1..].find('`')? + 1;
        return Some(&s[..=end]);
    }
    if quote != '"' {
        return None;
    }
    let mut escaped = false;
    for (i, ch) in s.char_indices().skip(1) {
        match ch {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some(&s[..=i]),
            '\n' => return None,
            _ => escaped = false,
        }
    }
    None
}

fn read_module_line(elem: &[&str]) -> Option<Module> {
    if elem.len() != 2 && elem.len() != 3 {
        return None;
    }
    Some(Module {
        path: elem[0].to_string(),
        version: elem[1].to_string(),
        sum: elem.get(2).copied().unwrap_or("").to_string(),
    })
}

/// Parses the build-info block that `build.rs` embeds (and that the pair-hands inspection greps
/// out of a remote binary): every line ends in '\n' (a trailing fragment is dropped), unknown
/// lines are skipped, and a malformed mod/dep/=>/build line makes the whole text unreadable
/// (None).
pub fn parse_build_info(text: &str) -> Option<BuildInfo> {
    let mut bi = BuildInfo {
        path: String::new(),
        main: Module::default(),
        deps: Vec::new(),
        settings: Vec::new(),
    };
    // The module a following "=>" replacement line applies to: the main module or the last dep.
    let mut last: Option<Option<usize>> = None;
    let mut data = text;
    while !data.is_empty() {
        let Some((line, rest)) = data.split_once('\n') else {
            break;
        };
        data = rest;
        if let Some(elem) = line.strip_prefix("path\t") {
            bi.path = elem.to_string();
        } else if let Some(elem) = line.strip_prefix("mod\t") {
            bi.main = read_module_line(&elem.split('\t').collect::<Vec<_>>())?;
            last = Some(None);
        } else if let Some(elem) = line.strip_prefix("dep\t") {
            bi.deps
                .push(read_module_line(&elem.split('\t').collect::<Vec<_>>())?);
            last = Some(Some(bi.deps.len() - 1));
        } else if let Some(elem) = line.strip_prefix("=>\t") {
            if elem.split('\t').count() != 3 {
                return None;
            }
            // Replacements are not kept (nothing reads them); the line must still be well formed.
            last.take()?;
        } else if let Some(kv) = line.strip_prefix("build\t") {
            let first = kv.chars().next()?;
            let (key, raw_value) = match first {
                '=' => return None,
                '`' | '"' => {
                    let raw_key = quoted_prefix(kv)?;
                    let after = &kv[raw_key.len()..];
                    if !after.starts_with('=') {
                        return None;
                    }
                    (unquote(raw_key)?, &after[1..])
                }
                _ => {
                    let (key, raw_value) = kv.split_once('=')?;
                    if quote_key(key) {
                        return None;
                    }
                    (key.to_string(), raw_value)
                }
            };
            let value = match raw_value.chars().next() {
                Some('`') | Some('"') => unquote(raw_value)?,
                _ => {
                    if quote_value(raw_value) {
                        return None;
                    }
                    raw_value.to_string()
                }
            };
            bi.settings.push(BuildSetting { key, value });
        }
    }
    Some(bi)
}

pub fn read_build_info() -> Option<BuildInfo> {
    parse_build_info(BUILD_INFO_TEXT)
}

pub fn build_version(info: &BuildInfo) -> String {
    if !info.main.version.is_empty() && info.main.version != "(devel)" {
        return info.main.version.clone();
    }
    let mut rev = "";
    let mut dirty = "";
    for s in &info.settings {
        match s.key.as_str() {
            "vcs.revision" => rev = &s.value,
            "vcs.modified" if s.value == "true" => dirty = "-dirty",
            _ => {}
        }
    }
    if rev.is_empty() {
        return "dev".to_string();
    }
    let rev = rev.chars().take(12).collect::<String>();
    format!("dev-{rev}{dirty}")
}

pub fn version() -> String {
    if let Some(v) = option_env!("PULSE_VERSION").filter(|v| !v.is_empty()) {
        return v.to_string();
    }
    read_build_info()
        .map(|info| build_version(&info))
        .unwrap_or_else(|| "dev".to_string())
}

pub fn revision() -> String {
    read_build_info()
        .and_then(|info| {
            info.settings
                .into_iter()
                .find(|s| s.key == "vcs.revision")
                .map(|s| s.value)
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_version() {
        let _serial = crate::test_helpers::serial();
        for tc in [
            ("module", "v1.2.3", "", "", "v1.2.3"),
            (
                "module-wins",
                "v1.2.3",
                "abcdef1234567890",
                "true",
                "v1.2.3",
            ),
            (
                "clean",
                "(devel)",
                "abcdef1234567890",
                "false",
                "dev-abcdef123456",
            ),
            (
                "dirty",
                "(devel)",
                "abcdef1234567890",
                "true",
                "dev-abcdef123456-dirty",
            ),
            ("short-revision", "", "abcdef", "", "dev-abcdef"),
            ("no-revision", "(devel)", "", "true", "dev"),
            ("no-metadata", "", "", "", "dev"),
        ] {
            let (name, version, revision, modified, want) = tc;
            let info = BuildInfo {
                path: String::new(),
                main: Module {
                    path: String::new(),
                    version: version.to_string(),
                    sum: String::new(),
                },
                deps: Vec::new(),
                settings: vec![
                    BuildSetting {
                        key: "vcs.revision".to_string(),
                        value: revision.to_string(),
                    },
                    BuildSetting {
                        key: "vcs.modified".to_string(),
                        value: modified.to_string(),
                    },
                ],
            };
            let got = super::build_version(&info);
            assert_eq!(got, want, "BuildVersion = {got:?}, want {want:?}: {name}");
        }
    }
}
