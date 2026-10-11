use super::*;
use rusqlite::params;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{LazyLock, Mutex};

static LIST_TEST_CALL: LazyLock<Mutex<(Vec<String>, String)>> =
    LazyLock::new(|| Mutex::new((Vec::new(), String::new())));
static LIST_TEST_HOOK_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static CACHE_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn stub_list_tests(_: &str, pkgs: &[String], regex: &str) -> std::io::Result<Vec<String>> {
    *LIST_TEST_CALL.lock().expect("list test call poisoned") = (pkgs.to_vec(), regex.to_string());
    if regex.starts_with("TestMissing") {
        return Ok(Vec::new());
    }
    if regex == "TestBroken" {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "broken"));
    }
    Ok(vec!["TestCompletionLanding".into()])
}

fn empty_list_tests(_: &str, _: &[String], _: &str) -> std::io::Result<Vec<String>> {
    Ok(Vec::new())
}

fn git_in(dir: impl AsRef<std::path::Path>, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir.as_ref()).args(args);
    cmd.env_clear();
    for kv in pulse_gitenv::env() {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd.env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let out = cmd.output().unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

const FIXTURE_CONSTITUTION_BODY: &str = r#"# Constitution (test fixture)

## Hard stops

Never cross a hard stop.

## Immutable paths

- `docs/evolution/constitution.md`

## Editable scope by stage

| File class | Paths | Stage | Autonomy | Clean cycles required | Clean cycles so far | Incidents |
| --- | --- | --- | --- | --- | --- | --- |
| product code | `src/` | 4 | human | 5 | 0 | 0 |
| memory | `docs/memory/` | 1 | human | 5 | 0 | 0 |

## Vote rules

| Rule | Value |
| --- | --- |
| Quorum to enter the queue | 2 distinct agent identities |
| Votes per identity per suggestion | 1 (the latest wins) |
| Amendment voter | one top-level session |
| Informed vote | text_hash plus a Read: line |
| Amendment quorum | 1 distinct voter |
| Amendment supermajority | 2/3 of counted votes |
| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter |

## Amendments

Per docs/workflows/amend.md.
"#;

const FIXTURE_CONSTITUTION_FILE: &str = concat!(
    "---\nloop: off\nlimits: none\n---\n\n",
    r#"# Constitution (test fixture)

## Hard stops

Never cross a hard stop.

## Immutable paths

- `docs/evolution/constitution.md`

## Editable scope by stage

| File class | Paths | Stage | Autonomy | Clean cycles required | Clean cycles so far | Incidents |
| --- | --- | --- | --- | --- | --- | --- |
| product code | `src/` | 4 | human | 5 | 0 | 0 |
| memory | `docs/memory/` | 1 | human | 5 | 0 | 0 |

## Vote rules

| Rule | Value |
| --- | --- |
| Quorum to enter the queue | 2 distinct agent identities |
| Votes per identity per suggestion | 1 (the latest wins) |
| Amendment voter | one top-level session |
| Informed vote | text_hash plus a Read: line |
| Amendment quorum | 1 distinct voter |
| Amendment supermajority | 2/3 of counted votes |
| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter |

## Amendments

Per docs/workflows/amend.md.
"#
);

const FIXTURE_AMENDMENT_BODY: &str = "## Current text\n\n```\n| Votes per identity per suggestion | 1 (the latest wins) |\n```\n\n## Proposed text\n\n```\n| Votes per identity per suggestion | 1 (the latest wins; a fleet is one voter) |\n```\n\n## Why\n\nFleets voted twice.\n\n## What it does not change\n\nQuorum and supermajority.\n";

fn amendment(current: &str, proposed: &str) -> String {
    format!(
        "## Current text\n\n```\n{current}\n```\n\n## Proposed text\n\n```\n{proposed}\n```\n\n## Why\n\nBecause.\n"
    )
}

const FIXTURE_NOW: &str = "2026-09-10T12:00:00Z";

pub(crate) mod test_fixture {
    use super::*;

    #[derive(Debug)]
    pub(crate) struct Fixture {
        pub(crate) home: tempfile::TempDir,
        pub(crate) repo: PathBuf,
        pub(crate) st: pulse_store::Store,
        pub(crate) ids: BTreeMap<String, String>,
        pub(crate) hash: String,
    }

    #[derive(Debug, Clone, Default)]
    pub(crate) struct FPost {
        pub(crate) kind: String,
        pub(crate) area: String,
        pub(crate) thread: String,
        pub(crate) agent: String,
        pub(crate) date: String,
        pub(crate) summary: String,
        pub(crate) session: String,
        pub(crate) body: String,
        pub(crate) on: String,
        pub(crate) vote: String,
        pub(crate) text_hash: String,
        pub(crate) expires: String,
        pub(crate) files: String,
        pub(crate) resolution: String,
        pub(crate) branch: String,
        pub(crate) project: String,
        pub(crate) extra: String,
    }

    impl FPost {
        pub(crate) fn new(
            kind: &str,
            area: &str,
            thread: &str,
            agent: &str,
            date: &str,
            summary: &str,
            session: &str,
        ) -> Self {
            Self {
                kind: kind.into(),
                area: area.into(),
                thread: thread.into(),
                agent: agent.into(),
                date: date.into(),
                summary: summary.into(),
                session: session.into(),
                ..Self::default()
            }
        }
    }

    pub(crate) fn fixture_post_id(date: &str, agent: &str) -> String {
        let d = date.replace(['-', ':'], "");
        format!("{}-{}", d, agent.replace('/', "_"))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_eligible_suggestion(
        f: &mut Fixture,
        key: &str,
        agent: &str,
        thread: &str,
        summary: &str,
        body: &str,
        extra: &str,
        votes: &[&str],
    ) -> String {
        let mut p = FPost::new(
            "suggestion",
            "evolution",
            thread,
            agent,
            &format!("2026-09-01T0{key}:00:00Z"),
            summary,
            &format!("s-{key}"),
        );
        p.body = body.into();
        p.extra = extra.into();
        let id = f.post(p);
        for (i, vote) in votes.iter().enumerate() {
            let voter = format!("runner-v{key}{}/model", (b'a' + i as u8) as char);
            let mut p = FPost::new(
                "vote",
                "evolution",
                thread,
                &voter,
                &format!("2026-09-02T0{key}:0{}:00Z", (b'1' + i as u8) as char),
                vote,
                &format!("s-{voter}"),
            );
            p.on = id.clone();
            p.vote = (*vote).into();
            f.post(p);
        }
        id
    }

    pub(crate) fn new_fixture(with_record: bool) -> Fixture {
        let home = tempfile::tempdir().expect("temp home");
        let repo = home.path().join("fixture-repo");
        fs::create_dir_all(repo.join("docs/evolution")).expect("create fixture repo");
        // The product tree the fixture cards' `File:` lines name.
        fs::create_dir_all(repo.join("src")).expect("create fixture src");
        fs::write(
            repo.join("docs/evolution/constitution.md"),
            FIXTURE_CONSTITUTION_FILE,
        )
        .expect("write constitution");
        pulse_store::Store::create(home.path().join("pulse.sqlite")).expect("create store");
        let st = pulse_store::Store::open(pulse_store::Options {
            home: home.path().display().to_string(),
            create: false,
            read_only: false,
            skip_records: false,
            now: FIXTURE_NOW.into(),
        })
        .expect("open store");
        let f = Fixture {
            home,
            repo,
            st,
            ids: BTreeMap::new(),
            hash: String::new(),
        };
        {
            let conn = f.st.conn().expect("conn");
            conn.execute(
                "INSERT INTO projects (slug, name, path, brain, default_branch, added_at, added_by) VALUES ('fixture', 'Fixture', ?, 1, 'main', ?, 'test')",
                params![f.repo.display().to_string(), FIXTURE_NOW],
            )
            .expect("insert project");
        }
        if with_record {
            let rec = pulse_store::Record {
                id: "EVO-constitution".into(),
                project: "fixture".into(),
                kind: "evolution".into(),
                slug: "constitution".into(),
                title: "Constitution (test fixture)".into(),
                status: "active".into(),
                date: "2026-09-01".into(),
                path: "docs/evolution/constitution.md".into(),
                ..Default::default()
            };
            let mut front = BTreeMap::new();
            front.insert("loop".into(), Value::String("off".into()));
            front.insert("limits".into(), Value::String("none".into()));
            f.st.put_record(rec, front, FIXTURE_CONSTITUTION_BODY, "test", "fixture")
                .expect("put constitution record");
        }
        let mut f = f;
        f.populate();
        f
    }

    impl Fixture {
        pub(crate) fn post(&mut self, mut p: FPost) -> String {
            let id = fixture_post_id(&p.date, &p.agent);
            let (runner, model) = p.agent.split_once('/').unwrap_or((&p.agent, ""));
            if p.body.is_empty() {
                p.body = p.summary.clone();
            }
            let body = format!("\n## {}\n\n{}\n", p.kind, p.body);
            if p.branch.is_empty() && p.kind != "finding" {
                p.branch = "main".into();
            }
            let read = if body.lines().any(|line| {
                let trimmed = line.trim_start_matches([' ', '\t']);
                trimmed.starts_with("Read:")
                    && trimmed
                        .strip_prefix("Read:")
                        .is_some_and(|rest| !rest.trim().is_empty())
            }) {
                1
            } else {
                0
            };
            if p.project.is_empty() {
                p.project = "fixture".into();
            }
            if p.extra.is_empty() {
                p.extra = "{}".into();
            }
            let conn = self.st.conn().expect("conn");
            conn.execute(
                "INSERT INTO posts (id, project, area, thread, kind, agent, runner, model, session, date, branch, expires,
                on_post, vote, text_hash, has_read_line, resolution, summary, body, links, files, extra, origin, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '[]', ?, ?, 'store', ?)",
                params![
                    id,
                    p.project,
                    p.area,
                    p.thread,
                    p.kind,
                    p.agent,
                    runner,
                    model,
                    p.session,
                    p.date,
                    p.branch,
                    p.expires,
                    p.on,
                    p.vote,
                    p.text_hash,
                    read,
                    p.resolution,
                    p.summary,
                    body,
                    p.files,
                    p.extra,
                    p.date
                ],
            )
            .unwrap_or_else(|e| panic!("insert {id}: {e}"));
            conn.execute(
                "INSERT OR IGNORE INTO agents (name, display, kind, status, created_at, last_active_at) VALUES (?, ?, 'agent', 'active', ?, ?)",
                params![p.agent, p.agent, p.date, p.date],
            )
            .expect("insert agent");
            id
        }

        fn populate(&mut self) {
            let (a, b, c, d) = (
                "runner-a/model-a",
                "runner-b/model-b",
                "runner-c/model-c",
                "runner-d/model-d",
            );
            let s1 = self.post(FPost::new(
                "suggestion",
                "evolution",
                "speedup",
                a,
                "2026-09-01T09:00:00Z",
                "Cache the parser table",
                "s-a",
            ));
            let mut p = FPost::new(
                "vote",
                "evolution",
                "speedup",
                b,
                "2026-09-02T09:00:00Z",
                "up: fewer retries",
                "s-b",
            );
            p.on = s1.clone();
            p.vote = "up".into();
            self.post(p);
            let mut p = FPost::new(
                "vote",
                "evolution",
                "speedup",
                c,
                "2026-09-02T10:00:00Z",
                "up at first",
                "s-c",
            );
            p.on = s1.clone();
            p.vote = "up".into();
            self.post(p);
            let mut p = FPost::new(
                "vote",
                "evolution",
                "speedup",
                c,
                "2026-09-03T10:00:00Z",
                "down after reading the archive",
                "s-c",
            );
            p.on = s1.clone();
            p.vote = "down".into();
            self.post(p);
            let mut p = FPost::new(
                "vote",
                "evolution",
                "speedup",
                d,
                "2026-09-03T11:00:00Z",
                "up",
                "s-d",
            );
            p.on = s1.clone();
            p.vote = "up".into();
            self.post(p);
            let mut p = FPost::new(
                "vote",
                "evolution",
                "speedup",
                a,
                "2026-09-03T12:00:00Z",
                "my own",
                "s-a",
            );
            p.on = s1.clone();
            p.vote = "up".into();
            self.post(p);
            self.ids.insert("eligible".into(), s1);

            let s3 = self.post(FPost::new(
                "suggestion",
                "evolution",
                "fresh",
                b,
                "2026-09-08T09:00:00Z",
                "Split the long fixture",
                "s-b",
            ));
            let mut p = FPost::new(
                "vote",
                "evolution",
                "fresh",
                c,
                "2026-09-09T09:00:00Z",
                "up",
                "s-c",
            );
            p.on = s3.clone();
            p.vote = "up".into();
            self.post(p);
            self.ids.insert("needs_votes".into(), s3);
            let expired = self.post(FPost::new(
                "suggestion",
                "evolution",
                "lonely",
                d,
                "2026-08-01T09:00:00Z",
                "Rename the archive columns",
                "s-d",
            ));
            self.ids.insert("expired".into(), expired);

            let mut p = FPost::new(
                "claim",
                "governance",
                "active-claim",
                a,
                "2026-09-09T08:00:00Z",
                "Working on the dashboard docs",
                "s-a",
            );
            p.expires = "2026-09-12T08:00:00Z".into();
            p.files = "[docs/workflows/dashboard.md]".into();
            let id = self.post(p);
            self.ids.insert("claim_active".into(), id);
            let mut p = FPost::new(
                "claim",
                "governance",
                "old-claim",
                b,
                "2026-08-20T08:00:00Z",
                "Forgotten claim",
                "s-b",
            );
            p.expires = "2026-08-22T08:00:00Z".into();
            p.files = "[README.md]".into();
            let id = self.post(p);
            self.ids.insert("claim_expired".into(), id);

            let q = self.post(FPost::new(
                "question",
                "governance",
                "closed",
                c,
                "2026-09-05T08:00:00Z",
                "Where do votes live?",
                "s-c",
            ));
            let mut p = FPost::new(
                "answer",
                "governance",
                "closed",
                d,
                "2026-09-05T09:00:00Z",
                "In docs/board, one file per vote",
                "s-d",
            );
            p.on = q;
            p.resolution = "answered".into();
            self.post(p);
            let mut p = FPost::new(
                "finding",
                "governance",
                "payload",
                "runner-x/model-x",
                "2026-09-06T08:00:00Z",
                "<script>alert(1)</script>",
                "s-x",
            );
            p.body = "Body with payload: <script>alert(1)</script>\n\nRead: this is a finding, not a vote.".into();
            self.post(p);
            self.post(FPost::new(
                "question",
                "governance",
                "questions/old",
                a,
                "2026-07-01T08:00:00Z",
                "Is the archive column order fixed?",
                "s-a",
            ));
            let pr = self.post(FPost::new(
                "proposal",
                "governance",
                "rename",
                b,
                "2026-09-04T08:00:00Z",
                "Rename the board to the commons",
                "s-b",
            ));
            let mut p = FPost::new(
                "answer",
                "governance",
                "rename",
                c,
                "2026-09-04T09:00:00Z",
                "agreed",
                "s-c",
            );
            p.on = pr;
            p.resolution = "agreed".into();
            self.post(p);

            let proposer = "runner-p/model-p";
            let mut p = FPost::new(
                "amendment",
                "governance",
                "amend",
                proposer,
                "2026-09-07T08:00:00Z",
                "Count a fleet as one voter",
                "s-p",
            );
            p.body = FIXTURE_AMENDMENT_BODY.into();
            let aid = self.post(p);
            let good = crate::text::text_hash(FIXTURE_AMENDMENT_BODY);
            self.hash = good.clone();
            self.ids.insert("amendment".into(), aid.clone());

            let mut p = FPost::new(
                "vote",
                "governance",
                "amend",
                "runner-1/model-1",
                "2026-09-08T08:00:00Z",
                "up",
                "s-1",
            );
            p.on = aid.clone();
            p.vote = "up".into();
            p.text_hash = good.clone();
            p.body = "up\n\nRead: replaces the votes-per-identity row so a fleet counts once; quorum untouched.".into();
            self.post(p);
            let mut p = FPost::new(
                "vote",
                "governance",
                "amend",
                "runner-2/model-2",
                "2026-09-08T09:00:00Z",
                "up",
                "s-2",
            );
            p.on = aid.clone();
            p.vote = "up".into();
            p.text_hash = "deadbeef".into();
            p.body = "up\n\nRead: same change.".into();
            self.post(p);
            let mut p = FPost::new(
                "vote",
                "governance",
                "amend",
                "runner-3/model-3",
                "2026-09-08T10:00:00Z",
                "up",
                "s-3",
            );
            p.on = aid.clone();
            p.vote = "up".into();
            p.text_hash = good.clone();
            p.body = "up, no Read line here".into();
            self.post(p);
            let mut p = FPost::new(
                "vote",
                "governance",
                "amend",
                proposer,
                "2026-09-08T11:00:00Z",
                "up",
                "s-p",
            );
            p.on = aid;
            p.vote = "up".into();
            p.text_hash = good;
            p.body = "up\n\nRead: my own proposal.".into();
            self.post(p);

            let blocked = "## Current text\n\n```\nNever cross a hard stop.\n```\n\n## Proposed text\n\n```\nCross hard stops when the human is asleep.\n```\n\n## Why\n\nSpeed.\n";
            let mut p = FPost::new(
                "amendment",
                "governance",
                "amend-hard",
                proposer,
                "2026-09-07T09:00:00Z",
                "Loosen hard stops",
                "s-p",
            );
            p.body = blocked.into();
            let id = self.post(p);
            self.ids.insert("blocked".into(), id);
        }
    }

    pub(crate) fn run_cli(
        st: &pulse_store::Store,
        name: &str,
        args: &[&str],
    ) -> anyhow::Result<String> {
        crate::register();
        let cmd = pulse_cli::lookup(name).unwrap_or_else(|| panic!("no command {name:?}"));
        let out = pulse_cli::Buffer::new();
        let mut c = pulse_cli::Context {
            store: Some(st.clone()),
            home: st.home.clone(),
            out: Box::new(out.clone()),
            err: Box::new(io::sink()),
            json: false,
            quiet: false,
            color: false,
            agent: String::new(),
            project: String::new(),
            session: String::new(),
            now: String::new(),
            args: Vec::new(),
        };
        let mut fs = pulse_cli::FlagSet::new(name);
        if let Some(flags) = cmd.flags {
            flags(&mut fs);
        }
        if fs.lookup("project").is_none() {
            fs.string_var("project", "", "");
        }
        if fs.lookup("agent").is_none() {
            fs.string_var("agent", "", "");
        }
        if fs.lookup("session").is_none() {
            fs.string_var("session", "", "");
        }
        if fs.lookup("now").is_none() {
            fs.string_var("now", "", "");
        }
        if fs.lookup("json").is_none() {
            fs.bool_var("json", false, "");
        }
        let mut positional = Vec::<String>::new();
        let mut rest: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        while !rest.is_empty() {
            if !rest[0].starts_with('-') {
                positional.push(rest.remove(0));
                continue;
            }
            fs.parse(&rest)?;
            if fs.args().len() == rest.len() {
                positional.push(rest.remove(0));
                continue;
            }
            rest = fs.args().to_vec();
        }
        c.project = fs.get_string("project");
        c.agent = fs.get_string("agent");
        c.session = fs.get_string("session");
        c.now = fs.get_string("now");
        c.json = fs.get_bool("json");
        c.args = positional.clone();
        (cmd.run)(&mut c, &fs, &positional)?;
        Ok(out.string())
    }

    pub(crate) fn open_read_only(home: &std::path::Path) -> pulse_store::Store {
        pulse_store::Store::open(pulse_store::Options {
            home: home.display().to_string(),
            create: false,
            read_only: true,
            skip_records: true,
            now: String::new(),
        })
        .expect("open read-only store")
    }
}

#[test]
fn card_class() {
    let _serial = crate::test_helpers::serial();
    let tests = [
        (
            "hub git dir",
            "GIT_DIR=/tmp/x/.git go test ./internal/hub goes FAIL -> ok",
            "Class: product\nHypothesis: passes under GIT_DIR",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "tools check",
            "check:tools/test.sh stays green",
            "Class: product\nFile: tools/test.sh\nVariable: shell check\nHypothesis: stays green",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "vitest case",
            "a new vitest case in __tests__/login.test.ts",
            "Class: product\nFile: __tests__/login.test.ts\nVariable: add a vitest case\nHypothesis: adds a vitest case",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "gofmt",
            "gofmt -l goes 1->0",
            "Class: product\nVariable: gofmt\nHypothesis: gofmt",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "grep helper",
            "grep -rn 'func gitEnv' shrinks by one",
            "Class: product\nVariable: helper removal\nHypothesis: check stays green",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "github workflow",
            "CI workflow stops failing",
            "Class: product\nFile: .github/workflows/test.yml\nVariable: runner setup",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "fixture",
            "fixture data is renamed",
            "Class: product\nFile: internal/testdata/store.json\nVariable: fixture",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "no class no path",
            "Make it better",
            "Variable: one constant\nHypothesis: score improves",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "no class signal only",
            "README gets clearer",
            "Signal: README.md\nVariable: wording\nHypothesis: users understand setup",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "oauth product",
            "German-locale user with a failed OAuth callback sees a localized error message",
            "Class: product\nFile: src/oauth/callback.go\nVariable: callback error copy\nHypothesis: a German-locale user sees the localized failure message",
            CARD_CLASS_PRODUCT,
        ),
        (
            "site data product by file",
            "CTA copy follows status",
            "File: lib/site-data.ts\nVariable: CTA copy per status\nHypothesis: users see the right CTA",
            CARD_CLASS_PRODUCT,
        ),
        (
            "declared operating wins",
            "Internal report wording",
            "Class: operating-layer\nFile: internal/runner/brief.go\nVariable: wording",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "git env plumbing behind a product path",
            "GIT_DIR=/tmp/x go test -run TestEnvSetShowCheck ./internal/runner goes FAIL->ok",
            "Class: product\nFile: internal/runner/util.go\nVariable: isGitCheckout runs git with gitEnv(nil) so an inherited GIT_DIR does not redirect it\nHypothesis: GIT_DIR=/tmp/x go test -run TestEnvSetShowCheck ./internal/runner goes FAIL->ok\nTier: 1",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "testutil path",
            "GIT_INDEX_FILE=/tmp/x go test ./internal/hub goes FAIL->ok",
            "Class: product\nFile: internal/testutil/gitenv.go\nVariable: strip GIT_DIR from os.Environ() inside GitEnv()\nHypothesis: GIT_INDEX_FILE=/tmp/x go test ./internal/hub goes FAIL->ok",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "test speed refactor",
            "TestLauncherRestores drops from 10.5s to <1s",
            "Class: product\nFile: internal/runner/service.go\nVariable: make the launcher's early-exit pause a parameter (default 2s, test passes 0)\nHypothesis: TestLauncherRestoresThePreviousPulse drops from 10.5s to <1s and `go test ./internal/runner` gains ~10s headroom",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "line count refactor",
            "js/tutor.test.mjs stays 33/33 while Session shrinks toward ~180 lines",
            "File: js/demo/tutor.js\nVariable: extract one remaining large method body out of Session\nHypothesis: js/tutor.test.mjs stays 33/33 while Session shrinks from 216 toward ~180 lines, giving margin against future regressions",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "gofmt with go test still ok",
            "gofmt -l ./... goes from 1 file to 0",
            "Class: product\nFile: internal/runner/exec.go\nVariable: gofmt exec.go\nHypothesis: gofmt -l ./... goes from 1 file to 0 with go test ./internal/runner still ok",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "precondition clause",
            "GIT_DIR=/tmp/x/.git go test ./internal/board goes 47 FAIL -> ok",
            "Class: product\nFile: internal/testutil/gitenv.go\nVariable: gitChildEnv everywhere\nHypothesis: with board.go gitChildEnv landed, GIT_DIR=/tmp/x/.git go test ./internal/board goes 47 FAIL -> ok",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "typecheck errors in a test dir",
            "tsc drops from 6 errors to 0",
            "Class: product\nFile: src/backend/Amistio.Api.Tests/Amistio.Api.Tests.csproj\nVariable: bump Mvc.Testing\nHypothesis: dotnet test stays green",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "pulse control plane docs",
            "docs match check:test:run reality",
            "Class: product\nFile: docs/memory/web.md\nVariable: update the full-suite note\nHypothesis: docs match check:test:run reality (94/94), no check changes",
            CARD_CLASS_OPERATING_LAYER,
        ),
        (
            "product outcome then check",
            "fewer runs miss their finish call",
            "Class: product\nFile: cmd/pulse/main.go\nVariable: call finish on every exit path\nHypothesis: fewer runs miss their finish call, go test ./internal/store stays green",
            CARD_CLASS_PRODUCT,
        ),
        (
            "product outcome with typecheck trailer",
            "In development count drops 3 -> 1",
            "Class: product\nFile: lib/site-data.ts\nVariable: set Lamplit Light status to Live\nHypothesis: out/index.html \"In development\" count drops 3 -> 1 with build/typecheck green",
            CARD_CLASS_PRODUCT,
        ),
        (
            "product outcome measured under a check",
            "posts without image drop 8/16 -> 7/16",
            "Class: product\nFile: _posts/2022-03-21-GRPC.md\nVariable: add one relevant screenshot header image\nHypothesis: posts without image drop 8/16 -> 7/16 under check:tools/test.sh",
            CARD_CLASS_PRODUCT,
        ),
        (
            "blog screenshot of a workflow run",
            "image-coverage drops 5/16 -> 4/16 and stays green",
            "Class: product\nFile: _posts/2022-07-19-Azurite-GitHubAction.md\nVariable: add a header screenshot of the Azurite GitHub Action workflow run\nHypothesis: check:tools/test.sh image-coverage drops 5/16 -> 4/16 and stays green",
            CARD_CLASS_PRODUCT,
        ),
        (
            "product change measured by a test assertion",
            "check:test-fast stays green and gains an assertion on the hint text",
            "Class: product\nFile: js/demo/tutor.js\nVariable: use pre.tooLong in the deterministic write feedback to emit a \"too long\" hint line\nHypothesis: check:test-fast stays green and gains an assertion on the hint text",
            CARD_CLASS_PRODUCT,
        ),
        (
            "missing label makes a test pass",
            "go test ./internal/hub -run TestSettingLabelsCoverCatalogue moves FAIL->ok",
            "Class: product\nFile: web/hub/src/lib/setting-labels.ts\nVariable: add a human label for records.auto_adopt\nHypothesis: go test ./internal/hub -run TestSettingLabelsCoverCatalogue moves FAIL->ok",
            CARD_CLASS_PRODUCT,
        ),
        (
            "error propagation closing a gap",
            "go vet and go test stay green while a new unit test can assert runJob returns an error",
            "Class: product\nFile: internal/runner/run.go\nVariable: check and propagate the error from os.MkdirAll instead of discarding it\nHypothesis: go vet ./... and go test ./internal/runner stay green while a new unit test can assert runJob returns a fail() error when workDir creation fails, closing a silent-failure gap",
            CARD_CLASS_PRODUCT,
        ),
        (
            "trailing not-just aside",
            "check:npm run typecheck covers web-shared too",
            "Class: product\nFile: packages/web-shared/package.json\nVariable: add a typecheck script\nHypothesis: check:npm run typecheck (root) covers web-shared and web too, not just web-fachsprachtrainer",
            CARD_CLASS_OPERATING_LAYER,
        ),
    ];
    for (name, summary, body, want) in tests {
        assert_eq!(crate::card_class::card_class(summary, body), want, "{name}");
    }
}

#[test]
fn is_operating_layer_path() {
    let _serial = crate::test_helpers::serial();
    for p in [
        "internal/runner/run_test.go",
        "src/a/__tests__/x.test.tsx",
        "internal/testutil/gitenv.go",
        "tools/test.sh",
        ".github/workflows/ci.yml",
        "docs/memory/lessons.md",
        "docs/scripts/pulse.py",
        "AGENTS.md",
        "js/check-ui.mjs",
        "docs/scripts/check-links.sh",
        "src/backend/Amistio.Api.Tests/Amistio.Api.Tests.csproj",
        "src/web/tsconfig.json",
        "src/web/vitest.config.ts",
        "pnpm-lock.yaml",
        "Makefile",
    ] {
        assert!(crate::card_class::is_operating_layer_path(p), "{p}");
    }
    for p in [
        "internal/runner/run.go",
        "src/web/app/page.tsx",
        "docs/runbooks/pretraining.md",
        "README.md",
        "_posts/2022-03-21-GRPC.md",
        "lib/site-data.ts",
        "package.json",
        "cmd/pulse/main.go",
        "web/hub/src/components/ui/sheet.tsx",
        "docs/testing.md",
        "",
    ] {
        assert!(!crate::card_class::is_operating_layer_path(p), "{p}");
    }
}

#[test]
fn check_amendment_messages() {
    let _serial = crate::test_helpers::serial();
    let protected = vec![
        "Hard stops".to_string(),
        "Kill switch".to_string(),
        "Amendments".to_string(),
        "frontmatter".to_string(),
    ];
    let cases = [
        (
            "no proposed",
            "## Current text\n\n```\nx\n```\n".to_string(),
            r###"malformed: no fenced block under "## Proposed text""###,
        ),
        (
            "no current",
            "## Proposed text\n\n```\nx\n```\n".to_string(),
            r###"malformed: no fenced block under "## Current text""###,
        ),
        (
            "lower quorum",
            amendment(
                "| Amendment quorum | 1 distinct voter |",
                "| Amendment quorum | 0 distinct voters |",
            ),
            "lowers or removes the amendment quorum",
        ),
        (
            "remove quorum",
            amendment(
                "| Amendment quorum | 1 distinct voter |",
                "| Quorum | gone |",
            ),
            "lowers or removes the amendment quorum",
        ),
        (
            "lower majority",
            amendment(
                "| Amendment supermajority | 2/3 of counted votes |",
                "| Amendment supermajority | 1/2 of counted votes |",
            ),
            "lowers or removes the amendment supermajority",
        ),
        (
            "remove guard",
            amendment(
                "| Informed vote | text_hash plus a Read: line |",
                "| Informed | anything |",
            ),
            r#"removes the "Informed vote" row"#,
        ),
        (
            "drop protected",
            amendment(
                "| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter |",
                "| Not amendable by vote | Hard stops, Amendments, frontmatter |",
            ),
            r#"drops "Kill switch" from the protected sections"#,
        ),
        (
            "protected section",
            amendment("Never cross a hard stop.", "Cross them."),
            r#"protected section "Hard stops""#,
        ),
        (
            "protected case-insensitive",
            amendment("Per docs/workflows/amend.md.", "Per nothing."),
            r#"protected section "Amendments""#,
        ),
        (
            "frontmatter",
            amendment("loop: off", "loop: on"),
            r#"protected section "frontmatter""#,
        ),
        (
            "ok",
            amendment(
                "| Votes per identity per suggestion | 1 (the latest wins) |",
                "| Votes per identity per suggestion | 1 |",
            ),
            "ok",
        ),
    ];
    for (name, body, want) in cases {
        let got = check_amendment(&body, FIXTURE_CONSTITUTION_FILE, &protected)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "ok".into());
        assert_eq!(got, want, "{name}");
    }
    let got = check_amendment(&amendment("a", "b"), "", &protected)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_else(|| "ok".into());
    assert_eq!(got, "unchecked: no constitution record");
    let ck = check(
        &amendment("this line is not in the constitution", "nor this"),
        FIXTURE_CONSTITUTION_FILE,
        PROTECTED_DEFAULT,
    );
    assert_eq!(ck.anchor, "0");
    assert_eq!(ck.verdict, "ok");
    assert!(!ck.hash.is_empty());
    let twice = format!("{FIXTURE_CONSTITUTION_FILE}\n## Extra\n\nNever cross a hard stop.\n");
    let ck = check(
        &amendment("Never cross a hard stop.", "Cross them."),
        &twice,
        PROTECTED_DEFAULT,
    );
    assert_eq!(ck.anchor, "2");
    assert_eq!(ck.section, "");
    assert_eq!(ck.verdict, "ok");
    let ck = check(
        &amendment(
            "| Votes per identity per suggestion | 1 (the latest wins) |",
            "| Votes per identity per suggestion | 1 |",
        ),
        FIXTURE_CONSTITUTION_FILE,
        PROTECTED_DEFAULT,
    );
    assert_eq!(ck.anchor, "1");
    assert_eq!(ck.section, "Vote rules");
    let ck = check(
        &amendment("loop: off", "loop: on"),
        FIXTURE_CONSTITUTION_BODY,
        PROTECTED_DEFAULT,
    );
    assert_eq!(ck.anchor, "0");
    assert_eq!(ck.verdict, "ok");
}

