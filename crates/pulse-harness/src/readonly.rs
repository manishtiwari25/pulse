//! Read-only runs: the commands a run that may look but not touch refuses
//! before they start, and the guard that puts the clone back when one
//! changes it anyway.
//!
//! "Read-only" is about the clone, not the machine: a scratch file under
//! the temp directory, a dependency install into an ignored directory and
//! a test run are all fine; `git commit`, `prettier --write`, `> notes.md`
//! and `npm install left-pad` are not. The checker reads the command the
//! way `/bin/sh` would (quotes, `&&`, pipes, redirections, `$(...)`,
//! heredocs, `sh -c`, `xargs`, `sudo`) and names the first thing that would
//! change the clone. What it cannot see, say a Python one-liner that opens
//! a file for writing, [`TreeSnapshot`] catches afterwards.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Why `command` may not run in a read-only run, or None when it may.
pub fn read_only_refusal(command: &str) -> Option<String> {
    let mut state = ShellState::default();
    refusal_in(command, &mut state)
}

fn refusal_in(command: &str, state: &mut ShellState) -> Option<String> {
    let items = tokenize(command);
    let mut words: Vec<String> = Vec::new();
    let mut redirects: Vec<String> = Vec::new();
    for item in items {
        match item {
            Item::Word(w) => words.push(w),
            Item::Redirect(target) => redirects.push(target),
            Item::Nested(inner) => {
                // The output of `$(...)` is text in the surrounding word;
                // what runs inside is a command of its own.
                let saved = state.clone();
                let found = refusal_in(&inner, state);
                *state = saved;
                if found.is_some() {
                    return found;
                }
            }
            Item::Sep => {
                let found = check_segment(
                    std::mem::take(&mut words),
                    std::mem::take(&mut redirects),
                    state,
                );
                if found.is_some() {
                    return found;
                }
            }
            Item::Open => {
                let found = check_segment(
                    std::mem::take(&mut words),
                    std::mem::take(&mut redirects),
                    state,
                );
                if found.is_some() {
                    return found;
                }
                state.stack.push(state.in_temp);
            }
            Item::Close => {
                let found = check_segment(
                    std::mem::take(&mut words),
                    std::mem::take(&mut redirects),
                    state,
                );
                if found.is_some() {
                    return found;
                }
                if let Some(saved) = state.stack.pop() {
                    state.in_temp = saved;
                }
            }
        }
    }
    check_segment(words, redirects, state)
}

/// What the checker tracks across the segments of one command line: whether
/// `cd` has taken the shell into the temp directory, where writes are fine,
/// with a stack for `( ... )` subshells whose `cd` does not last.
#[derive(Clone, Debug, Default)]
struct ShellState {
    in_temp: bool,
    stack: Vec<bool>,
}

#[derive(Debug, PartialEq, Eq)]
enum Item {
    Word(String),
    /// The target of an output redirection: a path, `&N` or `&-`.
    Redirect(String),
    /// The inside of `$(...)`, backticks or `<(...)`/`>(...)`.
    Nested(String),
    /// `;`, `&&`, `||`, `|`, `&`, newline.
    Sep,
    Open,
    Close,
}

/// Splits a `/bin/sh` command line into words, separators and redirection
/// targets, dropping quotes. Heredoc bodies are skipped: a `>` inside one
/// is text, not a redirection.
fn tokenize(command: &str) -> Vec<Item> {
    let chars: Vec<char> = command.chars().collect();
    let mut items = Vec::new();
    let mut word: Option<String> = None;
    let mut redirect_pending = false;
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let mut i = 0;

    fn push_word(
        items: &mut Vec<Item>,
        word: &mut Option<String>,
        redirect_pending: &mut bool,
        heredoc_delim: &mut Option<(bool, bool)>,
        heredocs: &mut Vec<(String, bool)>,
    ) {
        if let Some(w) = word.take() {
            if let Some((strip_tabs, _)) = heredoc_delim.take() {
                heredocs.push((w, strip_tabs));
            } else if *redirect_pending {
                *redirect_pending = false;
                items.push(Item::Redirect(w));
            } else {
                items.push(Item::Word(w));
            }
        }
    }

    // Some((strip_tabs, _)) while the next word names a heredoc delimiter.
    let mut heredoc_delim: Option<(bool, bool)> = None;

    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' => {
                push_word(
                    &mut items,
                    &mut word,
                    &mut redirect_pending,
                    &mut heredoc_delim,
                    &mut heredocs,
                );
                i += 1;
            }
            '\n' => {
                push_word(
                    &mut items,
                    &mut word,
                    &mut redirect_pending,
                    &mut heredoc_delim,
                    &mut heredocs,
                );
                items.push(Item::Sep);
                i += 1;
                for (delim, strip_tabs) in heredocs.drain(..) {
                    i = skip_heredoc(&chars, i, &delim, strip_tabs);
                }
            }
            '\\' => {
                if i + 1 < chars.len() {
                    if chars[i + 1] == '\n' {
                        i += 2;
                        continue;
                    }
                    word.get_or_insert_with(String::new).push(chars[i + 1]);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            '\'' => {
                let w = word.get_or_insert_with(String::new);
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    w.push(chars[i]);
                    i += 1;
                }
                i += 1;
            }
            '"' => {
                let mut w = word.take().unwrap_or_default();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    match chars[i] {
                        '\\' if i + 1 < chars.len() => {
                            let n = chars[i + 1];
                            if matches!(n, '"' | '$' | '`' | '\\') {
                                w.push(n);
                            } else if n != '\n' {
                                w.push('\\');
                                w.push(n);
                            }
                            i += 2;
                        }
                        '`' => {
                            let (inner, next) = read_until_backtick(&chars, i + 1);
                            items.push(Item::Nested(inner));
                            w.push_str("$(...)");
                            i = next;
                        }
                        '$' if i + 1 < chars.len() && chars[i + 1] == '(' => {
                            let (inner, next) = read_parenthesised(&chars, i + 2);
                            if inner.starts_with('(') {
                                w.push_str("$(...)");
                            } else {
                                items.push(Item::Nested(inner));
                                w.push_str("$(...)");
                            }
                            i = next;
                        }
                        other => {
                            w.push(other);
                            i += 1;
                        }
                    }
                }
                word = Some(w);
                i += 1;
            }
            '`' => {
                let (inner, next) = read_until_backtick(&chars, i + 1);
                items.push(Item::Nested(inner));
                word.get_or_insert_with(String::new).push_str("$(...)");
                i = next;
            }
            '$' if i + 1 < chars.len() && chars[i + 1] == '(' => {
                let (inner, next) = read_parenthesised(&chars, i + 2);
                let w = word.get_or_insert_with(String::new);
                if !inner.starts_with('(') {
                    items.push(Item::Nested(inner));
                }
                w.push_str("$(...)");
                i = next;
            }
            '$' if i + 1 < chars.len() && chars[i + 1] == '{' => {
                let w = word.get_or_insert_with(String::new);
                while i < chars.len() && chars[i] != '}' {
                    w.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() {
                    w.push('}');
                    i += 1;
                }
            }
            ';' => {
                push_word(
                    &mut items,
                    &mut word,
                    &mut redirect_pending,
                    &mut heredoc_delim,
                    &mut heredocs,
                );
                items.push(Item::Sep);
                i += 1;
                while i < chars.len() && chars[i] == ';' {
                    i += 1;
                }
            }
            '&' => {
                if i + 1 < chars.len() && chars[i + 1] == '>' {
                    // `&>file` and `&>>file`: both streams to a file.
                    push_word(
                        &mut items,
                        &mut word,
                        &mut redirect_pending,
                        &mut heredoc_delim,
                        &mut heredocs,
                    );
                    i += 2;
                    if i < chars.len() && chars[i] == '>' {
                        i += 1;
                    }
                    redirect_pending = true;
                } else {
                    push_word(
                        &mut items,
                        &mut word,
                        &mut redirect_pending,
                        &mut heredoc_delim,
                        &mut heredocs,
                    );
                    items.push(Item::Sep);
                    i += 1;
                    if i < chars.len() && chars[i] == '&' {
                        i += 1;
                    }
                }
            }
            '|' => {
                push_word(
                    &mut items,
                    &mut word,
                    &mut redirect_pending,
                    &mut heredoc_delim,
                    &mut heredocs,
                );
                items.push(Item::Sep);
                i += 1;
                if i < chars.len() && matches!(chars[i], '|' | '&') {
                    i += 1;
                }
            }
            '(' => {
                push_word(
                    &mut items,
                    &mut word,
                    &mut redirect_pending,
                    &mut heredoc_delim,
                    &mut heredocs,
                );
                items.push(Item::Open);
                i += 1;
            }
            ')' => {
                push_word(
                    &mut items,
                    &mut word,
                    &mut redirect_pending,
                    &mut heredoc_delim,
                    &mut heredocs,
                );
                items.push(Item::Close);
                i += 1;
            }
            '{' | '}' if word.is_none() && standalone_brace(&chars, i) => {
                // `{ a; b; }` groups run in the same shell: no scoping.
                i += 1;
            }
            '<' => {
                push_word(
                    &mut items,
                    &mut word,
                    &mut redirect_pending,
                    &mut heredoc_delim,
                    &mut heredocs,
                );
                if i + 2 < chars.len() && chars[i + 1] == '<' && chars[i + 2] == '<' {
                    // Here-string: the next word is data.
                    i += 3;
                    skip_data_word(&chars, &mut i);
                } else if i + 1 < chars.len() && chars[i + 1] == '<' {
                    i += 2;
                    let strip_tabs = i < chars.len() && chars[i] == '-';
                    if strip_tabs {
                        i += 1;
                    }
                    heredoc_delim = Some((strip_tabs, true));
                } else if i + 1 < chars.len() && chars[i + 1] == '(' {
                    let (inner, next) = read_parenthesised(&chars, i + 2);
                    items.push(Item::Nested(inner));
                    word.get_or_insert_with(String::new).push_str("<(...)");
                    i = next;
                } else {
                    // Input redirection: the next word is a file to read.
                    i += 1;
                    skip_data_word(&chars, &mut i);
                }
            }
            '>' => {
                // A word of digits right before `>` is the descriptor.
                if word
                    .as_deref()
                    .is_some_and(|w| w.chars().all(|c| c.is_ascii_digit()))
                {
                    word = None;
                }
                push_word(
                    &mut items,
                    &mut word,
                    &mut redirect_pending,
                    &mut heredoc_delim,
                    &mut heredocs,
                );
                i += 1;
                if i < chars.len() && chars[i] == '(' {
                    let (inner, next) = read_parenthesised(&chars, i + 1);
                    items.push(Item::Nested(inner));
                    word.get_or_insert_with(String::new).push_str(">(...)");
                    i = next;
                    continue;
                }
                if i < chars.len() && matches!(chars[i], '>' | '|') {
                    i += 1;
                }
                if i < chars.len() && chars[i] == '&' {
                    i += 1;
                    let mut dup = String::from("&");
                    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '-') {
                        dup.push(chars[i]);
                        i += 1;
                    }
                    if dup.len() > 1 {
                        items.push(Item::Redirect(dup));
                        continue;
                    }
                }
                redirect_pending = true;
            }
            _ => {
                word.get_or_insert_with(String::new).push(c);
                i += 1;
            }
        }
    }
    push_word(
        &mut items,
        &mut word,
        &mut redirect_pending,
        &mut heredoc_delim,
        &mut heredocs,
    );
    items
}

