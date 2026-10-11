//! Shell-style path matching and globbing for the scope patterns in a
//! constitution. The rules are fixed because stored constitutions rely on
//! them: `*` and `?` never cross a `/`, `**` is one `*`, a character class
//! may be negated with `^`, `\` escapes the next character, and a malformed
//! pattern is an error rather than a non-match. `filepath_glob` opens only
//! the directories the pattern names instead of walking a checkout (a tree
//! with node_modules or target took tens of seconds per project).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BadPattern;

impl std::fmt::Display for BadPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("syntax error in pattern")
    }
}

const RUNE_ERROR: char = '\u{FFFD}';

/// Decodes the first UTF-8 character of `s`: (U+FFFD, 0) on empty input,
/// (U+FFFD, 1) on an invalid byte so the caller can report a bad pattern.
fn decode_rune(s: &[u8]) -> (char, usize) {
    let Some(&first) = s.first() else {
        return (RUNE_ERROR, 0);
    };
    let width = match first {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return (RUNE_ERROR, 1),
    };
    if s.len() < width {
        return (RUNE_ERROR, 1);
    }
    match std::str::from_utf8(&s[..width]) {
        Ok(text) => (text.chars().next().unwrap_or(RUNE_ERROR), width),
        Err(_) => (RUNE_ERROR, 1),
    }
}

pub(crate) fn path_match(pattern: &str, name: &str) -> Result<bool, BadPattern> {
    let mut pattern = pattern.as_bytes();
    let mut name = name.as_bytes();
    'pattern: while !pattern.is_empty() {
        let (star, chunk, rest) = scan_chunk(pattern);
        pattern = rest;
        if star && chunk.is_empty() {
            // Trailing * matches rest of string unless it has a /.
            return Ok(!name.contains(&b'/'));
        }
        // Look for match at current position.
        let (t, ok, err) = match_chunk(chunk, name);
        // if we're the last chunk, make sure we've exhausted the name
        // otherwise we'll give a false result even if we could still match
        // using the star
        if ok && (t.is_empty() || !pattern.is_empty()) {
            name = t;
            continue;
        }
        if let Some(e) = err {
            return Err(e);
        }
        if star {
            // Look for match skipping i+1 bytes. Cannot skip /.
            let mut i = 0;
            while i < name.len() && name[i] != b'/' {
                let (t, ok, err) = match_chunk(chunk, &name[i + 1..]);
                if ok {
                    // if we're the last chunk, make sure we exhausted the name
                    if pattern.is_empty() && !t.is_empty() {
                        i += 1;
                        continue;
                    }
                    name = t;
                    continue 'pattern;
                }
                if let Some(e) = err {
                    return Err(e);
                }
                i += 1;
            }
        }
        // Before returning false with no error, check that the remainder of
        // the pattern is syntactically valid.
        while !pattern.is_empty() {
            let (_, chunk, rest) = scan_chunk(pattern);
            pattern = rest;
            if let (_, _, Some(e)) = match_chunk(chunk, b"") {
                return Err(e);
            }
        }
        return Ok(false);
    }
    Ok(name.is_empty())
}

fn scan_chunk(mut pattern: &[u8]) -> (bool, &[u8], &[u8]) {
    let mut star = false;
    while !pattern.is_empty() && pattern[0] == b'*' {
        pattern = &pattern[1..];
        star = true;
    }
    let mut inrange = false;
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            // error check handled in match_chunk: bad pattern.
            b'\\' if i + 1 < pattern.len() => i += 1,
            b'[' => inrange = true,
            b']' => inrange = false,
            b'*' if !inrange => break,
            _ => {}
        }
        i += 1;
    }
    (star, &pattern[..i], &pattern[i..])
}