#[test]
fn text_hash() {
    let _serial = crate::test_helpers::serial();
    fn sum(s: &str) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(s.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
    assert_eq!(
        crate::text::text_hash(&amendment("old", "  new line  \n\nsecond")),
        sum("  new line\n\nsecond\n")
    );
    assert_eq!(
        crate::text::text_hash("\n\nhello  \nworld\t\n\n"),
        sum("hello\nworld\n")
    );
    assert_eq!(crate::text::text_hash(""), sum("\n"));
    assert_eq!(
        crate::text::text_hash(FIXTURE_AMENDMENT_BODY),
        sum("| Votes per identity per suggestion | 1 (the latest wins; a fleet is one voter) |\n")
    );
}

#[test]
fn parse_majority() {
    let _serial = crate::test_helpers::serial();
    assert_eq!(crate::thresholds::parse_majority("2/3").unwrap(), (2, 3));
    assert_eq!(
        crate::thresholds::parse_majority(" 4 / 3 ").unwrap(),
        (4, 3)
    );
    for bad in ["", "2", "a/b", "3/0", "-1/2", "2/3/4"] {
        assert!(crate::thresholds::parse_majority(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn day_number_and_boundaries() {
    let _serial = crate::test_helpers::serial();
    assert_eq!(text::day_number("2026-09-06T21:00:00Z"), 739865);
    assert_eq!(text::day_number("1970-01-01"), 719163);
    assert_eq!(text::day_number("0001-01-01"), 1);
    assert_eq!(text::day_number("garbage"), 0);

    let home = tempfile::tempdir().unwrap();
    pulse_store::Store::create(home.path().join("pulse.sqlite")).unwrap();
    let st = pulse_store::Store::open(pulse_store::Options {
        home: home.path().display().to_string(),
        create: false,
        read_only: false,
        skip_records: false,
        now: FIXTURE_NOW.into(),
    })
    .unwrap();
    {
        let conn = st.conn().unwrap();
        conn.execute(
            "INSERT INTO projects (slug, name, path, added_at) VALUES ('fixture', 'P', ?, ?)",
            params![home.path().join("p").display().to_string(), FIXTURE_NOW],
        )
        .unwrap();
    }
    let mut f = test_fixture::Fixture {
        home,
        repo: PathBuf::new(),
        st,
        ids: BTreeMap::new(),
        hash: String::new(),
    };
    let edge = f.post(test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "edge",
        "r/a",
        "2026-09-08T00:00:00Z",
        "two days old",
        "s",
    ));
    let over = f.post(test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "over",
        "r/a",
        "2026-09-07T23:59:59Z",
        "three days old",
        "s",
    ));
    let mut p = test_fixture::FPost::new(
        "claim",
        "governance",
        "c1",
        "r/a",
        "2026-09-09T00:00:00Z",
        "still live",
        "s",
    );
    p.expires = "2026-09-10T12:00:01Z".into();
    let live = f.post(p);
    let mut p = test_fixture::FPost::new(
        "claim",
        "governance",
        "c2",
        "r/a",
        "2026-09-09T01:00:00Z",
        "just dead",
        "s",
    );
    p.expires = "2026-09-10T12:00:00Z".into();
    let dead = f.post(p);
    let noexp = f.post(test_fixture::FPost::new(
        "claim",
        "governance",
        "c3",
        "r/a",
        "2026-09-09T02:00:00Z",
        "no expiry",
        "s",
    ));
    f.post(test_fixture::FPost::new(
        "finding",
        "governance",
        "fresh",
        "r/a",
        "2026-09-05T00:00:00Z",
        "five days",
        "s",
    ));
    f.post(test_fixture::FPost::new(
        "finding",
        "governance",
        "old",
        "r/a",
        "2026-09-04T23:00:00Z",
        "six days",
        "s",
    ));

    let th = crate::thresholds::load_thresholds(
        &f.st,
        "fixture",
        Thresholds {
            now: FIXTURE_NOW.into(),
            expiry_days: 2,
            stale_days: 5,
            ..Default::default()
        },
    );
    let res = compute(&f.st, "fixture", "", th.clone()).unwrap();
    let mut status = BTreeMap::new();
    for s in &res.suggestions {
        status.insert(s.id.as_str(), s.status.as_str());
    }
    for c in &res.claims {
        status.insert(c.id.as_str(), c.status.as_str());
    }
    for x in &res.threads {
        status.insert(x.key.as_str(), x.status.as_str());
    }
    for (k, v) in [
        (edge.as_str(), "needs votes"),
        (over.as_str(), "expired"),
        (live.as_str(), "active"),
        (dead.as_str(), "expired"),
        (noexp.as_str(), "no expiry"),
        ("governance/fresh", "open"),
        ("governance/old", "stale"),
    ] {
        assert_eq!(status[k], v, "{k}");
    }
    let mut p = test_fixture::FPost::new(
        "vote",
        "evolution",
        "over",
        "r/b",
        "2026-09-10T00:00:00Z",
        "up",
        "t",
    );
    p.on = over.clone();
    p.vote = "up".into();
    f.post(p);
    let res = compute(&f.st, "fixture", "", th.clone()).unwrap();
    let voted = res.suggestions.iter().find(|s| s.id == over).unwrap();
    assert_eq!(voted.status, "needs votes");
    assert_eq!(voted.voters, 1);
    let tsv = res.records();
    assert!(
        tsv.starts_with("param\tboard_present\t1\n"),
        "records:\n{tsv}"
    );
    assert!(tsv.contains("\nparam\texpiry_days\t2\n"), "records:\n{tsv}");
    assert!(
        tsv.contains(&format!("\nclaim\t{dead}\t")),
        "records:\n{tsv}"
    );
    let tsv2 = crate::records::records(&f.st, "fixture", "", th).unwrap();
    assert_eq!(tsv2, tsv);
}

#[test]
fn duplicate_cards_gate_and_leave_ballot() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(true);
    let mut p = test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "dup-a",
        "runner-da/model",
        "2026-09-08T08:00:00Z",
        "Fix OAuth callback copy",
        "s-da",
    );
    p.body = "File: src/oauth.go\nVariable: callback copy".into();
    let earlier = f.post(p);
    let mut p = test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "dup-b",
        "runner-db/model",
        "2026-09-08T09:00:00Z",
        "`fix`  \"oauth\" callback   copy",
        "s-db",
    );
    p.body = "File: src/other.go\nVariable: other copy".into();
    let later = f.post(p);
    let mut p = test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "pair-a",
        "runner-pa/model",
        "2026-09-08T10:00:00Z",
        "Login message copy",
        "s-pa",
    );
    p.body = "File: src/login.go\nVariable: callback copy".into();
    let pair_earlier = f.post(p);
    let mut p = test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "pair-b",
        "runner-pb/model",
        "2026-09-08T11:00:00Z",
        "Localized login failure",
        "s-pb",
    );
    p.body = "File: src/login.go\nVariable: callback copy".into();
    let pair_later = f.post(p);
    let mut p = test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "not-dup",
        "runner-nd/model",
        "2026-09-08T12:00:00Z",
        "Localized billing failure",
        "s-nd",
    );
    p.body = "File: src/billing.go\nVariable: callback copy".into();
    let different_file = f.post(p);

    let res = compute(
        &f.st,
        "fixture",
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        duplicate_of(
            Some(&res),
            &Suggestion {
                id: later.clone(),
                ..Default::default()
            }
        ),
        earlier
    );
    assert_eq!(
        duplicate_of(
            Some(&res),
            &Suggestion {
                id: pair_later.clone(),
                ..Default::default()
            }
        ),
        pair_earlier
    );
    assert_eq!(
        duplicate_of(
            Some(&res),
            &Suggestion {
                id: different_file.clone(),
                ..Default::default()
            }
        ),
        ""
    );
    let ctx = plan_context_for(&f.st, "fixture", FIXTURE_NOW, Some(&res));
    let sgs: BTreeMap<_, _> = res
        .suggestions
        .iter()
        .map(|sg| (sg.id.as_str(), sg.clone()))
        .collect();
    let posts: BTreeMap<_, _> = res.posts.iter().map(|p| (p.id.as_str(), p)).collect();
    assert_eq!(
        plan_gate_with(
            &f.st,
            ctx,
            &f.repo.display().to_string(),
            &sgs[later.as_str()],
            Some(posts[later.as_str()])
        ),
        duplicate_gate(&earlier)
    );
    let ballot_ids: Vec<_> = res
        .ballot("", "")
        .iter()
        .filter_map(|it| it.suggestion.as_ref().map(|sg| sg.id.clone()))
        .collect();
    assert!(!ballot_ids.contains(&later), "ballot: {ballot_ids:?}");
    assert!(!ballot_ids.contains(&pair_later), "ballot: {ballot_ids:?}");
    assert!(ballot_ids.contains(&earlier), "ballot: {ballot_ids:?}");
    assert!(ballot_ids.contains(&pair_earlier), "ballot: {ballot_ids:?}");
    assert!(
        ballot_ids.contains(&different_file),
        "ballot: {ballot_ids:?}"
    );
}

