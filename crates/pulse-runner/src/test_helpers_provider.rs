#![allow(dead_code, unused_imports)]

use crate::test_helpers::*;
use anyhow::anyhow;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

pub(crate) fn invoke(
    s: &pulse_store::Store,
    name: &str,
    project: &str,
    args: &[&str],
) -> (String, String, i32) {
    crate::commands::register();
    let cmd = pulse_cli::lookup(name).unwrap_or_else(|| panic!("{name} not registered"));
    let mut fs = pulse_cli::FlagSet::new(name);
    if let Some(flags) = cmd.flags {
        flags(&mut fs);
    }
    let mut positional = Vec::<String>::new();
    let mut rest = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    while !rest.is_empty() {
        fs.parse(&rest)
            .unwrap_or_else(|e| panic!("{name} {args:?}: {e}"));
        let parsed_rest = fs.args().to_vec();
        if parsed_rest.is_empty() {
            break;
        }
        positional.push(parsed_rest[0].clone());
        rest = parsed_rest[1..].to_vec();
    }
    let out = pulse_cli::Buffer::default();
    let err = pulse_cli::Buffer::default();
    let mut c = pulse_cli::Context {
        store: Some(s.clone()),
        home: s.home.clone(),
        out: Box::new(out.clone()),
        err: Box::new(err.clone()),
        json: false,
        quiet: false,
        color: false,
        agent: String::new(),
        project: project.to_string(),
        session: String::new(),
        now: TEST_NOW.to_string(),
        args: positional.clone(),
    };
    let result = (cmd.run)(&mut c, &fs, &positional);
    let mut code = 0;
    if let Err(e) = result {
        use std::io::Write;
        if let Some(ce) = e.downcast_ref::<pulse_cli::Error>() {
            code = ce.exit_code();
            if !ce.what.is_empty() {
                let _ = write!(c.err, "{}", ce);
            }
        } else {
            code = 1;
            let _ = write!(c.err, "{e}");
        }
    }
    (out.string(), err.string(), code)
}

pub(crate) fn invoke_ok(
    s: &pulse_store::Store,
    name: &str,
    project: &str,
    args: &[&str],
) -> String {
    let (out, err_text, code) = invoke(s, name, project, args);
    if code != 0 {
        panic!("{name} {args:?}: {code} {err_text}");
    }
    out
}

/// Runs f with stdin reading text, as `... | pulse executor token --with-token` would. The pipe's
/// read end is dup2'd onto fd 0 for the process and restored afterwards. Callers hold `serial()`
/// so nothing else reads stdin meanwhile.
pub(crate) fn with_stdin(text: &str, f: impl FnOnce()) {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let (r, mut w) = std::io::pipe().expect("pipe");
    // SAFETY: fd 0 is open for the process's life; dup returns a fresh descriptor we own.
    let old = unsafe {
        let saved = libc::dup(0);
        assert!(saved >= 0, "dup(0): {}", std::io::Error::last_os_error());
        OwnedFd::from_raw_fd(saved)
    };
    // SAFETY: r stays open until the end of this function, so fd 0 aliases a live pipe.
    let rc = unsafe { libc::dup2(r.as_raw_fd(), 0) };
    assert!(rc >= 0, "dup2: {}", std::io::Error::last_os_error());
    let text = text.to_string();
    let writer = std::thread::spawn(move || {
        use std::io::Write;
        let _ = w.write_all(text.as_bytes());
        drop(w);
    });
    f();
    // SAFETY: old is the saved copy of the original fd 0, still open.
    let rc = unsafe { libc::dup2(old.as_raw_fd(), 0) };
    assert!(rc >= 0, "dup2 restore: {}", std::io::Error::last_os_error());
    drop(r);
    let _ = writer.join();
}

static STUB_GH_TOKEN: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

fn stub_gh_auth_token() -> anyhow::Result<String> {
    Ok(STUB_GH_TOKEN.lock().unwrap().clone())
}

/// Makes the hub's `gh auth token` answer tok for the test; dropping the guard puts the old
/// `GH_AUTH_TOKEN` back and forgets the gh source cache.
#[must_use = "hold the guard for the test's life; dropping it restores gh auth token"]
pub(crate) fn stub_gh(tok: &str) -> StubGh {
    let old = *crate::token::GH_AUTH_TOKEN.read().unwrap();
    crate::token::GH_SOURCE.lock().unwrap().forget();
    *STUB_GH_TOKEN.lock().unwrap() = tok.to_string();
    *crate::token::GH_AUTH_TOKEN.write().unwrap() = stub_gh_auth_token;
    StubGh { old }
}

pub(crate) struct StubGh {
    old: crate::token::GhAuthTokenFn,
}

impl Drop for StubGh {
    fn drop(&mut self) {
        *crate::token::GH_AUTH_TOKEN.write().unwrap() = self.old;
        crate::token::GH_SOURCE.lock().unwrap().forget();
    }
}

pub(crate) const TEST_HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE root@vm";

pub(crate) static PROVIDER_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static SSH_CALLS: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static PLACE_LOGIN_CALLS: LazyLock<Mutex<Vec<PlaceLoginCall>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

#[derive(Debug, Clone, Default)]
pub(crate) struct PlaceLoginCall {
    pub admin: String,
}

pub(crate) fn provider_test_lock() -> MutexGuard<'static, ()> {
    PROVIDER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
pub(crate) struct FakeCli {
    pub calls: Vec<Vec<String>>,
}

impl FakeCli {
    pub(crate) fn record(&mut self, args: &[String]) -> anyhow::Result<()> {
        self.calls.push(args.to_vec());
        for a in args {
            let mut value = a.trim_start_matches("resolve:ssm:").to_string();
            if let Some((k, v)) = a.split_once('=')
                && !k.contains([' ', ',', '{', '['])
            {
                value = v.to_string();
            }
            if pulse_store::secret_shaped(&value) {
                return Err(anyhow!("secret-shaped argument reached the CLI: {a}"));
            }
        }
        Ok(())
    }

