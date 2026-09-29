//! Skeleton port of internal/runner/heldout.go.

use anyhow::anyhow;
use std::sync::{LazyLock, RwLock};

#[derive(Debug, Clone, Default)]
pub(crate) struct HeldoutGen {
    pub(crate) inflight: bool,
    pub(crate) last_failure_at: String,
    pub(crate) lines: Vec<String>,
}

pub(crate) static HELDOUT_GEN: LazyLock<parking_lot::Mutex<HeldoutGen>> =
    LazyLock::new(|| parking_lot::Mutex::new(HeldoutGen::default()));
pub(crate) type SealedCallFn = dyn Fn(&pulse_bus::CancellationToken, &pulse_store::Store, String) -> anyhow::Result<String>
    + Send
    + Sync;
pub(crate) static SEALED_CALL: LazyLock<RwLock<Box<SealedCallFn>>> =
    LazyLock::new(|| RwLock::new(Box::new(run_sealed_call)));
pub(crate) fn set_sealed_call(f: Box<SealedCallFn>) {
    *SEALED_CALL.write().expect("sealed call lock") = f;
}

pub(crate) const HELDOUT_RETRY_MINUTES: i32 = 60;
pub(crate) const HELDOUT_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3 * 60);

/// Go: func runSealedCall(ctx context.Context, s *store.Store, prompt string) (string, error)
pub(crate) fn run_sealed_call(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    prompt: String,
) -> anyhow::Result<String> {
    if s.harness_native() {
        let p = pulse_provider::open(
            &s.setting("provider", pulse_provider::DEFAULT),
            pulse_provider::Options {
                home: s.home.clone(),
                settings: s.provider_settings(),
                ..Default::default()
            },
        )?;
        let provider_cancel = pulse_provider::CancelToken::new();
        if cancel.is_cancelled() {
            provider_cancel.cancel();
        }
        let resp = p.chat(
            &provider_cancel,
            pulse_provider::Request {
                model: s.light_model(),
                messages: vec![pulse_provider::Message {
                    role: pulse_provider::Role::USER,
                    content: prompt,
                    tool_calls: vec![],
                    tool_call_id: String::new(),
                    name: String::new(),
                }],
                tools: vec![],
                initiator: "user".into(),
                ..Default::default()
            },
        )?;
        return Ok(resp
            .message
            .map(|m| m.content)
            .unwrap_or_default()
            .trim()
            .to_string());
    }
    let dir = std::env::temp_dir().join(format!("pulse-heldout-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).map_err(|_| anyhow!("cannot create an empty working directory"))?;
    let mut args = vec![
        "-p".to_string(),
        prompt,
        "--no-ask-user".to_string(),
        "--disable-builtin-mcps".to_string(),
        "-s".to_string(),
    ];
    let model = s.light_model();
    if !model.is_empty() {
        args.extend(["--model".to_string(), model]);
    }
    let mut cmd = std::process::Command::new(crate::executor::runner_cmd(s));
    cmd.args(args).current_dir(&dir).env_clear();
    for kv in sealed_env(
        &std::env::vars()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>(),
    ) {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    let out = cmd.output()?;
    if !out.status.success() {
        let mut line = crate::executor::fault_line(&String::from_utf8_lossy(&out.stderr), 200);
        if cancel.deadline_exceeded() {
            line = "the model call timed out".into();
        }
        if line.is_empty() {
            line = format!("exit {}", out.status.code().unwrap_or(-1));
        }
        let _ = std::fs::remove_dir_all(&dir);
        anyhow::bail!(
            "{}",
            crate::executor::cut(&crate::compute::redact_secrets(&line), 200)
        );
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(text)
}

/// Go: func sealedEnv(environ []string) []string
pub(crate) fn sealed_env(environ: &[String]) -> Vec<String> {
    environ
        .iter()
        .filter(|entry| {
            let key = entry.split_once('=').map(|(k, _)| k).unwrap_or(entry);
            matches!(
                key,
                "PATH" | "HOME" | "TMPDIR" | "LANG" | "GH_TOKEN" | "GITHUB_TOKEN"
            ) || key.starts_with("COPILOT_")
        })
        .cloned()
        .collect()
}

/// Go: func tendHeldout(s *store.Store, now string) []string
pub(crate) fn tend_heldout(s: &pulse_store::Store, now: String) -> Vec<String> {
    let mut g = HELDOUT_GEN.lock();
    let lines = std::mem::take(&mut g.lines);
    if g.inflight || pulse_evolve::heldout_mode(s) != "auto" {
        return lines;
    }
    if !g.last_failure_at.is_empty()
        && crate::util::must_iso(&now)
            < crate::util::must_iso(&crate::util::plus_minutes(
                &g.last_failure_at,
                HELDOUT_RETRY_MINUTES as f64,
            ))
    {
        return lines;
    }
    if pulse_evolve::heldout_complete(pulse_evolve::heldout_count(s)) {
        return lines;
    }
    g.inflight = true;
    let s = s.clone();
    std::thread::spawn(move || generate_heldout(&s, now));
    lines
}

/// Go: func generateHeldout(s *store.Store, now string)
pub(crate) fn generate_heldout(s: &pulse_store::Store, now: String) {
    let ctx = pulse_bus::CancellationToken::new().with_timeout(HELDOUT_CALL_TIMEOUT);
    let result = pulse_evolve::generate(
        &ctx,
        s,
        |ctx, prompt| (SEALED_CALL.read().expect("sealed call lock"))(ctx, s, prompt.to_string()),
        &now,
    );
    let mut g = HELDOUT_GEN.lock();
    g.inflight = false;
    match result {
        Err(e) => {
            g.last_failure_at = now;
            g.lines.push(format!(
                "held-out: generation failed: {}",
                heldout_failure(e)
            ));
        }
        Ok(r) if r.wrote => {
            g.last_failure_at.clear();
            g.lines.push(format!(
                "held-out: generated fixture {} of {} (judge {})",
                r.count,
                2_i64.max(r.count + r.pending),
                r.judge
            ));
            if let Ok(conn) = s.conn() {
                crate::util::event_at(
                    &conn,
                    &now,
                    "daemon",
                    "evolve.heldout",
                    "",
                    serde_json::json!({"generated": r.generated, "count": r.count, "pending": r.pending}),
                );
            }
        }
        Ok(_) => {}
    }
}

/// Go: func heldoutFailure(err error) string
pub(crate) fn heldout_failure(err: anyhow::Error) -> String {
    let mut text = crate::run::err_message(&err);
    if let Some(ce) = err.downcast_ref::<pulse_cli::Error>() {
        text = ce.what.clone();
        if !ce.why.is_empty() {
            text.push_str(": ");
            text.push_str(&ce.why);
        }
    }
    crate::executor::cut(
        &crate::compute::redact_secrets(&crate::executor::fault_line(&text, 200)),
        200,
    )
}