fn match_chunk<'s>(mut chunk: &[u8], mut s: &'s [u8]) -> (&'s [u8], bool, Option<BadPattern>) {
    // failed records whether the match has failed. After the match fails,
    // the loop continues on processing chunk, checking that the pattern is
    // well-formed but no longer reading s.
    let mut failed = false;
    while !chunk.is_empty() {
        if !failed && s.is_empty() {
            failed = true;
        }
        match chunk[0] {
            b'[' => {
                // character class
                let mut r = '\0';
                if !failed {
                    let (rune, n) = decode_rune(s);
                    r = rune;
                    s = &s[n..];
                }
                chunk = &chunk[1..];
                // possibly negated
                let mut negated = false;
                if !chunk.is_empty() && chunk[0] == b'^' {
                    negated = true;
                    chunk = &chunk[1..];
                }
                // parse all ranges
                let mut matched = false;
                let mut nrange = 0;
                loop {
                    if !chunk.is_empty() && chunk[0] == b']' && nrange > 0 {
                        chunk = &chunk[1..];
                        break;
                    }
                    let lo = match get_esc(chunk) {
                        Ok((lo, rest)) => {
                            chunk = rest;
                            lo
                        }
                        Err(e) => return (b"", false, Some(e)),
                    };
                    let mut hi = lo;
                    if chunk[0] == b'-' {
                        match get_esc(&chunk[1..]) {
                            Ok((h, rest)) => {
                                hi = h;
                                chunk = rest;
                            }
                            Err(e) => return (b"", false, Some(e)),
                        }
                    }
                    if lo <= r && r <= hi {
                        matched = true;
                    }
                    nrange += 1;
                }
                if matched == negated {
                    failed = true;
                }
            }
            b'?' => {
                if !failed {
                    if s[0] == b'/' {
                        failed = true;
                    }
                    let (_, n) = decode_rune(s);
                    s = &s[n..];
                }
                chunk = &chunk[1..];
            }
            b'\\' => {
                chunk = &chunk[1..];
                if chunk.is_empty() {
                    return (b"", false, Some(BadPattern));
                }
                // fallthrough: the escaped byte is a literal
                if !failed {
                    if chunk[0] != s[0] {
                        failed = true;
                    }
                    s = &s[1..];
                }
                chunk = &chunk[1..];
            }
            _ => {
                if !failed {
                    if chunk[0] != s[0] {
                        failed = true;
                    }
                    s = &s[1..];
                }
                chunk = &chunk[1..];
            }
        }
    }
    if failed {
        return (b"", false, None);
    }
    (s, true, None)
}

fn get_esc(mut chunk: &[u8]) -> Result<(char, &[u8]), BadPattern> {
    if chunk.is_empty() || chunk[0] == b'-' || chunk[0] == b']' {
        return Err(BadPattern);
    }
    if chunk[0] == b'\\' {
        chunk = &chunk[1..];
        if chunk.is_empty() {
            return Err(BadPattern);
        }
    }
    let (r, n) = decode_rune(chunk);
    if r == RUNE_ERROR && n == 1 {
        return Err(BadPattern);
    }
    let nchunk = &chunk[n..];
    if nchunk.is_empty() {
        return Err(BadPattern);
    }
    Ok((r, nchunk))
}

pub(crate) fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let p = path.as_bytes();
    let rooted = p[0] == b'/';
    let n = p.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut r = 0;
    let mut dotdot = 0;
    if rooted {
        out.push(b'/');
        r = 1;
        dotdot = 1;
    }
    while r < n {
        if p[r] == b'/' {
            // empty path element
            r += 1;
        } else if p[r] == b'.' && (r + 1 == n || p[r + 1] == b'/') {
            // . element
            r += 1;
        } else if p[r] == b'.' && p[r + 1] == b'.' && (r + 2 == n || p[r + 2] == b'/') {
            // .. element: remove to last /
            r += 2;
            if out.len() > dotdot {
                // can backtrack: step w back until the byte at w is a
                // separator, then truncate there so that separator goes too.
                let mut w = out.len() - 1;
                while w > dotdot && out[w] != b'/' {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                // cannot backtrack, but not rooted, so append .. element.
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            // real path element. add slash if needed
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(b'/');
            }
            // copy element
            while r < n && p[r] != b'/' {
                out.push(p[r]);
                r += 1;
            }
        }
    }
    // Turn empty string into "."
    if out.is_empty() {
        out.push(b'.');
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

pub(crate) fn join(elem: &[&str]) -> String {
    for (i, e) in elem.iter().enumerate() {
        if !e.is_empty() {
            return clean(&elem[i..].join("/"));
        }
    }
    String::new()
}

fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    }
}

fn has_meta(path: &str) -> bool {
    path.contains(['*', '?', '[', '\\'])
}

fn clean_glob_path(path: &str) -> &str {
    match path {
        "" => ".",
        "/" => path,
        _ => &path[..path.len() - 1],
    }
}

pub(crate) fn filepath_glob(pattern: &str) -> Result<Vec<String>, BadPattern> {
    glob_with_limit(pattern, 0)
}

