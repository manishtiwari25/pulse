use anyhow::Result;

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
    let output = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .output();
    let stdout = match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout).to_string(),
        Err(e) => {
            writeln!(err, "metric command failed to start: {e}")?;
            String::new()
        }
    };
    let Some(raw) = last_number(&stdout) else {
        writeln!(out, "score: 0.0000")?;
        writeln!(err, "metric command printed no number")?;
        return Ok(());
    };
    let score = metric_score(raw, higher, scale, floor);
    writeln!(out, "score: {score:.4}")?;
    writeln!(err, "raw: {raw}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