    pub(crate) fn joined(&self) -> Vec<String> {
        self.calls.iter().map(|c| c.join(" ")).collect()
    }

    pub(crate) fn has(&self, prefix: &str) -> bool {
        self.count(prefix) > 0
    }

    pub(crate) fn count(&self, prefix: &str) -> usize {
        self.joined()
            .into_iter()
            .filter(|c| c.starts_with(prefix))
            .count()
    }

    pub(crate) fn first(&self, prefix: &str) -> String {
        self.joined()
            .into_iter()
            .find(|c| c.starts_with(prefix))
            .unwrap_or_default()
    }

    pub(crate) fn reset(&mut self) {
        self.calls.clear();
    }
}

pub(crate) struct InstallFakeGuard {
    old: Option<Box<crate::provider::CliRunner>>,
    setter: fn(Box<crate::provider::CliRunner>) -> Box<crate::provider::CliRunner>,
}

impl Drop for InstallFakeGuard {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            drop((self.setter)(old));
        }
    }
}

pub(crate) fn install_fake(
    setter: fn(Box<crate::provider::CliRunner>) -> Box<crate::provider::CliRunner>,
    runner: Box<crate::provider::CliRunner>,
) -> InstallFakeGuard {
    InstallFakeGuard {
        old: Some(setter(runner)),
        setter,
    }
}

fn arg_after(args: &[String], flag: &str) -> String {
    args.windows(2)
        .find_map(|w| (w[0] == flag).then(|| w[1].clone()))
        .unwrap_or_default()
}

fn contains(values: &[&str], want: &str) -> bool {
    values.contains(&want)
}

#[derive(Clone)]
pub(crate) struct FakeAz(pub Arc<Mutex<FakeAzState>>);

#[derive(Default)]
pub(crate) struct FakeAzState {
    pub calls: Vec<Vec<String>>,
    pub created: BTreeMap<String, bool>,
    pub deleted: Vec<String>,
}

pub(crate) fn new_fake_az() -> FakeAz {
    FakeAz(Arc::new(Mutex::new(FakeAzState::default())))
}

impl FakeAz {
    pub(crate) fn run(
        &self,
        _ctx: &pulse_bus::CancellationToken,
        args: Vec<String>,
        _timeout: Duration,
    ) -> anyhow::Result<(i32, String, String)> {
        let mut f = self.0.lock().expect("fake az");
        f.calls.push(args.clone());
        for a in &args {
            if pulse_store::secret_shaped(a) {
                return Err(anyhow!("secret-shaped argument reached az: {a}"));
            }
        }
        let cmd = args.join(" ");
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        Ok(match () {
            _ if cmd.starts_with("account show") => (0, "Pulse Sub\n".into(), String::new()),
            _ if cmd.starts_with("group show") => {
                (3, String::new(), "ResourceGroupNotFound".into())
            }
            _ if cmd.starts_with("group create") => (0, String::new(), String::new()),
            _ if cmd.starts_with("vm show") => {
                let name = arg_after(&args, "--name");
                if !f.created.get(&name).copied().unwrap_or(false) {
                    return Ok((3, String::new(), "ResourceNotFound".into()));
                }
                if contains(&refs, "powerState") {
                    return Ok((0, "VM running\n".into(), String::new()));
                }
                if contains(&refs, "publicIps") {
                    return Ok((0, "203.0.113.5\n".into(), String::new()));
                }
                (0, String::new(), String::new())
            }
            _ if cmd.starts_with("vm create") => {
                let name = arg_after(&args, "--name");
                f.created.insert(name, true);
                (
                    0,
                    r#"{"publicIpAddress": "203.0.113.5", "powerState": "VM running"}"#.into(),
                    String::new(),
                )
            }
            _ if cmd.starts_with("vm run-command invoke") => (
                0,
                "Enable succeeded: \n[stdout]\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE root@vm\n[stderr]\n".into(),
                String::new(),
            ),
            _ if cmd.starts_with("vm delete") => {
                let name = arg_after(&args, "--name");
                f.created.remove(&name);
                f.deleted.push(name);
                (0, String::new(), String::new())
            }
            _ if cmd.starts_with("network") => (3, String::new(), "ResourceNotFound".into()),
            _ => (1, String::new(), format!("unexpected az {cmd}")),
        })
    }

    pub(crate) fn has(&self, prefix: &str) -> bool {
        self.0
            .lock()
            .expect("fake az")
            .calls
            .iter()
            .any(|c| c.join(" ").starts_with(prefix))
    }

    pub(crate) fn calls(&self) -> Vec<Vec<String>> {
        self.0.lock().expect("fake az").calls.clone()
    }

    pub(crate) fn deleted(&self) -> Vec<String> {
        self.0.lock().expect("fake az").deleted.clone()
    }
}

#[derive(Clone)]
pub(crate) struct FakeAws(pub Arc<Mutex<FakeAwsState>>);

