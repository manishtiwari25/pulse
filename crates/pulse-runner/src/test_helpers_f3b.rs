//! Test helpers ported from Go runner retire/sweep/service/onboard tests owned by slice F3b.
#![allow(dead_code, clippy::type_complexity)]

use crate::test_helpers::{TEST_NOW, new_store, run};
use anyhow::{Result, anyhow};
use rusqlite::params;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

pub(crate) const TEST_DAEMON: &str = "daemon";
pub(crate) const TEST_EXECUTOR_TOKEN: &str = crate::test_helpers_f1b::TEST_EXECUTOR_TOKEN;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PlacedLogin {
    pub name: String,
    pub admin: String,
    pub token: String,
    pub actor: String,
}

#[derive(Debug, Default)]
struct PlaceLoginState {
    ok: bool,
    seen: Vec<PlacedLogin>,
}

static PLACE_LOGIN_STATE: LazyLock<Mutex<PlaceLoginState>> =
    LazyLock::new(|| Mutex::new(PlaceLoginState::default()));

pub(crate) struct PlaceLoginStub {
    old: crate::executor::PlaceLoginFn,
}

impl Drop for PlaceLoginStub {
    fn drop(&mut self) {
        *crate::executor::PLACE_LOGIN.write().expect("place login") = self.old;
        PLACE_LOGIN_STATE.lock().unwrap().seen.clear();
    }
}

impl PlaceLoginStub {
    pub(crate) fn calls(&self) -> Vec<PlacedLogin> {
        PLACE_LOGIN_STATE.lock().unwrap().seen.clone()
    }
}

#[allow(clippy::too_many_arguments)]
fn fake_place_login(
    s: &pulse_store::Store,
    now: &str,
    name: &str,
    admin: &str,
    token: &mut dyn std::io::Read,
    _is_tty: bool,
    _remove: bool,
    actor: &str,
) -> Result<crate::executor::LoginResult> {
    let mut raw = String::new();
    token.read_to_string(&mut raw)?;
    let ok = {
        let mut st = PLACE_LOGIN_STATE.lock().unwrap();
        st.seen.push(PlacedLogin {
            name: name.into(),
            admin: admin.into(),
            token: raw.trim().into(),
            actor: actor.into(),
        });
        st.ok
    };
    crate::executor::set_executor_admin(s, name, admin);
    if !ok {
        return Ok(crate::executor::LoginResult {
            ok: false,
            kind: "refused".into(),
            rc: 1,
            lines: vec!["copilot refuses the login: Copilot Requests permission missing".into()],
        });
    }
    let note = "ok: login placed";
    crate::executor::set_executor_status(s, now, name, "active", actor, Some(note))?;
    Ok(crate::executor::LoginResult {
        ok: true,
        kind: "pat".into(),
        lines: vec!["login placed".into()],
        ..Default::default()
    })
}

pub(crate) fn stub_place_login(ok: bool) -> PlaceLoginStub {
    let old = *crate::executor::PLACE_LOGIN.read().expect("place login");
    {
        let mut st = PLACE_LOGIN_STATE.lock().unwrap();
        st.ok = ok;
        st.seen.clear();
    }
    *crate::executor::PLACE_LOGIN.write().expect("place login") = fake_place_login;
    PlaceLoginStub { old }
}

#[derive(Clone, Default)]
pub(crate) struct RetireAz(pub Arc<Mutex<RetireAzState>>);

#[derive(Default)]
pub(crate) struct RetireAzState {
    pub calls: Vec<Vec<String>>,
    pub list_out: String,
    pub missing: bool,
    pub broken: bool,
}

impl RetireAz {
    pub(crate) fn with_list(out: &str) -> Self {
        let f = Self::default();
        f.0.lock().unwrap().list_out = out.into();
        f
    }

    pub(crate) fn missing() -> Self {
        let f = Self::default();
        f.0.lock().unwrap().missing = true;
        f
    }

    pub(crate) fn broken() -> Self {
        let f = Self::default();
        f.0.lock().unwrap().broken = true;
        f
    }

