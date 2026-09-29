use crate::harness::{Environment, Observation};
use pulse_provider::CancelToken;
use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct LocalEnv {
    cwd: String,
    env: Vec<String>,
}

pub fn new_local_env(cwd: impl Into<String>, env: Option<Vec<String>>) -> LocalEnv {
    let env = env.unwrap_or_else(|| std::env::vars().map(|(k, v)| format!("{k}={v}")).collect());
    LocalEnv {
        cwd: cwd.into(),
        env,
    }
}

pub use new_local_env as NewLocalEnv;

impl LocalEnv {
    pub fn new(cwd: impl Into<String>, env: Option<Vec<String>>) -> Self {
        new_local_env(cwd, env)
    }
}

impl Environment for LocalEnv {
    fn execute(&mut self, cancel: &CancelToken, command: &str, timeout: Duration) -> Observation {
        let start = Instant::now();
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();
        for kv in &self.env {
            if let Some((k, v)) = kv.split_once('=') {
                cmd.env(k, v);
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    libc::setpgid(0, 0);
                    Ok(())
                });
            }
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Observation {
                    output: e.to_string(),
                    exit_code: 127,
                    duration: start.elapsed(),
                    timed_out: false,
                };
            }
        };
        let pid = child.id() as i32;
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let out_handle = thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut r) = stdout.take() {
                let _ = r.read_to_end(&mut b);
            }
            b
        });
        let err_handle = thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut r) = stderr.take() {
                let _ = r.read_to_end(&mut b);
            }
            b
        });
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut data = out_handle.join().unwrap_or_default();
                    data.extend(err_handle.join().unwrap_or_default());
                    return Observation {
                        output: String::from_utf8_lossy(&data).to_string(),
                        exit_code: exit_code(status),
                        timed_out: false,
                        duration: start.elapsed(),
                    };
                }
                Ok(None) => {}
                Err(e) => {
                    return Observation {
                        output: e.to_string(),
                        exit_code: 127,
                        timed_out: false,
                        duration: start.elapsed(),
                    };
                }
            }
            if cancel.is_cancelled() || (!timeout.is_zero() && start.elapsed() >= timeout) {
                kill_process_group(pid);
                let _ = child.wait();
                let mut data = out_handle.join().unwrap_or_default();
                data.extend(err_handle.join().unwrap_or_default());
                return Observation {
                    output: String::from_utf8_lossy(&data).to_string(),
                    exit_code: -1,
                    timed_out: true,
                    duration: start.elapsed(),
                };
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn info(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("cwd".to_string(), self.cwd.clone()),
            ("os".to_string(), std::env::consts::OS.to_string()),
            ("shell".to_string(), "/bin/sh".to_string()),
        ])
    }
}

pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(sig) = status.signal() {
                return 128 + sig;
            }
        }
        127
    })
}

pub fn kill_process_group(pid: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    thread::sleep(Duration::from_millis(100));
    #[cfg(unix)]
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}