fn standalone_brace(chars: &[char], i: usize) -> bool {
    let before_ok = i == 0 || matches!(chars[i - 1], ' ' | '\t' | '\n' | ';' | '|' | '&' | '(');
    let after_ok = i + 1 >= chars.len() || matches!(chars[i + 1], ' ' | '\t' | '\n' | ';' | '&');
    before_ok && after_ok
}

/// Skips one word that is data, not a command (an input redirection's file
/// or a here-string), quotes included.
fn skip_data_word(chars: &[char], i: &mut usize) {
    while *i < chars.len() && matches!(chars[*i], ' ' | '\t') {
        *i += 1;
    }
    while *i < chars.len() {
        match chars[*i] {
            ' ' | '\t' | '\n' | ';' | '|' | '&' | '>' | '<' | '(' | ')' => return,
            '\'' => {
                *i += 1;
                while *i < chars.len() && chars[*i] != '\'' {
                    *i += 1;
                }
                *i += 1;
            }
            '"' => {
                *i += 1;
                while *i < chars.len() && chars[*i] != '"' {
                    if chars[*i] == '\\' {
                        *i += 1;
                    }
                    *i += 1;
                }
                *i += 1;
            }
            '\\' => *i += 2,
            _ => *i += 1,
        }
    }
}

/// Skips the lines of a heredoc body up to and including the delimiter
/// line; returns the index after it.
fn skip_heredoc(chars: &[char], mut i: usize, delim: &str, strip_tabs: bool) -> usize {
    while i < chars.len() {
        let end = chars[i..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(chars.len(), |p| i + p);
        let line: String = chars[i..end].iter().collect();
        let line = if strip_tabs {
            line.trim_start_matches('\t')
        } else {
            &line
        };
        i = (end + 1).min(chars.len());
        if line == delim {
            return i;
        }
    }
    i
}

/// Reads up to the backtick that closes a `` `...` `` substitution; returns
/// the inside and the index after the closing backtick.
fn read_until_backtick(chars: &[char], mut i: usize) -> (String, usize) {
    let mut inner = String::new();
    while i < chars.len() && chars[i] != '`' {
        if chars[i] == '\\' && i + 1 < chars.len() {
            inner.push(chars[i + 1]);
            i += 2;
            continue;
        }
        inner.push(chars[i]);
        i += 1;
    }
    (inner, (i + 1).min(chars.len()))
}

/// Reads up to the parenthesis that closes a `$(`, `<(` or `>(`; `start`
/// is the index after the opening one. Returns the inside and the index
/// after the closing parenthesis.
fn read_parenthesised(chars: &[char], start: usize) -> (String, usize) {
    let mut depth = 1;
    let mut i = start;
    let mut inner = String::new();
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() => {
                inner.push(c);
                inner.push(chars[i + 1]);
                i += 2;
                continue;
            }
            '\'' => {
                inner.push(c);
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    inner.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() {
                    inner.push('\'');
                }
                i += 1;
                continue;
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return (inner, i + 1);
                }
            }
            _ => {}
        }
        inner.push(c);
        i += 1;
    }
    (inner, i)
}

