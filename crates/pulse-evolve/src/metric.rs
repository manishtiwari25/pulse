use anyhow::Result;
use std::path::{Path, PathBuf};

/// The diff scope a scorer command runs in, read from the environment the
/// measurement exports: the changed-file list (`PULSE_MEASURE_CHANGED`) and
/// the frozen base checkout the scale is taken from
/// (`PULSE_MEASURE_BASE_TREE`), when there is one.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricScope {
    pub changed: PathBuf,
    pub base_tree: Option<PathBuf>,
}

/// The scope the measurement exported, or `None` for a whole-tree run:
/// `PULSE_MEASURE_CHANGED` unset, unreadable or naming no file.
pub fn metric_scope_from_env() -> Option<MetricScope> {
    let changed = PathBuf::from(std::env::var_os("PULSE_MEASURE_CHANGED")?);
    let text = std::fs::read_to_string(&changed).ok()?;
    if text.lines().all(|line| line.trim().is_empty()) {
        return None;
    }
    let base_tree = std::env::var_os("PULSE_MEASURE_BASE_TREE")
        .map(PathBuf::from)
        .filter(|p| p.is_dir());
    Some(MetricScope { changed, base_tree })
}

/// The scale a diff-scoped metric takes from the base tree's count, so the
/// base scores 0.5 and the variant's delta is relative to what the card
/// touched: the count itself for a lower-is-better metric (`S/(S+raw)`),
/// twice the count for a higher-is-better one (`min(1, raw/S)`); a count
/// below 1 counts as 1, so a card that brings the first test or the first
/// marker still moves the score.
pub fn diff_scale(higher: bool, base_raw: f64) -> f64 {
    let base = if base_raw.is_finite() {
        base_raw.max(0.0).round().max(1.0)
    } else {
        1.0
    };
    if higher { base * 2.0 } else { base }
}

pub fn metric_score(raw: f64, higher: bool, scale: f64, floor: Option<f64>) -> f64 {
    let raw = if raw.is_finite() { raw.max(0.0) } else { 0.0 };
    let mut score = if higher {
        (raw / scale).min(1.0)
    } else {
        scale / (scale + raw)
    };
    if let Some(floor) = floor {
        score = score.max(floor);
    }
    score.clamp(0.0, 1.0)
}

pub fn last_number(text: &str) -> Option<f64> {
    let plain = strip_ansi(text);
    let bytes = plain.as_bytes();
    let mut out = None;
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if matches!(bytes[i], b'+' | b'-') {
            i += 1;
        }
        let digits_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let mut has_digit = i > digits_start;
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            let frac_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            has_digit |= i > frac_start;
        }
        if has_digit
            && !adjacent_number_char(bytes, start.wrapping_sub(1), start > 0)
            && !adjacent_number_char(bytes, i, i < bytes.len())
            && let Ok(v) = plain[start..i].parse::<f64>()
        {
            out = Some(v);
        }
        if i == start {
            i += 1;
        }
    }
    out
}

fn adjacent_number_char(bytes: &[u8], i: usize, valid: bool) -> bool {
    valid && matches!(bytes[i], b',' | b'.' | b'0'..=b'9')
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

pub fn run_metric_score(
    out: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
    higher: bool,
    scale: f64,
    floor: Option<f64>,
    argv: &[String],
) -> Result<()> {
    run_metric_score_in(
        out,
        err,
        higher,
        scale,
        floor,
        argv,
        None,
        metric_scope_from_env().as_ref(),
    )
}

/// What a metric command printed, run from `cwd` (the current directory
/// when `None`) with the scorer's own environment.
fn command_stdout(argv: &[String], cwd: Option<&Path>, err: &mut dyn std::io::Write) -> String {
    let mut command = std::process::Command::new(&argv[0]);
    command.args(&argv[1..]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    match command.output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout).to_string(),
        Err(e) => {
            let _ = writeln!(err, "metric command failed to start: {e}");
            String::new()
        }
    }
}