#[test]
fn operating_layer_cap_gate() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(true);
    f.st.set_setting("test", "plan.operating_layer_per_day", "1")
        .unwrap();
    let mut p = test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "counted",
        "runner-counted/model",
        "2026-09-09T08:00:00Z",
        "check:tools/test.sh stays green",
        "s-counted",
    );
    p.body = "Class: operating-layer\nFile: tools/test.sh\nVariable: shell check".into();
    let counted = f.post(p);
    let op = test_fixture::add_eligible_suggestion(
        &mut f,
        "7",
        "runner-op/model",
        "op-cap",
        "gofmt -l goes 1->0",
        "Class: product\nVariable: gofmt\nHypothesis: gofmt",
        "",
        &["up", "up"],
    );
    let product = test_fixture::add_eligible_suggestion(
        &mut f,
        "8",
        "runner-product/model",
        "product-cap",
        "Localized OAuth error",
        "Class: product\nFile: src/oauth.go\nVariable: callback copy\nHypothesis: user sees localized error",
        "",
        &["up", "up"],
    );
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "INSERT INTO jobs (id, project, kind, title, brief, source, status, created_at) VALUES ('J-counted', 'fixture', 'improve', 'counted', '', ?, 'queued', ?)",
            params![format!("suggestion:{counted}"), FIXTURE_NOW],
        )
        .unwrap();
    }
    let res = compute(
        &f.st,
        "fixture",
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    )
    .unwrap();
    let ctx = plan_context_for(&f.st, "fixture", FIXTURE_NOW, Some(&res));
    let sgs: BTreeMap<_, _> = res
        .suggestions
        .iter()
        .map(|sg| (sg.id.as_str(), sg.clone()))
        .collect();
    let posts: BTreeMap<_, _> = res.posts.iter().map(|p| (p.id.as_str(), p)).collect();
    let repo = f.repo.display().to_string();
    let want = crate::plan_context::operating_layer_cap_gate(1, 1);
    assert_eq!(
        plan_gate_with(
            &f.st,
            ctx,
            &repo,
            &sgs[op.as_str()],
            Some(posts[op.as_str()])
        ),
        want
    );
    assert_eq!(
        plan_gate_with(
            &f.st,
            plan_context_for(&f.st, "fixture", FIXTURE_NOW, Some(&res)),
            &repo,
            &sgs[product.as_str()],
            Some(posts[product.as_str()])
        ),
        ""
    );
    f.st.set_setting("test", "plan.operating_layer_per_day", "0")
        .unwrap();
    assert_eq!(
        plan_gate_with(
            &f.st,
            plan_context_for(&f.st, "fixture", FIXTURE_NOW, Some(&res)),
            &repo,
            &sgs[op.as_str()],
            Some(posts[op.as_str()])
        ),
        ""
    );
    f.st.set_setting("test", "plan.operating_layer_per_day", "1")
        .unwrap();
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "UPDATE jobs SET created_at = '2026-09-09T23:59:59Z' WHERE id = 'J-counted'",
            [],
        )
        .unwrap();
    }
    assert_eq!(
        plan_gate_with(
            &f.st,
            plan_context_for(&f.st, "fixture", "2026-09-10T00:00:01Z", Some(&res)),
            &repo,
            &sgs[op.as_str()],
            Some(posts[op.as_str()])
        ),
        ""
    );
}

#[test]
fn operating_layer_cap_lifts_when_no_product_card_competes() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(true);
    f.st.set_setting("test", "plan.operating_layer_per_day", "1")
        .unwrap();
    let mut p = test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "counted",
        "runner-counted/model",
        "2026-09-09T08:00:00Z",
        "check:tools/test.sh stays green",
        "s-counted",
    );
    p.body = "Class: operating-layer\nFile: tools/test.sh\nVariable: shell check".into();
    let counted = f.post(p);
    let op = test_fixture::add_eligible_suggestion(
        &mut f,
        "7",
        "runner-op/model",
        "op-cap",
        "gofmt -l goes 1->0",
        "Class: product\nVariable: gofmt\nHypothesis: gofmt",
        "",
        &["up", "up"],
    );
    // The only product card names a path the checkout no longer has, so it
    // is held and does not compete for the slot.
    let gone_product = test_fixture::add_eligible_suggestion(
        &mut f,
        "8",
        "runner-product/model",
        "product-gone",
        "Localized OAuth error",
        "Class: product\nFile: legacy/oauth.go\nVariable: callback copy\nHypothesis: user sees localized error",
        "",
        &["up", "up"],
    );
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "INSERT INTO jobs (id, project, kind, title, brief, source, status, created_at) VALUES ('J-counted', 'fixture', 'improve', 'counted', '', ?, 'queued', ?)",
            params![format!("suggestion:{counted}"), FIXTURE_NOW],
        )
        .unwrap();
    }
    let th = Thresholds {
        now: FIXTURE_NOW.into(),
        ..Default::default()
    };
    let res = compute(&f.st, "fixture", "", th.clone()).unwrap();
    let sgs: BTreeMap<_, _> = res
        .suggestions
        .iter()
        .map(|sg| (sg.id.as_str(), sg.clone()))
        .collect();
    let posts: BTreeMap<_, _> = res.posts.iter().map(|p| (p.id.as_str(), p)).collect();
    let repo = f.repo.display().to_string();
    let ctx = plan_context_for(&f.st, "fixture", FIXTURE_NOW, Some(&res));
    assert_eq!((ctx.operating_layer_today, ctx.operating_layer_cap), (1, 1));
    assert!(ctx.operating_layer_filled, "cap not lifted: {ctx:?}");
    assert_eq!(
        plan_gate_with(
            &f.st,
            ctx.clone(),
            &repo,
            &sgs[gone_product.as_str()],
            Some(posts[gone_product.as_str()])
        ),
        GATE_FILE_GONE
    );
    assert_eq!(
        plan_gate_with(
            &f.st,
            ctx,
            &repo,
            &sgs[op.as_str()],
            Some(posts[op.as_str()])
        ),
        ""
    );
    // The owner may keep the cap strict.
    f.st.set_setting("test", "plan.operating_layer_fill", "off")
        .unwrap();
    let ctx = plan_context_for(&f.st, "fixture", FIXTURE_NOW, Some(&res));
    assert!(!ctx.operating_layer_filled);
    assert_eq!(
        plan_gate_with(
            &f.st,
            ctx,
            &repo,
            &sgs[op.as_str()],
            Some(posts[op.as_str()])
        ),
        crate::plan_context::operating_layer_cap_gate(1, 1)
    );
    // A product card that passes the gates makes the cap hold again.
    f.st.set_setting("test", "plan.operating_layer_fill", "on")
        .unwrap();
    test_fixture::add_eligible_suggestion(
        &mut f,
        "9",
        "runner-live/model",
        "product-live",
        "Sign-in keeps the typed email after a wrong password",
        "Class: product\nFile: src/login.go\nVariable: keep the email field\nHypothesis: fewer retyped sign-ins",
        "",
        &["up", "up"],
    );
    let res = compute(&f.st, "fixture", "", th).unwrap();
    let ctx = plan_context_for(&f.st, "fixture", FIXTURE_NOW, Some(&res));
    assert!(
        !ctx.operating_layer_filled,
        "a live product card competes: {ctx:?}"
    );
    let sgs: BTreeMap<_, _> = res
        .suggestions
        .iter()
        .map(|sg| (sg.id.as_str(), sg.clone()))
        .collect();
    let posts: BTreeMap<_, _> = res.posts.iter().map(|p| (p.id.as_str(), p)).collect();
    assert_eq!(
        plan_gate_with(
            &f.st,
            ctx,
            &repo,
            &sgs[op.as_str()],
            Some(posts[op.as_str()])
        ),
        crate::plan_context::operating_layer_cap_gate(1, 1)
    );
}

