#![allow(dead_code, clippy::type_complexity)]

use anyhow::{Result, anyhow};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

pub(crate) const TEST_DAEMON: &str = "daemon";
pub(crate) const TEST_EXECUTOR_TOKEN: &str = crate::test_helpers_hands::TEST_EXECUTOR_TOKEN;

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

pub(crate) struct SshHostnameGuard {
    old: Option<Box<dyn Fn(&str) -> String + Send + Sync>>,
}

impl Drop for SshHostnameGuard {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            drop(crate::retire::set_ssh_hostname(old));
        }
    }
}

pub(crate) fn install_ssh_hostname<F>(f: F) -> SshHostnameGuard
where
    F: Fn(&str) -> String + Send + Sync + 'static,
{
    SshHostnameGuard {
        old: Some(crate::retire::set_ssh_hostname(Box::new(f))),
    }
}

pub(crate) struct RunAzGuard {
    old: Option<Box<crate::provider::CliRunner>>,
}

impl Drop for RunAzGuard {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            drop(crate::azure::set_run_az(old));
        }
    }
}

pub(crate) fn install_run_az<F>(f: F) -> RunAzGuard
where
    F: Fn(
            &pulse_bus::CancellationToken,
            Vec<String>,
            std::time::Duration,
        ) -> anyhow::Result<(i32, String, String)>
        + Send
        + Sync
        + 'static,
{
    crate::azure::init();
    RunAzGuard {
        old: Some(crate::azure::set_run_az(Box::new(f))),
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

pub(crate) use crate::test_helpers_daemon::set_setting;
pub(crate) use crate::test_helpers_hands::write_executor_token;