/// `pulse evolve score metric` on `tree` (the current directory when
/// `None`) with an explicit scope. In a scope with a base tree the command
/// runs on both trees and the base's count sets the scale ([`diff_scale`])
/// in place of `--scale`; the command itself reads `PULSE_MEASURE_CHANGED`
/// to count inside the changed files, as the derived fixtures' scripts do.
/// The score line is followed by `scope: diff` so the measurement records
/// the fixture as diff-scoped.
#[allow(clippy::too_many_arguments)]
pub fn run_metric_score_in(
    out: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
    higher: bool,
    scale: f64,
    floor: Option<f64>,
    argv: &[String],
    tree: Option<&Path>,
    scope: Option<&MetricScope>,
) -> Result<()> {
    if !crate::fixtures::finite(scale) || scale <= 0.0 {
        return Err(crate::fixtures::input_error(
            "Cannot score metric",
            "scale must be a finite positive number",
            "pulse evolve score metric --higher --scale S -- CMD ARGS...",
        ));
    }
    if let Some(floor) = floor
        && (!crate::fixtures::finite(floor) || !(0.0..=1.0).contains(&floor))
    {
        return Err(crate::fixtures::input_error(
            "Cannot score metric",
            "floor must be a finite number in [0,1]",
            "pulse evolve score metric --higher --scale S --floor F -- CMD ARGS...",
        ));
    }
    if argv.is_empty() {
        return Err(crate::fixtures::input_error(
            "Cannot score metric",
            "a command after -- is required",
            "pulse evolve score metric --higher --scale S -- CMD ARGS...",
        ));
    }
    let stdout = command_stdout(argv, tree, err);
    let Some(raw) = last_number(&stdout) else {
        writeln!(out, "score: 0.0000")?;
        if scope.is_some() {
            writeln!(out, "scope: diff")?;
        }
        writeln!(err, "metric command printed no number")?;
        return Ok(());
    };
    let (scale, note) = match scope {
        Some(MetricScope {
            base_tree: Some(base_tree),
            ..
        }) => {
            let base_raw = last_number(&command_stdout(argv, Some(base_tree), err)).unwrap_or(0.0);
            let scale = diff_scale(higher, base_raw);
            (
                scale,
                format!(" (diff-scoped: base {base_raw}, scale {scale})"),
            )
        }
        Some(_) => (
            scale,
            " (diff-scoped: no base tree, scale as given)".to_string(),
        ),
        None => (scale, String::new()),
    };
    let score = metric_score(raw, higher, scale, floor);
    writeln!(out, "score: {score:.4}")?;
    if scope.is_some() {
        writeln!(out, "scope: diff")?;
    }
    writeln!(err, "raw: {raw}{note}")?;
    Ok(())
}

/// `pulse evolve score check -- CMD ARGS...`: the card-check scorer. The
/// command runs from the tree root; a single integer on stdout is a
/// lower-is-better count scored against the base tree's
/// (`PULSE_MEASURE_BASE_TREE`, the measurement's diff scope) with
/// [`diff_scale`], whatever the exit status (`grep -c` exits 1 on a count
/// of 0); anything else scores the exit status, 1 for success and 0 for a
/// failure.
pub fn run_check_score(
    out: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
    argv: &[String],
) -> Result<()> {
    let base_tree = std::env::var_os("PULSE_MEASURE_BASE_TREE")
        .map(PathBuf::from)
        .filter(|p| p.is_dir());
    run_check_score_in(out, err, argv, None, base_tree.as_deref())
}

/// What a check printed and whether it passed, run from `cwd` (the current
/// directory when `None`); a command that cannot start fails.
fn check_result(
    argv: &[String],
    cwd: Option<&Path>,
    err: &mut dyn std::io::Write,
) -> (bool, String) {
    let mut command = std::process::Command::new(&argv[0]);
    command.args(&argv[1..]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    match command.output() {
        Ok(output) => (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).to_string(),
        ),
        Err(e) => {
            let _ = writeln!(err, "check failed to start: {e}");
            (false, String::new())
        }
    }
}

/// Stdout that is one integer and nothing else.
fn single_integer(stdout: &str) -> Option<f64> {
    strip_ansi(stdout)
        .trim()
        .parse::<u64>()
        .ok()
        .map(|n| n as f64)
}