/// Checks one simple command (the words between two separators) with its
/// output redirections.
fn check_segment(
    mut words: Vec<String>,
    redirects: Vec<String>,
    state: &mut ShellState,
) -> Option<String> {
    for target in &redirects {
        if !target_ok(target, state.in_temp) {
            return Some(format!(
                "`> {target}` writes a file inside the clone (scratch output goes under /tmp or to /dev/null)"
            ));
        }
    }
    strip_assignments(&mut words);
    // After `xargs`, the program's targets arrive on stdin: a writer with
    // no target of its own is then writing wherever the pipeline says.
    let mut from_stdin = false;
    loop {
        if words.is_empty() {
            return None;
        }
        let prog = basename(&words[0]).to_string();
        match prog.as_str() {
            "sudo" | "doas" => {
                words.remove(0);
                strip_flags(
                    &mut words,
                    &["-u", "-g", "-C", "-h", "-p", "-r", "-t", "-U"],
                );
            }
            "env" => {
                words.remove(0);
                strip_flags(&mut words, &["-u", "-S", "--unset", "-C", "--chdir"]);
                strip_assignments(&mut words);
            }
            "nice" => {
                words.remove(0);
                strip_flags(&mut words, &["-n", "--adjustment"]);
            }
            "nohup" | "time" | "command" | "builtin" | "exec" | "unbuffer" | "noglob" => {
                words.remove(0);
                strip_flags(&mut words, &[]);
            }
            "caffeinate" => {
                words.remove(0);
                strip_flags(&mut words, &["-t", "-w"]);
            }
            "ionice" => {
                words.remove(0);
                strip_flags(&mut words, &["-c", "-n", "-p"]);
            }
            "stdbuf" => {
                words.remove(0);
                strip_flags(&mut words, &["-i", "-o", "-e"]);
            }
            "arch" => {
                words.remove(0);
                strip_flags(&mut words, &["-arch"]);
            }
            "timeout" => {
                words.remove(0);
                strip_flags(&mut words, &["-k", "-s", "--kill-after", "--signal"]);
                if !words.is_empty() {
                    words.remove(0); // the duration
                }
            }
            "xargs" => {
                words.remove(0);
                strip_flags(
                    &mut words,
                    &[
                        "-n", "-I", "-i", "-P", "-d", "-s", "-L", "-E", "-a", "-e", "-R", "-S",
                    ],
                );
                if words.is_empty() {
                    return None;
                }
                from_stdin = true;
            }
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" => {
                let flags: Vec<String> = words[1..]
                    .iter()
                    .take_while(|w| w.starts_with('-') && w.len() > 1)
                    .cloned()
                    .collect();
                let runs_string = flags
                    .iter()
                    .any(|f| !f.starts_with("--") && f.contains('c'));
                if !runs_string {
                    return None;
                }
                return words
                    .get(1 + flags.len())
                    .and_then(|script| refusal_in(script, state));
            }
            "eval" => {
                let script = words[1..].join(" ");
                return refusal_in(&script, state);
            }
            "find" => return find_refusal(&words[1..], state),
            "npx" | "bunx" | "pnpx" | "uvx" => {
                words.remove(0);
                strip_flags(
                    &mut words,
                    &["-p", "--package", "-c", "--call", "--from", "--with"],
                );
            }
            "pipx" if words.get(1).is_some_and(|w| w == "run") => {
                words.drain(..2);
                strip_flags(&mut words, &["--spec", "--python"]);
            }
            "pnpm"
                if words
                    .get(1)
                    .is_some_and(|w| matches!(w.as_str(), "dlx" | "exec")) =>
            {
                words.drain(..2);
                strip_flags(&mut words, &["-p", "--package", "-C", "--dir"]);
            }
            "yarn"
                if words
                    .get(1)
                    .is_some_and(|w| matches!(w.as_str(), "dlx" | "exec" | "run")) =>
            {
                words.drain(..2);
                strip_flags(&mut words, &["-p", "--package"]);
            }
            "npm"
                if words
                    .get(1)
                    .is_some_and(|w| matches!(w.as_str(), "exec" | "x")) =>
            {
                words.drain(..2);
                strip_flags(
                    &mut words,
                    &["-p", "--package", "-c", "--call", "-w", "--workspace"],
                );
            }
            "bun"
                if words
                    .get(1)
                    .is_some_and(|w| matches!(w.as_str(), "x" | "run")) =>
            {
                words.drain(..2);
                strip_flags(&mut words, &["-p", "--package"]);
            }
            "uv" if words.get(1).is_some_and(|w| w == "run") => {
                words.drain(..2);
                strip_flags(
                    &mut words,
                    &[
                        "--with",
                        "--python",
                        "-p",
                        "--extra",
                        "--group",
                        "--env-file",
                        "--project",
                        "--directory",
                        "--only-group",
                        "--with-requirements",
                        "--package",
                        "--index",
                        "--default-index",
                        "--find-links",
                        "-f",
                        "--config-file",
                        "--cache-dir",
                        "--color",
                        "--exclude-newer",
                        "--python-preference",
                        "--link-mode",
                        "--resolution",
                        "--prerelease",
                        "--module",
                        "-m",
                    ],
                );
            }
            "poetry" | "pdm" | "hatch" | "rye" if words.get(1).is_some_and(|w| w == "run") => {
                words.drain(..2);
                strip_flags(
                    &mut words,
                    &["-C", "--directory", "-P", "--project", "-p", "--python"],
                );
            }
            "bundle" if words.get(1).is_some_and(|w| w == "exec") => {
                words.drain(..2);
                strip_flags(&mut words, &[]);
            }
            "mise"
                if words
                    .get(1)
                    .is_some_and(|w| matches!(w.as_str(), "exec" | "x")) =>
            {
                words.drain(..2);
                strip_flags(&mut words, &["-C", "--cd", "-E", "--env", "-j", "--jobs"]);
            }
            "asdf" if words.get(1).is_some_and(|w| w == "exec") => {
                words.drain(..2);
            }
            "direnv" if words.get(1).is_some_and(|w| w == "exec") => {
                words.drain(..2);
                if !words.is_empty() {
                    words.remove(0); // the directory
                }
            }
            "nix" if words.get(1).is_some_and(|w| w == "develop") => {
                let pos = words.iter().position(|w| w == "-c" || w == "--command")?;
                words.drain(..=pos);
            }
            "nix-shell" => {
                let pos = words
                    .iter()
                    .position(|w| w == "--run" || w == "--command")?;
                return words.get(pos + 1).and_then(|s| refusal_in(s, state));
            }
            "cd" | "pushd" | "popd" => {
                change_dir(&words, state);
                return None;
            }
            _ => break,
        }
    }
    if words.is_empty() {
        return None;
    }
    let prog = basename(&words[0]).to_string();
    let args = &words[1..];
    let in_temp = state.in_temp;
    if from_stdin && !in_temp {
        let writer = matches!(
            prog.as_str(),
            "rm" | "rmdir"
                | "mkdir"
                | "touch"
                | "truncate"
                | "shred"
                | "unlink"
                | "chmod"
                | "chown"
                | "chgrp"
                | "chflags"
                | "cp"
                | "mv"
                | "ln"
                | "install"
                | "gzip"
                | "gunzip"
                | "bzip2"
                | "xz"
                | "zstd"
        );
        let explicit = positionals(args, &[]);
        let target_given = match prog.as_str() {
            "cp" | "mv" | "ln" | "install" => {
                flag_value(args, &["-t", "--target-directory"]).is_some() || !explicit.is_empty()
            }
            "chmod" | "chown" | "chgrp" | "chflags" => explicit.len() > 1,
            _ => !explicit.is_empty(),
        };
        if writer && !target_given {
            return Some(format!(
                "`xargs {prog}` changes the paths the pipeline feeds it, inside the clone"
            ));
        }
    }
    match prog.as_str() {
        "git" => git_refusal(args, in_temp),
        "rm" | "rmdir" | "mkdir" | "touch" | "truncate" | "shred" | "unlink" | "mkfifo"
        | "mknod" | "tee" => {
            let targets = positionals(args, &["-s", "--size", "-r", "--reference"]);
            if prog == "tee" && targets.is_empty() {
                return None;
            }
            all_targets_ok(&prog, &targets, in_temp)
        }
        "chmod" | "chown" | "chgrp" | "chflags" => {
            let targets = positionals(args, &["--reference"]);
            all_targets_ok(&prog, targets.get(1..).unwrap_or(&[]), in_temp)
        }
        "cp" | "mv" | "install" | "ln" | "rsync" | "scp" | "ditto" => {
            let value_flags: &[&str] = match prog.as_str() {
                "install" => &[
                    "-m",
                    "-o",
                    "-g",
                    "-b",
                    "-B",
                    "-S",
                    "-t",
                    "--target-directory",
                ],
                "rsync" => &[
                    "-e",
                    "--rsh",
                    "--exclude",
                    "--include",
                    "--filter",
                    "--exclude-from",
                    "--include-from",
                    "--files-from",
                    "--chmod",
                    "--log-file",
                    "--password-file",
                    "--timeout",
                    "--bwlimit",
                    "--max-size",
                    "--min-size",
                    "--partial-dir",
                    "--temp-dir",
                    "-T",
                    "--backup-dir",
                    "--suffix",
                    "--compare-dest",
                    "--copy-dest",
                    "--link-dest",
                    "--out-format",
                    "--info",
                    "--debug",
                    "-M",
                    "--remote-option",
                    "--iconv",
                    "--block-size",
                    "-B",
                    "--checksum-choice",
                    "--compress-choice",
                    "--compress-level",
                    "--skip-compress",
                    "--usermap",
                    "--groupmap",
                    "--chown",
                    "--rsync-path",
                    "--sockopts",
                    "--address",
                    "--port",
                    "--protocol",
                    "--outbuf",
                    "--write-batch",
                    "--only-write-batch",
                    "--read-batch",
                    "--contimeout",
                    "--modify-window",
                    "--mkpath",
                ],
                "scp" => &["-c", "-F", "-i", "-J", "-l", "-o", "-P", "-S", "-D"],
                "ln" => &["-t", "--target-directory", "-S", "--suffix"],
                _ => &["-t", "--target-directory", "-S", "--suffix", "--context"],
            };
            if let Some(dir) = flag_value(args, &["-t", "--target-directory"]) {
                return all_targets_ok(&prog, &[dir], in_temp);
            }
            if prog == "install" && args.iter().any(|a| a == "-d" || a == "--directory") {
                let targets = positionals(args, value_flags);
                return all_targets_ok(&prog, &targets, in_temp);
            }
            let targets = positionals(args, value_flags);
            match targets.last() {
                Some(dest) => all_targets_ok(&prog, std::slice::from_ref(dest), in_temp),
                None => None,
            }
        }
        "dd" => {
            let out = args
                .iter()
                .find_map(|a| a.strip_prefix("of="))
                .map(str::to_string)?;
            all_targets_ok(&prog, &[out], in_temp)
        }
        "patch" => {
            if args
                .iter()
                .any(|a| matches!(a.as_str(), "--dry-run" | "-C" | "--check"))
            {
                return None;
            }
            Some(
                "`patch` rewrites files in the clone (use --dry-run to see what it would do)"
                    .into(),
            )
        }
        "sort" | "shuf" => {
            let out = flag_value(args, &["-o", "--output"])?;
            all_targets_ok(&prog, &[out], in_temp)
        }
        "uniq" => {
            let targets = positionals(
                args,
                &[
                    "-f",
                    "-s",
                    "-w",
                    "--skip-fields",
                    "--skip-chars",
                    "--check-chars",
                ],
            );
            match targets.get(1) {
                Some(out) => all_targets_ok(&prog, std::slice::from_ref(out), in_temp),
                None => None,
            }
        }
        "split" | "csplit" => {
            let targets = positionals(
                args,
                &[
                    "-l",
                    "-b",
                    "-C",
                    "-n",
                    "-a",
                    "-d",
                    "--lines",
                    "--bytes",
                    "--line-bytes",
                    "--number",
                    "--suffix-length",
                    "--additional-suffix",
                    "-f",
                    "-b",
                    "--prefix",
                    "--suffix-format",
                ],
            );
            let prefix = flag_value(args, &["-f", "--prefix"]).or_else(|| targets.get(1).cloned());
            match prefix {
                Some(p) => all_targets_ok(&prog, &[p], in_temp),
                None if in_temp => None,
                None => Some(format!(
                    "`{prog}` writes its pieces into the working directory"
                )),
            }
        }
        "mktemp" => {
            if let Some(dir) = flag_value(args, &["-p", "--tmpdir"]) {
                return all_targets_ok(&prog, &[dir], in_temp);
            }
            let targets = positionals(args, &["-p", "--tmpdir", "--suffix"]);
            match targets.first() {
                Some(t) if t.contains('/') => all_targets_ok(&prog, std::slice::from_ref(t), in_temp),
                Some(_) if !in_temp => Some(
                    "`mktemp NAME` creates the file in the working directory (use plain mktemp or -p /tmp)".into(),
                ),
                _ => None,
            }
        }
        "sed" => {
            let has_script_flag = args.iter().any(|a| {
                a == "-e" || a == "-f" || a.starts_with("--expression") || a.starts_with("--file")
            });
            if !sed_in_place(args) {
                return None;
            }
            let mut files = positionals(
                args,
                &["-e", "-f", "--expression", "--file", "-l", "--line-length"],
            );
            if !has_script_flag && !files.is_empty() {
                files.remove(0);
            }
            if files.is_empty() {
                return Some("`sed -i` rewrites files in place".into());
            }
            all_targets_ok("sed -i", &files, in_temp)
        }
        "perl" => {
            let in_place = args
                .iter()
                .take_while(|a| a.starts_with('-'))
                .any(|a| cluster_letters(a, &['e', 'E', 'M', 'm', 'I', 'x']).contains(&'i'));
            if in_place {
                Some("`perl -i` rewrites files in place".into())
            } else {
                None
            }
        }
        "ruby" => {
            let in_place = args.iter().take_while(|a| a.starts_with('-')).any(|a| {
                cluster_letters(a, &['e', 'r', 'I', 'C', 'E', 'F', 'K', 'T', 'W', 'x'])
                    .contains(&'i')
            });
            if in_place {
                Some("`ruby -i` rewrites files in place".into())
            } else {
                None
            }
        }
        "awk" | "gawk" => {
            let in_place = args
                .iter()
                .any(|a| a == "--in-place" || a.starts_with("--in-place="))
                || args.windows(2).any(|w| w[0] == "-i" && w[1] == "inplace");
            if in_place {
                Some("`awk -i inplace` rewrites files in place".into())
            } else {
                None
            }
        }
        "tar" => tar_refusal(args, in_temp),
        "zip" | "7z" | "7za" | "7zz" => {
            if prog != "zip"
                && args
                    .first()
                    .is_some_and(|m| matches!(m.as_str(), "l" | "t" | "h" | "i" | "b"))
            {
                return None;
            }
            let targets = positionals(args, &["-x", "-i", "-b", "-t", "-tt", "-n", "-z", "-Z"]);
            match targets.first() {
                Some(archive) => all_targets_ok(&prog, std::slice::from_ref(archive), in_temp),
                None => None,
            }
        }
        "unzip" => {
            if args
                .iter()
                .any(|a| matches!(a.as_str(), "-l" | "-p" | "-c" | "-t" | "-z" | "-Z" | "-v"))
            {
                return None;
            }
            if let Some(dir) = flag_value(args, &["-d"]) {
                return all_targets_ok(&prog, &[dir], in_temp);
            }
            if in_temp {
                None
            } else {
                Some("`unzip` extracts into the working directory (use -d /tmp/... or -l)".into())
            }
        }
        "gzip" | "gunzip" | "bzip2" | "bunzip2" | "xz" | "unxz" | "zstd" | "unzstd" | "lz4"
        | "brotli" => {
            let to_stdout = args.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "--stdout" | "--to-stdout" | "--test" | "--list" | "--keep"
                ) || (a.starts_with('-')
                    && !a.starts_with("--")
                    && a.chars().any(|c| matches!(c, 'c' | 't' | 'l')))
            });
            let files = positionals(
                args,
                &["-S", "--suffix", "-T", "--threads", "-o", "--output"],
            );
            if to_stdout || files.is_empty() {
                return None;
            }
            all_targets_ok(&prog, &files, in_temp)
        }
        "curl" => {
            let saves = args.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "--output"
                        | "--remote-name"
                        | "--remote-name-all"
                        | "--remote-header-name"
                        | "--output-dir"
                        | "--cookie-jar"
                        | "--dump-header"
                        | "--trace"
                        | "--trace-ascii"
                        | "--create-dirs"
                        | "--etag-save"
                ) || a.starts_with("--output=")
                    || (a.starts_with('-')
                        && !a.starts_with("--")
                        && a.chars()
                            .skip(1)
                            .any(|c| matches!(c, 'o' | 'O' | 'J' | 'D' | 'c')))
            });
            if !saves {
                return None;
            }
            if let Some(out) = flag_value(
                args,
                &[
                    "-o",
                    "--output",
                    "--output-dir",
                    "-D",
                    "--dump-header",
                    "-c",
                    "--cookie-jar",
                ],
            ) && !args
                .iter()
                .any(|a| a == "-O" || a == "--remote-name" || a == "-J")
            {
                return all_targets_ok(&prog, &[out], in_temp);
            }
            if in_temp {
                None
            } else {
                Some("`curl -o/-O` saves a file into the working directory (write under /tmp, or read the body from stdout)".into())
            }
        }
        "wget" => {
            let out = flag_value(args, &["-O", "--output-document"]).or_else(|| {
                args.iter().find_map(|a| {
                    a.strip_prefix("--output-document=")
                        .map(str::to_string)
                        .or_else(|| {
                            (a.starts_with('-') && !a.starts_with("--") && a.contains('O'))
                                .then(|| {
                                    a.split_once('O')
                                        .map(|(_, v)| v.to_string())
                                        .unwrap_or_default()
                                })
                                .filter(|v| !v.is_empty())
                        })
                })
            });
            match out.as_deref() {
                Some("-") | Some("/dev/stdout") => None,
                Some(path) => all_targets_ok(&prog, &[path.to_string()], in_temp),
                None => {
                    if let Some(dir) = flag_value(args, &["-P", "--directory-prefix"]) {
                        return all_targets_ok(&prog, &[dir], in_temp);
                    }
                    if in_temp {
                        None
                    } else {
                        Some("`wget` saves a file into the working directory (use -O- or -P /tmp/...)".into())
                    }
                }
            }
        }
        "make" | "gmake" | "ninja" | "meson" | "cmake" => {
            let install = args.iter().any(|a| a == "install" || a == "--install");
            if install {
                Some(format!("`{prog} install` installs the build"))
            } else {
                None
            }
        }
        "npm" | "pnpm" | "yarn" | "bun" | "cargo" | "go" | "uv" | "poetry" | "pdm" | "pipenv"
        | "bundle" | "composer" | "mix" | "dotnet" | "swift" | "flutter" | "dart" | "deno" => {
            package_manager_refusal(&prog, args, in_temp)
        }
        _ => formatter_refusal(&prog, args),
    }
}

