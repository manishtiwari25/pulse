//! Register and run `pulse cred`, the owner's console for the daemon push key.

use anyhow::Result;
use pulse_cli::{Command, Context, FlagSet, errorf, register as register_command, write_json};
use pulse_cred_setup::{Deps as SetupDeps, Probe, Refusal};
use std::io::{self, IsTerminal, Read, Write};
use std::sync::{Arc, Once, OnceLock, RwLock};

type ResolverFactory = Arc<dyn Fn(&str) -> pulse_cred::Resolver + Send + Sync>;
type StdinFactory = Arc<dyn Fn() -> Box<dyn Read + Send> + Send + Sync>;
type StdinTtyFn = Arc<dyn Fn() -> bool + Send + Sync>;
type ProbeFn = Arc<dyn Fn(&str, &str, &str) -> Result<()> + Send + Sync>;
const ACTOR_PATTERN: &str = "^[A-Za-z0-9._:-]{1,64}$";

#[derive(Clone)]
struct Deps {
    resolver: ResolverFactory,
    stdin: StdinFactory,
    stdin_tty: StdinTtyFn,
    probe: ProbeFn,
}

impl Default for Deps {
    fn default() -> Self {
        Self {
            resolver: Arc::new(pulse_cred::new),
            stdin: Arc::new(|| Box::new(io::stdin())),
            stdin_tty: Arc::new(|| io::stdin().is_terminal()),
            probe: Arc::new(pulse_cred_setup::probe_remote),
        }
    }
}

static REGISTER: Once = Once::new();
static DEPS: OnceLock<RwLock<Deps>> = OnceLock::new();

fn deps() -> Deps {
    DEPS.get_or_init(|| RwLock::new(Deps::default()))
        .read()
        .expect("cred command deps poisoned")
        .clone()
}

#[cfg(test)]
fn set_deps(d: Deps) -> Deps {
    let lock = DEPS.get_or_init(|| RwLock::new(Deps::default()));
    let mut guard = lock.write().expect("cred command deps poisoned");
    std::mem::replace(&mut *guard, d)
}

/// Registers the `cred` command once.
pub fn register() {
    REGISTER.call_once(|| {
        register_command(Command {
            name: "cred".into(),
            group: "human".into(),
            summary: "the daemon's own push key, by reference: token, set, status, clear (ADR-041)"
                .into(),
            usage: format!(
                concat!(
                    "pulse cred token [--from-file PATH]      (1Password service-account token from stdin, kept in the keychain or a 0600 file)\n",
                    "       pulse cred set REF [--no-probe]         (REF: {})\n",
                    "       pulse cred status [--json]\n",
                    "       pulse cred clear [--token]"
                ),
                pulse_cred::SHAPES
            ),
            aliases: Vec::new(),
            no_store: true,
            read_only: false,
            raw: true,
            run,
            flags: None,
        });
    });
}

fn owner_actor(actor: &str) -> bool {
    pulse_store::human_actor(actor) && actor_pattern(actor) && !pulse_store::secret_shaped(actor)
}