fn glob_with_limit(pattern: &str, depth: usize) -> Result<Vec<String>, BadPattern> {
    // Bounds the recursion so a pattern with thousands of separators cannot
    // exhaust the stack (CVE-2022-30632).
    const PATH_SEPARATORS_LIMIT: usize = 10000;
    if depth == PATH_SEPARATORS_LIMIT {
        return Err(BadPattern);
    }
    // Check pattern is well-formed.
    path_match(pattern, "")?;
    if !has_meta(pattern) {
        return Ok(if std::fs::symlink_metadata(pattern).is_ok() {
            vec![pattern.to_string()]
        } else {
            Vec::new()
        });
    }
    let (dir, file) = split(pattern);
    let dir = clean_glob_path(dir);
    if !has_meta(dir) {
        return glob_dir(dir, file, Vec::new());
    }
    // Prevent infinite recursion when the directory part cleans back to the
    // pattern itself.
    if dir == pattern {
        return Err(BadPattern);
    }
    let mut matches = Vec::new();
    for d in glob_with_limit(dir, depth + 1)? {
        matches = glob_dir(&d, file, matches)?;
    }
    Ok(matches)
}

/// Searches for files matching pattern in the directory dir and appends them
/// to matches. If the directory cannot be opened, it returns the existing
/// matches. New matches are added in lexicographical order.
fn glob_dir(dir: &str, pattern: &str, mut matches: Vec<String>) -> Result<Vec<String>, BadPattern> {
    let Ok(fi) = std::fs::metadata(dir) else {
        return Ok(matches); // ignore I/O error
    };
    if !fi.is_dir() {
        return Ok(matches);
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(matches); // ignore I/O error
    };
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for n in names {
        if path_match(pattern, &n)? {
            matches.push(join(&[dir, &n]));
        }
    }
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_table() {
        let ok = Ok(true);
        let no = Ok(false);
        let bad = Err(BadPattern);
        let cases: Vec<(&str, &str, Result<bool, BadPattern>)> = vec![
            ("abc", "abc", ok),
            ("*", "abc", ok),
            ("*c", "abc", ok),
            ("a*", "a", ok),
            ("a*", "abc", ok),
            ("a*", "ab/c", no),
            ("a*/b", "abc/b", ok),
            ("a*/b", "a/c/b", no),
            ("a*b*c*d*e*/f", "axbxcxdxe/f", ok),
            ("a*b*c*d*e*/f", "axbxcxdxexxx/f", ok),
            ("a*b*c*d*e*/f", "axbxcxdxe/xxx/f", no),
            ("a*b*c*d*e*/f", "axbxcxdxexxx/fff", no),
            ("a*b?c*x", "abxbbxdbxebxczzx", ok),
            ("a*b?c*x", "abxbbxdbxebxczzy", no),
            ("ab[c]", "abc", ok),
            ("ab[b-d]", "abc", ok),
            ("ab[e-g]", "abc", no),
            ("ab[^c]", "abc", no),
            ("ab[^b-d]", "abc", no),
            ("ab[^e-g]", "abc", ok),
            ("a\\*b", "a*b", ok),
            ("a\\*b", "ab", no),
            ("a?b", "a☺b", ok),
            ("a[^a]b", "a☺b", ok),
            ("a???b", "a☺b", no),
            ("a[^a][^a][^a]b", "a☺b", no),
            ("[a-ζ]*", "α", ok),
            ("*[a-ζ]", "A", no),
            ("a?b", "a/b", no),
            ("a*b", "a/b", no),
            ("[\\]a]", "]", ok),
            ("[\\-]", "-", ok),
            ("[x\\-]", "x", ok),
            ("[x\\-]", "-", ok),
            ("[x\\-]", "z", no),
            ("[\\-x]", "x", ok),
            ("[\\-x]", "-", ok),
            ("[\\-x]", "a", no),
            ("[]a]", "]", bad),
            ("[-]", "-", bad),
            ("[x-]", "x", bad),
            ("[x-]", "-", bad),
            ("[x-]", "z", bad),
            ("[-x]", "x", bad),
            ("[-x]", "-", bad),
            ("[-x]", "a", bad),
            ("\\", "a", bad),
            ("[a-b-c]", "a", bad),
            ("[", "a", bad),
            ("[^", "a", bad),
            ("[^bc", "a", bad),
            ("a[", "a", bad),
            ("a[", "ab", bad),
            ("a[", "x", bad),
            ("a/b[", "x", bad),
            ("*x", "xxx", ok),
            // `**` is one `*`: it never crosses a separator.
            ("src/**", "src/a/b.rs", no),
            ("src/**", "src/a", ok),
            ("**/*.rs", "src/a.rs", ok),
            ("**/*.rs", "src/a/b.rs", no),
        ];
        for (pattern, name, want) in cases {
            assert_eq!(
                path_match(pattern, name),
                want,
                "Match({pattern:?}, {name:?})"
            );
        }
    }

    #[test]
    fn clean_and_join_follow_go() {
        for (input, want) in [
            ("abc", "abc"),
            ("abc/def", "abc/def"),
            ("a/b/c", "a/b/c"),
            (".", "."),
            ("..", ".."),
            ("../..", "../.."),
            ("../../abc", "../../abc"),
            ("/abc", "/abc"),
            ("/", "/"),
            ("", "."),
            ("abc/", "abc"),
            ("abc/def/", "abc/def"),
            ("a/b/c/", "a/b/c"),
            ("./", "."),
            ("../", ".."),
            ("../../", "../.."),
            ("/abc/", "/abc"),
            ("abc//def//ghi", "abc/def/ghi"),
            ("//abc", "/abc"),
            ("///abc", "/abc"),
            ("//abc//", "/abc"),
            ("abc//", "abc"),
            ("abc/./def", "abc/def"),
            ("/./abc/def", "/abc/def"),
            ("abc/.", "abc"),
            ("abc/def/ghi/../jkl", "abc/def/jkl"),
            ("abc/def/../ghi/../jkl", "abc/jkl"),
            ("abc/def/..", "abc"),
            ("abc/def/../..", "."),
            ("/abc/def/../..", "/"),
            ("abc/def/../../..", ".."),
            ("/abc/def/../../..", "/"),
            ("abc/def/../../../ghi/jkl/../../../mno", "../../mno"),
            ("/../abc", "/abc"),
            ("abc/./../def", "def"),
            ("abc//./../def", "def"),
            ("abc/../../././../def", "../../def"),
        ] {
            assert_eq!(clean(input), want, "Clean({input:?})");
        }
        for (elem, want) in [
            (vec!["a", "b"], "a/b"),
            (vec!["a", ""], "a"),
            (vec!["", "b"], "b"),
            (vec!["/", "a"], "/a"),
            (vec!["/", "a/b"], "/a/b"),
            (vec!["/", ""], "/"),
            (vec!["//", "a"], "/a"),
            (vec!["/a", "b"], "/a/b"),
            (vec!["a/", "b"], "a/b"),
            (vec!["a/", ""], "a"),
            (vec!["", ""], ""),
            (vec!["/", "a", "b"], "/a/b"),
            (vec![".", "x"], "x"),
        ] {
            assert_eq!(join(&elem), want, "Join({elem:?})");
        }
    }

    #[test]
    fn glob_opens_only_the_named_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap().to_string();
        for p in [
            "src/a.rs",
            "src/b.rs",
            "src/deep/c.rs",
            "docs/x.md",
            ".hidden/y.rs",
        ] {
            let full = dir.path().join(p);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, b"").unwrap();
        }
        let rel = |m: Vec<String>| -> Vec<String> {
            m.into_iter()
                .map(|s| s[root.len() + 1..].to_string())
                .collect()
        };
        assert_eq!(
            rel(filepath_glob(&join(&[&root, "src/*.rs"])).unwrap()),
            vec!["src/a.rs", "src/b.rs"]
        );
        // `**` is one segment: it does not descend.
        assert_eq!(
            rel(filepath_glob(&join(&[&root, "src/**/*.rs"])).unwrap()),
            vec!["src/deep/c.rs"]
        );
        assert_eq!(
            rel(filepath_glob(&join(&[&root, "*/[ab].rs"])).unwrap()),
            vec!["src/a.rs", "src/b.rs"]
        );
        // Directory listings include dotfiles; a literal pattern is a plain
        // existence check.
        assert_eq!(
            rel(filepath_glob(&join(&[&root, "*/y.rs"])).unwrap()),
            vec![".hidden/y.rs"]
        );
        assert_eq!(
            rel(filepath_glob(&join(&[&root, "docs/x.md"])).unwrap()),
            vec!["docs/x.md"]
        );
        assert!(
            filepath_glob(&join(&[&root, "docs/missing.md"]))
                .unwrap()
                .is_empty()
        );
        assert!(
            filepath_glob(&join(&[&root, "nowhere/*"]))
                .unwrap()
                .is_empty()
        );
        assert_eq!(filepath_glob(&join(&[&root, "[]a]"])), Err(BadPattern));
    }
}
