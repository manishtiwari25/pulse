//! Needs and capabilities: the words a project or a job uses to say what
//! kind of machine it must run on (`macos`, `linux`, `arm64`, `xcode`,
//! `docker`, ...) and the words a machine offers. Both sides are plain
//! lowercase words; a job runs on a machine that offers every word it needs.
//! Stored as a comma-separated list, empty when anything will do.

use std::collections::BTreeSet;

/// The operating-system words; a machine offers exactly one.
pub const OS_WORDS: [&str; 3] = ["macos", "linux", "windows"];
/// The architecture words; a machine offers exactly one.
pub const ARCH_WORDS: [&str; 2] = ["arm64", "amd64"];

/// One word in its canonical form, or `None` for an empty one.
/// `Err` names a word that is not a word: letters, digits, `-`, `_`, `.`
/// and `:` only, so `xcode:16` and `python3.12` pass and `macOS!` does not.
pub fn word(raw: &str) -> Result<Option<String>, String> {
    let w = raw.trim().to_ascii_lowercase();
    if w.is_empty() {
        return Ok(None);
    }
    if !w
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        || !w.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return Err(format!(
            "`{}` is not a need: use lowercase words such as macos, linux, arm64, xcode or docker",
            raw.trim()
        ));
    }
    Ok(Some(
        match w.as_str() {
            "darwin" | "osx" | "mac" | "mac-os" | "macosx" => "macos",
            "x86_64" | "x86-64" | "x64" | "intel" => "amd64",
            "aarch64" | "arm" | "apple-silicon" => "arm64",
            "win" | "win32" | "win64" => "windows",
            "gnu-linux" | "ubuntu" | "debian" => "linux",
            other => other,
        }
        .to_string(),
    ))
}

/// A list the owner or an agent typed: words split on commas, semicolons
/// and spaces, each canonical, in first-seen order, no repeats.
pub fn parse(list: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in list.split([',', ';', ' ', '\n', '\t']) {
        if let Some(w) = word(raw)?
            && !out.contains(&w)
        {
            out.push(w);
        }
    }
    Ok(out)
}

/// `parse` that keeps what it can read of a stored list and drops the rest:
/// the store is never the place to refuse.
pub fn stored(list: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in list.split([',', ';', ' ', '\n', '\t']) {
        if let Ok(Some(w)) = word(raw)
            && !out.contains(&w)
        {
            out.push(w);
        }
    }
    out
}

/// The stored form: comma-separated, canonical, or empty.
pub fn join(words: &[String]) -> String {
    words.join(",")
}

/// The stored form of a typed list, or why it cannot be one.
pub fn canonical(list: &str) -> Result<String, String> {
    parse(list).map(|w| join(&w))
}

/// The needs of `needs` that `caps` does not offer; empty when the machine
/// fits. A need that is an OS or architecture word is met only by that word,
/// like any other.
pub fn missing(needs: &str, caps: &BTreeSet<String>) -> Vec<String> {
    stored(needs)
        .into_iter()
        .filter(|n| !caps.contains(n))
        .collect()
}

/// Whether a machine offering `caps` can run a job that needs `needs`.
pub fn met(needs: &str, caps: &BTreeSet<String>) -> bool {
    missing(needs, caps).is_empty()
}

/// A SQL condition on the column `col` (for example `j.needs`) that holds
/// when a machine offering `caps` meets it, with the values to bind. The
/// stored list becomes a JSON array so SQLite walks its words.
pub fn sql_fit(col: &str, caps: &BTreeSet<String>) -> (String, Vec<String>) {
    if caps.is_empty() {
        return (format!("{col} = ''"), Vec::new());
    }
    let marks = vec!["?"; caps.len()].join(",");
    (
        format!(
            "({col} = '' OR NOT EXISTS (SELECT 1 FROM json_each('[\"' || replace({col}, ',', '\",\"') || '\"]') nw WHERE nw.value NOT IN ({marks})))"
        ),
        caps.iter().cloned().collect(),
    )
}

/// The capabilities of a machine from its stored columns: its OS and
/// architecture words plus its labels.
pub fn caps_of(os: &str, arch: &str, labels: &str) -> BTreeSet<String> {
    let mut caps = BTreeSet::new();
    for w in [os, arch] {
        if let Ok(Some(w)) = word(w) {
            caps.insert(w);
        }
    }
    caps.extend(stored(labels));
    caps
}

/// The OS word of the program's own platform.
pub fn this_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

/// The architecture word of the program's own platform.
pub fn this_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(words: &[&str]) -> BTreeSet<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn words_are_canonical_and_bad_ones_are_named() {
        assert_eq!(
            parse("Darwin, x86_64 xcode;docker,docker").unwrap(),
            ["macos", "amd64", "xcode", "docker"]
        );
        assert_eq!(canonical(" aarch64 ").unwrap(), "arm64");
        assert_eq!(canonical("").unwrap(), "");
        let err = parse("macos, mac OS!").unwrap_err();
        assert!(err.contains("`OS!` is not a need"), "{err}");
        assert!(parse("-x").is_err());
        assert_eq!(stored("macos,,BAD!,xcode:16"), ["macos", "xcode:16"]);
    }

    #[test]
    fn a_machine_fits_when_it_offers_every_word() {
        let mac = caps_of("macos", "arm64", "xcode,docker");
        assert!(met("", &mac));
        assert!(met("macos,xcode", &mac));
        assert_eq!(missing("linux,xcode", &mac), ["linux"]);
        let hand = caps_of("linux", "amd64", "");
        assert!(met("linux", &hand));
        assert!(!met("docker", &hand));
        assert_eq!(missing("macos,xcode", &hand), ["macos", "xcode"]);
    }

    #[test]
    fn the_sql_condition_walks_the_stored_list() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE j (id TEXT, needs TEXT NOT NULL DEFAULT '');
             INSERT INTO j VALUES ('any', ''), ('mac', 'macos,xcode'), ('lin', 'linux'), ('dock', 'docker');",
        )
        .unwrap();
        let fits = |caps: &BTreeSet<String>| -> Vec<String> {
            let (cond, vals) = sql_fit("j.needs", caps);
            let mut st = conn
                .prepare(&format!("SELECT id FROM j WHERE {cond} ORDER BY id"))
                .unwrap();
            let refs: Vec<&dyn rusqlite::ToSql> =
                vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
            st.query_map(refs.as_slice(), |r| r.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(fits(&caps(&["macos", "arm64", "xcode"])), ["any", "mac"]);
        assert_eq!(
            fits(&caps(&["linux", "amd64", "docker"])),
            ["any", "dock", "lin"]
        );
        assert_eq!(fits(&caps(&[])), ["any"]);
    }

    #[test]
    fn this_platform_has_words() {
        assert!(OS_WORDS.contains(&this_os()));
        assert!(!this_arch().is_empty());
    }
}