#[test]
fn compute_caches_result_for_unchanged_store() {
    let _serial = crate::test_helpers::serial();
    let _guard = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::cache::clear_cache_for_test();
    let f = test_fixture::new_fixture(true);
    let th = Thresholds {
        now: FIXTURE_NOW.into(),
        ..Default::default()
    };
    let r1 = compute(&f.st, "fixture", "", th.clone()).unwrap();
    let after_first = crate::cache::cache_stats();
    let r2 = compute(&f.st, "fixture", "", th).unwrap();
    let after_second = crate::cache::cache_stats();
    assert_eq!(r1, r2);
    assert_eq!(after_first.misses, 1);
    assert_eq!(after_second.hits, 1);
    assert_eq!(after_second.misses, 1);
    assert!(crate::cache::cache_len_for_test() > 0);
}

#[test]
fn compute_cache_buckets_the_clock_by_five_minutes() {
    let _serial = crate::test_helpers::serial();
    let _guard = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::cache::clear_cache_for_test();
    let f = test_fixture::new_fixture(true);
    let at = |now: &str| Thresholds {
        now: now.into(),
        ..Default::default()
    };
    compute_light(&f.st, "fixture", "", at("2026-09-10T12:00:07Z")).unwrap();
    assert_eq!(crate::cache::cache_stats().misses, 1);
    // Four minutes later, same bucket: the same tally.
    compute_light(&f.st, "fixture", "", at("2026-09-10T12:04:59Z")).unwrap();
    assert_eq!(crate::cache::cache_stats().hits, 1);
    assert_eq!(crate::cache::cache_stats().misses, 1);
    // The next bucket is a new key.
    compute_light(&f.st, "fixture", "", at("2026-09-10T12:05:00Z")).unwrap();
    assert_eq!(crate::cache::cache_stats().misses, 2);
    // A clock the key cannot read is hashed as it is, and still caches.
    compute_light(&f.st, "fixture", "", at("odd")).unwrap();
    compute_light(&f.st, "fixture", "", at("odd")).unwrap();
    assert_eq!(crate::cache::cache_stats().misses, 3);
    assert_eq!(crate::cache::cache_stats().hits, 2);
}

#[test]
fn compute_invalidates_cache_when_post_added() {
    let _serial = crate::test_helpers::serial();
    let _guard = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::cache::clear_cache_for_test();
    let mut f = test_fixture::new_fixture(true);
    let th = Thresholds {
        now: FIXTURE_NOW.into(),
        ..Default::default()
    };
    let r1 = compute(&f.st, "fixture", "", th.clone()).unwrap();
    let mut p = test_fixture::FPost::new(
        "claim",
        "governance",
        "new",
        "r/c",
        FIXTURE_NOW,
        "new claim",
        "s",
    );
    p.expires = "2026-12-31T00:00:00Z".into();
    let new_id = f.post(p);
    let r2 = compute(&f.st, "fixture", "", th).unwrap();
    assert_ne!(r1, r2);
    assert!(r2.claims.iter().any(|c| c.id == new_id));
}

#[test]
fn compute_invalidates_cache_when_settings_change() {
    let _serial = crate::test_helpers::serial();
    let _guard = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::cache::clear_cache_for_test();
    let f = test_fixture::new_fixture(true);
    let th = Thresholds {
        now: FIXTURE_NOW.into(),
        ..Default::default()
    };
    let r1 = compute(&f.st, "fixture", "", th.clone()).unwrap();
    {
        let conn = f.st.conn().unwrap();
        conn.execute("UPDATE settings SET value = '1' WHERE name = 'stop'", [])
            .unwrap();
    }
    let r2 = compute(&f.st, "fixture", "", th).unwrap();
    assert_ne!(r1.params.stop_present, r2.params.stop_present);
    assert!(r2.params.stop_present);
}

#[test]
fn compute_invalidates_cache_when_vote_job_or_landing_changes() {
    let _serial = crate::test_helpers::serial();
    let _guard = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::cache::clear_cache_for_test();
    let mut f = test_fixture::new_fixture(true);
    let th = Thresholds {
        now: FIXTURE_NOW.into(),
        ..Default::default()
    };
    let _ = compute(&f.st, "fixture", "", th.clone()).unwrap();
    assert_eq!(crate::cache::cache_stats().misses, 1);

    let mut vote = test_fixture::FPost::new(
        "vote",
        "evolution",
        "speedup",
        "runner-cache/model",
        "2026-09-02T15:00:00Z",
        "up",
        "s-cache",
    );
    vote.on = f.ids["eligible"].clone();
    vote.vote = "up".into();
    f.post(vote);
    let _ = compute(&f.st, "fixture", "", th.clone()).unwrap();
    assert_eq!(crate::cache::cache_stats().misses, 2);

    f.st.conn().unwrap().execute(
        "INSERT INTO jobs (id, project, kind, title, source, status, created_at) VALUES ('job-cache', 'fixture', 'improve', 'cache', 'suggestion:x', 'queued', ?)",
        [FIXTURE_NOW],
    ).unwrap();
    let _ = compute(&f.st, "fixture", "", th.clone()).unwrap();
    assert_eq!(crate::cache::cache_stats().misses, 3);

    f.st.conn().unwrap().execute(
        "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'test', 'land.pushed', 'fixture', '')",
        [FIXTURE_NOW],
    ).unwrap();
    let _ = compute(&f.st, "fixture", "", th).unwrap();
    assert_eq!(crate::cache::cache_stats().misses, 4);
}

#[test]
fn compute_cache_expires_after_ttl() {
    let _serial = crate::test_helpers::serial();
    let _guard = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::cache::clear_cache_for_test();
    let f = test_fixture::new_fixture(true);
    let th = Thresholds {
        now: FIXTURE_NOW.into(),
        ..Default::default()
    };
    let r1 = compute(&f.st, "fixture", "", th.clone()).unwrap();
    crate::cache::expire_cache_for_test();
    let r2 = compute(&f.st, "fixture", "", th).unwrap();
    assert_eq!(r1, r2);
    assert!(crate::cache::cache_len_for_test() > 0);
}