#[derive(Default)]
pub(crate) struct FakeAwsState {
    pub cli: FakeCli,
    pub instances: BTreeMap<String, AwsFake>,
    pub describes: i32,
    pub consoles: i32,
    pub terminate: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AwsFake {
    pub id: String,
    pub name: String,
    pub request: String,
    pub project: String,
    pub state: String,
    pub ip: String,
}

pub(crate) fn new_fake_aws() -> FakeAws {
    FakeAws(Arc::new(Mutex::new(FakeAwsState::default())))
}

impl FakeAws {
    pub(crate) fn run(
        &self,
        _ctx: &pulse_bus::CancellationToken,
        args: Vec<String>,
        _timeout: Duration,
    ) -> anyhow::Result<(i32, String, String)> {
        let mut f = self.0.lock().expect("fake aws");
        f.cli.record(&args)?;
        let cmd = args.join(" ");
        let ans = match () {
            _ if cmd.starts_with("sts get-caller-identity") => (
                0,
                r#"{"Account": "123456789012", "Arn": "arn:aws:iam::123456789012:user/pulse"}"#
                    .into(),
                String::new(),
            ),
            _ if cmd.starts_with("ec2 describe-vpcs") => {
                (0, r#"{"Vpcs": [{"VpcId": "vpc-1"}]}"#.into(), String::new())
            }
            _ if cmd.starts_with("ec2 describe-subnets") => (
                0,
                r#"{"Subnets": [{"SubnetId": "subnet-1", "VpcId": "vpc-1"}]}"#.into(),
                String::new(),
            ),
            _ if cmd.starts_with("ec2 describe-security-groups") => {
                (0, r#"{"SecurityGroups": []}"#.into(), String::new())
            }
            _ if cmd.starts_with("ec2 create-security-group") => {
                (0, r#"{"GroupId": "sg-pulse"}"#.into(), String::new())
            }
            _ if cmd.starts_with("ec2 authorize-security-group-ingress") => {
                (0, r#"{"Return": true}"#.into(), String::new())
            }
            _ if cmd.starts_with("ec2 run-instances") => {
                let tags = arg_after(&args, "--tag-specifications");
                let pick = |key: &str| -> String {
                    tags.split(&format!("{{Key={key},Value="))
                        .nth(1)
                        .and_then(|s| s.split('}').next())
                        .unwrap_or_default()
                        .to_string()
                };
                let i = AwsFake {
                    id: "i-0abc".into(),
                    name: pick("Name"),
                    request: pick("pulse-request"),
                    project: pick("pulse-project"),
                    state: "pending".into(),
                    ..Default::default()
                };
                let out = format!(r#"{{"Instances": [{}]}}"#, f.instance_json(&i));
                f.instances.insert(i.id.clone(), i);
                (0, out, String::new())
            }
            _ if cmd.starts_with("ec2 describe-instances") => {
                f.describes += 1;
                let describes = f.describes;
                let instances = f
                    .instances
                    .values_mut()
                    .filter_map(|i| {
                        if !aws_filter_match(&args, i) {
                            return None;
                        }
                        if i.state == "pending" && describes >= 2 {
                            i.state = "running".into();
                            i.ip = "203.0.113.9".into();
                        }
                        Some(i.clone())
                    })
                    .collect::<Vec<_>>();
                let out = instances
                    .iter()
                    .map(|i| f.instance_json(i))
                    .collect::<Vec<_>>()
                    .join(",");
                (
                    0,
                    format!(r#"{{"Reservations": [{{"Instances": [{out}]}}]}}"#),
                    String::new(),
                )
            }
            _ if cmd.starts_with("ec2 get-console-output") => {
                f.consoles += 1;
                if f.consoles < 2 {
                    (0, "None\n".into(), String::new())
                } else {
                    (
                        0,
                        format!(
                            "[    0.000000] Linux version 6.8\n{}{}\ncloud-init done\n",
                            crate::provider::HOSTKEY_MARKER,
                            TEST_HOST_KEY
                        ),
                        String::new(),
                    )
                }
            }
            _ if cmd.starts_with("ec2 stop-instances") => {
                if let Some(i) = f.instances.get_mut(&arg_after(&args, "--instance-ids")) {
                    i.state = "stopped".into();
                }
                (0, r#"{"StoppingInstances": []}"#.into(), String::new())
            }
            _ if cmd.starts_with("ec2 terminate-instances") => {
                let id = arg_after(&args, "--instance-ids");
                f.terminate.push(id.clone());
                if let Some(i) = f.instances.get_mut(&id) {
                    i.state = "terminated".into();
                }
                (0, r#"{"TerminatingInstances": []}"#.into(), String::new())
            }
            _ => (1, String::new(), format!("unexpected aws {cmd}")),
        };
        Ok(ans)
    }

    pub(crate) fn joined(&self) -> Vec<String> {
        self.0.lock().unwrap().cli.joined()
    }
    pub(crate) fn has(&self, prefix: &str) -> bool {
        self.0.lock().unwrap().cli.has(prefix)
    }
    pub(crate) fn first(&self, prefix: &str) -> String {
        self.0.lock().unwrap().cli.first(prefix)
    }
    pub(crate) fn reset(&self) {
        self.0.lock().unwrap().cli.reset();
    }
}

impl FakeAwsState {
    pub(crate) fn instance_json(&self, i: &AwsFake) -> String {
        let ip = if i.ip.is_empty() {
            String::new()
        } else {
            format!(r#""PublicIpAddress": "{}", "#, i.ip)
        };
        format!(
            r#"{{"InstanceId": "{}", {} "InstanceType": "t3.medium", "SubnetId": "subnet-1", "State": {{"Name": "{}"}},
        "Tags": [{{"Key": "Name", "Value": "{}"}}, {{"Key": "pulse", "Value": "hands"}}, {{"Key": "pulse-request", "Value": "{}"}}, {{"Key": "pulse-project", "Value": "{}"}}],
        "SecurityGroups": [{{"GroupId": "sg-pulse"}}]}}"#,
            i.id, ip, i.state, i.name, i.request, i.project
        )
    }
}

fn aws_filter_match(args: &[String], i: &AwsFake) -> bool {
    for a in args {
        let Some(rest) = a.strip_prefix("Name=") else {
            continue;
        };
        let Some((name, values)) = rest.split_once(",Values=") else {
            continue;
        };
        let have = match name {
            "instance-id" => &i.id,
            "instance-state-name" => &i.state,
            "tag:pulse" => "hands",
            "tag:pulse-request" => &i.request,
            "tag:Name" => &i.name,
            _ => return false,
        };
        if !contains(&values.split(',').collect::<Vec<_>>(), have) {
            return false;
        }
    }
    true
}

#[derive(Clone)]
pub(crate) struct FakeGcloud(pub Arc<Mutex<FakeGcloudState>>);

#[derive(Default)]
pub(crate) struct FakeGcloudState {
    pub cli: FakeCli,
    pub instances: BTreeMap<String, GcpFake>,
    pub describes: i32,
    pub serials: i32,
    pub deleted: Vec<String>,
    pub firewall: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GcpFake {
    pub name: String,
    pub status: String,
    pub ip: String,
    pub request: String,
    pub project: String,
}

pub(crate) fn new_fake_gcloud() -> FakeGcloud {
    FakeGcloud(Arc::new(Mutex::new(FakeGcloudState::default())))
}

impl FakeGcloud {
    pub(crate) fn run(
        &self,
        _ctx: &pulse_bus::CancellationToken,
        args: Vec<String>,
        _timeout: Duration,
    ) -> anyhow::Result<(i32, String, String)> {
        let mut f = self.0.lock().expect("fake gcloud");
        f.cli.record(&args)?;
        let cmd = args.join(" ");
        let ans = match () {
            _ if cmd.starts_with("auth list") => (0, "pulse@example.com\n".into(), String::new()),
            _ if cmd.starts_with("config get-value project") => {
                (0, "pulse-proj\n".into(), String::new())
            }
            _ if cmd.starts_with("compute firewall-rules describe") => {
                if f.firewall {
                    (0, r#"{"name": "pulse-hands-ssh"}"#.into(), String::new())
                } else {
                    (1, String::new(), "ERROR: (gcloud.compute.firewall-rules.describe) Could not fetch resource:\n - The resource 'projects/pulse-proj/global/firewalls/pulse-hands-ssh' was not found".into())
                }
            }
            _ if cmd.starts_with("compute firewall-rules create") => {
                f.firewall = true;
                (0, r#"[{"name": "pulse-hands-ssh"}]"#.into(), String::new())
            }
            _ if cmd.starts_with("compute instances describe") => {
                f.describes += 1;
                let describes = f.describes;
                let name = args[3].clone();
                if let Some(i) = f.instances.get_mut(&name) {
                    if i.status == "STAGING" && describes >= 2 {
                        i.status = "RUNNING".into();
                        i.ip = "203.0.113.11".into();
                    }
                    (0, FakeGcloudState::instance_json(i), String::new())
                } else {
                    (
                        1,
                        String::new(),
                        format!(
                            "ERROR: (gcloud.compute.instances.describe) Could not fetch resource:\n - The resource 'projects/pulse-proj/zones/europe-west1-b/instances/{name}' was not found"
                        ),
                    )
                }
            }
            _ if cmd.starts_with("compute instances create") => {
                let labels = arg_after(&args, "--labels")
                    .split(',')
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect::<BTreeMap<_, _>>();
                let i = GcpFake {
                    name: args[3].clone(),
                    status: "STAGING".into(),
                    request: labels.get("pulse-request").cloned().unwrap_or_default(),
                    project: labels.get("pulse-project").cloned().unwrap_or_default(),
                    ..Default::default()
                };
                let out = format!("[{}]", FakeGcloudState::instance_json(&i));
                f.instances.insert(i.name.clone(), i);
                f.describes = 0;
                (0, out, String::new())
            }
            _ if cmd.starts_with("compute instances get-serial-port-output") => {
                f.serials += 1;
                if f.serials < 2 {
                    (0, "[    0.000000] Linux version 6.8\n".into(), "Specify --start=1200 in the next get-serial-port-output invocation to get only the new output starting from here.".into())
                } else {
                    (
                        0,
                        format!(
                            "[    0.000000] Linux version 6.8\n{}{}\n",
                            crate::provider::HOSTKEY_MARKER,
                            TEST_HOST_KEY
                        ),
                        String::new(),
                    )
                }
            }
            _ if cmd.starts_with("compute instances list") => {
                let out = f
                    .instances
                    .values()
                    .map(FakeGcloudState::instance_json)
                    .collect::<Vec<_>>()
                    .join(",");
                (0, format!("[{out}]"), String::new())
            }
            _ if cmd.starts_with("compute instances stop") => {
                if let Some(i) = f.instances.get_mut(&args[3]) {
                    i.status = "TERMINATED".into();
                }
                (0, String::new(), String::new())
            }
            _ if cmd.starts_with("compute instances delete") => {
                f.deleted.push(args[3].clone());
                if f.instances.remove(&args[3]).is_none() {
                    (1, String::new(), "ERROR: (gcloud.compute.instances.delete) Could not fetch resource: - The resource was not found".into())
                } else {
                    (0, String::new(), String::new())
                }
            }
            _ => (1, String::new(), format!("unexpected gcloud {cmd}")),
        };
        Ok(ans)
    }
    pub(crate) fn joined(&self) -> Vec<String> {
        self.0.lock().unwrap().cli.joined()
    }
    pub(crate) fn has(&self, prefix: &str) -> bool {
        self.0.lock().unwrap().cli.has(prefix)
    }
    pub(crate) fn first(&self, prefix: &str) -> String {
        self.0.lock().unwrap().cli.first(prefix)
    }
    pub(crate) fn reset(&self) {
        self.0.lock().unwrap().cli.reset();
    }
}

impl FakeGcloudState {
    pub(crate) fn instance_json(i: &GcpFake) -> String {
        let access = if i.ip.is_empty() {
            String::new()
        } else {
            format!(r#"{{"natIP": "{}"}}"#, i.ip)
        };
        format!(
            r#"{{"name": "{}", "status": "{}", "zone": "https://www.googleapis.com/compute/v1/projects/pulse-proj/zones/europe-west1-b",
        "machineType": "https://www.googleapis.com/compute/v1/projects/pulse-proj/zones/europe-west1-b/machineTypes/e2-medium",
        "labels": {{"pulse": "hands", "pulse-request": "{}", "pulse-project": "{}"}}, "networkInterfaces": [{{"accessConfigs": [{}]}}]}}"#,
            i.name, i.status, i.request, i.project, access
        )
    }
}

#[derive(Clone)]
pub(crate) struct FakeMultipass(pub Arc<Mutex<FakeMultipassState>>);

#[derive(Default)]
pub(crate) struct FakeMultipassState {
    pub cli: FakeCli,
    pub instances: BTreeMap<String, MpFake>,
    pub infos: i32,
    pub execs: i32,
    pub deleted: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MpFake {
    pub name: String,
    pub state: String,
    pub ip: String,
}

pub(crate) fn new_fake_multipass() -> FakeMultipass {
    FakeMultipass(Arc::new(Mutex::new(FakeMultipassState::default())))
}

impl FakeMultipass {
    pub(crate) fn run(
        &self,
        _ctx: &pulse_bus::CancellationToken,
        args: Vec<String>,
        _timeout: Duration,
    ) -> anyhow::Result<(i32, String, String)> {
        let mut f = self.0.lock().expect("fake multipass");
        f.cli.record(&args)?;
        let cmd = args.join(" ");
        let ans = match () {
            _ if cmd == "version" => (
                0,
                "multipass   1.15.0\nmultipassd  1.15.0\n".into(),
                String::new(),
            ),
            _ if cmd.starts_with("info ") => {
                f.infos += 1;
                let infos = f.infos;
                let name = args[1].clone();
                if let Some(i) = f.instances.get_mut(&name) {
                    if i.state == "Starting" && infos >= 2 {
                        i.state = "Running".into();
                        i.ip = "192.168.64.7".into();
                    }
                    (
                        0,
                        format!(
                            r#"{{"errors": [], "info": {{"{}": {}}}}}"#,
                            i.name,
                            FakeMultipassState::info_json(i)
                        ),
                        String::new(),
                    )
                } else {
                    (
                        2,
                        String::new(),
                        format!(r#"info failed: instance "{name}" does not exist"#),
                    )
                }
            }
            _ if cmd.starts_with("launch ") => {
                let i = MpFake {
                    name: arg_after(&args, "--name"),
                    state: "Starting".into(),
                    ..Default::default()
                };
                f.infos = 0;
                f.instances.insert(i.name.clone(), i.clone());
                (0, format!("Launched: {}\n", i.name), String::new())
            }
            _ if cmd.starts_with("exec ") => {
                f.execs += 1;
                if f.execs < 2 {
                    (
                        1,
                        String::new(),
                        "cat: /etc/ssh/ssh_host_ed25519_key.pub: No such file or directory".into(),
                    )
                } else {
                    (0, format!("{TEST_HOST_KEY}\n"), String::new())
                }
            }
            _ if cmd == "list --format json" => {
                let out = f
                    .instances
                    .values()
                    .map(FakeMultipassState::info_json)
                    .collect::<Vec<_>>()
                    .join(",");
                (0, format!(r#"{{"list": [{out}]}}"#), String::new())
            }
            _ if cmd.starts_with("stop ") => {
                if let Some(i) = f.instances.get_mut(&args[1]) {
                    i.state = "Stopped".into();
                    i.ip.clear();
                }
                (0, String::new(), String::new())
            }
            _ if cmd.starts_with("delete --purge ") => {
                f.deleted.push(args[2].clone());
                if f.instances.remove(&args[2]).is_none() {
                    (
                        2,
                        String::new(),
                        format!(r#"delete failed: instance "{}" does not exist"#, args[2]),
                    )
                } else {
                    (0, String::new(), String::new())
                }
            }
            _ => (1, String::new(), format!("unexpected multipass {cmd}")),
        };
        Ok(ans)
    }
    pub(crate) fn joined(&self) -> Vec<String> {
        self.0.lock().unwrap().cli.joined()
    }
    pub(crate) fn has(&self, prefix: &str) -> bool {
        self.0.lock().unwrap().cli.has(prefix)
    }
    pub(crate) fn first(&self, prefix: &str) -> String {
        self.0.lock().unwrap().cli.first(prefix)
    }
    pub(crate) fn reset(&self) {
        self.0.lock().unwrap().cli.reset();
    }
}

impl FakeMultipassState {
    pub(crate) fn info_json(i: &MpFake) -> String {
        let ip = if i.ip.is_empty() {
            "[]".to_string()
        } else {
            format!(r#"["{}"]"#, i.ip)
        };
        format!(
            r#"{{"name": "{}", "state": "{}", "ipv4": {}, "image_release": "24.04 LTS", "release": "Ubuntu 24.04 LTS"}}"#,
            i.name, i.state, ip
        )
    }
}

#[derive(Clone, Default)]
pub(crate) struct PairedState {
    pub admin: String,
    pub name: String,
    pub opts: crate::pair::PairOpts,
}

pub(crate) struct ComputeHarness {
    pub hub_home: String,
    _hub_home_dir: tempfile::TempDir,
    pub paired: Arc<Mutex<PairedState>>,
    old_wait: Duration,
    old_ssh: crate::executor::SshRunner,
    old_pair: Option<Box<crate::provider::PairHandsFn>>,
    old_place: crate::executor::PlaceLoginFn,
}

impl Drop for ComputeHarness {
    fn drop(&mut self) {
        let _ = crate::provider::set_provision_wait(self.old_wait);
        crate::executor::restore_run_ssh(self.old_ssh.clone());
        if let Some(old) = self.old_pair.take() {
            let _ = crate::provider::set_pair_hands(old);
        }
        *crate::executor::PLACE_LOGIN.write().expect("place login") = self.old_place;
        SSH_CALLS.lock().unwrap().clear();
        PLACE_LOGIN_CALLS.lock().unwrap().clear();
    }
}

fn fake_run_ssh(
    _ctx: &pulse_bus::CancellationToken,
    argv: &[String],
    _stdin: &[u8],
    _timeout: Duration,
) -> anyhow::Result<(i32, String, String)> {
    SSH_CALLS.lock().unwrap().push(argv.join(" "));
    Ok((0, "pulse-ready\n".into(), String::new()))
}

#[allow(clippy::too_many_arguments)]
fn fake_place_login(
    _s: &pulse_store::Store,
    _now: &str,
    _name: &str,
    admin_address: &str,
    _token: &mut dyn std::io::Read,
    _is_tty: bool,
    _remove: bool,
    _actor: &str,
) -> anyhow::Result<crate::executor::LoginResult> {
    PLACE_LOGIN_CALLS.lock().unwrap().push(PlaceLoginCall {
        admin: admin_address.to_string(),
    });
    Ok(crate::executor::LoginResult {
        ok: true,
        kind: "placed".into(),
        lines: vec!["placed login".into()],
        ..Default::default()
    })
}

pub(crate) fn new_compute_harness(
    s: &pulse_store::Store,
    settings: &[(&str, &str)],
) -> ComputeHarness {
    pulse_bus::reset_default();
    crate::commands::register();
    new_bare_repo(s);
    join(s, "aurora");
    let hub_home_dir = tempfile::tempdir().expect("hub home");
    let hub_home = hub_home_dir.path().to_path_buf();
    let ssh = hub_home.join(".ssh");
    std::fs::create_dir_all(&ssh).expect("ssh dir");
    std::fs::write(ssh.join("pulse_hands_ed25519"), "fake private key\n").expect("private key");
    std::fs::write(
        ssh.join("pulse_hands_ed25519.pub"),
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUB pulse-hub\n",
    )
    .expect("public key");
    for (k, v) in [
        ("compute.max_vms", "2"),
        ("compute.max_hours_per_day", "24"),
        ("minutes_per_job", "1"),
    ] {
        s.set_setting(crate::util::HUMAN_AGENT, k, v)
            .expect("set default");
    }
    for (k, v) in settings {
        s.set_setting(crate::util::HUMAN_AGENT, k, v)
            .expect("set setting");
    }
    let old_wait = crate::provider::set_provision_wait(Duration::from_millis(5));
    crate::test_helpers_hands::write_executor_token(s);
    let old_ssh = crate::executor::run_ssh_hook();
    crate::executor::set_run_ssh(fake_run_ssh);
    let old_place = *crate::executor::PLACE_LOGIN.read().expect("place login");
    *crate::executor::PLACE_LOGIN.write().expect("place login") = fake_place_login;
    let paired = Arc::new(Mutex::new(PairedState::default()));
    let paired_for_fn = Arc::clone(&paired);
    let old_pair = crate::provider::set_pair_hands(Box::new(move |st, admin, name, now, o| {
        {
            let mut p = paired_for_fn.lock().expect("paired");
            p.admin = admin.to_string();
            p.name = name.to_string();
            p.opts = o.clone();
        }
        crate::executor::add_executor(
            st,
            now,
            name,
            admin,
            &o.workdir,
            o.workers,
            &o.projects,
            pulse_store::HUMAN_AGENT,
        )?;
        st.conn()?.execute(
            "UPDATE executors SET status = 'active' WHERE name = ?",
            [&name],
        )?;
        Ok(Some(Box::new(crate::pair::PairReport {
            ok: true,
            lines: vec![format!("paired {name}")],
            ..Default::default()
        })))
    }));
    ComputeHarness {
        hub_home: hub_home.display().to_string(),
        _hub_home_dir: hub_home_dir,
        paired,
        old_wait,
        old_ssh,
        old_pair: Some(old_pair),
        old_place,
    }
}

impl ComputeHarness {
    pub(crate) fn provision_and_release(&self, s: &pulse_store::Store, provider: &str, ip: &str) {
        let req = pulse_tasks::request_compute(
            s,
            "proj",
            "aurora",
            "",
            2,
            "the fixtures take an hour here",
        )
        .expect("request compute");
        pulse_tasks::approve(s, &req.id, "go ahead", pulse_store::HUMAN_AGENT).expect("approve");
        let log = crate::test_helpers::once_daemon(
            s,
            crate::daemon::DaemonOpts {
                hub_home: self.hub_home.clone(),
                ..Default::default()
            },
        );
        let got = pulse_tasks::get_compute(s, &req.id).expect("get compute");
        assert_eq!(
            (got.status.as_str(), got.executor.as_str()),
            ("ready", "hands-cr-001"),
            "after provisioning: {got:?}\n{log}"
        );
        let paired = self.paired.lock().expect("paired").clone();
        assert_eq!(paired.name, "hands-cr-001");
        assert_eq!(paired.admin, "pulseadmin@pulse-cr-001-admin");
        assert_eq!(paired.opts.projects, "proj");
        assert!(!paired.opts.install_copilot);
        assert_eq!(paired.opts.workdir, crate::provider::HANDS_WORKDIR);
        let ex = crate::executor::executor_row(s, "hands-cr-001");
        assert_eq!(ex.str("status"), "active", "executor: {ex:?}");
        assert_eq!(ex.str("admin"), format!("pulseadmin@{ip}"));
        let placed = PLACE_LOGIN_CALLS.lock().unwrap().clone();
        assert_eq!(placed.len(), 1, "login placement: {placed:?}");
        assert_eq!(placed[0].admin, format!("pulseadmin@{ip}"));
        let known = std::fs::read_to_string(format!("{}/.ssh/known_hosts", self.hub_home))
            .expect("known hosts");
        let host_key = TEST_HOST_KEY
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            known.contains(&format!("{ip} {host_key}")),
            "known_hosts: {known:?}"
        );
        let ssh_cfg =
            std::fs::read_to_string(format!("{}/.ssh/config", self.hub_home)).expect("ssh config");
        assert!(
            ssh_cfg.contains("Host pulse-cr-001-admin")
                && ssh_cfg.contains(&format!("HostName {ip}")),
            "ssh config: {ssh_cfg:?}"
        );
        let (m, mrow) = crate::compute::load_machine(s, &req.id);
        let m = m.expect("machine");
        assert_eq!(m.provider, provider);
        assert_eq!(m.name, "pulse-cr-001");
        assert_eq!(m.tags.get("pulse").map(String::as_str), Some("hands"));
        assert_eq!(
            m.tags.get("pulse-request").map(String::as_str),
            Some(req.id.as_str())
        );
        assert_eq!(m.detail.get("public_ip").map(String::as_str), Some(ip));
        assert_eq!(m.address, format!("pulseadmin@{ip}"));
        assert_eq!(mrow.str("released_at"), "");
        let ssh = SSH_CALLS.lock().unwrap().clone();
        assert!(
            ssh.first()
                .is_some_and(|s| s.contains("cloud-init status --wait")),
            "ssh: {ssh:?}"
        );
        let later = pulse_store::Store::open(pulse_store::Options {
            home: s.home.clone(),
            now: "2026-09-06T13:00:00Z".into(),
            ..Default::default()
        })
        .expect("later store");
        let log3 = crate::test_helpers::once_daemon(
            &later,
            crate::daemon::DaemonOpts {
                hub_home: self.hub_home.clone(),
                ..Default::default()
            },
        );
        let got = pulse_tasks::get_compute(&later, &req.id).expect("released compute");
        assert_eq!(got.status, "released", "after the hours: {got:?}\n{log3}");
        assert!(
            crate::executor::executor_row(&later, "hands-cr-001")
                .0
                .is_empty(),
            "executor still registered"
        );
        let (_, mrow) = crate::compute::load_machine(&later, &req.id);
        assert!(
            !mrow.str("released_at").is_empty(),
            "machine not marked released: {mrow:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{PostOpts, actor_agent, create_post, post_row};
    use crate::plan::{plan_jobs, project_posts};
    use crate::test_helpers::{TEST_NOW, new_bare_repo, new_store};
    use crate::test_helpers_completion::suppress_planner_reflect;
    use crate::util::{exec1, plus_hours, plus_minutes};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn extra(values: &[(&str, serde_json::Value)]) -> BTreeMap<String, serde_json::Value> {
        values
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    fn post_opts(
        kind: &str,
        project: &str,
        area: &str,
        thread: &str,
        summary: &str,
        body: &str,
        session: &str,
    ) -> PostOpts {
        PostOpts {
            kind: kind.into(),
            project: project.into(),
            area: area.into(),
            thread: thread.into(),
            summary: summary.into(),
            body: body.into(),
            agent: None,
            session: session.into(),
            ..Default::default()
        }
    }

    #[test]
    fn plan_jobs_for_owner_metadata() {
        let _serial = crate::test_helpers::serial();
        let cases = [
            (
                "marked",
                "review",
                "Choose the support window",
                extra(&[("for_owner", json!(true))]),
                true,
            ),
            (
                "frontmatter true",
                "review",
                "Choose the support window",
                extra(&[("for_owner", json!("true"))]),
                true,
            ),
            (
                "unmarked",
                "work",
                "Improve the parser",
                BTreeMap::new(),
                false,
            ),
            (
                "legacy wording",
                "owner-questions",
                "Release candidate for the owner; vote requested",
                BTreeMap::new(),
                false,
            ),
            (
                "explicit false",
                "owner-questions",
                "Owner question: document the response",
                extra(&[("for_owner", json!(false))]),
                false,
            ),
            (
                "frontmatter false",
                "owner-review",
                "Evaluator proposal for the owner",
                extra(&[("for_owner", json!("false"))]),
                false,
            ),
        ];
        for kind in ["suggestion", "handoff", "question"] {
            for (name, thread, summary, extra, for_owner) in cases.clone() {
                let s = new_store();
                new_bare_repo(&s);
                suppress_planner_reflect(&s);
                let mut first_opts = post_opts(
                    kind,
                    "proj",
                    "routing",
                    thread,
                    summary,
                    "Signal: check:owner-routing\nfor_owner: true\n",
                    "s-author-one",
                );
                first_opts.extra = extra;
                first_opts.agent = actor_agent(&s, "author-one".into());
                let first =
                    create_post(&s, plus_hours(TEST_NOW, -2.0), first_opts).expect("first post");
                let mut next_opts = post_opts(
                    kind,
                    "proj",
                    "routing",
                    "next-card",
                    "Continue the parser work",
                    "Signal: check:owner-routing\n",
                    "s-author-two",
                );
                next_opts.agent = actor_agent(&s, "author-two".into());
                let next =
                    create_post(&s, plus_hours(TEST_NOW, -1.0), next_opts).expect("next post");
                if kind == "suggestion" {
                    for card in [&first, &next] {
                        for (i, voter) in
                            ["voter-one", "voter-two", "voter-three"].iter().enumerate()
                        {
                            if card.str("id") == next.str("id") && i == 2 {
                                break;
                            }
                            create_post(
                                &s,
                                TEST_NOW.into(),
                                PostOpts {
                                    kind: "vote".into(),
                                    project: "proj".into(),
                                    area: card.str("area"),
                                    thread: card.str("thread"),
                                    summary: "One measurable change".into(),
                                    on: card.str("id"),
                                    vote: "up".into(),
                                    agent: actor_agent(&s, (*voter).into()),
                                    session: format!("s-{voter}"),
                                    ..Default::default()
                                },
                            )
                            .expect("vote post");
                        }
                    }
                }
                let want = if for_owner {
                    next.str("id")
                } else {
                    first.str("id")
                };
                let jobs =
                    plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into()).expect(name);
                assert_eq!(
                    jobs.len(),
                    1,
                    "{kind}/{name}: for_owner={for_owner}: planned {} jobs, want one",
                    jobs.len()
                );
                assert_eq!(
                    jobs[0].str("source"),
                    format!("{kind}:{want}"),
                    "{kind}/{name}"
                );
            }
        }
    }

    #[test]
    fn post_row_parses_for_owner() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        for (name, extra, want) in [
            ("missing", BTreeMap::new(), false),
            ("true", extra(&[("for_owner", json!(true))]), true),
            ("false", extra(&[("for_owner", json!(false))]), false),
            (
                "frontmatter true",
                extra(&[("for_owner", json!("true"))]),
                true,
            ),
            (
                "frontmatter false",
                extra(&[("for_owner", json!("false"))]),
                false,
            ),
        ] {
            let p = create_post(
                &s,
                TEST_NOW.into(),
                PostOpts {
                    kind: "question".into(),
                    project: "proj".into(),
                    area: "routing".into(),
                    thread: "owner-review".into(),
                    summary: "Owner question".into(),
                    extra,
                    agent: actor_agent(&s, "author-one".into()),
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                p.get("for_owner").and_then(|v| v.as_bool()) == Some(true),
                want
            );
            let conn = s.conn().expect("conn");
            assert_eq!(
                post_row(&conn, p.str("id"))
                    .get("for_owner")
                    .and_then(|v| v.as_bool())
                    == Some(true),
                want
            );
            let mut found = false;
            for r in project_posts(&conn, "proj".into()) {
                if r.str("id") == p.str("id") {
                    found = true;
                    assert_eq!(
                        r.get("for_owner").and_then(|v| v.as_bool()) == Some(true),
                        want
                    );
                }
            }
            assert!(found, "post missing from project rows");
        }
        let conn = s.conn().expect("conn");
        assert!(post_row(&conn, "missing".into()).is_empty());
    }

    #[test]
    fn plan_jobs_never_answers_machine_questions() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        suppress_planner_reflect(&s);
        let mut at = plus_hours(TEST_NOW, -4.0);
        for (name, agent, summary, signal) in [
            (
                "legacy stop question by the daemon, unflagged",
                "daemon",
                "Evolution stopped; owner review needed",
                "check:owner-routing",
            ),
            (
                "daemon question with another summary",
                "daemon",
                "Which recipe should the clone use?",
                "check:owner-routing",
            ),
            (
                "stop summary from a worker",
                "kestrel",
                pulse_evolve::STOP_SUMMARY,
                "check:owner-routing",
            ),
            (
                "owner-path question",
                "kestrel",
                "Should the stop file be lifted?",
                "docs/evolution/STOP",
            ),
        ] {
            at = plus_minutes(&at, 5.0);
            create_post(
                &s,
                at.clone(),
                PostOpts {
                    kind: "question".into(),
                    project: "proj".into(),
                    area: "german-tutor".into(),
                    thread: "v8-only-site".into(),
                    summary: summary.into(),
                    body: format!("Signal: {signal}\n{name}\n"),
                    agent: actor_agent(&s, agent.into()),
                    session: format!("s-{agent}"),
                    ..Default::default()
                },
            )
            .expect("machine question");
        }
        let jobs =
            plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into()).expect("plan jobs");
        assert!(
            jobs.is_empty(),
            "planned {} job(s) for machine or owner questions: {}",
            jobs.len(),
            jobs.first().map(|j| j.str("source")).unwrap_or_default()
        );
        at = plus_minutes(&at, 5.0);
        let human = create_post(
            &s,
            at,
            PostOpts {
                kind: "question".into(),
                project: "proj".into(),
                area: "german-tutor".into(),
                thread: "vocabulary".into(),
                summary: "Why does the parser drop umlauts?".into(),
                body: "Signal: check:owner-routing\n".into(),
                agent: actor_agent(&s, "author-one".into()),
                session: "s-author-one".into(),
                ..Default::default()
            },
        )
        .expect("human question");
        let jobs =
            plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into()).expect("plan jobs");
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].str("source"),
            format!("question:{}", human.str("id"))
        );
        assert_eq!(jobs[0].str("kind"), "answer");
    }

    #[test]
    fn plan_jobs_one_tried_question_per_thread() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        suppress_planner_reflect(&s);
        s.set_setting(crate::util::HUMAN_AGENT, "parallel_per_project", "3")
            .expect("parallel setting");
        let ask = |at: String, agent: &str, thread: &str, summary: &str| {
            create_post(
                &s,
                at,
                PostOpts {
                    kind: "question".into(),
                    project: "proj".into(),
                    area: "api".into(),
                    thread: thread.into(),
                    summary: summary.into(),
                    body: "Signal: check:owner-routing\n".into(),
                    agent: actor_agent(&s, agent.into()),
                    session: format!("s-{agent}"),
                    ..Default::default()
                },
            )
            .expect("ask")
        };
        let first = ask(
            plus_hours(TEST_NOW, -3.0),
            "author-one",
            "rate-limits",
            "Which fixture covers 429s?",
        );
        let second = ask(
            plus_hours(TEST_NOW, -2.0),
            "author-two",
            "rate-limits",
            "Should the fixture also cover 503s?",
        );
        let elsewhere = ask(
            plus_hours(TEST_NOW, -1.0),
            "author-two",
            "retries",
            "How long is the retry pause?",
        );
        let plan = || plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into()).expect("plan");
        let mut jobs = plan();
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].str("source"),
            format!("question:{}", first.str("id"))
        );
        jobs = plan();
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].str("source"),
            format!("question:{}", elsewhere.str("id"))
        );
        exec1(
            &s.conn().expect("conn"),
            "UPDATE jobs SET status = 'failed', finished_at = ?, result = 'blocked: owner disposition required' WHERE source = ?",
            &[&TEST_NOW, &format!("question:{}", first.str("id"))],
        );
        jobs = plan();
        assert!(
            jobs.is_empty(),
            "after the failed attempt, planned {}; want the thread to wait for a human",
            jobs.first().map(|j| j.str("source")).unwrap_or_default()
        );
        create_post(
            &s,
            plus_minutes(TEST_NOW, 1.0),
            PostOpts {
                kind: "answer".into(),
                project: "proj".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                summary: "the 429 fixture is tests/limits.json".into(),
                on: first.str("id"),
                agent: actor_agent(&s, "author-one".into()),
                session: "s-author-one".into(),
                ..Default::default()
            },
        )
        .expect("answer");
        let third = ask(
            plus_minutes(TEST_NOW, 2.0),
            "author-two",
            "rate-limits",
            "Does tests/limits.json need a 503 case?",
        );
        jobs = plan();
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].str("source"),
            format!("question:{}", third.str("id"))
        );
        assert_ne!(
            jobs[0].str("source"),
            format!("question:{}", second.str("id"))
        );
    }
}