/// The change `cd`, `pushd` or `popd` makes to where relative paths land.
fn change_dir(words: &[String], state: &mut ShellState) {
    let dir = words
        .iter()
        .skip(1)
        .find(|w| !w.starts_with('-') || *w == "-");
    match (words[0].as_str(), dir.map(String::as_str)) {
        ("popd", _) | (_, None) | (_, Some("-")) | (_, Some("~")) => state.in_temp = false,
        (_, Some(d)) => {
            state.in_temp = if d.starts_with('/') || d.starts_with('$') || d.starts_with('~') {
                is_temp_path(d)
            } else {
                state.in_temp && !has_dotdot(d)
            };
        }
    }
}

fn find_refusal(args: &[String], state: &mut ShellState) -> Option<String> {
    let starts: Vec<String> = args
        .iter()
        .take_while(|a| !a.starts_with('-') && *a != "(" && *a != "!")
        .cloned()
        .collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-delete" => {
                let roots: Vec<String> = if starts.is_empty() {
                    vec![".".into()]
                } else {
                    starts.clone()
                };
                if let Some(bad) = all_targets_ok("find -delete", &roots, state.in_temp) {
                    return Some(bad);
                }
            }
            "-fprint" | "-fprint0" | "-fprintf" | "-fls" => {
                if let Some(target) = args.get(i + 1)
                    && let Some(bad) = all_targets_ok(
                        &format!("find {}", args[i]),
                        std::slice::from_ref(target),
                        state.in_temp,
                    )
                {
                    return Some(bad);
                }
            }
            "-exec" | "-execdir" | "-ok" | "-okdir" => {
                let sub: Vec<String> = args[i + 1..]
                    .iter()
                    .take_while(|a| *a != ";" && *a != "+")
                    .filter(|a| *a != "{}")
                    .cloned()
                    .collect();
                let n = sub.len();
                let mut inner = state.clone();
                if let Some(bad) = check_segment(sub, Vec::new(), &mut inner) {
                    return Some(format!("find -exec: {bad}"));
                }
                i += n;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn tar_refusal(args: &[String], in_temp: bool) -> Option<String> {
    let mut letters = String::new();
    let mut long: Vec<&str> = Vec::new();
    let mut archive: Option<String> = None;
    let mut dir: Option<String> = None;
    let mut iter = args.iter().enumerate().peekable();
    while let Some((idx, a)) = iter.next() {
        if a.starts_with("--") {
            long.push(a.as_str());
            if let Some(v) = a.strip_prefix("--file=") {
                archive = Some(v.to_string());
            }
            if let Some(v) = a.strip_prefix("--directory=") {
                dir = Some(v.to_string());
            }
            if (a == "--file" || a == "--directory" || a == "-C")
                && let Some((_, v)) = iter.next()
            {
                if a == "--file" {
                    archive = Some(v.clone());
                } else {
                    dir = Some(v.clone());
                }
            }
        } else if a.starts_with('-') && a.len() > 1 || idx == 0 {
            let cluster = a.trim_start_matches('-');
            for c in cluster.chars() {
                letters.push(c);
                if c == 'f' {
                    if let Some((_, v)) = iter.next() {
                        archive = Some(v.clone());
                    }
                } else if c == 'C'
                    && let Some((_, v)) = iter.next()
                {
                    dir = Some(v.clone());
                }
            }
        }
    }
    let lists = letters.contains('t') || long.contains(&"--list");
    let to_stdout = letters.contains('O') || long.contains(&"--to-stdout");
    let extracts = letters.contains('x') || long.contains(&"--extract") || long.contains(&"--get");
    let creates = letters.chars().any(|c| matches!(c, 'c' | 'r' | 'u' | 'A'))
        || long.iter().any(|l| {
            matches!(
                *l,
                "--create" | "--append" | "--update" | "--concatenate" | "--catenate" | "--delete"
            )
        });
    if lists || to_stdout {
        return None;
    }
    if extracts {
        let into = dir.unwrap_or_else(|| ".".into());
        return all_targets_ok("tar -x", &[into], in_temp);
    }
    if creates {
        return match archive.as_deref() {
            Some("-") | None => None,
            Some(path) => all_targets_ok("tar -c", &[path.to_string()], in_temp),
        };
    }
    None
}

fn git_refusal(args: &[String], in_temp: bool) -> Option<String> {
    let mut i = 0;
    let mut elsewhere = in_temp;
    while i < args.len() && args[i].starts_with('-') {
        let a = args[i].as_str();
        match a {
            "-C" | "--git-dir" | "--work-tree" => {
                if let Some(dir) = args.get(i + 1) {
                    elsewhere = elsewhere || is_temp_path(dir);
                }
                i += 2;
                continue;
            }
            "-c" | "--namespace" | "--super-prefix" | "--config-env" | "--exec-path"
            | "--list-cmds" | "--attr-source" => {
                i += 2;
                continue;
            }
            _ => {}
        }
        if let Some(dir) = a
            .strip_prefix("--git-dir=")
            .or_else(|| a.strip_prefix("--work-tree="))
        {
            elsewhere = elsewhere || is_temp_path(dir);
        }
        i += 1;
    }
    if elsewhere {
        return None;
    }
    let sub = args.get(i)?.as_str();
    let rest = &args[i + 1..];
    let has = |flags: &[&str]| rest.iter().any(|a| flags.contains(&a.as_str()));
    let first = rest
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str);
    let refuse = |what: &str| Some(format!("`git {sub}` {what}"));
    match sub {
        "add" | "rm" | "clean" => {
            if has(&["-n", "--dry-run"]) {
                None
            } else {
                refuse("changes the index or the working tree")
            }
        }
        "commit" => {
            if has(&["--dry-run"]) {
                None
            } else {
                refuse("makes a commit (a read-only run lands nothing)")
            }
        }
        "apply" => {
            if has(&["--check", "--stat", "--numstat", "--summary"]) {
                None
            } else {
                refuse("changes the working tree")
            }
        }
        "bisect" => {
            if matches!(first, Some("log" | "view" | "visualize" | "terms")) {
                None
            } else {
                refuse("moves HEAD")
            }
        }
        "stash" => {
            if matches!(first, Some("list" | "show")) {
                None
            } else {
                refuse("changes the working tree")
            }
        }
        "notes" => {
            if matches!(first, None | Some("list" | "show" | "get-ref")) {
                None
            } else {
                refuse("writes notes")
            }
        }
        "reflog" => {
            if matches!(first, None | Some("show" | "exists")) {
                None
            } else {
                refuse("rewrites the reflog")
            }
        }
        "remote" => {
            if matches!(first, None | Some("show" | "get-url")) {
                None
            } else {
                refuse("changes the remotes")
            }
        }
        "replace" => {
            if has(&["-l", "--list"]) {
                None
            } else {
                refuse("writes replace refs")
            }
        }
        "submodule" => {
            if matches!(first, None | Some("status" | "summary" | "foreach")) {
                None
            } else {
                refuse("changes the submodules")
            }
        }
        "worktree" => {
            if matches!(first, None | Some("list")) {
                None
            } else {
                refuse("changes the worktrees")
            }
        }
        "sparse-checkout" => {
            if matches!(first, None | Some("list" | "check-rules")) {
                None
            } else {
                refuse("changes the checkout")
            }
        }
        "lfs" => {
            if matches!(
                first,
                Some(
                    "track"
                        | "untrack"
                        | "install"
                        | "uninstall"
                        | "migrate"
                        | "pull"
                        | "checkout"
                        | "prune"
                        | "dedup"
                )
            ) {
                refuse("changes the working tree")
            } else {
                None
            }
        }
        "rerere" => {
            if matches!(first, Some("clear" | "forget" | "gc")) {
                refuse("changes recorded resolutions")
            } else {
                None
            }
        }
        "hash-object" => {
            if has(&["-w"]) {
                refuse("writes objects")
            } else {
                None
            }
        }
        "format-patch" => {
            if has(&["--stdout"]) {
                None
            } else {
                refuse("writes patch files (use --stdout)")
            }
        }
        "archive" => {
            if has(&["-o", "--output"]) || rest.iter().any(|a| a.starts_with("--output=")) {
                refuse("writes an archive file")
            } else {
                None
            }
        }
        "bundle" => {
            if matches!(first, Some("create")) {
                refuse("writes a bundle file")
            } else {
                None
            }
        }
        "symbolic-ref" => {
            let positional = rest.iter().filter(|a| !a.starts_with('-')).count();
            if positional > 1 || has(&["-d", "--delete"]) {
                refuse("moves a ref")
            } else {
                None
            }
        }
        "config" => {
            let reads = has(&[
                "--get",
                "--get-all",
                "--get-regexp",
                "--get-urlmatch",
                "--list",
                "-l",
                "--show-origin",
                "--show-scope",
                "--get-color",
                "--get-colorbool",
            ]) || matches!(first, Some("get" | "list"));
            if reads {
                None
            } else {
                refuse("writes git configuration (reads: --get, --list)")
            }
        }
        "tag" => {
            let writes = has(&[
                "-d",
                "--delete",
                "-a",
                "--annotate",
                "-s",
                "--sign",
                "-f",
                "--force",
                "-m",
                "--message",
                "-F",
                "--file",
                "-u",
                "--local-user",
                "-e",
                "--edit",
            ]);
            let lists = has(&[
                "-l",
                "--list",
                "-n",
                "--contains",
                "--no-contains",
                "--points-at",
                "--merged",
                "--no-merged",
                "--sort",
                "--format",
                "--column",
                "--no-column",
            ]) || rest.iter().any(|a| {
                a.starts_with("-n")
                    || a.starts_with("--sort=")
                    || a.starts_with("--format=")
                    || a.starts_with("--contains=")
                    || a.starts_with("--points-at=")
            });
            if writes || (!lists && first.is_some()) {
                refuse("creates or deletes a tag")
            } else {
                None
            }
        }
        "branch" => {
            let writes = has(&[
                "-d",
                "-D",
                "--delete",
                "-m",
                "-M",
                "--move",
                "-c",
                "-C",
                "--copy",
                "-f",
                "--force",
                "-u",
                "--set-upstream-to",
                "--unset-upstream",
                "--edit-description",
                "-t",
                "--track",
            ]) || rest.iter().any(|a| a.starts_with("--set-upstream-to="));
            let lists = has(&[
                "-l",
                "--list",
                "-a",
                "--all",
                "-r",
                "--remotes",
                "-v",
                "-vv",
                "--verbose",
                "--show-current",
                "--contains",
                "--no-contains",
                "--merged",
                "--no-merged",
                "--points-at",
                "--format",
                "--sort",
                "--column",
            ]) || rest.iter().any(|a| {
                a.starts_with("--sort=")
                    || a.starts_with("--format=")
                    || a.starts_with("--contains=")
                    || a.starts_with("--merged=")
                    || a.starts_with("--points-at=")
            });
            if writes || (!lists && first.is_some()) {
                refuse("creates, moves or deletes a branch")
            } else {
                None
            }
        }
        "am" | "checkout" | "checkout-index" | "cherry-pick" | "clone" | "commit-tree"
        | "fast-import" | "filter-branch" | "filter-repo" | "gc" | "init" | "merge" | "mv"
        | "pack-refs" | "prune" | "pull" | "push" | "read-tree" | "rebase" | "repack" | "reset"
        | "restore" | "revert" | "switch" | "update-index" | "update-ref" | "maintenance"
        | "update-server-info" | "write-tree" | "mktree" | "mktag" | "prune-packed" | "svn" => {
            refuse("changes the clone")
        }
        _ => None,
    }
}