/// [`run_check_score`] on `tree` against an explicit base tree.
pub fn run_check_score_in(
    out: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
    argv: &[String],
    tree: Option<&Path>,
    base_tree: Option<&Path>,
) -> Result<()> {
    if argv.is_empty() {
        return Err(crate::fixtures::input_error(
            "Cannot score check",
            "a command after -- is required",
            "pulse evolve score check -- CMD ARGS...",
        ));
    }
    let (passed, stdout) = check_result(argv, tree, err);
    match single_integer(&stdout) {
        Some(raw) => {
            let base_raw = base_tree
                .and_then(|base| single_integer(&check_result(argv, Some(base), err).1))
                .unwrap_or(raw);
            let scale = diff_scale(false, base_raw);
            writeln!(out, "score: {:.4}", metric_score(raw, false, scale, None))?;
            writeln!(err, "count: {raw} (base {base_raw}, scale {scale})")?;
        }
        None => {
            writeln!(out, "score: {:.4}", if passed { 1.0 } else { 0.0 })?;
            writeln!(
                err,
                "{}",
                if passed {
                    "the check passed"
                } else {
                    "the check failed"
                }
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked(argv: &[&str], tree: Option<&Path>, base: Option<&Path>) -> (String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        run_check_score_in(&mut out, &mut err, &argv, tree, base).unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn the_card_check_scores_an_exit_status_or_a_falling_count() {
        assert_eq!(
            checked(&["sh", "-c", "exit 0"], None, None).0,
            "score: 1.0000\n"
        );
        let (out, err) = checked(&["sh", "-c", "echo not a number; exit 1"], None, None);
        assert_eq!(out, "score: 0.0000\n");
        assert_eq!(err, "the check failed\n");
        // A count, with no base tree, sits at 0.5 on its own.
        let (out, err) = checked(&["echo", "3"], None, None);
        assert_eq!(out, "score: 0.5000\n");
        assert_eq!(err, "count: 3 (base 3, scale 3)\n");
        // Against a base tree the count is read there too: 6 on the base,
        // 3 on the variant, so the variant scores above 0.5; a count that
        // exits non-zero (grep -c with no match) is still a count.
        let base = crate::test_helpers::temp_path();
        let variant = crate::test_helpers::temp_path();
        std::fs::write(base.join("n"), "6").unwrap();
        std::fs::write(variant.join("n"), "3").unwrap();
        let count = ["sh", "-c", "cat n; exit 1"];
        let (out, err) = checked(&count, Some(&variant), Some(&base));
        assert_eq!(out, "score: 0.6667\n");
        assert_eq!(err, "count: 3 (base 6, scale 6)\n");
        let (out, _) = checked(&count, Some(&base), Some(&base));
        assert_eq!(out, "score: 0.5000\n");
        std::fs::write(variant.join("n"), "0").unwrap();
        assert_eq!(
            checked(&count, Some(&variant), Some(&base)).0,
            "score: 1.0000\n"
        );
        // A command that cannot start fails the check.
        let (out, err) = checked(&["/nonexistent/check"], None, None);
        assert_eq!(out, "score: 0.0000\n");
        assert!(err.starts_with("check failed to start:"), "{err}");
        assert!(run_check_score_in(&mut Vec::new(), &mut Vec::new(), &[], None, None).is_err());
    }

    #[test]
    fn metric_score_boundaries() {
        assert_eq!(metric_score(0.0, true, 10.0, None), 0.0);
        assert_eq!(metric_score(5.0, true, 10.0, None), 0.5);
        assert_eq!(metric_score(20.0, true, 10.0, None), 1.0);
        assert_eq!(metric_score(0.0, false, 10.0, None), 1.0);
        assert_eq!(metric_score(10.0, false, 10.0, None), 0.5);
        assert_eq!(metric_score(-2.0, true, 10.0, Some(0.25)), 0.25);
        assert_eq!(metric_score(f64::NAN, false, 10.0, None), 1.0);
    }

    #[test]
    fn diff_scale_puts_the_base_at_half() {
        assert_eq!(diff_scale(true, 7.0), 14.0);
        assert_eq!(metric_score(7.0, true, diff_scale(true, 7.0), None), 0.5);
        assert_eq!(diff_scale(false, 7.0), 7.0);
        assert_eq!(metric_score(7.0, false, diff_scale(false, 7.0), None), 0.5);
        // Nothing on the base yet: the first test or marker still moves it.
        assert_eq!(diff_scale(true, 0.0), 2.0);
        assert_eq!(diff_scale(false, 0.0), 1.0);
        assert_eq!(diff_scale(true, f64::NAN), 2.0);
        assert_eq!(diff_scale(false, -3.0), 1.0);
        assert_eq!(diff_scale(false, 2.6), 3.0);
    }

    /// A card that brings one test to a file with two is a visible step in
    /// its own scope and noise against the whole tree.
    #[test]
    fn a_diff_scoped_delta_is_kept_as_an_improvement() {
        let tolerance = 0.02;
        let whole = |raw: f64| crate::api::Scores {
            total: metric_score(raw, true, 3916.0, None),
            ..Default::default()
        };
        let (kept, why) = crate::api::keep(Some(&whole(2339.0)), Some(&whole(2340.0)), tolerance);
        assert!(kept && why.starts_with("neutral"), "{why}");
        let scoped = |raw: f64| crate::api::Scores {
            total: metric_score(raw, true, diff_scale(true, 2.0), None),
            diff_scoped: vec!["derived-tests".into()],
            ..Default::default()
        };
        assert_eq!(scoped(2.0).total, 0.5);
        let (kept, why) = crate::api::keep(Some(&scoped(2.0)), Some(&scoped(3.0)), tolerance);
        assert!(kept && why.starts_with("improved"), "{why}");
        let (kept, why) = crate::api::keep(Some(&scoped(2.0)), Some(&scoped(1.0)), tolerance);
        assert!(!kept && why.starts_with("discarded"), "{why}");
    }

    fn scored(
        higher: bool,
        argv: &[&str],
        tree: &Path,
        scope: Option<&MetricScope>,
    ) -> (String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        run_metric_score_in(
            &mut out,
            &mut err,
            higher,
            100.0,
            None,
            &argv,
            Some(tree),
            scope,
        )
        .unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn metric_scorer_counts_the_changed_files_against_the_base_tree() {
        let base = crate::test_helpers::temp_path();
        let variant = crate::test_helpers::temp_path();
        let list = crate::test_helpers::temp_path().join("changed");
        std::fs::write(&list, "a.txt\n").unwrap();
        // Two files, one changed: the changed file has 4 markers on the base
        // and 1 on the variant, the other 2 on both.
        for (tree, a) in [(&base, "x\nx\nx\nx\n"), (&variant, "x\n")] {
            std::fs::write(tree.join("a.txt"), a).unwrap();
            std::fs::write(tree.join("b.txt"), "x\nx\n").unwrap();
        }
        let whole = ["sh", "-c", "cat a.txt b.txt | grep -c x"];
        let list_arg = list.display().to_string();
        // What a scope-aware command does with the list the measurement
        // exports; the list comes as an argument here.
        let in_list = ["sh", "-c", "cat $(cat \"$1\") | grep -c x", "sh", &list_arg];
        // Whole tree: --scale as given, no scope line.
        let (out, err) = scored(false, &whole, &variant, None);
        assert_eq!(out, "score: 0.9709\n");
        assert_eq!(err, "raw: 3\n");
        // A scope with a base tree: the base's count in the list is the
        // scale, so the base scores 0.5 and the variant above it.
        let scope = MetricScope {
            changed: list.clone(),
            base_tree: Some(base.clone()),
        };
        let (out, err) = scored(false, &in_list, &variant, Some(&scope));
        assert_eq!(out, "score: 0.8000\nscope: diff\n");
        assert_eq!(err, "raw: 1 (diff-scoped: base 4, scale 4)\n");
        let (out, _) = scored(false, &in_list, &base, Some(&scope));
        assert_eq!(out, "score: 0.5000\nscope: diff\n");
        let (out, err) = scored(true, &in_list, &variant, Some(&scope));
        assert_eq!(out, "score: 0.1250\nscope: diff\n");
        assert_eq!(err, "raw: 1 (diff-scoped: base 4, scale 8)\n");
        // A scope without a base tree keeps --scale and still says so.
        let bare = MetricScope {
            changed: list.clone(),
            base_tree: None,
        };
        let (out, err) = scored(false, &in_list, &variant, Some(&bare));
        assert_eq!(out, "score: 0.9901\nscope: diff\n");
        assert_eq!(err, "raw: 1 (diff-scoped: no base tree, scale as given)\n");
    }

    #[test]
    fn last_number_finds_plain_numbers_and_ignores_ansi() {
        assert_eq!(last_number("no numbers"), None);
        assert_eq!(last_number("first 1 then .25"), Some(0.25));
        assert_eq!(last_number("\x1b[31mraw: -2.5\x1b[0m"), Some(-2.5));
        assert_eq!(last_number("1,234"), None);
    }

    #[test]
    fn metric_runner_validates_scale() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let got = run_metric_score(&mut out, &mut err, true, 0.0, None, &["echo".into()]);
        assert!(got.is_err());
        assert!(got.unwrap_err().to_string().contains("scale must"));
    }
}