#[test]
fn digest_shows_unanswered_mentions_for_agent() {
    let _serial = crate::test_helpers::serial();
    let f = test_fixture::new_fixture(false);
    pulse_board::join_agent(
        &f.st,
        pulse_board::JoinInput {
            name: "alice".into(),
            runner: "copilot".into(),
            model: "m".into(),
            session: "sess-alice".into(),
            ..Default::default()
        },
    )
    .unwrap();
    pulse_board::join_agent(
        &f.st,
        pulse_board::JoinInput {
            name: "bob".into(),
            runner: "copilot".into(),
            model: "m".into(),
            session: "sess-bob".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let p = pulse_board::create_post(
        &f.st,
        pulse_board::PostInput {
            kind: "question".into(),
            project: "fixture".into(),
            area: "api".into(),
            thread: "rate-limits".into(),
            summary: "@bob which fixture covers 429s?".into(),
            agent: "alice".into(),
            date: "2026-09-10T10:00:00Z".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let text = digest_for(
        &f.st,
        "fixture",
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
        "bob",
        "sess-bob",
    )
    .unwrap();
    for want in [
        "MENTIONS FOR YOU (1)",
        p.id.as_str(),
        "api/rate-limits",
        "by alice",
        "@bob which fixture covers 429s?",
    ] {
        assert!(text.contains(want), "missing {want:?}:\n{text}");
    }
    pulse_board::create_post(
        &f.st,
        pulse_board::PostInput {
            kind: "answer".into(),
            project: "fixture".into(),
            area: p.area,
            thread: p.thread,
            summary: "answered".into(),
            agent: "bob".into(),
            date: "2026-09-10T10:30:00Z".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let text = digest_for(
        &f.st,
        "fixture",
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
        "bob",
        "sess-bob",
    )
    .unwrap();
    assert!(!text.contains("MENTIONS FOR YOU"), "{text}");
}

#[test]
fn fixture_hash_matches() {
    let _serial = crate::test_helpers::serial();
    let f = test_fixture::new_fixture(true);
    let st = test_fixture::open_read_only(f.home.path());
    let got = test_fixture::run_cli(&st, "hash", &[&f.ids["amendment"]]).unwrap();
    assert_eq!(got.trim(), f.hash);
}

#[test]
fn fixture_statuses() {
    let _serial = crate::test_helpers::serial();
    let f = test_fixture::new_fixture(true);
    let th = crate::thresholds::load_thresholds(
        &f.st,
        "fixture",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    );
    assert_eq!(th.amend_quorum, 1, "thresholds {th:?}");
    assert_eq!(th.amend_majority, "2/3", "thresholds {th:?}");
    assert!(th.constitution_present, "thresholds {th:?}");
    assert!(!th.stop_present, "thresholds {th:?}");
    let res = compute(&f.st, "fixture", "", th.clone()).unwrap();
    let sug: BTreeMap<_, _> = res
        .suggestions
        .iter()
        .map(|s| (s.id.as_str(), s.clone()))
        .collect();
    let eligible = &sug[f.ids["eligible"].as_str()];
    assert_eq!(eligible.status, "eligible", "eligible: {eligible:?}");
    assert_eq!((eligible.up, eligible.down, eligible.voters), (2, 1, 3));
    let needs = &sug[f.ids["needs_votes"].as_str()];
    assert_eq!(needs.status, "needs votes", "needs_votes: {needs:?}");
    assert_eq!(needs.voters, 1);
    assert_eq!(sug[f.ids["expired"].as_str()].status, "expired");

    let cl: BTreeMap<_, _> = res
        .claims
        .iter()
        .map(|c| (c.id.as_str(), c.clone()))
        .collect();
    assert_eq!(cl[f.ids["claim_active"].as_str()].status, "active");
    assert_eq!(cl[f.ids["claim_expired"].as_str()].status, "expired");

    let th2: BTreeMap<_, _> = res
        .threads
        .iter()
        .map(|x| (x.key.as_str(), x.clone()))
        .collect();
    assert_eq!(th2["governance/closed"].status, "closed");
    assert_eq!(th2["governance/questions/old"].status, "stale");
    assert_eq!(th2["evolution/speedup"].status, "open");
    assert_eq!(res.proposals.len(), 1, "proposals: {:?}", res.proposals);
    assert_eq!(res.proposals[0].status, "awaiting");

    let am: BTreeMap<_, _> = res
        .amendments
        .iter()
        .map(|a| (a.id.as_str(), a.clone()))
        .collect();
    let a = &am[f.ids["amendment"].as_str()];
    assert_eq!(
        a.status, "passed: ratify with pulse amend apply",
        "amendment: {a:?}"
    );
    assert_eq!(
        (a.up, a.voters, a.self_, a.badhash, a.noread),
        (1, 1, 1, 1, 1)
    );
    assert_eq!(a.section, "Vote rules");
    let b = &am[f.ids["blocked"].as_str()];
    assert!(
        b.status.starts_with("blocked: protected section"),
        "blocked amendment: {b:?}"
    );
    let passed = passed_amendments(&f.st, "fixture", th.clone()).unwrap();
    assert_eq!(passed.len(), 1);
    assert_eq!(passed[0].id, f.ids["amendment"]);

    let items = ballot(&f.st, "fixture", "runner-1/model-1", "s-1", th).unwrap();
    let kinds: Vec<String> = items
        .iter()
        .map(|it| {
            if let Some(sg) = &it.suggestion {
                format!("{}:{}", it.kind, sg.id)
            } else if let Some(am) = &it.amendment {
                format!("{}:{}", it.kind, am.id)
            } else {
                format!("{}:", it.kind)
            }
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            format!("suggestion:{}", f.ids["needs_votes"]),
            format!("amendment:{}", f.ids["blocked"])
        ]
    );
}

#[test]
fn apply_amendment() {
    let _serial = crate::test_helpers::serial();
    let f = test_fixture::new_fixture(true);
    let th = crate::thresholds::load_thresholds(
        &f.st,
        "fixture",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    );
    let aid = &f.ids["amendment"];
    let err = crate::amend::apply_amendment(&f.st, &f.ids["blocked"], th.clone()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("protected section") && msg.contains("pulse decide"),
        "{msg}"
    );
    let err = crate::amend::apply_amendment(&f.st, &f.ids["eligible"], th.clone()).unwrap_err();
    assert!(err.to_string().contains("not an amendment"), "{err}");
    assert!(crate::amend::apply_amendment(&f.st, "nope", th.clone()).is_err());

    let got = crate::amend::apply_amendment(&f.st, aid, th).unwrap();
    assert_eq!(got.rev, 2, "applied: {got:?}");
    assert_eq!(got.record_id, "EVO-constitution");
    assert_eq!(got.author, "runner-p/model-p");
    assert_eq!(got.already_at, "");
    assert_eq!(got.section, "Vote rules");
    let (_, rev) = constitution_record(&f.st, "fixture", 0).unwrap();
    let rev = rev.unwrap();
    assert!(rev.body.contains("a fleet is one voter"));
    assert!(!rev.body.contains("| 1 (the latest wins) |"));
    assert_eq!(rev.author, "runner-p/model-p");
    assert_eq!(rev.note, *aid);
    let n: i64 =
        f.st.conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE action = 'constitution.amended' AND subject = ?",
                [aid],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(n, 1);
    let (_, r1) = constitution_record(&f.st, "fixture", 1).unwrap();
    assert!(r1.unwrap().body.contains("| 1 (the latest wins) |"));
    let again = crate::amend::apply_amendment(
        &f.st,
        aid,
        crate::thresholds::load_thresholds(
            &f.st,
            "fixture",
            Thresholds {
                now: FIXTURE_NOW.into(),
                ..Default::default()
            },
        ),
    )
    .unwrap();
    assert!(!again.already_at.is_empty(), "second apply: {again:?}");
    assert_eq!(again.rev, 2);
    let n: i64 =
        f.st.conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE action = 'constitution.amended'",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(n, 1);
    let res = compute(
        &f.st,
        "fixture",
        "",
        crate::thresholds::load_thresholds(
            &f.st,
            "fixture",
            Thresholds {
                now: FIXTURE_NOW.into(),
                ..Default::default()
            },
        ),
    )
    .unwrap();
    let amended = res.amendments.iter().find(|a| a.id == *aid).unwrap();
    assert!(
        amended.status.starts_with("stale"),
        "after apply: {amended:?}"
    );
    let notice = ratify(&f.st, &got, "").unwrap();
    assert_eq!(notice.kind, "result");
    assert_eq!(notice.resolution, "ratified");
    assert_eq!(notice.on, *aid);
    assert_eq!(notice.agent, SYSTEM_AGENT);
    let res = compute(
        &f.st,
        "fixture",
        "",
        crate::thresholds::load_thresholds(
            &f.st,
            "fixture",
            Thresholds {
                now: FIXTURE_NOW.into(),
                ..Default::default()
            },
        ),
    )
    .unwrap();
    let amended = res.amendments.iter().find(|a| a.id == *aid).unwrap();
    assert!(!amended.open);
    assert!(amended.status.starts_with("closed: ratified"));
    let out = test_fixture::run_cli(&f.st, "constitution", &["--project", "fixture"]).unwrap();
    assert!(out.contains("a fleet is one voter"));
    let out = test_fixture::run_cli(
        &f.st,
        "constitution",
        &["--project", "fixture", "--rev", "1"],
    )
    .unwrap();
    assert!(out.contains("| 1 (the latest wins) |"));
    let out = test_fixture::run_cli(&f.st, "amend", &["apply", aid, "--now", FIXTURE_NOW]).unwrap();
    assert!(out.contains("already applied"), "{out}");
}

#[test]
fn ratify_summary_limit() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(false);
    let prefix = "ratified: ";
    let at_limit = pulse_board::MAX_SUMMARY_CHARS - prefix.len();
    for (name, ch, count, want_count, suffix) in [
        ("ascii-at", "x", at_limit, at_limit, ""),
        ("ascii-over", "x", at_limit + 1, at_limit - 3, "..."),
        ("cjk-at", "界", at_limit, at_limit, ""),
        ("cjk-over", "界", at_limit + 1, at_limit - 3, "..."),
    ] {
        let mut p = test_fixture::FPost::new(
            "amendment",
            "board",
            "summary-limit",
            &format!("summary-{name}"),
            FIXTURE_NOW,
            &ch.repeat(count),
            "",
        );
        p.body = FIXTURE_AMENDMENT_BODY.into();
        let id = f.post(p);
        let notice = ratify(
            &f.st,
            &Applied {
                post_id: id,
                ..Default::default()
            },
            "",
        )
        .unwrap();
        let want = format!("{prefix}{}{}", ch.repeat(want_count), suffix);
        assert_eq!(notice.summary, want, "{name}");
        assert_eq!(
            notice.summary.chars().count(),
            pulse_board::MAX_SUMMARY_CHARS
        );
    }
}

#[test]
fn apply_amendment_without_record() {
    let _serial = crate::test_helpers::serial();
    let f = test_fixture::new_fixture(false);
    let th = crate::thresholds::load_thresholds(
        &f.st,
        "fixture",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    );
    assert!(th.constitution_present);
    let err = crate::amend::apply_amendment(&f.st, &f.ids["amendment"], th)
        .unwrap_err()
        .to_string();
    assert!(err.contains("no constitution record"), "{err}");
    struct LegacyGuard(bool);
    impl Drop for LegacyGuard {
        fn drop(&mut self) {
            crate::amend::set_legacy_files(self.0);
        }
    }
    let _guard = LegacyGuard(crate::amend::legacy_files());
    crate::amend::set_legacy_files(false);
    let th = crate::thresholds::load_thresholds(
        &f.st,
        "fixture",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    );
    assert!(!th.constitution_present);
    assert_eq!(th.amend_quorum, 40);
    assert!(!th.stop_present);
    let res = compute(&f.st, "fixture", "", th).unwrap();
    for a in &res.amendments {
        assert_eq!(a.status, "unchecked: no constitution record");
    }
}

#[test]
fn signal_unlanded_under_hook_git_env() {
    let _serial = crate::test_helpers::serial();
    let _env = pulse_testutil::env_lock();
    let f = test_fixture::new_fixture(false);
    fs::create_dir_all(f.repo.join("internal/x")).unwrap();
    fs::write(f.repo.join("internal/x/landed.go"), "package x\n").unwrap();
    git_in(&f.repo, &["init", "-q", "-b", "main"]);
    git_in(&f.repo, &["add", "."]);
    git_in(
        &f.repo,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "landed"],
    );

    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("README"), "hook repo\n").unwrap();
    git_in(other.path(), &["init", "-q", "-b", "main"]);
    git_in(other.path(), &["add", "."]);
    git_in(
        other.path(),
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "other"],
    );
    unsafe {
        std::env::set_var("GIT_DIR", other.path().join(".git"));
        std::env::set_var("GIT_WORK_TREE", other.path());
        std::env::set_var("GIT_INDEX_FILE", other.path().join(".git/index"));
        std::env::set_var("GIT_PREFIX", "");
    }
    reset_landed_cache();
    assert!(!crate::gate::signal_unlanded(
        &f.st,
        &f.repo.display().to_string(),
        "card",
        "Signal: internal/x/landed.go"
    ));
    unsafe {
        std::env::remove_var("GIT_DIR");
        std::env::remove_var("GIT_WORK_TREE");
        std::env::remove_var("GIT_INDEX_FILE");
        std::env::remove_var("GIT_PREFIX");
    }
}

#[test]
fn plan_gate_outcomes() {
    let _serial = crate::test_helpers::serial();
    reset_landed_cache();
    let mut f = test_fixture::new_fixture(true);
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "INSERT INTO jobs (id, project, kind, title, brief, source, status, created_at) VALUES ('J-blocked', 'fixture', 'improve', 'blocked', '', ?, 'blocked', ?)",
            params![format!("suggestion:{}", f.ids["eligible"]), FIXTURE_NOW],
        )
        .unwrap();
    }
    let net = test_fixture::add_eligible_suggestion(
        &mut f,
        "1",
        "runner-net/model",
        "net",
        "Break even",
        "No signal needed.",
        "",
        &["up", "down"],
    );
    let owner = test_fixture::add_eligible_suggestion(
        &mut f,
        "2",
        "runner-owner/model",
        "support-window",
        "Choose the support window",
        "No signal needed.",
        r#"{"for_owner":true}"#,
        &["up", "up"],
    );
    let no_evidence = test_fixture::add_eligible_suggestion(
        &mut f,
        "3",
        "runner-worker/model",
        "worker-no-signal",
        "Worker card",
        "No signal line.",
        "",
        &["up", "up"],
    );
    let owner_path = test_fixture::add_eligible_suggestion(
        &mut f,
        "4",
        "runner-worker-path/model",
        "worker-owner-path",
        "Worker owner path",
        "Signal: docs/decisions/adr.md",
        "",
        &["up", "up"],
    );
    let plannable = test_fixture::add_eligible_suggestion(
        &mut f,
        "5",
        "runner-plain/model",
        "owner-review",
        "Release candidate wording fix",
        "No signal needed for non-workers.",
        "",
        &["up", "up"],
    );
    // Evidence that lives in the store outlives the run that saw it: a job, a post named
    // with its `post:` prefix, a memory record. A dotfile is a file like any other.
    let job_evidence = test_fixture::add_eligible_suggestion(
        &mut f,
        "6",
        "runner-worker/model",
        "worker-job-signal",
        "Worker card with a job as its signal",
        "Signal: job:J-blocked",
        "",
        &["up", "up"],
    );
    let post_signal = format!("Signal: post:{}", f.ids["eligible"]);
    let post_evidence = test_fixture::add_eligible_suggestion(
        &mut f,
        "7",
        "runner-worker/model",
        "worker-post-signal",
        "Worker card with a post as its signal",
        &post_signal,
        "",
        &["up", "up"],
    );
    let memory_evidence = test_fixture::add_eligible_suggestion(
        &mut f,
        "8",
        "runner-worker/model",
        "worker-memory-signal",
        "Worker card with a memory entry as its signal",
        "Signal: MEM-failure-pattern-fixture.",
        "",
        &["up", "up"],
    );
    let dotfile_evidence = test_fixture::add_eligible_suggestion(
        &mut f,
        "9",
        "runner-worker/model",
        "worker-dotfile-signal",
        "Worker card with a dotfile as its signal",
        "Signal: .nvmrc",
        "",
        &["up", "up"],
    );
    let gone_evidence = test_fixture::add_eligible_suggestion(
        &mut f,
        "10",
        "runner-worker/model",
        "worker-gone-signal",
        "Worker card whose signal is a log that is gone",
        "Signal: /tmp/webtest.log (this run) and job-20990101T000000Z-000000",
        "",
        &["up", "up"],
    );
    // A card written against a tree that was since removed (a port) names a file whose
    // directory is gone; a card that adds a file to an existing directory is fine.
    let file_gone = test_fixture::add_eligible_suggestion(
        &mut f,
        "11",
        "runner-plain/model",
        "ported-away",
        "Card from before the port",
        "Class: product\nFile: internal/tally/gate.go\nVariable: strip GIT_DIR\nHypothesis: goes FAIL -> ok\nSignal: docs/decisions/adr.md",
        "",
        &["up", "up"],
    );
    let file_new = test_fixture::add_eligible_suggestion(
        &mut f,
        "12",
        "runner-plain/model",
        "new-file",
        "Card that adds a file",
        "Class: product\nFile: docs/decisions/0099-new.md\nVariable: record the decision\nHypothesis: the ADR exists\nSignal: docs/decisions/adr.md",
        "",
        &["up", "up"],
    );
    // A worker that took "a path in the clone" literally: the file it names
    // is checked at the project's checkout. The clone's root alone is not evidence.
    let clone_evidence = test_fixture::add_eligible_suggestion(
        &mut f,
        "13",
        "runner-worker/model",
        "worker-clone-signal",
        "Worker card whose signal is an absolute path into its clone",
        "Signal: /Users/w/.pulse/work/job-20260929T160938Z-3b72b7/docs/decisions/adr.md:12",
        "",
        &["up", "up"],
    );
    let clone_root = test_fixture::add_eligible_suggestion(
        &mut f,
        "14",
        "runner-worker/model",
        "worker-clone-root",
        "Worker card whose signal is its clone's root",
        "Signal: /Users/w/.pulse/work/job-20260929T160938Z-3b72b7",
        "",
        &["up", "up"],
    );
    // A card whose own Signal is a gone log, given its evidence by an opinion
    // on its thread; another whose opinion resolves nothing either.
    let thread_evidence = test_fixture::add_eligible_suggestion(
        &mut f,
        "15",
        "runner-worker/model",
        "worker-thread-signal",
        "Worker card whose thread supplies the evidence",
        "Signal: /tmp/webtest.log",
        "",
        &["up", "up"],
    );
    let thread_no_evidence = test_fixture::add_eligible_suggestion(
        &mut f,
        "16",
        "runner-worker/model",
        "worker-thread-no-signal",
        "Worker card whose thread supplies no evidence",
        "Signal: /tmp/webtest.log",
        "",
        &["up", "up"],
    );
    for (thread, on, signal, date) in [
        (
            "worker-thread-signal",
            &thread_evidence,
            "Signal: docs/decisions/adr.md",
            "2026-09-03T00:00:00Z",
        ),
        (
            "worker-thread-no-signal",
            &thread_no_evidence,
            "Signal: /tmp/other.log",
            "2026-09-03T00:01:00Z",
        ),
    ] {
        let mut p = test_fixture::FPost::new(
            "opinion",
            "evolution",
            thread,
            "runner-peer/model",
            date,
            "Seen in the checkout too",
            "s-peer",
        );
        p.body = signal.into();
        p.on = on.clone();
        f.post(p);
    }
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "UPDATE agents SET kind = 'worker' WHERE name IN ('runner-worker/model', 'runner-worker-path/model')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO records (id, project, kind, slug, title, created_at, created_by, updated_at, origin) VALUES ('MEM-failure-pattern-fixture', 'fixture', 'memory', 'failure-pattern-fixture', 'Memory', ?, 'worker', ?, 'store')",
            params![FIXTURE_NOW, FIXTURE_NOW],
        )
        .unwrap();
    }
    fs::create_dir_all(f.repo.join("docs/decisions")).unwrap();
    fs::write(f.repo.join("docs/decisions/adr.md"), "# ADR\n").unwrap();
    fs::write(f.repo.join(".nvmrc"), "22\n").unwrap();

    let res = compute(
        &f.st,
        "fixture",
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    )
    .unwrap();
    let suggestions: BTreeMap<_, _> = res
        .suggestions
        .iter()
        .map(|sg| (sg.id.as_str(), sg.clone()))
        .collect();
    let posts: BTreeMap<_, _> = res.posts.iter().map(|p| (p.id.as_str(), p)).collect();
    for (id, want) in [
        (f.ids["eligible"].as_str(), "blocked as job J-blocked"),
        (net.as_str(), GATE_NET_VOTES),
        (owner.as_str(), GATE_OWNER),
        (no_evidence.as_str(), GATE_NO_EVIDENCE),
        (owner_path.as_str(), GATE_OWNER_PATHS),
        (plannable.as_str(), ""),
        (job_evidence.as_str(), ""),
        (post_evidence.as_str(), ""),
        (memory_evidence.as_str(), ""),
        (dotfile_evidence.as_str(), ""),
        (gone_evidence.as_str(), GATE_NO_EVIDENCE),
        (file_gone.as_str(), GATE_FILE_GONE),
        (file_new.as_str(), ""),
        (clone_evidence.as_str(), ""),
        (clone_root.as_str(), GATE_NO_EVIDENCE),
        (thread_evidence.as_str(), ""),
        (thread_no_evidence.as_str(), GATE_NO_EVIDENCE),
    ] {
        assert_eq!(
            plan_gate(&f.st, &res.repo, &suggestions[id], Some(posts[id])),
            want,
            "PlanGate({id}) suggestion {:?}",
            suggestions[id]
        );
    }
}

#[test]
fn run_check_is_no_op() {
    let _serial = crate::test_helpers::serial();
    let _guard = LIST_TEST_HOOK_LOCK
        .lock()
        .expect("list test hook lock poisoned");
    crate::gate::set_list_tests_hook(Some(stub_list_tests));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::gate::set_list_tests_hook(None);
        }
    }
    let _reset = Reset;
    let cases = [
        (
            "matches",
            "check: go test ./internal/runner -run TestCompletionLanding.",
            false,
            "TestCompletionLanding.",
            true,
        ),
        (
            "quoted",
            "check: go test ./internal/runner -run 'TestCompletion.*'",
            false,
            "TestCompletion.*",
            true,
        ),
        (
            "quoted alternation",
            "check: go test ./internal/runner -run 'TestMissing|Test B' -count=1",
            true,
            "TestMissing|Test B",
            true,
        ),
        (
            "double quoted",
            "check: go test ./internal/runner -run \"TestMissing|Test's\"",
            true,
            "TestMissing|Test's",
            true,
        ),
        (
            "backquoted",
            "check: go test ./internal/runner -run `TestMissing|Test B`",
            true,
            "TestMissing|Test B",
            true,
        ),
        (
            "no-op",
            "check: go test ./internal/runner -run TestMissing",
            true,
            "TestMissing",
            true,
        ),
        (
            "equals",
            "check: go test ./internal/runner -run=TestMissing -count=1",
            true,
            "TestMissing",
            true,
        ),
        (
            "toolchain err",
            "check: go test ./internal/runner -run TestBroken",
            false,
            "TestBroken",
            true,
        ),
        (
            "bad regex",
            "check: go test ./internal/runner -run 'Test(' ",
            true,
            "",
            false,
        ),
        (
            "no -run",
            "check: go test ./internal/runner",
            false,
            "",
            false,
        ),
        (
            "no pkgs",
            "check: go test -run TestMissing",
            false,
            "",
            false,
        ),
        (
            "pkgs after flags",
            "check `go test -run TestMissing ./internal/runner` and Signal: x",
            true,
            "TestMissing",
            true,
        ),
        (
            "pkgs after count",
            "check: go test -run 'TestMissing|Test B' -count=1 ./internal/runner.",
            true,
            "TestMissing|Test B",
            true,
        ),
        (
            "prose after flags",
            "check: go test -run TestMissing then Signal: ./internal/runner",
            false,
            "",
            false,
        ),
    ];
    for (name, text, want, want_regex, want_pkg) in cases {
        *LIST_TEST_CALL.lock().expect("list test call poisoned") = (Vec::new(), String::new());
        assert_eq!(
            crate::gate::run_check_is_no_op("/repo", text, ""),
            want,
            "{name}"
        );
        let (got_pkgs, got_regex) = LIST_TEST_CALL
            .lock()
            .expect("list test call poisoned")
            .clone();
        assert_eq!(got_regex, want_regex, "{name}: regex");
        if want_pkg {
            assert_eq!(
                got_pkgs,
                vec!["./internal/runner".to_string()],
                "{name}: pkgs"
            );
        }
    }
    assert!(!crate::gate::run_check_is_no_op(
        "",
        "check: go test ./x -run TestMissing",
        ""
    ));
}