fn package_manager_refusal(prog: &str, args: &[String], in_temp: bool) -> Option<String> {
    if in_temp {
        return None;
    }
    let sub = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str)?;
    let second = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .nth(1)
        .map(String::as_str);
    let has = |flags: &[&str]| args.iter().any(|a| flags.contains(&a.as_str()));
    let pkgs = || {
        args.iter()
            .skip_while(|a| a.as_str() != sub)
            .skip(1)
            .any(|a| !a.starts_with('-'))
    };
    let refuse = |what: &str| Some(format!("`{prog} {sub}` {what}"));
    match (prog, sub) {
        ("npm", "install" | "i" | "add" | "in" | "ins" | "inst" | "isntall") => {
            if pkgs() {
                refuse(
                    "edits package.json and the lockfile (a plain install from the lockfile is fine)",
                )
            } else {
                None
            }
        }
        (
            "npm",
            "uninstall" | "remove" | "rm" | "r" | "un" | "unlink" | "update" | "up" | "upgrade"
            | "dedupe" | "ddp" | "version" | "init" | "create" | "pkg" | "publish" | "unpublish"
            | "deprecate" | "link" | "ln" | "set-script",
        ) => refuse("edits package.json or the lockfile"),
        ("npm", "audit") => {
            if second == Some("fix") {
                refuse("edits the lockfile")
            } else {
                None
            }
        }
        ("pnpm", "install" | "i") => {
            if pkgs() {
                refuse("edits package.json and the lockfile")
            } else {
                None
            }
        }
        (
            "pnpm",
            "add" | "remove" | "rm" | "un" | "uninstall" | "update" | "up" | "upgrade" | "dedupe"
            | "import" | "patch" | "patch-commit" | "init" | "version" | "link" | "unlink"
            | "publish" | "deploy",
        ) => refuse("edits package.json or the lockfile"),
        (
            "yarn",
            "add"
            | "remove"
            | "upgrade"
            | "up"
            | "upgrade-interactive"
            | "import"
            | "init"
            | "version"
            | "dedupe"
            | "set"
            | "link"
            | "unlink"
            | "publish"
            | "create"
            | "patch"
            | "patch-commit"
            | "plugin",
        ) => refuse("edits package.json or the lockfile"),
        (
            "bun",
            "add" | "a" | "remove" | "rm" | "update" | "link" | "unlink" | "init" | "create" | "c"
            | "patch" | "publish" | "pm",
        ) => refuse("edits package.json or the lockfile"),
        ("bun", "install" | "i") => {
            if pkgs() {
                refuse("edits package.json and the lockfile")
            } else {
                None
            }
        }
        (
            "cargo",
            "add" | "rm" | "remove" | "fix" | "init" | "new" | "generate-lockfile" | "vendor"
            | "yank" | "publish" | "owner" | "login" | "logout" | "set-version" | "release"
            | "upgrade" | "edit" | "workspaces" | "autoinherit" | "machete",
        ) => refuse("edits Cargo.toml, Cargo.lock or the sources"),
        ("cargo", "update") => {
            if has(&["--dry-run", "-n", "--workspace", "-w"]) && !pkgs() || has(&["--dry-run"]) {
                None
            } else {
                refuse("rewrites Cargo.lock (use --dry-run)")
            }
        }
        ("cargo", "fmt") => {
            if has(&["--check"]) {
                None
            } else {
                refuse("rewrites the sources (use --check)")
            }
        }
        ("cargo", "clippy") => {
            if has(&["--fix"]) {
                refuse("rewrites the sources")
            } else {
                None
            }
        }
        ("go", "get" | "fmt" | "generate" | "fix") => refuse("edits go.mod or the sources"),
        ("go", "mod") => {
            if matches!(second, Some("tidy" | "vendor" | "edit" | "init")) {
                refuse("edits go.mod or vendor/")
            } else {
                None
            }
        }
        ("go", "work") => {
            if matches!(second, Some("init" | "use" | "edit" | "sync" | "vendor")) {
                refuse("edits go.work")
            } else {
                None
            }
        }
        ("uv", "add" | "remove" | "lock" | "init" | "version" | "publish" | "build") => {
            refuse("edits pyproject.toml or uv.lock")
        }
        ("uv", "pip") => {
            if matches!(second, Some("compile"))
                && !has(&["--dry-run"])
                && (has(&["-o", "--output-file"])
                    || args.iter().any(|a| a.starts_with("--output-file=")))
            {
                refuse("writes a requirements file")
            } else {
                None
            }
        }
        (
            "poetry",
            "add" | "remove" | "lock" | "init" | "update" | "version" | "publish" | "new" | "build",
        ) => refuse("edits pyproject.toml or poetry.lock"),
        (
            "pdm",
            "add" | "remove" | "lock" | "init" | "update" | "publish" | "build" | "import" | "fix",
        ) => refuse("edits pyproject.toml or pdm.lock"),
        ("pipenv", "install") => {
            if pkgs() {
                refuse("edits the Pipfile")
            } else {
                None
            }
        }
        ("pipenv", "uninstall" | "update" | "lock" | "upgrade") => {
            refuse("edits the Pipfile or Pipfile.lock")
        }
        (
            "bundle",
            "add" | "remove" | "lock" | "init" | "update" | "binstubs" | "pristine" | "gem",
        ) => refuse("edits the Gemfile or Gemfile.lock"),
        (
            "composer",
            "require" | "remove" | "update" | "init" | "config" | "bump" | "create-project",
        ) => refuse("edits composer.json or composer.lock"),
        ("mix", "format") => {
            if has(&["--check-formatted"]) {
                None
            } else {
                refuse("rewrites the sources (use --check-formatted)")
            }
        }
        ("mix", "new" | "igniter.new" | "phx.new") => refuse("creates a project"),
        ("mix", "deps.update" | "deps.unlock" | "deps.clean" | "hex.publish" | "release") => {
            refuse("edits mix.lock or the sources")
        }
        ("dotnet", "add" | "remove" | "new") => refuse("edits the project files"),
        ("dotnet", "format") => {
            if has(&["--verify-no-changes"]) {
                None
            } else {
                refuse("rewrites the sources (use --verify-no-changes)")
            }
        }
        ("swift", "package") => {
            if matches!(
                second,
                Some("init" | "add-dependency" | "add-target" | "add-product" | "edit" | "unedit")
            ) {
                refuse("edits Package.swift")
            } else {
                None
            }
        }
        ("flutter" | "dart", "create") => refuse("creates a project"),
        ("flutter" | "dart", "pub") => {
            if matches!(second, Some("add" | "remove" | "upgrade" | "publish")) {
                refuse("edits pubspec.yaml")
            } else {
                None
            }
        }
        ("dart", "format") => {
            if has(&["--set-exit-if-changed"])
                || args
                    .iter()
                    .any(|a| a == "--output=none" || a == "-o" || a == "--output")
                    && args.iter().any(|a| a == "none" || a == "--output=none")
            {
                None
            } else {
                refuse("rewrites the sources (use --output=none)")
            }
        }
        ("dart", "fix") => {
            if has(&["--apply"]) {
                refuse("rewrites the sources")
            } else {
                None
            }
        }
        ("deno", "fmt") => {
            if has(&["--check"]) {
                None
            } else {
                refuse("rewrites the sources (use --check)")
            }
        }
        ("deno", "lint") => {
            if has(&["--fix"]) {
                refuse("rewrites the sources")
            } else {
                None
            }
        }
        ("deno", "add" | "remove" | "init" | "install" | "i" | "outdated") => {
            if sub == "outdated" && !has(&["--update"]) || sub == "install" && !pkgs() {
                None
            } else {
                refuse("edits deno.json or the lockfile")
            }
        }
        _ => None,
    }
}

