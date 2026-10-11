//! Lexical path cleaning and relativisation with Unix separators, used by
//! the containment checks (`within`, `is_inside`) and by paths stored as
//! text; `std::path` neither collapses `..` nor rejects mixing an absolute
//! path with a relative one.
//!
//! `clean` applies these rules in order until none applies: collapse
//! repeated separators, drop `.` elements, drop `..` together with the
//! element before it, drop leading `..` from a rooted path; the result has
//! no trailing separator, and an empty result becomes `.`. `rel` returns the
//! cleaned path that leads from `basepath` to `targpath` (`.` when equal),
//! failing when exactly one argument is rooted or when the first element of
//! `basepath` that `targpath` does not share is `..` (the answer would then
//! depend on the working directory).

pub fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let p = path.as_bytes();
    let rooted = p[0] == b'/';
    let n = p.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut r = 0usize;
    let mut dotdot = 0usize;
    if rooted {
        out.push(b'/');
        r = 1;
        dotdot = 1;
    }
    while r < n {
        if p[r] == b'/' || (p[r] == b'.' && (r + 1 == n || p[r + 1] == b'/')) {
            // A separator or a "." element adds nothing.
            r += 1;
        } else if p[r] == b'.' && p[r + 1] == b'.' && (r + 2 == n || p[r + 2] == b'/') {
            r += 2;
            if out.len() > dotdot {
                // Back up over the previous element: pop its bytes until the
                // separator before it goes too (or the root/`..` prefix is hit).
                let mut dropped = out.pop();
                while out.len() > dotdot && dropped != Some(b'/') {
                    dropped = out.pop();
                }
            } else if !rooted {
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(b'/');
            }
            while r < n && p[r] != b'/' {
                out.push(p[r]);
                r += 1;
            }
        }
    }
    if out.is_empty() {
        return ".".to_string();
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Joins a relative path onto the working directory and cleans it; the
/// path need not exist.
pub fn abs(path: &str) -> std::io::Result<String> {
    if path.starts_with('/') {
        return Ok(clean(path));
    }
    let cwd = std::env::current_dir()?;
    Ok(clean(&format!("{}/{}", cwd.display(), path)))
}

pub fn rel(basepath: &str, targpath: &str) -> Result<String, String> {
    let base = clean(basepath);
    let targ = clean(targpath);
    if targ == base {
        return Ok(".".to_string());
    }
    let base = if base == "." { String::new() } else { base };
    let base_slashed = base.starts_with('/');
    let targ_slashed = targ.starts_with('/');
    let cannot = || Err(format!("Rel: can't make {targpath} relative to {basepath}"));
    if base_slashed != targ_slashed {
        return cannot();
    }
    let b = base.as_bytes();
    let t = targ.as_bytes();
    let (bl, tl) = (b.len(), t.len());
    let (mut b0, mut bi, mut t0, mut ti) = (0usize, 0usize, 0usize, 0usize);
    loop {
        while bi < bl && b[bi] != b'/' {
            bi += 1;
        }
        while ti < tl && t[ti] != b'/' {
            ti += 1;
        }
        if t[t0..ti] != b[b0..bi] {
            break;
        }
        if bi < bl {
            bi += 1;
        }
        if ti < tl {
            ti += 1;
        }
        b0 = bi;
        t0 = ti;
    }
    if &b[b0..bi] == b".." {
        return cannot();
    }
    if b0 != bl {
        let seps = b[b0..bl].iter().filter(|c| **c == b'/').count();
        let mut buf = String::from("..");
        for _ in 0..seps {
            buf.push_str("/..");
        }
        if t0 != tl {
            buf.push('/');
            buf.push_str(&targ[t0..]);
        }
        return Ok(buf);
    }
    Ok(targ[t0..].to_string())
}

/// Whether `child` is `parent` or below it (lexically, after cleaning).
pub fn within(child: &str, parent: &str) -> bool {
    match rel(parent, child) {
        Ok(r) => r != ".." && !r.starts_with("../") && !r.starts_with('/'),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_matches_go() {
        let _serial = crate::test_helpers::serial();
        for (input, want) in [
            ("", "."),
            ("abc", "abc"),
            ("abc/def", "abc/def"),
            ("a/b/c", "a/b/c"),
            (".", "."),
            ("..", ".."),
            ("../..", "../.."),
            ("../../abc", "../../abc"),
            ("/abc", "/abc"),
            ("/", "/"),
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
            ("a/../b:/../../c", "../c"),
            ("abc/./../def", "def"),
            ("abc//./../def", "def"),
            ("abc/../../././../def", "../../def"),
            ("a/../.git", ".git"),
        ] {
            assert_eq!(clean(input), want, "Clean({input:?})");
        }
    }

    #[test]
    fn rel_matches_go() {
        let _serial = crate::test_helpers::serial();
        for (root, path, want) in [
            ("a/b", "a/b", "."),
            ("a/b/.", "a/b", "."),
            ("a/b", "a/b/.", "."),
            ("./a/b", "a/b", "."),
            ("a/b", "./a/b", "."),
            ("ab/cd", "ab/cde", "../cde"),
            ("ab/cd", "ab/c", "../c"),
            ("a/b", "a/b/c/d", "c/d"),
            ("a/b", "a/b/../c", "../c"),
            ("a/b/../c", "a/b", "../b"),
            ("a/b/c", "a/c/d", "../../c/d"),
            ("a/b", "c/d", "../../c/d"),
            ("a/b/c/d", "a/b", "../.."),
            ("a/b/c/d", "a/b/", "../.."),
            ("a/b/c/d/", "a/b", "../.."),
            ("a/b/c/d/", "a/b/", "../.."),
            ("../../a/b", "../../a/b/c/d", "c/d"),
            ("/a/b", "/a/b", "."),
            ("/a/b/.", "/a/b", "."),
            ("/a/b", "/a/b/.", "."),
            ("/ab/cd", "/ab/cde", "../cde"),
            ("/ab/cd", "/ab/c", "../c"),
            ("/a/b", "/a/b/c/d", "c/d"),
            ("/a/b", "/a/b/../c", "../c"),
            ("/a/b/../c", "/a/b", "../b"),
            ("/a/b/c", "/a/c/d", "../../c/d"),
            ("/a/b", "/c/d", "../../c/d"),
            ("/a/b/c/d", "/a/b", "../.."),
            ("/a/b/c/d", "/a/b/", "../.."),
            ("/a/b/c/d/", "/a/b", "../.."),
            ("/a/b/c/d/", "/a/b/", "../.."),
            ("/../../a/b", "/../../a/b/c/d", "c/d"),
            (".", "a/b", "a/b"),
            (".", "..", ".."),
        ] {
            assert_eq!(
                rel(root, path).as_deref(),
                Ok(want),
                "Rel({root:?}, {path:?})"
            );
        }
        for (root, path) in [("..", "."), ("../..", ".."), ("a", "/a"), ("/a", "a")] {
            assert_eq!(
                rel(root, path),
                Err(format!("Rel: can't make {path} relative to {root}")),
                "Rel({root:?}, {path:?})"
            );
        }
    }

    #[test]
    fn within_follows_rel() {
        let _serial = crate::test_helpers::serial();
        assert!(within("/a/b/c", "/a/b"));
        assert!(within("/a/b", "/a/b"));
        assert!(within("/a/b/../b/c", "/a/b"));
        assert!(!within("/a/bc", "/a/b"));
        assert!(!within("/a", "/a/b"));
        assert!(!within("/a/b/..", "/a/b"));
        assert!(!within("a/b", "/a"));
        assert!(!within("/a/b", "a"));
    }
}