#[test]
fn card_file_gone_reads_the_file_line() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("crates/pulse-tally/src")).unwrap();
    fs::write(root.join("crates/pulse-tally/src/gate.rs"), "").unwrap();
    let repo = root.display().to_string();
    let gone = |body: &str| card_file_gone(&repo, body);
    assert!(gone("File: internal/tally/gate.go\nVariable: x"));
    assert!(gone(
        "- **File:** `internal/tally/gate.go`, internal/runner/exec.go"
    ));
    assert!(gone("Path: cmd/pulse/main.go"));
    assert!(!gone("File: crates/pulse-tally/src/gate.rs"), "exists");
    assert!(
        !gone("File: crates/pulse-tally/src/new.rs"),
        "new file, directory exists"
    );
    assert!(!gone("File: CHANGELOG.md"), "new top-level file");
    assert!(
        !gone("File: crates/pulse-new/src/lib.rs"),
        "new directory under a tree that exists"
    );
    assert!(
        !gone("File: internal/tally/gate.go, crates/pulse-tally/src/gate.rs"),
        "one named file exists"
    );
    assert!(
        !gone("File: the three git callers in the tally"),
        "a class, not a path"
    );
    assert!(!gone("File: internal/**/*.go"), "a glob");
    assert!(!gone("File: /etc/hosts"), "absolute");
    assert!(!gone("File: ../other/x.go"), "escapes the checkout");
    assert!(!gone("Variable: no file line"));
    assert!(!gone(""));
    assert!(
        !card_file_gone("", "File: internal/tally/gate.go"),
        "no checkout"
    );
}

#[test]
fn plan_gate_holds_no_op_run_check() {
    let _serial = crate::test_helpers::serial();
    let _guard = LIST_TEST_HOOK_LOCK
        .lock()
        .expect("list test hook lock poisoned");
    crate::gate::set_list_tests_hook(Some(empty_list_tests));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::gate::set_list_tests_hook(None);
        }
    }
    let _reset = Reset;
    let mut f = test_fixture::new_fixture(true);
    let id = test_fixture::add_eligible_suggestion(
        &mut f,
        "6",
        "runner-noop/model",
        "worker-noop",
        "Worker no-op check",
        "check: go test ./internal/runner -run TestGone. Signal: README.md",
        "",
        &["up", "up"],
    );
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "UPDATE agents SET kind = 'worker' WHERE name = 'runner-noop/model'",
            [],
        )
        .unwrap();
    }
    fs::write(f.repo.join("README.md"), "# r\n").unwrap();
    let res = compute(
        &f.st,
        "fixture",
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    )
    .unwrap();
    let sg = res.suggestions.iter().find(|sg| sg.id == id).unwrap();
    let post = res.posts.iter().find(|p| p.id == id).unwrap();
    assert_eq!(
        plan_gate(&f.st, &res.repo, sg, Some(post)),
        GATE_NO_OP_CHECK
    );
}

#[test]
fn signal_unlanded() {
    let _serial = crate::test_helpers::serial();
    let f = test_fixture::new_fixture(false);
    fs::create_dir_all(f.repo.join("internal/x")).unwrap();
    fs::write(
        f.repo.join("internal/x/landed.go"),
        "package x\n\nfunc Landed() {}\n",
    )
    .unwrap();
    git_in(&f.repo, &["init", "-q", "-b", "main"]);
    git_in(&f.repo, &["add", "."]);
    git_in(
        &f.repo,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "landed"],
    );
    git_in(&f.repo, &["checkout", "-q", "-b", "pulse/agent/job"]);
    fs::write(f.repo.join("internal/x/unlanded.go"), "package x\n").unwrap();
    fs::write(
        f.repo.join("internal/x/landed.go"),
        "package x\n\nfunc Landed() {}\n\nfunc Unlanded() {}\n",
    )
    .unwrap();
    git_in(&f.repo, &["add", "."]);
    git_in(
        &f.repo,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "unlanded",
        ],
    );
    reset_landed_cache();
    let repo = f.repo.display().to_string();
    for (body, want) in [
        ("Signal: internal/x/landed.go#Landed", false),
        ("Signal: internal/x/landed.go#Unlanded", true),
        ("Signal: internal/x/landed.go#Unlanded:5", true),
        ("Signal: internal/x/landed.go#Land", true),
        ("Signal: internal/x/landed.go", false),
        ("Signal: internal/x/unlanded.go", true),
        (
            "Signal: internal/x/unlanded.go\nSignal: internal/x/landed.go",
            false,
        ),
        ("Signal: check:go-test", false),
        ("Signal: internal/x/missing.go", false),
    ] {
        assert_eq!(
            crate::gate::signal_unlanded(&f.st, &repo, "card", body),
            want,
            "{body}"
        );
    }
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "UPDATE projects SET default_branch = '' WHERE slug = 'fixture'",
            [],
        )
        .unwrap();
    }
    assert!(!crate::gate::signal_unlanded(
        &f.st,
        &repo,
        "card",
        "Signal: internal/x/unlanded.go"
    ));
    assert!(!crate::gate::signal_unlanded(
        &f.st,
        tempfile::tempdir().unwrap().path().to_str().unwrap(),
        "card",
        "Signal: internal/x/unlanded.go"
    ));
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "UPDATE projects SET default_branch = 'no-such-branch' WHERE slug = 'fixture'",
            [],
        )
        .unwrap();
    }
    reset_landed_cache();
    assert!(!crate::gate::signal_unlanded(
        &f.st,
        &repo,
        "card",
        "Signal: internal/x/unlanded.go"
    ));
}

#[test]
fn signal_unlanded_cache_follows_landing() {
    let _serial = crate::test_helpers::serial();
    let f = test_fixture::new_fixture(false);
    fs::create_dir_all(f.repo.join("internal/x")).unwrap();
    fs::write(
        f.repo.join("internal/x/a.go"),
        "package x\n\nfunc Landed() {}\n",
    )
    .unwrap();
    git_in(&f.repo, &["init", "-q", "-b", "main"]);
    git_in(&f.repo, &["add", "."]);
    git_in(
        &f.repo,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "base"],
    );
    fs::write(f.repo.join("internal/x/b.go"), "package x\n").unwrap();
    reset_landed_cache();
    let repo = f.repo.display().to_string();
    assert!(crate::gate::signal_unlanded(
        &f.st,
        &repo,
        "card",
        "Signal: internal/x/b.go"
    ));
    git_in(&f.repo, &["add", "."]);
    git_in(
        &f.repo,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "land b"],
    );
    crate::gate::expire_landed_head_cache_for_test();
    assert!(!crate::gate::signal_unlanded(
        &f.st,
        &repo,
        "card",
        "Signal: internal/x/b.go"
    ));
}

#[test]
fn plan_gate_holds_unlanded_signal() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(true);
    fs::create_dir_all(f.repo.join("internal/x")).unwrap();
    fs::write(f.repo.join("internal/x/landed.go"), "package x\n").unwrap();
    git_in(&f.repo, &["init", "-q", "-b", "main"]);
    git_in(&f.repo, &["add", "."]);
    git_in(
        &f.repo,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "landed"],
    );
    git_in(&f.repo, &["checkout", "-q", "-b", "pulse/agent/job"]);
    fs::write(f.repo.join("internal/x/unlanded.go"), "package x\n").unwrap();
    git_in(&f.repo, &["add", "."]);
    git_in(
        &f.repo,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "unlanded",
        ],
    );
    reset_landed_cache();
    let unlanded = test_fixture::add_eligible_suggestion(
        &mut f,
        "6",
        "runner-worker/model",
        "worker-unlanded",
        "Worker unlanded card",
        "Signal: internal/x/unlanded.go",
        "",
        &["up", "up"],
    );
    let landed = test_fixture::add_eligible_suggestion(
        &mut f,
        "7",
        "runner-worker/model",
        "worker-landed",
        "Worker landed card",
        "Signal: internal/x/landed.go",
        "",
        &["up", "up"],
    );
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "UPDATE agents SET kind = 'worker' WHERE name = 'runner-worker/model'",
            [],
        )
        .unwrap();
    }
    let res = compute(
        &f.st,
        "fixture",
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    )
    .unwrap();
    let suggestions: BTreeMap<_, _> = res
        .suggestions
        .iter()
        .map(|sg| (sg.id.as_str(), sg.clone()))
        .collect();
    let posts: BTreeMap<_, _> = res.posts.iter().map(|p| (p.id.as_str(), p)).collect();
    for (id, want) in [(unlanded.as_str(), GATE_UNLANDED), (landed.as_str(), "")] {
        assert_eq!(
            plan_gate(&f.st, &res.repo, &suggestions[id], Some(posts[id])),
            want,
            "{id}"
        );
    }
}