fn formatter_refusal(prog: &str, args: &[String]) -> Option<String> {
    let has = |flags: &[&str]| args.iter().any(|a| flags.contains(&a.as_str()));
    let first = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str);
    let refuse = |what: &str| Some(format!("`{prog}` {what}"));
    let in_place = "rewrites files in place (use the check or diff mode to see what it would do)";
    match prog {
        "gofmt" | "goimports" | "gofumpt" | "shfmt" | "buf" | "clang-format" | "swift-format" => {
            if has(&["-w", "-i", "--write", "--in-place"]) {
                refuse(in_place)
            } else {
                None
            }
        }
        "rustfmt"
        | "stylua"
        | "taplo"
        | "nixfmt"
        | "alejandra"
        | "scalafmt"
        | "ormolu"
        | "fourmolu"
        | "cljfmt"
        | "zprint"
        | "treefmt"
        | "yapf"
        | "autopep8"
        | "docformatter"
        | "pyupgrade"
        | "reorder-python-imports" => {
            let checks_only = has(&[
                "--check",
                "--test",
                "--diff",
                "--mode=check",
                "--lint",
                "-c",
            ]) || matches!(first, Some("check"))
                || has(&["--mode", "check"])
                || (matches!(prog, "yapf" | "autopep8" | "docformatter")
                    && !has(&["-i", "--in-place"]))
                || (prog == "taplo" && !matches!(first, Some("fmt" | "format")));
            if checks_only { None } else { refuse(in_place) }
        }
        "prettier" | "biome" | "rome" | "dprint" | "stylelint" | "eslint" | "tslint" | "xo"
        | "standard" | "oxlint" | "markdownlint" | "markdownlint-cli2" | "eslint_d"
        | "prettierd" => {
            let writes = has(&[
                "--write",
                "-w",
                "--fix",
                "--apply",
                "--apply-unsafe",
                "--fix-type",
                "--unsafe",
            ]) || args.iter().any(|a| {
                a.starts_with("--fix-type=") || a == "--write=true" || a.starts_with("--fix=")
            }) || (prog == "dprint" && first == Some("fmt"));
            if writes && !has(&["--fix-dry-run", "--check", "--list-different", "-l"]) {
                refuse(in_place)
            } else {
                None
            }
        }
        "black" | "isort" | "blue" => {
            if has(&["--check", "--diff", "-c", "--check-only", "--df"]) {
                None
            } else {
                refuse(in_place)
            }
        }
        "ruff" => match first {
            Some("format") => {
                if has(&["--check", "--diff"]) {
                    None
                } else {
                    refuse(in_place)
                }
            }
            Some("check") => {
                if has(&["--fix", "--fix-only", "--unsafe-fixes"]) && !has(&["--diff", "--no-fix"])
                {
                    refuse(in_place)
                } else {
                    None
                }
            }
            _ => None,
        },
        "rubocop" | "standardrb" => {
            if has(&[
                "-a",
                "-A",
                "--autocorrect",
                "--autocorrect-all",
                "--auto-correct",
                "--auto-correct-all",
                "--fix-layout",
                "-x",
                "--fix",
            ]) {
                refuse(in_place)
            } else {
                None
            }
        }
        "php-cs-fixer" => {
            if first == Some("fix") && !has(&["--dry-run"]) {
                refuse(in_place)
            } else {
                None
            }
        }
        "pint" => {
            if has(&["--test"]) {
                None
            } else {
                refuse(in_place)
            }
        }
        "elm-format" => {
            if has(&["--validate"]) {
                None
            } else {
                refuse(in_place)
            }
        }
        "swiftformat" => {
            if has(&["--lint", "--dryrun"]) {
                None
            } else {
                refuse(in_place)
            }
        }
        "swiftlint" => {
            if has(&["--fix", "--autocorrect"]) {
                refuse(in_place)
            } else {
                None
            }
        }
        "ktlint" => {
            if has(&["-F", "--format"]) {
                refuse(in_place)
            } else {
                None
            }
        }
        "google-java-format" => {
            if has(&["-i", "--replace", "-r"]) {
                refuse(in_place)
            } else {
                None
            }
        }
        "terraform" | "tofu" => {
            if first == Some("fmt") && !has(&["-check", "-diff", "--check", "--diff"]) {
                refuse(in_place)
            } else {
                None
            }
        }
        "codespell" | "typos" => {
            if has(&["-w", "--write-changes"]) {
                refuse(in_place)
            } else {
                None
            }
        }
        "sqlfluff" => {
            if first == Some("fix") {
                refuse(in_place)
            } else {
                None
            }
        }
        "yamlfmt" => {
            if has(&["-lint", "--lint", "-dry", "--dry"]) {
                None
            } else {
                refuse(in_place)
            }
        }
        "jq" | "yq" if has(&["-i", "--in-place", "--inplace"]) => refuse(in_place),
        "sd" | "fastmod" | "codemod" | "comby" | "ast-grep" | "sg" | "ruplacer" => {
            if has(&[
                "-p",
                "--preview",
                "--dry-run",
                "-n",
                "--diff",
                "--in-place=false",
            ]) || (matches!(prog, "ast-grep" | "sg")
                && first != Some("run")
                && !has(&["-U", "--update-all", "-r", "--rewrite"]))
                || (prog == "comby" && !has(&["-in-place", "-i"]))
            {
                None
            } else {
                refuse(in_place)
            }
        }
        "gradle" | "gradlew" | "mvn" | "mvnw" | "mvnd" => {
            if args.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "spotlessApply"
                        | "spotless:apply"
                        | "ktlintFormat"
                        | "googleJavaFormat"
                        | "licenseFormat"
                        | "detektFormat"
                        | "formatKotlin"
                        | "fmt:format"
                        | "com.spotify.fmt:fmt-maven-plugin:format"
                        | "versions:set"
                        | "versions:use-latest-versions"
                        | "release:prepare"
                        | "release:perform"
                        | "deploy"
                        | "publish"
                        | "publishToMavenLocal"
                        | "wrapper"
                )
            }) {
                refuse("rewrites the sources or the build files")
            } else {
                None
            }
        }
        _ => None,
    }
}

fn sed_in_place(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| a.starts_with('-') && a.len() > 1)
        .any(|a| {
            a == "--in-place"
                || a.starts_with("--in-place=")
                || (!a.starts_with("--")
                    && cluster_letters(a, &['e', 'f', 'l'])
                        .iter()
                        .any(|c| matches!(c, 'i' | 'I')))
        })
}

/// The option letters of a single-dash cluster such as `-ni` up to and
/// including the first letter that takes a value (`-ne` is `n`, `e`: what
/// follows `e` is its script, not more options).
fn cluster_letters(arg: &str, value_letters: &[char]) -> Vec<char> {
    let mut out = Vec::new();
    for c in arg.trim_start_matches('-').chars() {
        out.push(c);
        if value_letters.contains(&c) {
            break;
        }
    }
    out
}

fn all_targets_ok(prog: &str, targets: &[String], in_temp: bool) -> Option<String> {
    let bad = targets.iter().find(|t| !target_ok(t, in_temp))?;
    Some(format!(
        "`{prog} {bad}` writes inside the clone (scratch files go under /tmp)"
    ))
}

/// Whether writing to `target` leaves the clone alone: a descriptor, a
/// device, the temp directory, or a relative path when the shell has `cd`'d
/// into the temp directory.
fn target_ok(target: &str, in_temp: bool) -> bool {
    if target.starts_with('&') {
        return true;
    }
    if is_device(target) || is_temp_path(target) {
        return true;
    }
    in_temp
        && !target.starts_with('/')
        && !target.starts_with('~')
        && !target.starts_with('$')
        && !has_dotdot(target)
}

fn is_device(path: &str) -> bool {
    matches!(
        path,
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty" | "/dev/zero"
    ) || path.starts_with("/dev/fd/")
}

