//! Port of internal/runner/steps_mirror.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub(crate) struct RemoteStepsMirror {
    pub(crate) e: Option<Box<crate::exec::SshExec>>,
    pub(crate) id: String,
    pub(crate) offset: i64,
    pub(crate) pending: Vec<u8>,
    pub(crate) begun: bool,
    pub(crate) finished: bool,
    pub(crate) reported: bool,
    pub(crate) stopped: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(dead_code)]
pub(crate) struct RemoteStepsBegin {
    #[serde(rename = "event")]
    pub event: String,
    #[serde(rename = "session")]
    pub session: String,
    #[serde(rename = "run_id")]
    pub run_id: String,
    #[serde(rename = "job_id")]
    pub job_id: String,
    #[serde(rename = "agent")]
    pub agent: String,
    #[serde(rename = "provider")]
    pub provider: String,
    #[serde(rename = "model")]
    pub model: String,
    #[serde(rename = "cwd")]
    pub cwd: String,
    #[serde(rename = "at")]
    pub at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(dead_code)]
pub(crate) struct RemoteStepsStep {
    #[serde(rename = "event")]
    pub event: String,
    #[serde(rename = "n")]
    pub n: i64,
    #[serde(rename = "at")]
    pub at: String,
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "content")]
    pub content: String,
    #[serde(rename = "command", default)]
    pub command: String,
    #[serde(rename = "exit_code", default)]
    pub exit: Option<i64>,
    #[serde(rename = "duration_ms", default)]
    pub ms: i64,
    #[serde(rename = "usage", default)]
    pub usage: pulse_provider::Usage,
    #[serde(rename = "model", default)]
    pub model: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(dead_code)]
