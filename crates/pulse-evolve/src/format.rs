/// Go: func formatDelta(verdict string, delta, tolerance float64) string
pub(crate) fn format_delta(verdict: &str, delta: f64, tolerance: f64) -> String {
    format!("{verdict}: variant {delta:+.4} vs baseline (tolerance {tolerance:.4})")
}