#[test]
fn json_and_hash_inputs() {
    let _serial = crate::test_helpers::serial();
    let f = test_fixture::new_fixture(true);
    for cmd in ["board", "queue", "ballot", "proposals", "tally"] {
        let out = test_fixture::run_cli(
            &f.st,
            cmd,
            &["--project", "fixture", "--now", FIXTURE_NOW, "--json"],
        )
        .unwrap_or_else(|e| panic!("{cmd} --json: {e}"));
        let _: Value = serde_json::from_str(&out)
            .unwrap_or_else(|e| panic!("{cmd} --json is not JSON: {e}\n{out}"));
        if cmd == "tally" {
            assert!(out.contains(r#""stale_days""#), "{out}");
            assert!(out.contains(r#""suggestions""#), "{out}");
        }
    }
    let want = test_fixture::run_cli(&f.st, "hash", &[&f.ids["amendment"]]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("amend.md");
    fs::write(&path, FIXTURE_AMENDMENT_BODY).unwrap();
    let got = test_fixture::run_cli(&f.st, "hash", &[path.to_str().unwrap()]).unwrap();
    assert_eq!(got, want);
    let err = test_fixture::run_cli(&f.st, "hash", &[&f.ids["eligible"]])
        .unwrap_err()
        .to_string();
    assert!(err.contains("Proposed text"), "{err}");
    assert!(
        test_fixture::run_cli(
            &f.st,
            "tally",
            &["--project", "fixture", "--amend-majority", "most"]
        )
        .is_err()
    );
    assert!(test_fixture::run_cli(&f.st, "board", &["--project", "nope"]).is_err());
}

#[test]
fn post_for_owner_round_trip() {
    let _serial = crate::test_helpers::serial();
    for (name, extra, want) in [
        ("missing", "{}", false),
        ("true", r#"{"for_owner":true,"ticket":"T-12"}"#, true),
        ("false", r#"{"for_owner":false}"#, false),
        ("frontmatter true", r#"{"for_owner":"true"}"#, true),
        ("frontmatter false", r#"{"for_owner":"false"}"#, false),
    ] {
        let mut f = test_fixture::new_fixture(true);
        let mut p = test_fixture::FPost::new(
            "question",
            "routing",
            "owner-review",
            "runner-routing/model",
            FIXTURE_NOW,
            "Owner question",
            "",
        );
        p.extra = extra.into();
        let id = f.post(p);
        let p = load_post(&f.st, &id).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(p.for_owner, want, "{name}: loaded {p:?}");
        assert_eq!(p.extra, extra);
        let posts = posts_for(&f.st, "fixture", "routing").unwrap();
        assert_eq!(posts.len(), 1, "{name}: {posts:?}");
        assert_eq!(posts[0].for_owner, want, "{name}");
        let shown = show(&f.st, &id).unwrap();
        assert_eq!(
            shown.contains("\nfor_owner: true\n"),
            want,
            "{name}:\n{shown}"
        );
        assert_eq!(
            post_json(&p)
                .get("for_owner")
                .and_then(|v| v.as_str())
                .is_some_and(|v| v == "true"),
            want,
            "{name}"
        );
        let raw = serde_json::to_string(&p).unwrap();
        assert_eq!(raw.contains(r#""for_owner":true"#), want, "{name}: {raw}");
    }
}

#[test]
fn post_for_owner_invalid_metadata_surfaces_error() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(true);
    let mut p = test_fixture::FPost::new(
        "question",
        "routing",
        "review",
        "runner-routing/model",
        FIXTURE_NOW,
        "Choose the window",
        "",
    );
    p.extra = r#"{"for_owner":"maybe"}"#.into();
    let id = f.post(p);
    let err = load_post(&f.st, &id).unwrap_err().to_string();
    assert!(err.contains(&id), "{err}");
    let err = posts_for(&f.st, "fixture", "routing")
        .unwrap_err()
        .to_string();
    assert!(err.contains(&id), "{err}");
}

#[test]
fn queue_for_owner_metadata() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(true);
    let before = crate::queue::work_wanted(
        &f.st,
        None,
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    )
    .unwrap();
    let before_plannable = before
        .work
        .iter()
        .filter(|it| {
            it.blocked.is_empty()
                && matches!(
                    it.kind.as_str(),
                    WORK_HANDOFF | WORK_QUESTION | WORK_ELIGIBLE
                )
        })
        .count();
    let mut ids = BTreeMap::new();
    for kind in ["handoff", "question"] {
        let mut p = test_fixture::FPost::new(
            kind,
            "routing",
            kind,
            &format!("runner-{kind}/model"),
            FIXTURE_NOW,
            "Choose the window",
            "",
        );
        p.extra = r#"{"for_owner":true}"#.into();
        ids.insert(f.post(p), true);
    }
    let id = test_fixture::add_eligible_suggestion(
        &mut f,
        "2",
        "runner-routing/model",
        "support-window",
        "Choose the support window",
        "No signal needed.",
        r#"{"for_owner":true}"#,
        &["up", "up"],
    );
    ids.insert(id, true);
    let after = crate::queue::work_wanted(
        &f.st,
        None,
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    )
    .unwrap();
    let mut found = 0;
    for it in &after.work {
        if ids.contains_key(&it.id) {
            found += 1;
            assert_eq!(it.blocked, GATE_OWNER, "{} {:?}", it.kind, it);
        }
    }
    let after_plannable = after
        .work
        .iter()
        .filter(|it| {
            it.blocked.is_empty()
                && matches!(
                    it.kind.as_str(),
                    WORK_HANDOFF | WORK_QUESTION | WORK_ELIGIBLE
                )
        })
        .count();
    assert_eq!(found, 3);
    assert_eq!(before_plannable, after_plannable);
}

#[test]
fn project_goal_canonical_slug_board_and_ballot() {
    let _serial = crate::test_helpers::serial();
    unsafe {
        std::env::remove_var("PULSE_JOB");
        std::env::remove_var("PULSE_AGENT");
        std::env::remove_var("PULSE_SESSION");
    }
    let f = test_fixture::new_fixture(false);
    const SLUG: &str = "customer-support-ticket-management-syste";
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "INSERT INTO projects (slug,name,path,added_at) VALUES (?,?,?,?)",
            params![
                SLUG,
                "Customer support tickets",
                f.home.path().join("long-project").display().to_string(),
                FIXTURE_NOW
            ],
        )
        .unwrap();
    }
    let ro = test_fixture::open_read_only(f.home.path());
    for want in ["", "Improve useful ticket results using existing checks."] {
        if !want.is_empty() {
            pulse_goals::set(&f.st, SLUG, want, "human").unwrap();
        }
        let goal = pulse_goals::get(&ro, SLUG).unwrap();
        for command in ["board", "ballot"] {
            let text = test_fixture::run_cli(&ro, command, &["--project", SLUG]).unwrap();
            assert!(
                text.contains(&pulse_goals::context(&goal)),
                "{command}: {text}"
            );
            let text = test_fixture::run_cli(&ro, command, &["--project", SLUG, "--json"]).unwrap();
            let got: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                got.get("project").and_then(|v| v.as_str()),
                Some(SLUG),
                "{command}: {text}"
            );
            let got_goal: pulse_goals::Goal =
                serde_json::from_value(got.get("goal").cloned().unwrap()).unwrap();
            assert_eq!(got_goal, goal, "{command}: {text}");
        }
    }
}

#[test]
fn project_goal_board_and_ballot_read_surfaces() {
    let _serial = crate::test_helpers::serial();
    unsafe {
        std::env::remove_var("PULSE_JOB");
        std::env::remove_var("PULSE_AGENT");
        std::env::remove_var("PULSE_SESSION");
    }
    let f = test_fixture::new_fixture(false);
    let ro = test_fixture::open_read_only(f.home.path());
    let read_all = |goal: pulse_goals::Goal| {
        let before: i64 =
            f.st.conn()
                .unwrap()
                .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
                .unwrap();
        for command in ["board", "ballot"] {
            let text = test_fixture::run_cli(&ro, command, &["--project", "fixture"]).unwrap();
            assert!(
                text.contains(&pulse_goals::context(&goal)),
                "{command}: {text}"
            );
            assert!(text.contains("not permission"), "{command}: {text}");
            let text =
                test_fixture::run_cli(&ro, command, &["--project", "fixture", "--json"]).unwrap();
            let got: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(got.get("project").and_then(|v| v.as_str()), Some("fixture"));
            let got_goal: pulse_goals::Goal =
                serde_json::from_value(got.get("goal").cloned().unwrap()).unwrap();
            assert_eq!(got_goal, goal, "{command}: {text}");
        }
        let after: i64 =
            f.st.conn()
                .unwrap()
                .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
                .unwrap();
        assert_eq!(before, after);
    };
    let fallback = pulse_goals::get(&ro, "fixture").unwrap();
    read_all(fallback);
    let n: i64 =
        f.st.conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM settings WHERE name LIKE 'project.goal.%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(n, 0);
    let owner = pulse_goals::set(
        &f.st,
        "fixture",
        "Make product error handling reliable and useful with existing checks.",
        "human",
    )
    .unwrap();
    read_all(owner.clone());
    for command in ["board", "ballot"] {
        let text = test_fixture::run_cli(&ro, command, &["--project", "commons"]).unwrap();
        assert!(!text.contains(&owner.text), "{command}: {text}");
        assert!(!text.contains("## Project goal"), "{command}: {text}");
    }
}

#[test]
fn project_goal_board_read_propagates_storage_failure() {
    let _serial = crate::test_helpers::serial();
    let f = test_fixture::new_fixture(false);
    {
        let conn = f.st.conn().unwrap();
        conn.execute("DROP TABLE settings", []).unwrap();
    }
    let err = digest_for(&f.st, "fixture", "", Thresholds::default(), "", "")
        .unwrap_err()
        .to_string();
    assert!(!err.is_empty());
}

#[test]
fn open_handoffs() {
    let _serial = crate::test_helpers::serial();
    let res = TallyResult {
        threads: vec![
            Thread {
                key: "governance/left".into(),
                status: "open".into(),
                ..Default::default()
            },
            Thread {
                key: "governance/taken".into(),
                status: "open".into(),
                ..Default::default()
            },
            Thread {
                key: "governance/reopened".into(),
                status: "stale".into(),
                ..Default::default()
            },
            Thread {
                key: "governance/closed".into(),
                status: "closed".into(),
                ..Default::default()
            },
        ],
        posts: vec![
            Post {
                id: "h-taken".into(),
                area: "governance".into(),
                thread: "taken".into(),
                kind: "handoff".into(),
                date: "2026-09-01T01:00:00Z".into(),
                summary: "taken".into(),
                ..Default::default()
            },
            Post {
                id: "c-taken".into(),
                area: "governance".into(),
                thread: "taken".into(),
                kind: "claim".into(),
                date: "2026-09-02T01:00:00Z".into(),
                summary: "claimed".into(),
                ..Default::default()
            },
            Post {
                id: "h-left".into(),
                area: "governance".into(),
                thread: "left".into(),
                kind: "handoff".into(),
                date: "2026-09-01T00:00:00Z".into(),
                summary: "left".into(),
                ..Default::default()
            },
            Post {
                id: "c-before".into(),
                area: "governance".into(),
                thread: "reopened".into(),
                kind: "claim".into(),
                date: "2026-09-01T02:00:00Z".into(),
                summary: "old claim".into(),
                ..Default::default()
            },
            Post {
                id: "h-reopened".into(),
                area: "governance".into(),
                thread: "reopened".into(),
                kind: "handoff".into(),
                date: "2026-09-03T00:00:00Z".into(),
                summary: "new handoff".into(),
                ..Default::default()
            },
            Post {
                id: "h-closed".into(),
                area: "governance".into(),
                thread: "closed".into(),
                kind: "handoff".into(),
                date: "2026-09-04T00:00:00Z".into(),
                summary: "closed".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let ids: Vec<_> = crate::queue::open_handoffs(&res)
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids.join(","), "h-left,h-reopened");
}

#[test]
fn open_handoffs_for_reads_handoffs_and_claims_alone() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(true);
    let agent = "runner-h/model";
    // left: a handoff nobody claimed, with a later note that must not count as a claim.
    f.post(test_fixture::FPost::new(
        "handoff",
        "governance",
        "left",
        agent,
        "2026-09-01T10:00:00Z",
        "left for someone",
        "s-h1",
    ));
    f.post(test_fixture::FPost::new(
        "note",
        "governance",
        "left",
        agent,
        "2026-09-01T11:00:00Z",
        "a note after it",
        "s-h2",
    ));
    // taken: a handoff followed by a claim.
    f.post(test_fixture::FPost::new(
        "handoff",
        "governance",
        "taken",
        agent,
        "2026-09-01T12:00:00Z",
        "taken soon",
        "s-h3",
    ));
    f.post(test_fixture::FPost::new(
        "claim",
        "governance",
        "taken",
        "runner-c/model",
        "2026-09-01T13:00:00Z",
        "on it",
        "s-h4",
    ));
    let direct: Vec<_> = crate::queue::open_handoffs_for(&f.st, "fixture")
        .expect("open handoffs")
        .into_iter()
        .map(|p| p.id)
        .collect();
    let full = compute(&f.st, "fixture", "", Thresholds::default()).expect("tally");
    let reference: Vec<_> = crate::queue::open_handoffs(&full)
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(direct, reference);
    assert!(
        direct.contains(&test_fixture::fixture_post_id(
            "2026-09-01T10:00:00Z",
            agent
        )),
        "the unclaimed handoff is open: {direct:?}"
    );
    assert!(!direct.contains(&test_fixture::fixture_post_id(
        "2026-09-01T12:00:00Z",
        agent
    )));
    let kinds: std::collections::BTreeSet<_> =
        crate::tally::posts_of_kinds(&f.st, "fixture", &["handoff", "claim"])
            .expect("posts of kinds")
            .into_iter()
            .map(|p| p.kind)
            .collect();
    assert!(
        kinds.iter().all(|k| k == "handoff" || k == "claim"),
        "{kinds:?}"
    );
    assert!(
        crate::tally::posts_of_kinds(&f.st, "fixture", &[])
            .expect("none")
            .is_empty()
    );
}

#[test]
fn owner_wording_does_not_set_plan_gate() {
    let _serial = crate::test_helpers::serial();
    let f = test_fixture::new_fixture(true);
    for (name, summary, thread) in [
        ("owner questions", "anything", "owner-questions"),
        ("owner question", "anything", "owner-question"),
        ("owner prefix", "anything", "owner-review"),
        (
            "evaluator prefix",
            " evaluator proposal for the owner: adopt it",
            "work",
        ),
        ("release candidate", "Release candidate 1", "work"),
        ("owner question summary", "Owner question: pick one", "work"),
        ("vote requested prefix", "vote requested: choose", "work"),
        (
            "for owner contains",
            "ready for the owner to inspect",
            "work",
        ),
        (
            "vote requested contains",
            "please vote requested today",
            "work",
        ),
        ("worker", "implement the cache", "ownerish"),
    ] {
        let sg = Suggestion {
            score: 1,
            summary: summary.into(),
            key: format!("routing/{thread}"),
            ..Default::default()
        };
        let mut p = Post {
            summary: summary.into(),
            thread: thread.into(),
            ..Default::default()
        };
        assert_eq!(
            plan_gate(&f.st, &f.repo.display().to_string(), &sg, Some(&p)),
            "",
            "{name}"
        );
        assert_eq!(
            plan_gate(&f.st, &f.repo.display().to_string(), &sg, None),
            "",
            "{name}"
        );
        p.for_owner = true;
        assert_eq!(
            plan_gate(&f.st, &f.repo.display().to_string(), &sg, Some(&p)),
            GATE_OWNER,
            "{name}"
        );
    }
}

#[test]
fn work_wanted() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(true);
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "INSERT INTO projects (slug, name, path, added_at) VALUES ('other', 'Other', ?, ?)",
            params![
                f.home.path().join("other").display().to_string(),
                FIXTURE_NOW
            ],
        )
        .unwrap();
    }
    let mut post = |mut p: test_fixture::FPost| {
        p.project = "other".into();
        f.post(p)
    };
    let h = post(test_fixture::FPost::new(
        "handoff",
        "governance",
        "left",
        "r/h",
        "2026-09-01T00:00:00Z",
        "finish the export",
        "s",
    ));
    post(test_fixture::FPost::new(
        "handoff",
        "governance",
        "taken",
        "r/h",
        "2026-09-01T01:00:00Z",
        "taken over",
        "s",
    ));
    let mut p = test_fixture::FPost::new(
        "claim",
        "governance",
        "taken",
        "r/c",
        "2026-09-02T00:00:00Z",
        "on it",
        "s",
    );
    p.expires = "2026-09-30T00:00:00Z".into();
    post(p);
    post(test_fixture::FPost::new(
        "",
        "governance",
        "odd",
        "r/w",
        "2026-09-02T00:00:00Z",
        "no kind",
        "s",
    ));
    post(test_fixture::FPost::new(
        "question",
        "governance",
        "why",
        "r/q",
        "2026-08-15T00:00:00Z",
        "why is the export slow?",
        "s",
    ));
    let th = Thresholds {
        now: FIXTURE_NOW.into(),
        ..Default::default()
    };
    let q = crate::queue::work_wanted(&f.st, None, "", th.clone()).unwrap();
    assert_eq!(q.projects.len(), 2, "projects: {:?}", q.projects);
    assert_eq!(q.projects[0].slug, "fixture");
    assert!(q.projects[0].brain);
    assert_eq!(q.projects[1].slug, "other");
    assert_eq!(q.for_the_human.len(), 1, "for human: {:?}", q.for_the_human);
    assert_eq!(q.for_the_human[0].project, "fixture");
    let kinds: Vec<_> = q
        .work
        .iter()
        .map(|it| format!("{}@{}", it.kind, it.project))
        .collect();
    assert_eq!(
        kinds.join(","),
        "handoff@other,eligible@fixture,question@fixture,question@other,stale-claim@fixture,hygiene@other"
    );
    assert_eq!(q.work[0].id, h);
    assert_eq!(q.work[0].age_days, 9);
    assert_eq!(q.work[1].id, f.ids["eligible"]);
    assert_eq!(q.work[1].score, 1);
    assert!(
        q.work[2].age_days > q.work[3].age_days,
        "question order: {:?}",
        q.work
    );
    assert_eq!(q.work[3].thread, "governance/why");
    let last = q.work.last().unwrap();
    assert_eq!(last.kind, WORK_HYGIENE);
    assert!(last.summary.contains("1 post(s) without a kind"));
    assert_eq!(last.age_days, -1);

    let q = crate::queue::work_wanted(&f.st, None, "runner-1/model-1", th.clone()).unwrap();
    let mut ballots = 0;
    for it in &q.work {
        if it.kind == WORK_BALLOT {
            ballots += 1;
            assert_ne!(it.id, f.ids["amendment"]);
        }
    }
    assert_eq!(ballots, 2);

    {
        let conn = f.st.conn().unwrap();
        conn.execute("UPDATE settings SET value = '1' WHERE name = 'stop'", [])
            .unwrap();
    }
    let q = crate::queue::work_wanted(&f.st, Some(vec!["fixture".into()]), "", th.clone()).unwrap();
    assert!(!q.work.iter().any(|it| it.kind == WORK_ELIGIBLE));
    assert!(q.projects[0].stop);
    assert!(work_text(&q, 0).contains("STOP is on"));
    {
        let conn = f.st.conn().unwrap();
        conn.execute("UPDATE settings SET value = '0' WHERE name = 'stop'", [])
            .unwrap();
    }

    let out =
        test_fixture::run_cli(&f.st, "queue", &["--now", FIXTURE_NOW, "--limit", "2"]).unwrap();
    assert!(out.starts_with(&format!(
        "QUEUE - work wanted across 2 projects - {FIXTURE_NOW}"
    )));
    assert!(out.contains("showing 2"));
    assert!(out.contains("fixture *"));
    let out = test_fixture::run_cli(&f.st, "queue", &["--now", FIXTURE_NOW, "--json"]).unwrap();
    let parsed: Queue = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed.work.len(), 6);
    assert_eq!(parsed.work[0].kind, "handoff");
    let out = test_fixture::run_cli(
        &f.st,
        "queue",
        &["--now", FIXTURE_NOW, "--project", "fixture"],
    )
    .unwrap();
    assert!(!out.contains("QUEUE -"));
    assert!(out.contains("eligible"));
    let q = crate::queue::work_wanted(&f.st, Some(vec![]), "", th).unwrap();
    assert!(q.work.is_empty());
    assert!(work_text(&q, 0).contains("none - nothing on any board"));
}

#[test]
fn work_wanted_blocked_eligible_and_plannable() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(true);
    let zero = f.post(test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "split",
        "runner-z/model-z",
        "2026-09-01T08:00:00Z",
        "Split the worker queue",
        "s-z",
    ));
    for (agent, date, vote) in [
        ("runner-e/model-e", "2026-09-02T08:00:00Z", "up"),
        ("runner-f/model-f", "2026-09-02T09:00:00Z", "down"),
    ] {
        let mut p = test_fixture::FPost::new("vote", "evolution", "split", agent, date, vote, "s");
        p.on = zero.clone();
        p.vote = vote.into();
        f.post(p);
    }
    let mut p = test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "owner-question",
        "runner-o/model-o",
        "2026-09-01T07:00:00Z",
        "Pick the release owner",
        "s-o",
    );
    p.extra = r#"{"for_owner":true}"#.into();
    let owner = f.post(p);
    for (agent, date) in [
        ("runner-g/model-g", "2026-09-02T07:00:00Z"),
        ("runner-h/model-h", "2026-09-02T07:30:00Z"),
    ] {
        let mut p = test_fixture::FPost::new(
            "vote",
            "evolution",
            "owner-question",
            agent,
            date,
            "up",
            "s",
        );
        p.on = owner.clone();
        p.vote = "up".into();
        f.post(p);
    }
    let q = crate::queue::work_wanted(
        &f.st,
        Some(vec!["fixture".into()]),
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(q.projects.len(), 1);
    assert_eq!(q.projects[0].plannable, 2, "projects: {:?}", q.projects);
    let eligible: Vec<_> = q
        .work
        .iter()
        .filter(|it| it.kind == WORK_ELIGIBLE)
        .cloned()
        .collect();
    let by_id: BTreeMap<_, _> = eligible.iter().map(|it| (it.id.as_str(), it)).collect();
    assert_eq!(eligible.len(), 2, "eligible items: {eligible:?}");
    assert_eq!(eligible[0].id, f.ids["eligible"]);
    assert_eq!(eligible[0].blocked, "");
    assert!(!by_id.contains_key(zero.as_str()));
    assert_eq!(by_id[owner.as_str()].blocked, GATE_OWNER);
    let text = work_text(&q, 0);
    assert!(text.contains("PROJECTS (open suggestions, work items, plannable, kill switch)"));
    assert!(text.contains(" 2 plannable"), "{text}");
}

#[test]
fn work_wanted_uses_plan_gate_for_in_flight_and_worker_evidence() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(true);
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "INSERT INTO jobs (id, project, kind, title, brief, source, status, created_at) VALUES ('J-queued', 'fixture', 'improve', 'queued suggestion', '', ?, 'queued', ?)",
            params![format!("suggestion:{}", f.ids["eligible"]), FIXTURE_NOW],
        )
        .unwrap();
    }
    let mut p = test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "worker-card",
        "runner-worker/model",
        "2026-09-01T06:00:00Z",
        "Worker card without signal",
        "s-worker",
    );
    p.body = "No Signal line.".into();
    let worker = f.post(p);
    for (agent, date) in [
        ("runner-wv1/model", "2026-09-02T06:00:00Z"),
        ("runner-wv2/model", "2026-09-02T07:00:00Z"),
    ] {
        let mut p =
            test_fixture::FPost::new("vote", "evolution", "worker-card", agent, date, "up", "s");
        p.on = worker.clone();
        p.vote = "up".into();
        f.post(p);
    }
    {
        let conn = f.st.conn().unwrap();
        conn.execute(
            "UPDATE agents SET kind = 'worker' WHERE name = 'runner-worker/model'",
            [],
        )
        .unwrap();
    }
    let q = crate::queue::work_wanted(
        &f.st,
        Some(vec!["fixture".into()]),
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    )
    .unwrap();
    let eligible: Vec<_> = q
        .work
        .iter()
        .filter(|it| it.kind == WORK_ELIGIBLE)
        .cloned()
        .collect();
    let by_id: BTreeMap<_, _> = eligible.iter().map(|it| (it.id.as_str(), it)).collect();
    assert_eq!(
        by_id[f.ids["eligible"].as_str()].blocked,
        "queued as job J-queued"
    );
    assert_eq!(
        by_id[f.ids["eligible"].as_str()].next,
        "already queued: wait for job J-queued"
    );
    assert_eq!(by_id[worker.as_str()].blocked, GATE_NO_EVIDENCE);
    let eligible_plannable = eligible
        .iter()
        .filter(|it| {
            it.blocked.is_empty()
                && matches!(
                    it.kind.as_str(),
                    WORK_HANDOFF | WORK_QUESTION | WORK_ELIGIBLE
                )
        })
        .count();
    assert_eq!(eligible_plannable, 0, "eligible items {eligible:?}");
    assert_eq!(q.projects[0].plannable, 1, "project: {:?}", q.projects[0]);
}