pub(crate) struct RemoteStepsFinish {
    #[serde(rename = "event")]
    pub event: String,
    #[serde(rename = "exit_status")]
    pub exit_status: String,
    #[serde(rename = "submission")]
    pub submission: String,
    #[serde(rename = "error")]
    pub error: String,
    #[serde(rename = "cost")]
    pub cost: f64,
    #[serde(rename = "steps")]
    pub steps: i64,
    #[serde(rename = "at")]
    pub at: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RemoteStepsHead {
    event: String,
}

/// Go: func newRemoteStepsMirror(e *sshExec, id string) *remoteStepsMirror
#[allow(dead_code)]
pub(crate) fn new_remote_steps_mirror(
    e: Option<crate::exec::SshExec>,
    id: String,
) -> Option<RemoteStepsMirror> {
    Some(RemoteStepsMirror {
        e: e.map(Box::new),
        id,
        ..Default::default()
    })
}

#[allow(dead_code)]
fn open_store(e: &crate::exec::SshExec) -> Result<pulse_store::Store> {
    pulse_store::Store::open(pulse_store::Options {
        home: e.local.home.clone(),
        ..Default::default()
    })
}

#[allow(dead_code)]
impl RemoteStepsMirror {
    /// Go: func (m *remoteStepsMirror) watch(ctx context.Context)
    pub(crate) fn watch(&mut self, cancel: &pulse_bus::CancellationToken) {
        let mut period = Duration::from_secs(1);
        let (mut ticks, mut stop) = crate::heartbeat::HEARTBEAT_TICKS
            .read()
            .expect("heartbeat ticks lock")(period);
        loop {
            if cancel.is_cancelled() || self.stopped {
                stop();
                return;
            }
            if self.fetch(cancel) && period != pulse_board::HEARTBEAT_INTERVAL {
                stop();
                period = pulse_board::HEARTBEAT_INTERVAL;
                (ticks, stop) = crate::heartbeat::HEARTBEAT_TICKS
                    .read()
                    .expect("heartbeat ticks lock")(period);
            }
            if cancel.is_cancelled() || ticks.recv().is_err() {
                stop();
                return;
            }
        }
    }
    /// Go: func (m *remoteStepsMirror) fetch(ctx context.Context) bool
    pub(crate) fn fetch(&mut self, cancel: &pulse_bus::CancellationToken) -> bool {
        if self.stopped {
            return false;
        }
        let Some(e) = self.e.as_ref() else {
            return false;
        };
        let args = vec![
            "steps".to_string(),
            e.local.job.str("id"),
            self.id.clone(),
            self.offset.to_string(),
        ];
        let res = crate::executor::ssh_run_context(
            cancel,
            &e.local.row,
            &args,
            &[],
            Duration::from_secs(10),
        );
        if cancel.is_cancelled() {
            return false;
        }
        let (rc, raw, err_text) = match res {
            Ok(v) => v,
            Err(err) => {
                self.unavailable(err);
                return false;
            }
        };
        if rc == 3 {
            return false;
        }
        if rc != 0 {
            let text = if err_text.is_empty() { raw } else { err_text };
            let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
            self.unavailable(anyhow!(
                "hands steps unavailable (exit {rc}): {}",
                crate::executor::tail(&compact, 300)
            ));
            return false;
        }
        if raw.is_empty() {
            return false;
        }
        self.offset += raw.len() as i64;
        if let Err(err) = self.apply(raw.into_bytes()) {
            self.unavailable(err);
            return false;
        }
        true
    }
    /// Go: func (m *remoteStepsMirror) apply(chunk []byte) error
    pub(crate) fn apply(&mut self, chunk: Vec<u8>) -> anyhow::Result<()> {
        self.pending.extend(chunk);
        while let Some(i) = self.pending.iter().position(|b| *b == b'\n') {
            let line = self.pending[..i].to_vec();
            self.pending.drain(..=i);
            let line = String::from_utf8_lossy(&line).trim().as_bytes().to_vec();
            if line.is_empty() {
                continue;
            }
            let head: RemoteStepsHead = serde_json::from_slice(&line)
                .map_err(|e| anyhow!("invalid harness steps line: {e}"))?;
            match head.event.as_str() {
                "begin" => {
                    let mut ev: RemoteStepsBegin = serde_json::from_slice(&line)
                        .map_err(|e| anyhow!("invalid harness begin line: {e}"))?;
                    if ev.run_id.is_empty() {
                        ev.run_id = self.id.clone();
                    }
                    let e = self
                        .e
                        .as_ref()
                        .ok_or_else(|| anyhow!("missing remote executor"))?;
                    let s = open_store(e)?;
                    s.begin_harness_session(pulse_store::HarnessSession {
                        id: self.id.clone(),
                        run_id: ev.run_id,
                        job_id: ev.job_id,
                        agent: ev.agent,
                        project: e.local.project.str("slug"),
                        executor: e.local.row.str("name"),
                        provider: ev.provider,
                        model: ev.model,
                        cwd: ev.cwd,
                        status: "running".into(),
                        started_at: ev.at.clone(),
                        updated_at: ev.at,
                        ..Default::default()
                    })?;
                    self.begun = true;
                }
                "step" => {
                    let ev: RemoteStepsStep = serde_json::from_slice(&line)
                        .map_err(|e| anyhow!("invalid harness step line: {e}"))?;
                    let at = if ev.at.is_empty() {
                        String::new()
                    } else {
                        chrono::DateTime::parse_from_rfc3339(&ev.at)
                            .map(|d| {
                                d.with_timezone(&chrono::Utc)
                                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                            })
                            .unwrap_or_default()
                    };
                    let e = self
                        .e
                        .as_ref()
                        .ok_or_else(|| anyhow!("missing remote executor"))?;
                    let s = open_store(e)?;
                    s.append_harness_step(pulse_store::HarnessStep {
                        session_id: self.id.clone(),
                        n: ev.n,
                        at,
                        kind: ev.kind,
                        content: ev.content,
                        command: ev.command,
                        exit_code: ev.exit,
                        duration_ms: ev.ms,
                        prompt_tokens: ev.usage.prompt_tokens as i64,
                        completion_tokens: ev.usage.completion_tokens as i64,
                        model: ev.model,
                    })?;
                }
                "finish" => {
                    let ev: RemoteStepsFinish = serde_json::from_slice(&line)
                        .map_err(|e| anyhow!("invalid harness finish line: {e}"))?;
                    let e = self
                        .e
                        .as_ref()
                        .ok_or_else(|| anyhow!("missing remote executor"))?;
                    let s = open_store(e)?;
                    s.finish_harness_session(
                        &self.id,
                        &ev.exit_status,
                        &ev.exit_status,
                        &ev.submission,
                        &ev.error,
                        ev.cost,
                        "",
                    )?;
                    self.finished = true;
                }
                other => return Err(anyhow!("unknown harness steps event {:?}", other)),
            }
        }
        Ok(())
    }
    /// Go: func (m *remoteStepsMirror) finishIfMissing(out outcome, runErr error)
    pub(crate) fn finish_if_missing(
        &mut self,
        out: crate::proc::Outcome,
        run_err: Option<anyhow::Error>,
    ) {
        if !self.begun || self.finished {
            return;
        }
        let mut exit_status = "provider_error";
        if out.timeout {
            exit_status = "time_limit";
        } else if run_err
            .as_ref()
            .is_some_and(|e| e.to_string().contains("cancel") || e.to_string().contains("deadline"))
        {
            exit_status = "cancelled";
        }
        let res = (|| -> Result<()> {
            let e = self
                .e
                .as_ref()
                .ok_or_else(|| anyhow!("missing remote executor"))?;
            let s = open_store(e)?;
            s.finish_harness_session(
                &self.id,
                "lost",
                exit_status,
                "",
                "worker ended without a finish line",
                0.0,
                "",
            )
        })();
        if let Err(err) = res {
            self.unavailable(err);
        }
    }
    /// Go: func (m *remoteStepsMirror) unavailable(err error)
    pub(crate) fn unavailable(&mut self, err: anyhow::Error) {
        if self.reported {
            return;
        }
        self.reported = true;
        self.stopped = true;
        let _ = err;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::new_store;
    use crate::util::Row;
    use serde_json::json;

    fn row(pairs: &[(&str, serde_json::Value)]) -> Row {
        let mut r = Row::new();
        for (k, v) in pairs {
            r.insert(*k, v.clone());
        }
        r
    }

    fn mirror(s: &pulse_store::Store) -> RemoteStepsMirror {
        new_remote_steps_mirror(
            Some(crate::exec::SshExec {
                local: crate::exec::LocalExec {
                    row: row(&[
                        ("name", json!("remote-one")),
                        ("address", json!("ssh://remote-one")),
                    ]),
                    home: s.home.clone(),
                    job: row(&[("id", json!("job-steps"))]),
                    project: row(&[("slug", json!("proj"))]),
                    ..Default::default()
                },
                ..Default::default()
            }),
            "run-steps".into(),
        )
        .unwrap()
    }

    #[test]
    fn remote_steps_mirror_applies_split_lines_and_missing_finish() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let mut m = mirror(&s);
        let begin = r#"{"event":"begin","session":"run-steps","run_id":"run-steps","job_id":"job-steps","agent":"agent-one","provider":"fake","model":"m1","cwd":"/work","at":"2026-09-11T07:00:00Z"}"#;
        let step = r#"{"event":"step","n":1,"at":"2026-09-11T07:00:01Z","kind":"assistant","content":"hello","usage":{"prompt_tokens":3,"completion_tokens":5,"cost":0.2},"model":"m1"}"#;
        m.apply(format!("{}\n{}", begin, &step[..step.len() / 2]).into_bytes())
            .unwrap();
        m.apply(format!("{}\n", &step[step.len() / 2..]).into_bytes())
            .unwrap();
        m.finish_if_missing(
            crate::proc::Outcome {
                exit_code: 0,
                ..Default::default()
            },
            None,
        );
        let (session, steps) = s.harness_session("run-steps").unwrap();
        let session = session.unwrap();
        assert_eq!(session.executor, "remote-one");
        assert_eq!(session.job_id, "job-steps");
        assert_eq!(session.agent, "agent-one");
        assert_eq!(session.status, "lost");
        assert_eq!(session.exit_status, "provider_error");
        assert_eq!(session.error, "worker ended without a finish line");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, "assistant");
        assert_eq!(steps[0].prompt_tokens, 3);
        assert_eq!(steps[0].completion_tokens, 5);
    }

