use std::path::Path;

/// Go: func landingStop(ctx context.Context, s *store.Store, root string) string
pub(crate) fn landing_stop(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    root: String,
) -> String {
    let why = crate::run::runner_stopped(s);
    if !why.is_empty() {
        return why;
    }
    if crate::util::file_exists(
        &Path::new(&root)
            .join("docs")
            .join("evolution")
            .join("STOP")
            .display()
            .to_string(),
    ) {
        return format!("docs/evolution/STOP is present in {root}");
    }
    if ctx.is_cancelled() {
        return "cancelled".to_string();
    }
    String::new()
}

/// Go: func shWithContext(ctx context.Context, dir, cmd string, budget time.Duration) (rc *int, stdout, stderr string)
pub(crate) fn sh_with_context(
    ctx: &pulse_bus::CancellationToken,
    dir: String,
    cmd: String,
    budget: std::time::Duration,
) -> (Option<Box<i32>>, String, String) {
    let argv = vec!["/bin/sh".to_string(), "-c".to_string(), cmd];
    match crate::proc::run_group(ctx, &argv, &dir, &crate::proc::environ(), &[], budget) {
        Err(e) => (Some(Box::new(127)), String::new(), e.to_string()),
        Ok(out) if out.timeout => (None, out.stdout, out.stderr),
        Ok(out) => (Some(Box::new(out.exit_code)), out.stdout, out.stderr),
    }
}
