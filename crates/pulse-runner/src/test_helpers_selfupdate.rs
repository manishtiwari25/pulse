use std::path::Path;
use std::time::{Duration, Instant};

use crate::selfupdate::{SELF_UPDATE_STATE, SelfUpdateEnv, SelfUpdateState};
use crate::test_helpers_daemon::test_wait;

pub(crate) fn fake_cargo() -> &'static str {
    r#"#!/bin/sh
if [ -n "$FAKE_CARGO_FAIL" ]; then
  echo "error[E0425]: cannot find value \`nope\` in this scope"
  echo " --> crates/pulse/src/main.rs:9:2"
  echo "error: could not compile \`pulse\` (bin \"pulse\") due to 1 previous error"
  exit 1
fi
target=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--target-dir" ]; then target=$2; shift 2; continue; fi
  shift
done
rev=$(git rev-parse HEAD | cut -c1-12)
if [ -n "$FAKE_CARGO_BADVERSION" ]; then rev=cafebabe0000; fi
mkdir -p "$target/release"
printf '#!/bin/sh\ncase "$1" in version) echo dev-%s;; help) exit 0;; *) exit 1;; esac\n' "$rev" > "$target/release/pulse"
chmod +x "$target/release/pulse"
"#
}

pub(crate) fn reset_self_update() {
    *SELF_UPDATE_STATE
        .write()
        .expect("self-update state poisoned") = SelfUpdateState::default();
}

pub(crate) struct ResetSelfUpdateGuard;

impl Drop for ResetSelfUpdateGuard {
    fn drop(&mut self) {
        reset_self_update();
    }
}

pub(crate) struct SigningSystemGuard(String);

impl Drop for SigningSystemGuard {
    fn drop(&mut self) {
        *crate::sign::SIGNING_SYSTEM.write().unwrap() = self.0.clone();
    }
}

pub(crate) struct SelfUpdateFixtureGuards {
    _env: Vec<pulse_testutil::SetenvGuard>,
    _signing: SigningSystemGuard,
    _reset: ResetSelfUpdateGuard,
}

pub(crate) fn wait_self_update_idle() {
    let deadline = Instant::now() + test_wait(Duration::from_secs(20));
    while Instant::now() < deadline {
        let busy = SELF_UPDATE_STATE
            .read()
            .expect("self-update state poisoned")
            .inflight;
        if !busy {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the build did not finish");
}

pub(crate) fn self_update_fixture() -> (
    crate::test_helpers::TestStore,
    String,
    SelfUpdateEnv,
    SelfUpdateFixtureGuards,
) {
    reset_self_update();
    let reset = ResetSelfUpdateGuard;
    let old_signing = crate::sign::SIGNING_SYSTEM.read().unwrap().clone();
    *crate::sign::SIGNING_SYSTEM.write().unwrap() = "linux".into();
    let signing = SigningSystemGuard(old_signing);
    let s = crate::test_helpers::new_store();
    let (_, repo) = crate::test_helpers::new_bare_repo(&s);
    std::fs::write(Path::new(&repo).join(".gitignore"), "target/\n").unwrap();
    crate::test_helpers::run(&repo, &["git", "add", ".gitignore"]);
    crate::test_helpers::run(&repo, &["git", "commit", "-q", "-m", "ignore cargo target"]);
    s.conn()
        .unwrap()
        .execute("UPDATE projects SET brain = 1 WHERE slug = 'proj'", [])
        .unwrap();
    let tools = Path::new(&s.home).join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let cargo = tools.join("cargo");
    std::fs::write(&cargo, fake_cargo()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let bin_dir = Path::new(&s.home).join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let bin = bin_dir.join("pulse");
    std::fs::write(&bin, b"old binary").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    crate::test_helpers_daemon::set_setting(&s, "daemon.self_update", "on");
    // The fixtures build as soon as HEAD differs; the settle time has its own test.
    crate::test_helpers_daemon::set_setting(&s, "daemon.self_update_settle_minutes", "0");
    let path = format!(
        "{}{}{}",
        tools.display(),
        if cfg!(windows) { ";" } else { ":" },
        std::env::var("PATH").unwrap_or_default()
    );
    let guards = vec![
        pulse_testutil::setenv("PATH", path),
        pulse_testutil::setenv("FAKE_CARGO_FAIL", ""),
        pulse_testutil::setenv("FAKE_CARGO_BADVERSION", ""),
    ];
    (
        s,
        repo,
        SelfUpdateEnv {
            binary: bin.display().to_string(),
            cargo_cmd: "cargo".into(),
            timeout: Duration::from_secs(10),
            revision: "0000000000000000000000000000000000000000".into(),
            actor: "daemon".into(),
            inflight: 2,
            measuring: false,
            local_inflight: 0,
            governor: None,
            refused_queued: String::new(),
        },
        SelfUpdateFixtureGuards {
            _env: guards,
            _signing: signing,
            _reset: reset,
        },
    )
}

pub(crate) fn head_of(repo: &str) -> String {
    crate::test_helpers::run(repo, &["git", "rev-parse", "HEAD"])
        .trim()
        .to_string()
}

pub(crate) fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        let ty = entry.file_type().unwrap();
        if ty.is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}