#[test]
fn code_delivery_votes_reject_incomplete_iteration() {
    let _serial = crate::test_helpers::serial();
    for (name, want_cancelled) in [("row-error", false), ("cancelled-at-eof", true)] {
        let f = test_fixture::new_fixture(true);
        let source = "source";
        let ctx = pulse_store::CancellationToken::new();
        if want_cancelled {
            ctx.cancel();
        } else {
            f.st.conn()
                .unwrap()
                .execute("DROP TABLE posts", [])
                .unwrap();
        }
        let err = suggestion_vote_counts_context(&ctx, &f.st, "demo", source)
            .unwrap_err()
            .to_string();
        assert!(
            if want_cancelled {
                err.contains("context canceled")
            } else {
                err.contains("no such table") || err.contains("cancel")
            },
            "{name}: {err}"
        );
    }
    let f = test_fixture::new_fixture(true);
    let source = &f.ids["eligible"];
    {
        let conn = f.st.conn().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        for i in 0..10_000 {
            let id = format!("bulk-vote-{i:05}");
            let agent = format!("bulk/{i:05}");
            let date = format!("2026-09-04T{:02}:{:02}:00Z", (i / 60) % 24, i % 60);
            tx.execute(
                "INSERT INTO posts (id, project, area, thread, kind, agent, runner, model, session, date, branch, on_post, vote, summary, body, links, extra, origin, created_at)
                 VALUES (?, 'fixture', 'evolution', 'speedup', 'vote', ?, 'bulk', ?, ?, ?, 'main', ?, 'up', 'up', '\n## vote\n\nup\n', '[]', '{}', 'store', ?)",
                params![id, agent, format!("{i:05}"), format!("s-{i:05}"), date, source, date],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let err = suggestion_vote_counts(&f.st, "fixture", source)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("suggestion vote evidence exceeds the bounded scan"),
        "{err}"
    );
    assert_eq!(
        suggestion_vote_counts(&f.st, "fixture", "missing").unwrap(),
        (0, 0, false)
    );
}

#[test]
fn load_thresholds() {
    let _serial = crate::test_helpers::serial();
    let home = tempfile::tempdir().unwrap();
    pulse_store::Store::create(home.path().join("pulse.sqlite")).unwrap();
    let st = pulse_store::Store::open(pulse_store::Options {
        home: home.path().display().to_string(),
        create: false,
        read_only: false,
        skip_records: false,
        now: FIXTURE_NOW.into(),
    })
    .unwrap();
    {
        let conn = st.conn().unwrap();
        conn.execute(
            "INSERT INTO projects (slug, name, path, added_at) VALUES ('p', 'P', ?, ?)",
            params![home.path().join("p").display().to_string(), FIXTURE_NOW],
        )
        .unwrap();
    }
    let th = crate::thresholds::load_thresholds(
        &st,
        "p",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    );
    assert_eq!(th.quorum, 2, "defaults: {th:?}");
    assert_eq!(th.expiry_days, 14);
    assert_eq!(th.stale_days, 30);
    assert_eq!(th.amend_quorum, 40);
    assert_eq!(th.amend_majority, "2/3");
    assert_eq!((th.amend_num, th.amend_den), (2, 3));
    assert_eq!(th.protected, PROTECTED_DEFAULT);
    assert!(!th.constitution_present);
    assert!(!th.stop_present);
    assert_eq!(th.now, FIXTURE_NOW);

    let th = crate::thresholds::load_thresholds(
        &st,
        "p",
        Thresholds {
            quorum: 5,
            expiry_days: 9,
            stale_days: 3,
            amend_quorum: 7,
            amend_majority: "3/4".into(),
            ..Default::default()
        },
    );
    assert_eq!(th.quorum, 5);
    assert_eq!(th.expiry_days, 9);
    assert_eq!(th.stale_days, 3);
    assert_eq!(th.amend_quorum, 7);
    assert_eq!((th.amend_num, th.amend_den), (3, 4));

    for (k, v) in [
        ("quorum", "3"),
        ("stale_days", "10"),
        ("amend_majority", "3/5"),
        ("stop", "1"),
    ] {
        let conn = st.conn().unwrap();
        conn.execute(
            "UPDATE settings SET value = ? WHERE name = ?",
            params![v, k],
        )
        .unwrap();
    }
    let th = crate::thresholds::load_thresholds(&st, "p", Thresholds::default());
    assert_eq!(th.quorum, 3);
    assert_eq!(th.stale_days, 10);
    assert_eq!(th.amend_majority, "3/5");
    assert_eq!(th.amend_num, 3);
    assert!(th.stop_present);
    {
        let conn = st.conn().unwrap();
        conn.execute(
            "UPDATE settings SET value = 'most' WHERE name = 'amend_majority'",
            [],
        )
        .unwrap();
    }
    let th = crate::thresholds::load_thresholds(&st, "p", Thresholds::default());
    assert_eq!(th.amend_majority, "2/3");
    assert_eq!((th.amend_num, th.amend_den), (2, 3));

    let rec = pulse_store::Record {
        id: "EVO-constitution-p".into(),
        project: "p".into(),
        kind: "evolution".into(),
        slug: "constitution".into(),
        title: "C".into(),
        status: "active".into(),
        date: "2026-09-01".into(),
        ..Default::default()
    };
    st.put_record(rec, BTreeMap::new(), FIXTURE_CONSTITUTION_BODY, "test", "")
        .unwrap();
    let th = crate::thresholds::load_thresholds(&st, "p", Thresholds::default());
    assert_eq!(th.amend_quorum, 1);
    assert_eq!(th.amend_majority, "2/3");
    assert!(th.constitution_present);
    assert_eq!(th.constitution, FIXTURE_CONSTITUTION_BODY);
    let th = crate::thresholds::load_thresholds(
        &st,
        "p",
        Thresholds {
            amend_quorum: 9,
            ..Default::default()
        },
    );
    assert_eq!(th.amend_quorum, 9);
    let th =
        crate::thresholds::load_thresholds(&st, pulse_store::COMMONS_SLUG, Thresholds::default());
    assert!(!th.constitution_present, "commons: {th:?}");
}

#[test]
fn suggestion_quorum_decides_and_ballot_caps() {
    let _serial = crate::test_helpers::serial();
    let mut f = test_fixture::new_fixture(true);
    let rejected = f.post(test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "reject",
        "runner-r/model-r",
        "2026-09-01T05:00:00Z",
        "Rejected card",
        "s-r",
    ));
    let mut p = test_fixture::FPost::new(
        "vote",
        "evolution",
        "reject",
        "runner-rv1/model",
        "2026-09-02T05:00:00Z",
        "down",
        "s-rv1",
    );
    p.on = rejected.clone();
    p.vote = "down".into();
    f.post(p);
    let mut p = test_fixture::FPost::new(
        "vote",
        "evolution",
        "reject",
        "runner-rv2/model",
        "2026-09-02T06:00:00Z",
        "down",
        "s-rv2",
    );
    p.on = rejected.clone();
    p.vote = "down".into();
    f.post(p);
    let tie = f.post(test_fixture::FPost::new(
        "suggestion",
        "evolution",
        "tie",
        "runner-t/model-t",
        "2026-09-01T04:00:00Z",
        "Tied card",
        "s-t",
    ));
    let mut p = test_fixture::FPost::new(
        "vote",
        "evolution",
        "tie",
        "runner-tv1/model",
        "2026-09-02T04:00:00Z",
        "up",
        "s-tv1",
    );
    p.on = tie.clone();
    p.vote = "up".into();
    f.post(p);
    let mut p = test_fixture::FPost::new(
        "vote",
        "evolution",
        "tie",
        "runner-tv2/model",
        "2026-09-02T04:30:00Z",
        "down",
        "s-tv2",
    );
    p.on = tie.clone();
    p.vote = "down".into();
    f.post(p);

    let res = compute(
        &f.st,
        "fixture",
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    )
    .unwrap();
    let status: BTreeMap<_, _> = res
        .suggestions
        .iter()
        .map(|sg| (sg.id.as_str(), sg.status.as_str()))
        .collect();
    assert_eq!(status[f.ids["eligible"].as_str()], "eligible");
    assert_eq!(status[rejected.as_str()], "rejected");
    assert_eq!(status[tie.as_str()], "needs votes");

    let items = res.ballot("", "");
    let got: BTreeMap<_, _> = items
        .iter()
        .filter_map(|it| {
            it.suggestion
                .as_ref()
                .map(|sg| (sg.id.as_str(), sg.status.as_str()))
        })
        .collect();
    assert_eq!(got[tie.as_str()], "needs votes");
    assert_eq!(got[f.ids["needs_votes"].as_str()], "needs votes");
    assert!(!got.contains_key(f.ids["eligible"].as_str()));
    assert!(!got.contains_key(rejected.as_str()));

    f.st.set_setting("test", "lead.ballot_cap", "1").unwrap();
    let capped = ballot(
        &f.st,
        "fixture",
        "",
        "",
        Thresholds {
            now: FIXTURE_NOW.into(),
            ..Default::default()
        },
    )
    .unwrap();
    let sugs: Vec<_> = capped
        .iter()
        .filter_map(|it| it.suggestion.as_ref())
        .collect();
    assert_eq!(sugs.len(), 1, "capped ballot suggestions = {sugs:?}");
    assert_eq!(sugs[0].id, tie);
}

#[test]
fn board_vote_summary_rendering() {
    let _serial = crate::test_helpers::serial();
    fn run_board_capture(
        st: &pulse_store::Store,
        project: &str,
        area: &str,
        json: bool,
    ) -> (String, String) {
        crate::register();
        let cmd = pulse_cli::lookup("board").unwrap();
        let out = pulse_cli::Buffer::new();
        let err = pulse_cli::Buffer::new();
        let mut c = pulse_cli::Context {
            store: Some(st.clone()),
            home: st.home.clone(),
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            json,
            quiet: false,
            color: false,
            agent: String::new(),
            project: project.into(),
            session: String::new(),
            now: FIXTURE_NOW.into(),
            args: Vec::new(),
        };
        let mut fs = pulse_cli::FlagSet::new("board");
        if let Some(flags) = cmd.flags {
            flags(&mut fs);
        }
        fs.set("area", area).unwrap();
        (cmd.run)(&mut c, &fs, &[]).unwrap();
        (out.string(), err.string())
    }
    for (name, kind, vote, summary, json, all_areas, warnings) in [
        (
            "explained up",
            "vote",
            "up",
            "up: fewer retries",
            false,
            false,
            0,
        ),
        (
            "explained down",
            "vote",
            "down",
            "down: no measured improvement",
            false,
            false,
            0,
        ),
        ("bare up", "vote", "up", "up", false, false, 1),
        ("bare down", "vote", "down", "down", false, false, 1),
        (
            "padded mixed case",
            "vote",
            "down",
            " \tDoWn \t",
            false,
            false,
            1,
        ),
        ("all areas", "vote", "up", "up", false, true, 2),
        ("one word reason", "vote", "up", "Agreed", false, false, 0),
        ("non-vote up", "finding", "", "up", false, false, 0),
        ("non-vote down", "finding", "", "down", false, false, 0),
        ("JSON bare vote", "vote", "up", "up", true, false, 1),
    ] {
        let home = tempfile::tempdir().unwrap();
        pulse_store::Store::create(home.path().join("pulse.sqlite")).unwrap();
        let st = pulse_store::Store::open(pulse_store::Options {
            home: home.path().display().to_string(),
            create: false,
            read_only: false,
            skip_records: false,
            now: FIXTURE_NOW.into(),
        })
        .unwrap();
        {
            let conn = st.conn().unwrap();
            for project in ["fixture", "other"] {
                conn.execute(
                    "INSERT INTO projects (slug, name, path, added_at) VALUES (?, ?, ?, ?)",
                    params![
                        project,
                        project,
                        home.path().join(project).display().to_string(),
                        FIXTURE_NOW
                    ],
                )
                .unwrap();
            }
        }
        let mut f = test_fixture::Fixture {
            home,
            repo: PathBuf::new(),
            st,
            ids: BTreeMap::new(),
            hash: String::new(),
        };
        let suggestion = f.post(test_fixture::FPost::new(
            "suggestion",
            "governance",
            "vote-reasons",
            "test/proposer",
            "2026-09-09T12:00:00Z",
            "Show why agents voted.",
            "s-proposer",
        ));
        let mut p = test_fixture::FPost::new(
            kind,
            "governance",
            "vote-reasons",
            "test/voter",
            FIXTURE_NOW,
            summary,
            "s-voter",
        );
        p.body = "Body detail is not the displayed reason.".into();
        p.on = suggestion.clone();
        p.vote = vote.into();
        let id = f.post(p);
        for (project, area) in [("fixture", "outside"), ("other", "governance")] {
            let mut p = test_fixture::FPost::new(
                "suggestion",
                area,
                "outside",
                &format!("{project}/proposer"),
                "2026-09-09T12:00:00Z",
                "Another suggestion.",
                "",
            );
            p.project = project.into();
            let target = f.post(p);
            let mut p = test_fixture::FPost::new(
                "vote",
                area,
                "outside",
                &format!("{project}/voter"),
                FIXTURE_NOW,
                "up",
                "",
            );
            p.project = project.into();
            p.on = target;
            p.vote = "up".into();
            f.post(p);
        }
        let area = if all_areas { "" } else { "governance" };
        let (out, diagnostics) = run_board_capture(&f.st, "fixture", area, json);
        let want_warning = if warnings > 0 {
            format!(
                "warning: {warnings} vote post(s) have a bare up/down summary; include an explanatory phrase in the vote summary\n"
            )
        } else {
            String::new()
        };
        assert_eq!(diagnostics, want_warning, "{name}");
        if json {
            let result: TallyResult = serde_json::from_str(&out).unwrap();
            let post = result.posts.iter().find(|p| p.id == id).unwrap();
            assert_eq!(post.summary, summary, "{name}");
            continue;
        }
        let want_text = digest_for(
            &f.st,
            "fixture",
            area,
            Thresholds {
                now: FIXTURE_NOW.into(),
                ..Default::default()
            },
            "",
            "",
        )
        .unwrap();
        assert_eq!(out, want_text, "{name}");
        assert!(
            !out.contains("Body detail is not the displayed reason."),
            "{name}"
        );
        let found = out.lines().any(|line| {
            line.contains("governance/vote-reasons")
                && line.contains("2 posts")
                && line.ends_with(summary)
        });
        assert!(
            found,
            "{name}: board text is missing vote-reasons thread\n{out}"
        );
    }
}
