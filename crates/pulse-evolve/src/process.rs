use anyhow::{Result, anyhow};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Go: func commandWords(text string) ([]string, error)
pub(crate) fn command_words(text: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut active = false;
    for r in text.chars() {
        if escaped {
            word.push(r);
            escaped = false;
            active = true;
            continue;
        }
        if r == '\\' && quote != Some('\'') {
            escaped = true;
            active = true;
            continue;
        }
        if let Some(q) = quote {
            if r == q {
                quote = None;
            } else {
                word.push(r);
            }
            continue;
        }
        match r {
            '\'' | '"' => {
                quote = Some(r);
                active = true;
            }
            ' ' | '\t' | '\n' => {
                if active {
                    words.push(std::mem::take(&mut word));
                    active = false;
                }
            }
            '\0' => return Err(anyhow!("NUL in command")),
            _ => {
                word.push(r);
                active = true;
            }
        }
    }
    if quote.is_some() || escaped {
        return Err(anyhow!("unclosed quote or escape in command"));
    }
    if active {
        words.push(word);
    }
    if words.is_empty() || words[0].is_empty() {
        return Err(anyhow!("empty command"));
    }
    Ok(words)
}
/// Go: func harnessArgs(command, request string) ([]string, error)
pub(crate) fn harness_args(command: &str, request: &str) -> Result<Vec<String>> {
    let mut args = command_words(command).map_err(|_| {
        crate::fixtures::input_error(
            "Cannot start measurement harness",
            "runner_cmd is not a quoted executable and arguments",
            "pulse settings set runner_cmd copilot",
        )
    })?;
    let mut replaced = false;
    for arg in args.iter_mut().skip(1) {
        if arg == "<request>" || arg == "{prompt}" {
            *arg = request.to_string();
            replaced = true;
        }
    }
    if !replaced {
        for i in 0..args.len() {
            if args[i] == "-p" || args[i] == "--prompt" {
                if i != args.len() - 1 {
                    return Err(crate::fixtures::input_error(
                        "Cannot start measurement harness",
                        "use {prompt} for an existing prompt argument in runner_cmd",
                        "pulse settings set runner_cmd 'copilot -p {prompt} --allow-all-tools'",
                    ));
                }
                args.push(request.to_string());
                replaced = true;
                break;
            }
        }
    }
    if !replaced {
        args.push("-p".to_string());
        args.push(request.to_string());
    }
    Ok(args)
}
/// Go: func isolatedEnv(home string) []string
pub(crate) fn isolated_env(home: &str) -> Vec<String> {
    let mut env = Vec::new();
    for (key, value) in std::env::vars() {
        if key.starts_with("PULSE_") || key.starts_with("GIT_") {
            continue;
        }
        env.push(format!("{key}={value}"));
    }
    env.extend([
        format!("PULSE_HOME={home}"),
        "PULSE_JOB=measurement".to_string(),
        "GIT_CONFIG_NOSYSTEM=1".to_string(),
        format!("GIT_CONFIG_GLOBAL={}", std::env::consts::EXE_SUFFIX).replace('\0', ""),
        "GIT_TERMINAL_PROMPT=0".to_string(),
        "GIT_OPTIONAL_LOCKS=0".to_string(),
    ]);
    if let Some(item) = env.iter_mut().find(|v| v.starts_with("GIT_CONFIG_GLOBAL=")) {
        *item = format!(
            "GIT_CONFIG_GLOBAL={}",
            std::path::Path::new("/dev/null").display()
        );
    }
    env
}
#[derive(Debug, Default)]
pub(crate) struct BoundedOutput {
    pub(crate) buf: Vec<u8>,
    pub(crate) limit: usize,
    pub(crate) overflow: bool,
}
impl Write for BoundedOutput {
    /// Go: func (b *boundedOutput) Write(p []byte) (int, error)
    fn write(&mut self, p: &[u8]) -> std::io::Result<usize> {
        let n = p.len();
        let remaining = self.limit.saturating_sub(self.buf.len());
        if n > remaining {
            self.overflow = true;
        }
        self.buf.extend_from_slice(&p[..p.len().min(remaining)]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
/// Go: func runProcess(ctx context.Context, cwd string, env, argv []string, stdout io.Writer) error
pub(crate) fn run_process(
    cancel: &pulse_bus::CancellationToken,
    cwd: &str,
    env: &[String],
    argv: &[String],
    stdout: &mut dyn Write,
) -> Result<()> {
    run_process_to(cancel, cwd, env, argv, stdout, &mut std::io::sink())
}
/// Go: func runProcessTo(ctx context.Context, cwd string, env, argv []string, stdout, stderr io.Writer) error
pub(crate) fn run_process_to(
    cancel: &pulse_bus::CancellationToken,
    cwd: &str,
    env: &[String],
    argv: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(anyhow!("context canceled"));
    }
    if argv.is_empty() {
        return Err(anyhow!("empty command"));
    }
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(cwd);
    cmd.env_clear();
    for entry in env {
        if let Some((k, v)) = entry.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let cleanup = contain_process(&mut cmd);
    let mut child = cmd.spawn()?;
    loop {
        if cancel.is_cancelled() {
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .arg("-KILL")
                    .arg(format!("-{}", child.id()))
                    .status();
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill();
            }
            cleanup();
            let _ = child.wait();
            return Err(anyhow!("context canceled"));
        }
        if let Some(status) = child.try_wait()? {
            let output = child.wait_with_output()?;
            stdout.write_all(&output.stdout)?;
            stderr.write_all(&output.stderr)?;
            return if status.success() {
                Ok(())
            } else {
                Err(anyhow!("exit status: {status}"))
            };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
/// Go: func gitOutput(ctx context.Context, repo string, args ...string) (string, error)
pub(crate) fn git_output(
    cancel: &pulse_bus::CancellationToken,
    repo: &str,
    args: &[String],
) -> Result<String> {
    Ok(git_raw(cancel, repo, args)?.trim().to_string())
}
/// Go: func gitRaw(ctx context.Context, repo string, args ...string) (string, error)
pub(crate) fn git_raw(
    cancel: &pulse_bus::CancellationToken,
    repo: &str,
    args: &[String],
) -> Result<String> {
    let mut out = BoundedOutput {
        limit: 8 << 20,
        ..BoundedOutput::default()
    };
    let mut argv = vec![
        "git".to_string(),
        "-c".to_string(),
        format!(
            "core.hooksPath={}",
            std::path::Path::new("/dev/null").display()
        ),
    ];
    argv.extend(args.iter().cloned());
    run_process(cancel, repo, &pulse_gitenv::env(), &argv, &mut out)?;
    if out.overflow {
        return Err(anyhow!("git output exceeds the diagnostic limit"));
    }
    Ok(String::from_utf8_lossy(&out.buf).to_string())
}
#[cfg(unix)]
/// Go: func containProcess(cmd *exec.Cmd) func()
pub(crate) fn contain_process(cmd: &mut std::process::Command) -> Box<dyn FnOnce()> {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    Box::new(|| {})
}
#[cfg(not(unix))]
/// Go: func containProcess(cmd *exec.Cmd) func()
pub(crate) fn contain_process(cmd: &mut std::process::Command) -> Box<dyn FnOnce()> {
    Box::new(|| {})
}
#[cfg(unix)]
/// Go: func multipleLinks(info os.FileInfo) bool
pub(crate) fn multiple_links(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.is_file() && metadata.nlink() > 1
}
#[cfg(not(unix))]
/// Go: func multipleLinks(info os.FileInfo) bool
pub(crate) fn multiple_links(metadata: &std::fs::Metadata) -> bool {
    let _ = metadata;
    false
}
#[cfg(unix)]
/// Go: func stopSignals() []os.Signal
#[allow(dead_code)]
pub(crate) fn stop_signals() -> Vec<i32> {
    vec![2, 15]
}
#[cfg(not(unix))]
/// Go: func stopSignals() []os.Signal
pub(crate) fn stop_signals() -> Vec<i32> {
    vec![2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    #[cfg(unix)]
    fn process_cancellation_stops_descendant_work() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = pulse_bus::CancellationToken::new();
        let (tx, rx) = mpsc::channel();
        let cwd = dir.path().display().to_string();
        let env = isolated_env(&dir.path().join("empty").display().to_string());
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            "(sleep 1; printf escaped > descendant-survived) & printf ready > started; wait"
                .to_string(),
        ];
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            let mut sink = std::io::sink();
            let err = run_process(&c2, &cwd, &env, &argv, &mut sink)
                .expect_err("process unexpectedly succeeded");
            tx.send(err).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(4);
        while !dir.path().join("started").exists() {
            assert!(Instant::now() < deadline, "temporary process did not start");
            assert!(
                rx.try_recv().is_err(),
                "temporary process exited before cancellation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        cancel.cancel();
        let err = rx.recv_timeout(Duration::from_secs(8)).unwrap();
        assert!(err.to_string().contains("context canceled"));
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!dir.path().join("descendant-survived").exists());
    }
}