    pub(crate) fn run(
        &self,
        _ctx: &pulse_bus::CancellationToken,
        args: Vec<String>,
        _timeout: Duration,
    ) -> Result<(i32, String, String)> {
        let mut f = self.0.lock().unwrap();
        f.calls.push(args.clone());
        for a in &args {
            if pulse_store::secret_shaped(a) {
                return Err(anyhow!("secret-shaped argument reached az: {a}"));
            }
        }
        if f.broken {
            return Err(anyhow!(r#"exec: "az": executable file not found in $PATH"#));
        }
        let cmd = args.join(" ");
        match () {
            _ if cmd.starts_with("vm list -d --query") => Ok((
                0,
                if f.list_out.is_empty() {
                    "[]\n".into()
                } else {
                    f.list_out.clone()
                },
                String::new(),
            )),
            _ if cmd.starts_with("vm deallocate") => Ok((0, String::new(), String::new())),
            _ if cmd.starts_with("vm show") => {
                if f.missing {
                    Ok((3, String::new(), "(ResourceNotFound) The Resource 'Microsoft.Compute/virtualMachines/x' under resource group 'pulse-hands' was not found.".into()))
                } else {
                    Ok((0, r#"{"name":"pulse-cr-007","storageProfile":{"osDisk":{"managedDisk":{"id":"/subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Compute/disks/pulse-cr-007_OsDisk"}}},"networkProfile":{"networkInterfaces":[{"id":"/subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Network/networkInterfaces/pulse-cr-007VMNic"}]}}"#.into(), String::new()))
                }
            }
            _ if cmd.starts_with("network nic show") => Ok((0, r#"{"ipConfigurations":[{"publicIPAddress":{"id":"/subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Network/publicIPAddresses/pulse-cr-007PublicIP"}}],"networkSecurityGroup":{"id":"/subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Network/networkSecurityGroups/pulse-cr-007NSG"}}"#.into(), String::new())),
            _ if cmd.starts_with("vm delete")
                || cmd.starts_with("network nic delete")
                || cmd.starts_with("network public-ip delete")
                || cmd.starts_with("disk delete") =>
            {
                Ok((0, String::new(), String::new()))
            }
            _ => Ok((1, String::new(), format!("unexpected az {cmd}"))),
        }
    }

    pub(crate) fn joined(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .calls
            .iter()
            .map(|c| c.join(" "))
            .collect()
    }

    pub(crate) fn has(&self, prefix: &str) -> bool {
        self.joined().iter().any(|c| c.starts_with(prefix))
    }

    pub(crate) fn reset(&self) {
        self.0.lock().unwrap().calls.clear();
    }
}

pub(crate) struct RetireAzGuard {
    old_az: Option<Box<crate::provider::CliRunner>>,
    old_host: Option<Box<dyn Fn(&str) -> String + Send + Sync>>,
}

impl Drop for RetireAzGuard {
    fn drop(&mut self) {
        if let Some(old) = self.old_az.take() {
            drop(crate::azure::set_run_az(old));
        }
        if let Some(old) = self.old_host.take() {
            drop(crate::retire::set_ssh_hostname(old));
        }
    }
}

pub(crate) fn install_retire_az(f: &RetireAz) -> RetireAzGuard {
    crate::azure::init();
    let fake = f.clone();
    let old_az = crate::azure::set_run_az(Box::new(move |ctx, args, timeout| {
        fake.run(ctx, args, timeout)
    }));
    let old_host = crate::retire::set_ssh_hostname(Box::new(|host| {
        if host == "vm-seven" {
            "203.0.113.7".into()
        } else {
            host.into()
        }
    }));
    RetireAzGuard {
        old_az: Some(old_az),
        old_host: Some(old_host),
    }
}

pub(crate) fn set_setting(s: &pulse_store::Store, k: &str, v: &str) {
    s.set_setting(pulse_store::HUMAN_AGENT, k, v).unwrap();
}

pub(crate) fn write_executor_token(s: &pulse_store::Store) {
    crate::test_helpers_f1b::write_executor_token(s);
}

pub(crate) fn sweep_store() -> crate::test_helpers::TestStore {
    let s = new_store();
    set_setting(&s, "daemon.sweep_grace_minutes", "0");
    set_setting(&s, "daemon.sweep_branch_days", "0");
    s
}

pub(crate) fn sweep_job(s: &pulse_store::Store, status: &str, branch: &str) -> String {
    let (job, _) = crate::jobs::add_job(
        s,
        "proj".into(),
        "task".into(),
        format!("t-{}", crate::util::new_id("x", TEST_NOW)),
        "b".into(),
        1,
        0,
        "test".into(),
        pulse_store::HUMAN_AGENT.into(),
        false,
    )
    .unwrap();
    s.conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET status = ?, branch = ? WHERE id = ?",
            params![status, branch, job.str("id")],
        )
        .unwrap();
    job.str("id")
}

pub(crate) fn sweep_run(s: &pulse_store::Store, job_id: &str, status: &str) -> String {
    let id = crate::util::new_id("run", TEST_NOW);
    s.conn()
        .unwrap()
        .execute(
            "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES (?, ?, 'w', 'proj', ?, ?)",
            params![id, job_id, TEST_NOW, status],
        )
        .unwrap();
    id
}

pub(crate) fn mkdir_file(path: impl AsRef<Path>, body: &str) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

pub(crate) fn swept_paths(items: &[crate::sweep::Swept], kind: &str) -> Vec<String> {
    items
        .iter()
        .filter(|x| x.kind == kind)
        .map(|x| x.path.clone())
        .collect()
}

pub(crate) fn held_why(items: &[crate::sweep::Swept], path: impl AsRef<Path>) -> String {
    let path = path.as_ref().display().to_string();
    held_why_text(items, &path)
}

pub(crate) fn held_why_text(items: &[crate::sweep::Swept], path: &str) -> String {
    items
        .iter()
        .find(|x| x.path == path)
        .map(|x| x.why.clone())
        .unwrap_or_default()
}

pub(crate) fn project_rows(s: &pulse_store::Store) -> Vec<crate::util::Row> {
    crate::board::project_rows(&*s.conn().unwrap(), false)
}

pub(crate) fn onboard_repo(name: &str, mut files: Vec<(&str, String)>) -> String {
    let dir = tempfile::tempdir().unwrap().keep().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    run(&dir, &["git", "init", "-q", "-b", "main"]);
    files.push(("README.md", format!("# {name}\n")));
    for (rel, body) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    run(&dir, &["git", "add", "-A"]);
    run(&dir, &["git", "commit", "-q", "-m", "init"]);
    std::fs::canonicalize(dir).unwrap().display().to_string()
}

pub(crate) fn simple_repo(name: &str) -> String {
    onboard_repo(name, vec![])
}

pub(crate) fn path_join(base: &str, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(PathBuf::from(base), |p, part| p.join(part))
}