    #[test]
    fn remote_steps_mirror_finish_line() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let mut m = mirror(&s);
        let lines = [
            r#"{"event":"begin","session":"run-steps","run_id":"run-steps","job_id":"job-steps","agent":"agent-one","provider":"fake","model":"m1","cwd":"/work","at":"2026-09-11T07:00:00Z"}"#,
            r#"{"event":"finish","exit_status":"submitted","submission":"SUMMARY: ok","error":"","cost":0.75,"steps":1,"at":"2026-09-11T07:00:02Z"}"#,
        ];
        m.apply(format!("{}\n", lines.join("\n")).into_bytes())
            .unwrap();
        let (session, _) = s.harness_session("run-steps").unwrap();
        let session = session.unwrap();
        assert_eq!(session.status, "submitted");
        assert_eq!(session.exit_status, "submitted");
        assert_eq!(session.submission, "SUMMARY: ok");
        assert_eq!(session.cost, 0.75);
    }

    #[test]
    fn remote_steps_mirror_invalid_json_stops() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let mut m = mirror(&s);
        assert!(m.apply(b"{bad}\n".to_vec()).is_err());
    }

    #[test]
    #[ignore = "cross-slice: hands::probe"]
    fn probe_advertises_steps() {}

    #[test]
    #[ignore = "cross-slice: hands::probe"]
    fn probe_version_hook_without_path() {}
}