fn actor_pattern(actor: &str) -> bool {
    let _go_pattern = ACTOR_PATTERN;
    !actor.is_empty()
        && actor.len() <= 64
        && actor
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

fn require_owner(home: &str, actor: &str) -> Result<()> {
    let env_actor = std::env::var("PULSE_AGENT").unwrap_or_default();
    if !owner_actor(actor)
        || (!env_actor.is_empty() && !owner_actor(&env_actor))
        || pulse_store::worker_guard(home)
    {
        return Err(errorf(
            2,
            "Only the owner handles the push credential",
            "pulse cred names the key the daemon pushes with (ADR-041); agents may read pulse cred status and suggest, never set or clear it.",
            "Run pulse cred from your own shell, without PULSE_AGENT.",
        )
        .into());
    }
    Ok(())
}

fn run(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    let (sub, rest_args) = if let Some((first, rest)) = args.split_first() {
        (first.as_str(), rest)
    } else {
        ("status", &[][..])
    };

    let mut fs = FlagSet::new(format!("cred {sub}"));
    fs.set_output(pulse_cli::flags::Output::Discard);
    match sub {
        "token" => fs.string_var(
            "from-file",
            "",
            "read the token from this file instead of stdin",
        ),
        "set" => fs.bool_var(
            "no-probe",
            false,
            "save without pushing a dry run to the remote",
        ),
        "status" => fs.bool_var("json", false, "machine-readable"),
        "clear" => fs.bool_var(
            "token",
            false,
            "also remove the stored service-account token",
        ),
        _ => {
            return Err(errorf(
                2,
                &format!("Unknown cred command {sub}"),
                "Use token, set REF, status or clear.",
                "pulse help cred",
            )
            .into());
        }
    }

    let mut flag_args = Vec::new();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < rest_args.len() {
        let a = &rest_args[i];
        if a == "--" {
            rest.extend(rest_args[i + 1..].iter().cloned());
            break;
        }
        if a.starts_with('-') && a.len() > 1 && !pulse_cred::is_private_key(a.as_bytes()) {
            flag_args.push(a.clone());
            if a.trim_start_matches('-') == "from-file" && i + 1 < rest_args.len() {
                i += 1;
                flag_args.push(rest_args[i].clone());
            }
        } else {
            rest.push(a.clone());
        }
        i += 1;
    }

    if let Err(err) = fs.parse(&flag_args) {
        for a in rest_args {
            if pulse_cred::is_private_key(a.as_bytes()) || pulse_store::secret_shaped(a) {
                return Err(not_a_reference());
            }
        }
        return Err(errorf(
            2,
            &format!(
                "pulse cred {sub}: {}",
                pulse_cred::redact(&err.to_string(), &[])
            ),
            "",
            "pulse help cred",
        )
        .into());
    }
    rest.extend(fs.args().iter().cloned());
    if (sub == "set" && rest.len() != 1) || (sub != "set" && !rest.is_empty()) {
        return Err(errorf(
            2,
            "Wrong cred arguments",
            "set takes one reference; token, status and clear take flags only.",
            "pulse help cred",
        )
        .into());
    }

    let mut actor = c.agent.clone();
    if actor.is_empty() {
        actor = std::env::var("PULSE_AGENT").unwrap_or_default();
    }
    if actor.is_empty() {
        actor = pulse_store::HUMAN_AGENT.into();
    }

    let mut home = c.home.clone();
    if let Some(s) = &c.store {
        home = s.home.clone();
    }
    if sub != "status" {
        require_owner(&home, &actor)?;
    }

    let opened;
    let s = if let Some(s) = &c.store {
        s.clone()
    } else {
        opened = pulse_store::Store::open(pulse_store::Options {
            home,
            read_only: sub == "status",
            skip_records: true,
            now: c.now.clone(),
            ..pulse_store::Options::default()
        })?;
        opened
    };
    let d = deps();
    let r = (d.resolver)(&s.home);

    match sub {
        "token" => run_token(c, &r, &fs.get_string("from-file"), &d),
        "set" => run_set(c, &s, &r, &actor, &rest[0], fs.get_bool("no-probe"), &d),
        "status" => run_status(c, &s, &r, fs.get_bool("json") || c.json),
        "clear" => run_clear(c, &s, &r, &actor, fs.get_bool("token")),
        _ => Ok(()),
    }
}

fn run_token(c: &mut Context, r: &pulse_cred::Resolver, from_file: &str, d: &Deps) -> Result<()> {
    let raw = if !from_file.is_empty() {
        std::fs::read(from_file)
    } else {
        if (d.stdin_tty)() {
            c.err.write_all(
                b"Paste the 1Password service-account token, then press Enter and Ctrl-D:\n",
            )?;
        }
        let mut raw = Vec::new();
        (d.stdin)()
            .take(64 * 1024)
            .read_to_end(&mut raw)
            .map(|_| raw)
    };
    let raw = raw.map_err(|err| {
        errorf(
            1,
            "Could not read the token",
            &err.to_string(),
            "pulse cred token < token.txt, or --from-file PATH",
        )
    })?;
    let where_ = r.store_token(&String::from_utf8_lossy(&raw)).map_err(|err| {
        errorf(
            2,
            "Token not stored",
            &err.to_string(),
            "Copy the token from 1Password > Developer > Service Accounts and paste it on one line.",
        )
    })?;
    c.printf(format_args!(
        "Service-account token kept in {where_}. It never appears on argv, in the store or in events.\n"
    ))?;
    c.println("Next: pulse cred set \"op://VAULT/ITEM/private key?ssh-format=openssh\"")?;
    Ok(())
}

fn not_a_reference() -> anyhow::Error {
    refusal(&pulse_cred_setup::not_a_reference())
}

fn refusal(r: &Refusal) -> anyhow::Error {
    errorf(r.code, &r.what, &r.why, &r.fix).into()
}

fn run_set(
    c: &mut Context,
    s: &pulse_store::Store,
    r: &pulse_cred::Resolver,
    actor: &str,
    text: &str,
    no_probe: bool,
    d: &Deps,
) -> Result<()> {
    let probe = Arc::clone(&d.probe);
    let res = pulse_cred_setup::apply(
        s,
        r,
        actor,
        text,
        !no_probe,
        SetupDeps {
            probe: Some(Arc::new(move |root, remote, ssh| probe(root, remote, ssh))),
            keygen: None,
        },
    );
    match res {
        Ok(res) => {
            c.printf(format_args!(
                "Key read from {}: {}\n",
                res.source, res.fingerprint
            ))?;
            c.printf(format_args!("Public key: {}\n", res.public_key))?;
            if res.probed {
                let mut probed = 0;
                for p in &res.probes {
                    if p.skipped.is_empty() {
                        probed += 1;
                    }
                    print_probe(c, p)?;
                }
                if probed == 0 {
                    c.println("  no project with an upstream to probe; saved unprobed")?;
                }
            }
            c.printf(format_args!("push.credential={}\n", res.reference))?;
            if res.landing_push != "main" && res.landing_push != "pr" {
                c.println(
                    "The daemon pushes only with landing.push=main (the default branch) or pr (a pull request from pulse/landed): pulse settings set landing.push main",
                )?;
            }
            Ok(())
        }
        Err(err) => {
            if let Some(refusal_value) = err.downcast_ref::<Refusal>() {
                if !refusal_value.public_key.is_empty() {
                    c.printf(format_args!("Key read: {}\n", refusal_value.fingerprint))?;
                    c.printf(format_args!("Public key: {}\n", refusal_value.public_key))?;
                    for p in &refusal_value.probes {
                        print_probe(c, p)?;
                    }
                }
                return Err(refusal(refusal_value));
            }
            Err(err)
        }
    }
}

fn print_probe(c: &mut Context, p: &Probe) -> Result<()> {
    if !p.skipped.is_empty() {
        c.printf(format_args!("  {}: {}; not probed\n", p.project, p.skipped))?;
    } else if p.ok {
        c.printf(format_args!(
            "  {}: {} answers this key\n",
            p.project, p.remote
        ))?;
    } else {
        c.printf(format_args!(
            "  {}: {} refused this key\n",
            p.project, p.remote
        ))?;
    }
    Ok(())
}

fn run_status(
    c: &mut Context,
    s: &pulse_store::Store,
    r: &pulse_cred::Resolver,
    json_out: bool,
) -> Result<()> {
    let rep = pulse_cred_setup::status(s, r);
    if json_out {
        return write_json(&mut c.out, &rep);
    }
    if rep.reference.is_empty() {
        c.println(
            "push.credential: not set - the daemon pushes with the git credentials this machine has (ADR-037)",
        )?;
    } else {
        c.printf(format_args!("push.credential: {}", rep.reference))?;
        if !rep.fingerprint.is_empty() {
            c.printf(format_args!(" ({})", rep.fingerprint))?;
        }
        c.println("")?;
        if !rep.set_at.is_empty() {
            c.printf(format_args!("set {} by {}\n", rep.set_at, rep.set_by))?;
        }
    }
    c.printf(format_args!("landing.push: {}\n", rep.landing_push))?;
    c.printf(format_args!(
        "1Password service-account token: {}\n",
        rep.token
    ))?;
    Ok(())
}

fn run_clear(
    c: &mut Context,
    s: &pulse_store::Store,
    r: &pulse_cred::Resolver,
    actor: &str,
    token: bool,
) -> Result<()> {
    let out = pulse_cred_setup::clear(s, r, actor, token)?;
    if !out.was.is_empty() {
        c.println(
            "push.credential cleared - the daemon pushes with the git credentials this machine has again (ADR-037)",
        )?;
    } else {
        c.println("push.credential was not set")?;
    }
    if token {
        if out.token_removed.is_empty() {
            c.println("no stored service-account token to remove")?;
        } else {
            c.printf(format_args!(
                "service-account token removed from {}\n",
                out.token_removed.join(" and ")
            ))?;
        }
        c.println("Revoke the service account in 1Password too: Developer > Service Accounts.")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_cli::Buffer;
    use pulse_store::{Options, Store};
    use std::path::Path;
    use std::sync::Mutex;
    use tempfile::tempdir;

    const TEST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n";
    const TEST_TOKEN: &str = "ops_abcdefghijklmnopqrstuvwxyz0123456789";

    #[derive(Clone, Default)]
    struct FakeTools {
        calls: Arc<Mutex<Vec<String>>>,
        key_out: Arc<Mutex<String>>,
        token_in: Arc<Mutex<String>>,
    }

    impl FakeTools {
        fn with_key(key: &str) -> Self {
            let f = Self::default();
            *f.key_out.lock().unwrap() = key.into();
            f
        }

        fn set_token(&self, token: &str) {
            *self.token_in.lock().unwrap() = token.into();
        }

        fn token(&self) -> String {
            self.token_in.lock().unwrap().clone()
        }

        fn runner(&self) -> pulse_cred::Runner {
            let calls = Arc::clone(&self.calls);
            let key_out = Arc::clone(&self.key_out);
            let token_in = Arc::clone(&self.token_in);
            Arc::new(move |name, args, stdin, env| {
                calls
                    .lock()
                    .unwrap()
                    .push(format!("{name} {}", args.join(" ")));
                match name {
                    "security" => {
                        let joined = args.join(" ");
                        if joined == "-i" {
                            if !stdin.contains("add-generic-password") {
                                return Err(pulse_cred::RunError::new("bad stdin"));
                            }
                            let token = stdin
                                .rsplit_once("-w '")
                                .and_then(|(_, rest)| rest.split_once("'"))
                                .map(|(token, _)| token)
                                .unwrap_or_default();
                            *token_in.lock().unwrap() = token.into();
                            return Ok((String::new(), String::new()));
                        }
                        if joined == "find-generic-password -s pulse -a OP_SERVICE_ACCOUNT_TOKEN -w"
                        {
                            let token = token_in.lock().unwrap().clone();
                            if token.is_empty() {
                                return Err(pulse_cred::RunError::new(
                                    "The specified item could not be found in the keychain.",
                                ));
                            }
                            return Ok((format!("{token}\n"), String::new()));
                        }
                        if joined == "delete-generic-password -s pulse -a OP_SERVICE_ACCOUNT_TOKEN"
                        {
                            if token_in.lock().unwrap().is_empty() {
                                return Err(pulse_cred::RunError::new("not found"));
                            }
                            token_in.lock().unwrap().clear();
                            return Ok((String::new(), String::new()));
                        }
                    }
                    "op" => {
                        let ok = env.is_some_and(|env| {
                            env.iter()
                                .any(|kv| kv == &format!("OP_SERVICE_ACCOUNT_TOKEN={TEST_TOKEN}"))
                        });
                        if ok {
                            return Ok((key_out.lock().unwrap().clone(), String::new()));
                        }
                        return Err(pulse_cred::RunError::new("[ERROR] no token"));
                    }
                    "ssh-keygen" => {
                        if args.first().is_some_and(|a| a == "-y") {
                            return Ok((
                                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakePublicKey pulse-push\n"
                                    .into(),
                                String::new(),
                            ));
                        }
                        return Ok((
                            "256 SHA256:FakeFingerprint0123456789 pulse-push (ED25519)\n".into(),
                            String::new(),
                        ));
                    }
                    _ => {}
                }
                Err(pulse_cred::RunError::new("not faked"))
            })
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ProbeCall {
        root: String,
        remote: String,
        ssh: String,
    }

    struct Installed {
        old: Deps,
        probes: Arc<Mutex<Vec<ProbeCall>>>,
        stdin: Arc<Mutex<String>>,
    }

    impl Drop for Installed {
        fn drop(&mut self) {
            set_deps(self.old.clone());
        }
    }

    fn setup_store() -> (pulse_testutil::EnvGuard, tempfile::TempDir, Store) {
        let guard = pulse_testutil::env_lock();
        for key in [
            "PULSE_JOB",
            "PULSE_AGENT",
            "PULSE_PROJECT",
            "PULSE_SESSION",
            "OP_SERVICE_ACCOUNT_TOKEN",
        ] {
            unsafe { std::env::set_var(key, "") };
        }
        let home = tempdir().unwrap();
        unsafe { std::env::set_var("PULSE_HOME", home.path().display().to_string()) };
        let store = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-09-15T06:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        (guard, home, store)
    }

    fn install(tools: &FakeTools, probe_err: Option<String>) -> Installed {
        let probes = Arc::new(Mutex::new(Vec::new()));
        let stdin = Arc::new(Mutex::new(format!("{TEST_TOKEN}\n")));
        let probes_sink = Arc::clone(&probes);
        let stdin_src = Arc::clone(&stdin);
        let runner_tools = tools.clone();
        let old = set_deps(Deps {
            resolver: Arc::new(move |home| pulse_cred::Resolver {
                home: home.into(),
                run: Some(runner_tools.runner()),
                goos: "darwin".into(),
                getenv: Some(Arc::new(|_| String::new())),
                ..pulse_cred::Resolver::default()
            }),
            stdin: Arc::new(move || {
                Box::new(std::io::Cursor::new(stdin_src.lock().unwrap().clone()))
                    as Box<dyn Read + Send>
            }),
            stdin_tty: Arc::new(|| false),
            probe: Arc::new(move |root, remote, ssh| {
                probes_sink.lock().unwrap().push(ProbeCall {
                    root: root.into(),
                    remote: remote.into(),
                    ssh: ssh.into(),
                });
                let path = ssh
                    .trim_start_matches(
                        "ssh -oBatchMode=yes -oIdentitiesOnly=yes -oIdentityAgent=none -i ",
                    )
                    .trim_matches('\'');
                assert!(
                    std::fs::metadata(path).is_ok(),
                    "key not on disk during the probe: {path}"
                );
                match &probe_err {
                    Some(e) => Err(anyhow::anyhow!(e.clone())),
                    None => Ok(()),
                }
            }),
        });
        Installed { old, probes, stdin }
    }

    fn cred_command(c: &mut Context, args: &[&str]) -> (String, Result<()>) {
        register();
        let cmd = pulse_cli::lookup("cred").expect("cred command registered");
        assert!(
            cmd.no_store && cmd.raw,
            "cred must be raw and open its own store"
        );
        let out = Buffer::new();
        c.out = Box::new(out.clone());
        c.err = Box::new(io::sink());
        let argv: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        let err = (cmd.run)(c, &FlagSet::new("cred"), &argv);
        (out.string(), err)
    }

    fn context(home: &str) -> Context {
        Context {
            store: None,
            home: home.into(),
            out: Box::new(io::sink()),
            err: Box::new(io::sink()),
            json: false,
            quiet: false,
            color: false,
            agent: String::new(),
            project: String::new(),
            session: String::new(),
            now: String::new(),
            args: Vec::new(),
        }
    }

    fn no_secret(text: &str) {
        for leak in ["b3BlbnNzaC", TEST_TOKEN, "BEGIN OPENSSH"] {
            assert!(!text.contains(leak), "secret in output: {text:?}");
        }
    }

    fn push_project(s: &Store, slug: &str) -> String {
        let root = Path::new(&s.home).join(format!("project-{slug}"));
        let bare = Path::new(&s.home).join(format!("{slug}.git"));
        let root_s = root.display().to_string();
        let bare_s = bare.display().to_string();
        for args in [
            vec!["init", "-q", "--bare", bare_s.as_str()],
            vec!["init", "-q", "-b", "main", root_s.as_str()],
            vec![
                "-C",
                root_s.as_str(),
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
            vec![
                "-C",
                root_s.as_str(),
                "remote",
                "add",
                "origin",
                bare_s.as_str(),
            ],
            vec!["-C", root_s.as_str(), "push", "-q", "-u", "origin", "main"],
        ] {
            pulse_testutil::git(&args).unwrap();
        }
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug,name,path,default_branch,added_at) VALUES (?,?,?,?,?)",
                rusqlite::params![slug, slug, root_s, "main", s.now()],
            )
            .unwrap();
        root.display().to_string()
    }

    #[test]
    fn cred_token_set_status_clear() {
        let _serial = crate::test_helpers::serial();
        let (_guard, _home, s) = setup_store();
        let root = push_project(&s, "demo");
        let tools = FakeTools::with_key(TEST_KEY);
        let installed = install(&tools, None);
        let mut ctx = context(&s.home);

        let (out, err) = cred_command(&mut ctx, &["status"]);
        assert!(err.is_ok(), "status before: {err:?} {out:?}");
        assert!(out.contains("push.credential: not set"));
        assert!(out.contains("token: none"));

        let (out, err) = cred_command(&mut ctx, &["token"]);
        assert!(err.is_ok(), "token: {err:?}");
        no_secret(&out);
        assert!(out.contains("kept in the keychain"));
        assert_eq!(tools.token(), TEST_TOKEN);
        for call in tools.calls.lock().unwrap().iter() {
            assert!(!call.contains(TEST_TOKEN), "token on argv: {call}");
        }

        let reference = "op://PULSE/pulse push key/private key?ssh-format=openssh";
        let (out, err) = cred_command(&mut ctx, &["set", reference]);
        assert!(err.is_ok(), "set: {err:?}\n{out}");
        no_secret(&out);
        for want in [
            "Key read from 1Password (token from the keychain): SHA256:FakeFingerprint0123456789",
            "Public key: ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakePublicKey pulse-push",
            "demo: origin answers this key",
            &format!("push.credential={reference}"),
            "landing.push main",
        ] {
            assert!(out.contains(want), "set output lacks {want:?}:\n{out}");
        }
        let probes = installed.probes.lock().unwrap().clone();
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].root, root);
        assert_eq!(probes[0].remote, "origin");
        assert_eq!(s.setting("push.credential", ""), reference);
        let detail: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT detail FROM events WHERE action = 'cred.set'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        no_secret(&detail);
        assert!(detail.contains("SHA256:FakeFingerprint0123456789"));
        let tmp = Path::new(&s.home).join("tmp");
        if tmp.exists() {
            for e in std::fs::read_dir(tmp).unwrap() {
                let e = e.unwrap();
                assert!(
                    !e.file_name().to_string_lossy().starts_with("push-"),
                    "key directory left after set"
                );
            }
        }

        let (out, err) = cred_command(&mut ctx, &["status", "--json"]);
        assert!(err.is_ok());
        let rep: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(rep["reference"], reference);
        assert_eq!(rep["scheme"], "op");
        assert_eq!(rep["fingerprint"], "SHA256:FakeFingerprint0123456789");
        assert_eq!(rep["token"], "the keychain");
        assert_eq!(rep["set_by"], "human");
        assert_eq!(rep["landing_push"], "off");

        let (out, err) = cred_command(&mut ctx, &["clear", "--token"]);
        assert!(err.is_ok());
        assert!(out.contains("push.credential cleared"));
        assert!(out.contains("removed from the keychain item"));
        assert!(out.contains("Revoke"));
        assert_eq!(s.setting("push.credential", ""), "");
        assert_eq!(tools.token(), "");
        let (out, _) = cred_command(&mut ctx, &["clear"]);
        assert!(out.contains("was not set"));
    }

    #[test]
    fn cred_set_refuses_keys_bad_refs_and_failed_probes() {
        let _serial = crate::test_helpers::serial();
        let (_guard, _home, s) = setup_store();
        push_project(&s, "demo");
        let mut ctx = context(&s.home);

        let tools = FakeTools::with_key(TEST_KEY);
        tools.set_token(TEST_TOKEN);
        let mut installed = install(&tools, None);
        for bad in [TEST_KEY, "ghp_abcdefghijklmnopqrstuvwxyz0123456789"] {
            let (out, err) = cred_command(&mut ctx, &["set", bad]);
            let err = err.expect_err("set(secret) should fail");
            assert!(err.to_string().contains("the key, not a reference"));
            no_secret(&(out + &err.to_string()));
        }
        let (_, err) = cred_command(&mut ctx, &["set", "vault://x"]);
        assert!(
            err.expect_err("bad ref")
                .to_string()
                .contains("Not a push credential reference")
        );
        let (_, err) = cred_command(&mut ctx, &["set"]);
        assert!(
            err.expect_err("no ref")
                .to_string()
                .contains("Wrong cred arguments")
        );

        drop(installed);
        let tools = FakeTools::with_key(TEST_KEY);
        installed = install(&tools, None);
        let (_, err) = cred_command(&mut ctx, &["set", "op://V/I/F"]);
        let err = err.expect_err("no token");
        assert!(err.to_string().contains("pulse cred token"));
        assert!(err.to_string().contains("Nothing was saved"));
        assert_eq!(s.setting("push.credential", ""), "");

        drop(installed);
        let tools = FakeTools::with_key(TEST_KEY);
        tools.set_token(TEST_TOKEN);
        installed = install(
            &tools,
            Some("ERROR: The key you are authenticating with has been marked as read only.".into()),
        );
        let (out, err) = cred_command(&mut ctx, &["set", "op://V/I/F?ssh-format=openssh"]);
        let err = err.expect_err("probe fail");
        assert!(
            err.to_string()
                .contains("does not open demo's remote origin")
        );
        assert!(err.to_string().contains("read only"));
        assert!(err.to_string().contains("deploy key"));
        assert!(out.contains("Public key: ssh-ed25519"));
        no_secret(&out);
        assert_eq!(s.setting("push.credential", ""), "");

        drop(installed);
        let tools = FakeTools::with_key(TEST_KEY);
        tools.set_token(TEST_TOKEN);
        installed = install(&tools, Some("must not run".into()));
        let (_, err) = cred_command(
            &mut ctx,
            &["set", "op://V/I/F?ssh-format=openssh", "--no-probe"],
        );
        assert!(err.is_ok(), "no-probe: {err:?}");
        assert_eq!(installed.probes.lock().unwrap().len(), 0);
        assert_eq!(
            s.setting("push.credential", ""),
            "op://V/I/F?ssh-format=openssh"
        );
    }

    #[test]
    fn cred_is_the_owners() {
        let _serial = crate::test_helpers::serial();
        let (_guard, _home, s) = setup_store();
        let tools = FakeTools::with_key(TEST_KEY);
        tools.set_token(TEST_TOKEN);
        let _installed = install(&tools, None);
        for agent in ["granite", "human:dashboard"] {
            for args in [vec!["set", "op://V/I/F"], vec!["token"], vec!["clear"]] {
                let mut c = context(&s.home);
                c.agent = agent.into();
                let (_, err) = cred_command(&mut c, &args);
                if agent == "granite" {
                    assert!(
                        err.expect_err("agent should be refused")
                            .to_string()
                            .contains("Only the owner")
                    );
                } else if let Err(err) = err {
                    assert!(!err.to_string().contains("Only the owner"));
                }
            }
        }
        let mut c = context(&s.home);
        c.agent = "granite".into();
        assert!(cred_command(&mut c, &["status"]).1.is_ok());

        unsafe { std::env::set_var("PULSE_JOB", "job-1") };
        let (_, err) = cred_command(&mut context(&s.home), &["set", "op://V/I/F"]);
        assert!(
            err.expect_err("worker")
                .to_string()
                .contains("Only the owner")
        );
        unsafe { std::env::set_var("PULSE_JOB", "") };
        unsafe { std::env::set_var("PULSE_AGENT", "granite") };
        let mut c = context(&s.home);
        c.agent = "human".into();
        let (_, err) = cred_command(&mut c, &["clear"]);
        assert!(
            err.expect_err("--agent human from agent")
                .to_string()
                .contains("Only the owner")
        );
        unsafe { std::env::set_var("PULSE_AGENT", "") };
    }

    #[test]
    fn cred_token_from_file_and_bad_tokens() {
        let _serial = crate::test_helpers::serial();
        let (_guard, _home, s) = setup_store();
        let tools = FakeTools::default();
        let installed = install(&tools, None);
        let mut ctx = context(&s.home);
        let dir = tempdir().unwrap();
        let path = dir.path().join("token.txt");
        std::fs::write(&path, format!("{TEST_TOKEN}\n")).unwrap();
        let (_, err) = cred_command(&mut ctx, &["token", "--from-file", path.to_str().unwrap()]);
        assert!(err.is_ok());
        assert_eq!(tools.token(), TEST_TOKEN);

        *installed.stdin.lock().unwrap() = "not a token\n".into();
        let (_, err) = cred_command(&mut ctx, &["token"]);
        assert!(
            err.expect_err("bad token")
                .to_string()
                .contains("Token not stored")
        );
        let missing_dir = tempdir().unwrap();
        let missing = missing_dir.path().join("missing");
        let (_, err) = cred_command(
            &mut ctx,
            &["token", "--from-file", missing.to_str().unwrap()],
        );
        assert!(
            err.expect_err("missing file")
                .to_string()
                .contains("Could not read")
        );
        let (_, err) = cred_command(&mut ctx, &["frobnicate"]);
        assert!(
            err.expect_err("unknown")
                .to_string()
                .contains("Unknown cred command")
        );
    }
}
