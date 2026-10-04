//! Test helpers ported from the Go test files owned by slice F1B (see files/RUNNER_TEST_WAVE.txt).
//! Other slices call these as `crate::test_helpers_f1b::<go_name_in_snake_case>`.

use base64::Engine;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Go: func base64Std(s string) string
#[allow(dead_code)]
pub(crate) fn base64_std(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

#[derive(Clone, Default)]
pub(crate) struct CaptureWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl CaptureWriter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn boxed(&self) -> Box<dyn Write + Send> {
        Box::new(self.clone())
    }

    pub(crate) fn take(&self) -> String {
        let mut buf = self.buf.lock().unwrap();
        let out = String::from_utf8_lossy(&buf).to_string();
        buf.clear();
        out
    }
}

impl Write for CaptureWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn hands_exit_code(err: anyhow::Error) -> i32 {
    err.downcast_ref::<crate::hands::HandsExit>()
        .map(|e| e.code)
        .unwrap_or(1)
}

/// Go: TestMain PULSE_TEST_HANDS bridge.
pub(crate) fn hands_helper_process() {
    if std::env::var("PULSE_TEST_HANDS").as_deref() != Ok("1") {
        return;
    }
    // The harness runs this entry like any other test (and a test that set PULSE_TEST_HANDS in-process may
    // have run before it); the bridge is only for a child the parent spawned the way sshd would, with the
    // hands command line in SSH_ORIGINAL_COMMAND.
    if std::env::var("SSH_ORIGINAL_COMMAND")
        .unwrap_or_default()
        .is_empty()
    {
        return;
    }
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut h = crate::hands::new_hands(
        Box::new(std::io::stdin()),
        Box::new(std::io::stdout()),
        Box::new(std::io::stderr()),
    );
    let code = match h.hands_main(&args) {
        Ok(()) => 0,
        Err(e) => {
            if e.downcast_ref::<crate::hands::HandsError>().is_some() {
                eprintln!("{e}");
                2
            } else if let Some(exit) = e.downcast_ref::<crate::hands::HandsExit>() {
                exit.code
            } else {
                eprintln!("{e}");
                1
            }
        }
    };
    std::process::exit(code);
}

pub(crate) struct RunSshGuard {
    old: Option<crate::executor::SshRunner>,
}

impl Drop for RunSshGuard {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            crate::executor::restore_run_ssh(old);
        }
    }
}

pub(crate) fn install_run_ssh<F>(f: F) -> RunSshGuard
where
    F: Fn(
            &pulse_bus::CancellationToken,
            &[String],
            &[u8],
            Duration,
        ) -> anyhow::Result<(i32, String, String)>
        + Send
        + Sync
        + 'static,
{
    RunSshGuard {
        old: Some(crate::executor::set_run_ssh(f)),
    }
}

pub(crate) const TEST_EXECUTOR_TOKEN: &str = "github_pat_11ATESTTOKEN0000000000_TESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTEST";

pub(crate) fn write_executor_token(s: &pulse_store::Store) -> String {
    let path = crate::token::executor_token_path(s);
    std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
    std::fs::write(&path, format!("{TEST_EXECUTOR_TOKEN}\n")).unwrap();
    path
}

pub(crate) const TEST_CONSTITUTION: &str = "# Constitution\n\n## Hard stops\n\nNever cross a hard stop.\n\n## Vote rules\n\n| Rule | Value |\n| --- | --- |\n| Votes per identity per suggestion | 1 (the latest wins) |\n| Amendment quorum | 1 distinct voter |\n| Amendment supermajority | 2/3 of counted votes |\n| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter |\n\n## Amendments\n\nPer docs/workflows/amend.md.\n";

pub(crate) const TEST_AMENDMENT: &str = "## Current text\n\n```\n| Votes per identity per suggestion | 1 (the latest wins) |\n```\n\n## Proposed text\n\n```\n| Votes per identity per suggestion | 1 (the latest wins; a fleet is one voter) |\n```\n\n## Why\n\nFleets voted twice.\n";

pub(crate) fn passed_amendment(s: &pulse_store::Store) -> String {
    let am = open_amendment(s, "2026-09-02T08:00:00Z");
    let hash = crate::board::post_row(&s.conn().unwrap(), am.clone()).str("text_hash");
    crate::board::create_post(
        s,
        "2026-09-03T08:00:00Z".into(),
        crate::board::PostOpts {
            kind: "vote".into(),
            project: "proj".into(),
            area: "governance".into(),
            thread: "fleet-votes".into(),
            summary: "up".into(),
            body: "up\n\nRead: a fleet counts once; quorum untouched.".into(),
            agent: crate::board::actor_agent(s, "voter".into()),
            session: "s-voter".into(),
            on: am.clone(),
            vote: "up".into(),
            text_hash: hash,
            ..Default::default()
        },
    )
    .unwrap();
    am
}

/// A constitution record for `proj` (amendment quorum 1), two worker agents
/// `prop` and `voter`, and an open amendment by `prop` posted at `date` with
/// no vote yet: `needs votes` until `voter` casts an informed one.
pub(crate) fn open_amendment(s: &pulse_store::Store, date: &str) -> String {
    let rec = pulse_store::Record {
        id: "EVO-constitution-proj".into(),
        project: "proj".into(),
        kind: "evolution".into(),
        slug: "constitution".into(),
        title: "Constitution".into(),
        status: "active".into(),
        date: "2026-09-01".into(),
        path: "docs/evolution/constitution.md".into(),
        ..Default::default()
    };
    let front = std::collections::BTreeMap::from([
        ("loop".to_string(), serde_json::json!("off")),
        ("limits".to_string(), serde_json::json!("none")),
    ]);
    s.put_record(
        rec,
        front,
        TEST_CONSTITUTION,
        crate::util::HUMAN_AGENT,
        "proj",
    )
    .unwrap();
    for a in ["prop", "voter"] {
        crate::board::join_agent(
            s,
            a.into(),
            "r".into(),
            "m".into(),
            format!("s-{a}"),
            String::new(),
            "worker".into(),
            String::new(),
        )
        .unwrap();
    }
    let am = crate::board::create_post(
        s,
        date.into(),
        crate::board::PostOpts {
            kind: "amendment".into(),
            project: "proj".into(),
            area: "governance".into(),
            thread: "fleet-votes".into(),
            summary: "Count a fleet as one voter".into(),
            body: TEST_AMENDMENT.into(),
            agent: crate::board::actor_agent(s, "prop".into()),
            session: "s-prop".into(),
            ..Default::default()
        },
    )
    .unwrap();
    am.str("id")
}

#[cfg(test)]
#[test]
fn hands_helper_process_entry() {
    let _serial = crate::test_helpers::serial();
    hands_helper_process();
}