/// Whether `path` is under the temp directory by any of its spellings.
fn is_temp_path(path: &str) -> bool {
    if has_dotdot(path) {
        return false;
    }
    const ROOTS: &[&str] = &[
        "/tmp",
        "/private/tmp",
        "/var/tmp",
        "/private/var/tmp",
        "/var/folders",
        "/private/var/folders",
        "/dev/shm",
        "$TMPDIR",
        "${TMPDIR}",
        "$TMP",
        "${TMP}",
        "$TEMP",
        "${TEMP}",
    ];
    ROOTS.iter().any(|root| {
        path == *root
            || path
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn has_dotdot(path: &str) -> bool {
    path.split('/').any(|seg| seg == "..")
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn strip_assignments(words: &mut Vec<String>) {
    while let Some(first) = words.first() {
        let Some((name, _)) = first.split_once('=') else {
            break;
        };
        let is_name = !name.is_empty()
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !is_name {
            break;
        }
        words.remove(0);
    }
}

/// Drops the leading options of a wrapper command; `value_flags` take the
/// next word as their value. A `--` ends the options and is dropped too.
fn strip_flags(words: &mut Vec<String>, value_flags: &[&str]) {
    while let Some(first) = words.first() {
        if first == "--" {
            words.remove(0);
            break;
        }
        if !first.starts_with('-') || first.len() < 2 {
            break;
        }
        let takes_value = value_flags.contains(&first.as_str()) && !first.contains('=');
        words.remove(0);
        if takes_value && !words.is_empty() {
            words.remove(0);
        }
    }
}

/// The non-option arguments, skipping the values of `value_flags`; a `--`
/// makes everything after it positional.
fn positionals(args: &[String], value_flags: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut literal = false;
    while i < args.len() {
        let a = &args[i];
        if literal || !a.starts_with('-') || a == "-" {
            out.push(a.clone());
        } else if a == "--" {
            literal = true;
        } else if value_flags.contains(&a.as_str()) {
            i += 1;
        }
        i += 1;
    }
    out
}

/// The value of the first of `flags` present, attached (`-oFILE`,
/// `--output=FILE`) or as the next word.
fn flag_value(args: &[String], flags: &[&str]) -> Option<String> {
    for (i, a) in args.iter().enumerate() {
        for f in flags {
            if a == f {
                return args.get(i + 1).cloned();
            }
            if f.starts_with("--") {
                if let Some(v) = a.strip_prefix(&format!("{f}=")) {
                    return Some(v.to_string());
                }
            } else if let Some(v) = a.strip_prefix(f)
                && !v.is_empty()
                && !a.starts_with("--")
            {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// What a read-only run's clone looked like before a command: the branch,
/// HEAD and every dirty or untracked path. [`TreeSnapshot::restore`] puts
/// back whatever a command changed in spite of the checker.
#[derive(Clone, Debug)]
pub struct TreeSnapshot {
    dir: PathBuf,
    branch: String,
    head: String,
    status: BTreeMap<String, String>,
    /// Local branches, tags and the stash: `refname -> object`.
    refs: BTreeMap<String, String>,
}

impl TreeSnapshot {
    /// None when `dir` is not inside a git work tree.
    pub fn take(dir: &Path) -> Option<Self> {
        let inside = git(dir, &["rev-parse", "--is-inside-work-tree"])?;
        if inside.trim() != "true" {
            return None;
        }
        let status = git_status(dir)?;
        Some(Self {
            dir: dir.to_path_buf(),
            branch: current_branch(dir),
            head: current_head(dir),
            status,
            refs: local_refs(dir),
        })
    }

    /// Reverts what changed since the snapshot and says what it put back,
    /// one entry per path or ref; empty when nothing changed.
    pub fn restore(&self) -> Vec<String> {
        let dir = &self.dir;
        let mut reverted = Vec::new();
        let branch_now = current_branch(dir);
        if branch_now != self.branch {
            if self.branch == "HEAD" {
                let _ = git(dir, &["checkout", "-q", "--detach", &self.head]);
            } else {
                let _ = git(dir, &["checkout", "-q", &self.branch]);
            }
            reverted.push(format!("branch {branch_now} -> {}", self.branch));
        }
        let head_now = current_head(dir);
        if head_now != self.head && !self.head.is_empty() {
            // Soft: the committed changes come back as staged changes and
            // are put back path by path below.
            let _ = git(dir, &["reset", "-q", "--soft", &self.head]);
            reverted.push(format!(
                "HEAD {} -> {}",
                short(&head_now),
                short(&self.head)
            ));
        }
        let now = git_status(dir).unwrap_or_default();
        for (path, code) in &now {
            if self.status.get(path) == Some(code) {
                continue;
            }
            let full = dir.join(path);
            if code == "??" {
                remove_path(&full);
                remove_empty_parents(dir, &full);
                reverted.push(format!("{path} (new, removed)"));
                continue;
            }
            let _ = git(dir, &["reset", "-q", "--", path]);
            if self.status.contains_key(path) {
                reverted.push(format!("{path} (unstaged)"));
                continue;
            }
            if git(dir, &["checkout", "-q", "--", path]).is_none() {
                remove_path(&full);
                remove_empty_parents(dir, &full);
            }
            reverted.push(path.clone());
        }
        for (path, code) in &self.status {
            if !now.contains_key(path) {
                reverted.push(format!(
                    "{path} (was {code}; the command discarded that change, which cannot be put back)",
                    code = code.trim()
                ));
            }
        }
        // Branches, tags and the stash: new ones go, moved ones move back,
        // deleted ones come back.
        let refs_now = local_refs(dir);
        for (name, object) in &refs_now {
            match self.refs.get(name) {
                Some(old) if old == object => {}
                Some(old) => {
                    let _ = git(dir, &["update-ref", name, old]);
                    reverted.push(format!(
                        "{} {} -> {}",
                        short_ref(name),
                        short(object),
                        short(old)
                    ));
                }
                None => {
                    let _ = git(dir, &["update-ref", "-d", name]);
                    reverted.push(format!("{} (new, removed)", short_ref(name)));
                }
            }
        }
        for (name, object) in &self.refs {
            if !refs_now.contains_key(name) {
                let _ = git(dir, &["update-ref", name, object]);
                reverted.push(format!("{} (deleted, restored)", short_ref(name)));
            }
        }
        reverted
    }
}

/// `refname -> object` for the local branches, tags and the stash; the
/// remote-tracking refs are left to `git fetch`.
fn local_refs(dir: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Some(out) = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads/",
            "refs/tags/",
            "refs/stash",
        ],
    ) else {
        return map;
    };
    for line in out.lines() {
        if let Some((name, object)) = line.split_once(' ') {
            map.insert(name.to_string(), object.trim().to_string());
        }
    }
    map
}

fn short_ref(name: &str) -> String {
    name.strip_prefix("refs/heads/")
        .map(|b| format!("branch {b}"))
        .or_else(|| name.strip_prefix("refs/tags/").map(|t| format!("tag {t}")))
        .unwrap_or_else(|| name.to_string())
}

fn current_branch(dir: &Path) -> String {
    git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn current_head(dir: &Path) -> String {
    git(dir, &["rev-parse", "--verify", "-q", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn short(rev: &str) -> &str {
    if rev.is_empty() {
        "(none)"
    } else {
        &rev[..rev.len().min(12)]
    }
}

/// `path -> XY` for every dirty or untracked path.
fn git_status(dir: &Path) -> Option<BTreeMap<String, String>> {
    let out = git(
        dir,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
        ],
    )?;
    let mut map = BTreeMap::new();
    for entry in out.split('\0') {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(2);
        map.insert(path[1..].to_string(), code.to_string());
    }
    Some(map)
}

fn remove_path(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

/// Removes the directories a removed file leaves empty, up to (not
/// including) `root`; git does not track them, so status would not show
/// them.
fn remove_empty_parents(root: &Path, path: &Path) {
    let mut cur = path.parent();
    while let Some(dir) = cur {
        if dir == root || !dir.starts_with(root) {
            break;
        }
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
        cur = dir.parent();
    }
}

/// Runs git in `dir` with the hermetic environment and returns stdout on
/// success.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(pulse_gitenv::git_program());
    cmd.args(args).current_dir(dir).env_clear();
    for kv in pulse_gitenv::env() {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(cmd: &str) -> String {
        read_only_refusal(cmd).unwrap_or_else(|| panic!("expected a refusal for `{cmd}`"))
    }

    fn allowed(cmd: &str) {
        if let Some(why) = read_only_refusal(cmd) {
            panic!("`{cmd}` should be allowed, got: {why}");
        }
    }

    #[test]
    fn refuses_git_commands_that_change_the_clone() {
        assert!(refused("git commit -m x").contains("git commit"));
        assert!(refused("git add -A && git commit -m x").contains("git add"));
        assert!(refused("git checkout -b feature").contains("git checkout"));
        assert!(refused("git stash").contains("git stash"));
        assert!(refused("git reset --hard HEAD~1").contains("git reset"));
        assert!(refused("git push origin main").contains("git push"));
        assert!(refused("git clean -fd").contains("git clean"));
    }

    #[test]
    fn allows_git_commands_that_only_look() {
        allowed("git status");
        allowed("git log --oneline -5");
        allowed("git diff HEAD~1");
        allowed("git show HEAD:README.md");
        allowed("git blame src/lib.rs");
        allowed("git stash list");
        allowed("git add -n .");
        allowed("git commit --dry-run");
        allowed("git -C /tmp/scratch init");
        allowed("git rev-parse HEAD");
        allowed("git branch --show-current");
    }

    #[test]
    fn refuses_file_writers_inside_the_clone() {
        assert!(refused("touch x").contains("touch"));
        assert!(refused("mkdir newdir").contains("mkdir"));
        assert!(refused("rm -rf src").contains("rm"));
        assert!(refused("cp a.txt b.txt").contains("cp"));
        assert!(refused("mv a.txt b.txt").contains("mv"));
        assert!(refused("echo hi > out.txt").contains("out.txt"));
        assert!(refused("cargo test 2> errors.log").contains("errors.log"));
        assert!(refused("cargo test >> log.txt").contains("log.txt"));
        assert!(refused("sed -i 's/a/b/' src/lib.rs").contains("sed"));
        assert!(refused("echo hi | tee notes.md").contains("tee"));
        assert!(refused("dd if=/dev/zero of=blob bs=1 count=1").contains("dd"));
        assert!(refused("patch -p1 < fix.diff").contains("patch"));
    }

    #[test]
    fn allows_writes_outside_the_clone() {
        allowed("echo hi > /dev/null");
        allowed("cargo test 2>&1 | head -50");
        allowed("mkdir -p /tmp/scratch");
        allowed("touch /tmp/scratch/x");
        allowed("echo hi > /tmp/out.txt");
        allowed("echo hi > $TMPDIR/out.txt");
        allowed("echo hi > ${TMPDIR}/out.txt");
        allowed("echo hi | tee /tmp/out.txt");
        allowed("cp src/lib.rs /tmp/lib.rs");
        allowed("cd /tmp && touch y");
        allowed("cd /tmp/x; mkdir y");
        allowed("patch --dry-run -p1 < fix.diff");
    }

    #[test]
    fn cd_into_temp_does_not_leak_out_of_a_subshell() {
        allowed("(cd /tmp && touch y)");
        assert!(refused("(cd /tmp && touch y); touch z").contains("touch"));
        assert!(refused("cd /tmp && cd - && touch z").contains("touch"));
    }

    #[test]
    fn rejects_temp_paths_that_climb_back_out() {
        assert!(refused("echo hi > /tmp/../Users/me/notes.md").contains("notes.md"));
        assert!(refused("cd /tmp && touch ../../repo/x").contains("touch"));
    }

    #[test]
    fn heredoc_bodies_and_quoted_text_are_not_redirections() {
        allowed("cat <<'EOF'\nfoo > bar\nbaz >> qux\nEOF");
        allowed("cat <<-EOF\n\t> indented\n\tEOF");
        allowed("echo 'a > b'");
        allowed("echo \"a > b\"");
        allowed("grep -rn 'fn main' src");
        allowed("python3 -c 'print(1 > 0)'");
    }

    #[test]
    fn sees_through_wrappers_and_nested_commands() {
        assert!(refused("sudo rm -rf src").contains("rm"));
        assert!(refused("env FOO=1 touch x").contains("touch"));
        assert!(refused("nohup touch x").contains("touch"));
        assert!(refused("timeout 10 touch x").contains("touch"));
        assert!(refused("sh -c 'touch x'").contains("touch"));
        assert!(refused("bash -lc \"git commit -m x\"").contains("git commit"));
        assert!(refused("eval touch x").contains("touch"));
        assert!(refused("echo $(touch x)").contains("touch"));
        assert!(refused("echo `touch x`").contains("touch"));
        assert!(refused("find . -name '*.rs' | xargs rm").contains("xargs rm"));
        assert!(refused("ls | xargs -I{} mv {} old/").contains("mv"));
        assert!(refused("ls | xargs sed -i 's/a/b/'").contains("sed"));
        allowed("ls | xargs -I{} cp {} /tmp/keep/");
        allowed("ls | xargs wc -l");
        allowed("ls | xargs grep -n foo");
        allowed("cd /tmp && ls | xargs rm");
        assert!(refused("find . -name '*.orig' -delete").contains("find"));
        assert!(refused("npx prettier --write src").contains("prettier"));
        allowed("sudo ls");
        allowed("sh -c 'cargo test'");
        allowed("bash script.sh");
        allowed("find . -name '*.rs'");
        allowed("echo $(git rev-parse HEAD)");
    }

    #[test]
    fn refuses_formatters_that_rewrite_and_allows_their_checks() {
        assert!(refused("cargo fmt").contains("fmt"));
        assert!(refused("rustfmt src/lib.rs").contains("rustfmt"));
        assert!(refused("prettier --write .").contains("prettier"));
        assert!(refused("eslint --fix src").contains("eslint"));
        assert!(refused("gofmt -w .").contains("gofmt"));
        assert!(refused("black .").contains("black"));
        assert!(refused("cargo clippy --fix").contains("clippy"));
        allowed("cargo fmt --check");
        allowed("cargo fmt -- --check");
        allowed("prettier --check .");
        allowed("eslint src");
        allowed("gofmt -l .");
        allowed("black --check .");
        allowed("cargo clippy --workspace -- -D warnings");
    }

    #[test]
    fn package_managers_may_install_but_not_edit_manifests() {
        assert!(refused("npm install left-pad").contains("npm"));
        assert!(refused("npm i -D typescript").contains("npm"));
        assert!(refused("npm uninstall left-pad").contains("npm"));
        assert!(refused("pnpm add zod").contains("pnpm"));
        assert!(refused("yarn add zod").contains("yarn"));
        assert!(refused("cargo add serde").contains("cargo"));
        assert!(refused("cargo update").contains("cargo"));
        assert!(refused("go get github.com/x/y").contains("go"));
        assert!(refused("go mod tidy").contains("go"));
        assert!(refused("uv add requests").contains("uv"));
        assert!(refused("poetry add requests").contains("poetry"));
        allowed("npm install");
        allowed("npm ci");
        allowed("pnpm install --frozen-lockfile");
        allowed("yarn install");
        allowed("cargo build");
        allowed("cargo test -p foo");
        allowed("cargo check --workspace");
        allowed("go build ./...");
        allowed("go test ./...");
        allowed("uv sync");
        allowed("poetry install");
        allowed("pip install -r requirements.txt");
        allowed("npm test");
        allowed("npm run build");
    }

    #[test]
    fn plain_reads_and_runs_are_fine() {
        allowed("ls -la");
        allowed("cat README.md");
        allowed("head -n 20 src/lib.rs");
        allowed("rg -n TODO");
        allowed("wc -l src/*.rs");
        allowed("cargo test -q 2>&1 | tail -20");
        allowed("make test");
        allowed("pytest -q");
        allowed("node -e 'console.log(1)'");
        allowed("curl -sS https://example.com");
        allowed("");
        allowed("   ");
    }

    #[test]
    fn archive_and_download_writers_are_caught() {
        assert!(refused("curl -o out.bin https://x").contains("curl"));
        assert!(refused("curl -O https://x/f.tgz").contains("curl"));
        assert!(refused("wget https://x/f.tgz").contains("wget"));
        assert!(refused("tar xzf f.tgz").contains("tar"));
        assert!(refused("unzip f.zip").contains("unzip"));
        assert!(refused("gzip big.log").contains("gzip"));
        allowed("curl -o /tmp/out.bin https://x");
        allowed("wget -O /tmp/f.tgz https://x/f.tgz");
        allowed("tar tzf f.tgz");
        allowed("tar xzf f.tgz -C /tmp/unpack");
        allowed("unzip -l f.zip");
        allowed("gzip -k -c big.log > /tmp/big.log.gz");
    }

    #[test]
    fn the_first_offending_segment_is_named() {
        let why = refused("cargo test && git add . && git commit -m 'x'");
        assert!(why.contains("git add"), "{why}");
        let why = refused("ls; mkdir out; touch out/x");
        assert!(why.contains("mkdir"), "{why}");
    }

    // ---- TreeSnapshot -------------------------------------------------

    fn sh(dir: &Path, cmd: &str) {
        let out = Command::new("sh")
            .args(["-c", cmd])
            .current_dir(dir)
            .env_clear()
            .envs(
                pulse_gitenv::env()
                    .iter()
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, v)| (k.to_string(), v.to_string())),
            )
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("sh runs");
        assert!(
            out.status.success(),
            "`{cmd}` failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        sh(
            dir.path(),
            "git init -q -b main && echo one > a.txt && mkdir -p sub && echo two > sub/b.txt && git add . && git commit -q -m init",
        );
        dir
    }

    #[test]
    fn snapshot_is_none_outside_a_work_tree() {
        let dir = tempfile::tempdir().unwrap();
        assert!(TreeSnapshot::take(dir.path()).is_none());
    }

    #[test]
    fn snapshot_restore_is_a_no_op_on_an_untouched_tree() {
        let dir = repo();
        let snap = TreeSnapshot::take(dir.path()).unwrap();
        assert!(snap.restore().is_empty());
    }

    #[test]
    fn snapshot_removes_new_files_and_their_empty_directories() {
        let dir = repo();
        let snap = TreeSnapshot::take(dir.path()).unwrap();
        sh(
            dir.path(),
            "mkdir -p deep/er && echo x > deep/er/new.txt && echo y > top.txt",
        );
        let reverted = snap.restore();
        assert_eq!(reverted.len(), 2, "{reverted:?}");
        assert!(reverted.iter().any(|r| r.starts_with("deep/er/new.txt")));
        assert!(reverted.iter().any(|r| r.starts_with("top.txt")));
        assert!(!dir.path().join("top.txt").exists());
        assert!(!dir.path().join("deep").exists());
        assert!(dir.path().join("a.txt").exists());
    }

    #[test]
    fn snapshot_restores_modified_and_deleted_tracked_files() {
        let dir = repo();
        let snap = TreeSnapshot::take(dir.path()).unwrap();
        sh(dir.path(), "echo changed > a.txt && rm sub/b.txt");
        let reverted = snap.restore();
        assert_eq!(reverted.len(), 2, "{reverted:?}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("sub/b.txt")).unwrap(),
            "two\n"
        );
        assert!(snap.restore().is_empty());
    }

    #[test]
    fn snapshot_undoes_a_commit_and_a_branch_switch() {
        let dir = repo();
        let head = current_head(dir.path());
        let snap = TreeSnapshot::take(dir.path()).unwrap();
        sh(
            dir.path(),
            "git checkout -q -b feature && echo three > c.txt && echo changed > a.txt && git add . && git commit -q -m feat",
        );
        let reverted = snap.restore();
        assert!(
            reverted
                .iter()
                .any(|r| r.starts_with("branch feature -> main")),
            "{reverted:?}"
        );
        assert!(
            reverted
                .iter()
                .any(|r| r == "branch feature (new, removed)"),
            "{reverted:?}"
        );
        assert_eq!(current_branch(dir.path()), "main");
        assert_eq!(current_head(dir.path()), head);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(!dir.path().join("c.txt").exists());
        assert!(git_status(dir.path()).unwrap().is_empty());
        assert!(!local_refs(dir.path()).contains_key("refs/heads/feature"));
    }

    #[test]
    fn snapshot_undoes_a_commit_on_the_current_branch_and_a_tag() {
        let dir = repo();
        let head = current_head(dir.path());
        let snap = TreeSnapshot::take(dir.path()).unwrap();
        sh(
            dir.path(),
            "echo three > c.txt && echo changed > a.txt && git add . && git commit -q -m feat && git tag v9",
        );
        let reverted = snap.restore();
        assert!(
            reverted.iter().any(|r| r.starts_with("HEAD ")),
            "{reverted:?}"
        );
        assert!(
            reverted.iter().any(|r| r == "tag v9 (new, removed)"),
            "{reverted:?}"
        );
        assert_eq!(current_head(dir.path()), head);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(!dir.path().join("c.txt").exists());
        assert!(git_status(dir.path()).unwrap().is_empty());
        assert!(snap.restore().is_empty());
    }

    #[test]
    fn snapshot_puts_back_a_deleted_branch_and_a_moved_one() {
        let dir = repo();
        sh(dir.path(), "git branch keep && git branch moved");
        let snap = TreeSnapshot::take(dir.path()).unwrap();
        sh(
            dir.path(),
            "git branch -D keep && echo x > x.txt && git add x.txt && git commit -q -m x && git branch -f moved HEAD && git reset -q --hard HEAD~1",
        );
        let reverted = snap.restore();
        assert!(
            reverted
                .iter()
                .any(|r| r == "branch keep (deleted, restored)"),
            "{reverted:?}"
        );
        assert!(
            reverted.iter().any(|r| r.starts_with("branch moved ")),
            "{reverted:?}"
        );
        let refs = local_refs(dir.path());
        assert!(refs.contains_key("refs/heads/keep"));
        assert_eq!(refs["refs/heads/moved"], refs["refs/heads/main"]);
    }

    #[test]
    fn snapshot_leaves_pre_existing_changes_alone_and_reports_discarded_ones() {
        let dir = repo();
        sh(dir.path(), "echo dirty > a.txt && echo scratch > notes.txt");
        let snap = TreeSnapshot::take(dir.path()).unwrap();
        // Nothing of ours changed: nothing to put back.
        assert!(snap.restore().is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "dirty\n"
        );
        // A command that throws the owner's dirty change away is reported.
        sh(dir.path(), "git checkout -q -- a.txt && rm notes.txt");
        let reverted = snap.restore();
        assert_eq!(reverted.len(), 2, "{reverted:?}");
        assert!(
            reverted.iter().all(|r| r.contains("cannot be put back")),
            "{reverted:?}"
        );
    }

    #[test]
    fn git_status_parses_porcelain_z() {
        let dir = repo();
        sh(
            dir.path(),
            "echo x > 'with space.txt' && echo changed > a.txt && git add a.txt",
        );
        let status = git_status(dir.path()).unwrap();
        assert_eq!(status.get("with space.txt").map(String::as_str), Some("??"));
        assert_eq!(status.get("a.txt").map(String::as_str), Some("M "));
    }
}
