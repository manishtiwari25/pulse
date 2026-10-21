//! Derived fixtures: evaluators the loop reads off the project's own tree,
//! so a project never runs on request fixtures alone. The starter set tests
//! the harness's conduct (hard stops, injection, tiers) and ties on every
//! good change; the owner's hand-made metrics guard code health and move
//! only when the default branch does. Neither tells a variant that did its
//! work from one that did not. A derived fixture counts what voted changes
//! actually move - the tests a tree carries, the markers of work left
//! undone - with the metric scorer, deterministically, without a model.
//!
//! Derivation is idempotent: a fixture that exists is kept, a disabled one
//! stays the owner's choice, and a `higher` fixture whose raw count reached
//! its scale (so it could only tie from then on) is re-armed with a larger
//! scale and a fresh `at`, which the flat gate reads as a new fixture.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

pub const DERIVED_PREFIX: &str = "derived-";

/// Directories no stack's count looks into: dependencies, build output,
/// virtual environments, and the repository's own metadata.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    "vendor",
    ".venv",
    "venv",
    "__pycache__",
    "coverage",
];

/// One language stack: how a tree declares it, which files carry its code
/// and how it declares a test.
pub struct Stack {
    pub name: &'static str,
    pub markers: &'static [&'static str],
    pub includes: &'static [&'static str],
    pub test_pattern: &'static str,
}

pub static STACKS: &[Stack] = &[
    Stack {
        name: "rust",
        markers: &["Cargo.toml"],
        includes: &["*.rs"],
        test_pattern: r"^[[:space:]]*#\[([A-Za-z_]+::)*test(\]|\()",
    },
    Stack {
        name: "node",
        markers: &["package.json"],
        includes: &["*.ts", "*.tsx", "*.js", "*.jsx", "*.mjs", "*.cjs"],
        test_pattern: r"^[[:space:]]*(it|test|Deno\.test)(\.[a-z]+)*\(",
    },
    Stack {
        name: "python",
        markers: &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
        ],
        includes: &["*.py"],
        test_pattern: r"^[[:space:]]*(async[[:space:]]+)?def[[:space:]]+test_",
    },
    Stack {
        name: "go",
        markers: &["go.mod"],
        includes: &["*_test.go"],
        test_pattern: r"^func (Test|Example|Benchmark|Fuzz)[A-Z_]",
    },
];

/// The stacks a tree declares at its root or up to two directories down
/// (a workspace with `crates/*` and `web/hub`), in `STACKS` order.
pub fn detect_stacks(tree: &Path) -> Vec<&'static Stack> {
    let mut found = vec![false; STACKS.len()];
    let mut dirs = vec![(tree.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = dirs.pop() {
        for (i, stack) in STACKS.iter().enumerate() {
            if !found[i] && stack.markers.iter().any(|m| dir.join(m).is_file()) {
                found[i] = true;
            }
        }
        if depth >= 2 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() && !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                dirs.push((path, depth + 1));
            }
        }
    }
    STACKS
        .iter()
        .enumerate()
        .filter(|(i, _)| found[*i])
        .map(|(_, s)| s)
        .collect()
}

/// Which way a derived count should go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Higher,
    Lower,
}

/// One derived fixture: its name, direction, the shell script that prints
/// its raw count from the tree root, and what it measures.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedSpec {
    pub name: String,
    pub direction: Direction,
    pub script: String,
    pub about: String,
}

fn grep_scope(includes: &[&str]) -> String {
    let mut words: Vec<String> = includes
        .iter()
        .map(|glob| format!("--include=\"{glob}\""))
        .collect();
    words.extend(SKIP_DIRS.iter().map(|d| format!("--exclude-dir=\"{d}\"")));
    words.join(" ")
}

/// The derived fixtures for a set of stacks; none without a stack.
pub fn specs(stacks: &[&Stack]) -> Vec<DerivedSpec> {
    if stacks.is_empty() {
        return Vec::new();
    }
    let tests: Vec<String> = stacks
        .iter()
        .map(|s| {
            format!(
                "grep -rhIE \"{}\" {} .",
                s.test_pattern,
                grep_scope(s.includes)
            )
        })
        .collect();
    let mut includes: Vec<&str> = Vec::new();
    for s in stacks {
        for glob in s.includes {
            if !includes.contains(glob) {
                includes.push(glob);
            }
        }
    }
    let names: Vec<&str> = stacks.iter().map(|s| s.name).collect();
    vec![
        DerivedSpec {
            name: format!("{DERIVED_PREFIX}tests"),
            direction: Direction::Higher,
            script: format!("{{ {}; }} | wc -l", tests.join("; ")),
            about: format!(
                "test declarations in the tree ({}); a change that brings its tests scores higher, one that drops tests lower",
                names.join(", ")
            ),
        },
        DerivedSpec {
            name: format!("{DERIVED_PREFIX}todo-markers"),
            direction: Direction::Lower,
            script: format!(
                "grep -rhoIwE \"TODO|FIXME|XXX|HACK\" {} . | wc -l",
                grep_scope(&includes)
            ),
            about: format!(
                "TODO, FIXME, XXX and HACK markers in the code ({}); a change that settles one scores higher, one that leaves more lower",
                names.join(", ")
            ),
        },
    ]
}

/// The scale a fresh fixture gets from the tree's current count: a `higher`
/// fixture scores 0.5 now with as much room above as below; a `lower` one
/// scores 0.5 now and never saturates.
pub fn scale_for(direction: Direction, raw: f64) -> f64 {
    let raw = if raw.is_finite() {
        raw.max(0.0).round()
    } else {
        0.0
    };
    match direction {
        Direction::Higher => (raw * 2.0).max(20.0),
        Direction::Lower => raw.max(10.0),
    }
}

/// The scorer command a derived fixture stores.
pub fn scorer_command(spec: &DerivedSpec, scale: f64) -> String {
    let way = match spec.direction {
        Direction::Higher => "--higher",
        Direction::Lower => "--lower",
    };
    format!(
        "pulse evolve score metric {way} --scale {} -- sh -c '{}'",
        scale as i64, spec.script
    )
}

/// The scale a stored derived scorer carries.
pub fn scorer_scale(scorer: &str) -> Option<f64> {
    let rest = scorer.split("--scale ").nth(1)?;
    rest.split_whitespace().next()?.parse::<f64>().ok()
}

/// A derived fixture's raw count on a tree, read the way the scorer reads
/// it: the last number the script prints.
pub fn count(tree: &Path, script: &str) -> Result<f64> {
    let cancel = pulse_bus::CancellationToken::new().with_timeout(Duration::from_secs(60));
    let env = vec![
        format!(
            "PATH={}",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string())
        ),
        "LC_ALL=C".to_string(),
    ];
    let argv = vec!["sh".to_string(), "-c".to_string(), script.to_string()];
    let mut out = Vec::new();
    crate::process::run_process_to(
        &cancel,
        &tree.display().to_string(),
        &env,
        &argv,
        &mut out,
        &mut std::io::sink(),
    )?;
    crate::metric::last_number(&String::from_utf8_lossy(&out))
        .ok_or_else(|| anyhow!("the count printed no number"))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DeriveResult {
    pub stacks: Vec<String>,
    pub added: Vec<String>,
    pub rearmed: Vec<String>,
    pub notes: Vec<String>,
}

impl DeriveResult {
    pub fn changed(&self) -> bool {
        !self.added.is_empty() || !self.rearmed.is_empty()
    }

    /// One line for a log or a post.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if !self.added.is_empty() {
            parts.push(format!("added {}", self.added.join(", ")));
        }
        if !self.rearmed.is_empty() {
            parts.push(format!("re-armed {}", self.rearmed.join(", ")));
        }
        if parts.is_empty() && (!self.stacks.is_empty() || self.notes.is_empty()) {
            parts.push(if self.stacks.is_empty() {
                "no known stack in the tree, nothing derived".to_string()
            } else {
                "derived fixtures already present".to_string()
            });
        }
        if !self.stacks.is_empty() {
            parts.push(format!("stacks: {}", self.stacks.join(", ")));
        }
        parts.extend(self.notes.iter().cloned());
        parts.join("; ")
    }
}

/// Derive the project's fixtures from its checkout: add the ones it lacks,
/// re-arm a saturated one, leave the owner's disabled ones alone.
pub fn derive_fixtures(s: &pulse_store::Store, project: &str, actor: &str) -> Result<DeriveResult> {
    let p = crate::fixtures::valid_project(s, project)?;
    let actor = if actor.is_empty() {
        pulse_store::HUMAN_AGENT
    } else {
        actor
    };
    let tree = Path::new(&p.path);
    let mut result = DeriveResult::default();
    if p.path.is_empty() || !tree.is_dir() {
        result
            .notes
            .push("the project's checkout is not on this machine, nothing derived".to_string());
        return Ok(result);
    }
    let stacks = detect_stacks(tree);
    result.stacks = stacks.iter().map(|s| s.name.to_string()).collect();
    let existing = crate::fixtures::list_fixtures(s, project, false)?;
    for spec in specs(&stacks) {
        let current = existing.iter().find(|f| f.name == spec.name);
        if let Some(f) = current
            && !f.enabled
        {
            result
                .notes
                .push(format!("{} stays disabled (the owner's choice)", f.name));
            continue;
        }
        let raw = match count(tree, &spec.script) {
            Ok(raw) => raw,
            Err(e) => {
                result.notes.push(format!(
                    "{} not derived: its count failed: {}",
                    spec.name,
                    crate::measure::first_line(&e.to_string(), 160)
                ));
                continue;
            }
        };
        match current {
            None => {
                let scale = scale_for(spec.direction, raw);
                crate::fixtures::add_fixture(
                    s,
                    crate::fixtures::FixtureInput {
                        project: project.to_string(),
                        name: spec.name.clone(),
                        request: String::new(),
                        scorer: scorer_command(&spec, scale),
                        kind: "public".to_string(),
                        metric: true,
                        actor: actor.to_string(),
                        weight: 1.0,
                        answer_keys: Vec::new(),
                    },
                )?;
                result
                    .added
                    .push(format!("{} ({} now)", spec.name, raw as i64));
            }
            Some(f) => {
                let scale = scorer_scale(&f.scorer).unwrap_or(0.0);
                let saturated = spec.direction == Direction::Higher && raw >= scale;
                let stacks_changed = scorer_command(&spec, scale) != f.scorer;
                if !saturated && !stacks_changed {
                    continue;
                }
                let scale = if saturated {
                    scale_for(spec.direction, raw)
                } else {
                    scale
                };
                crate::fixtures::rearm_fixture(
                    s,
                    project,
                    &spec.name,
                    &scorer_command(&spec, scale),
                    actor,
                )?;
                result.rearmed.push(format!(
                    "{} ({}; {} now, scale {})",
                    spec.name,
                    if saturated {
                        "its count reached its scale"
                    } else {
                        "the tree's stacks changed"
                    },
                    raw as i64,
                    scale as i64
                ));
            }
        }
    }
    if result.changed() {
        s.event(
            actor,
            "evolution.fixture.derived",
            project,
            serde_json::json!({
                "project": project,
                "stacks": result.stacks,
                "added": result.added,
                "rearmed": result.rearmed,
            }),
        )?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    fn write(tree: &Path, rel: &str, text: &str) {
        let path = tree.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn polyglot(tree: &Path) {
        write(tree, "Cargo.toml", "[package]\nname = \"demo\"\n");
        write(
            tree,
            "src/lib.rs",
            "// TODO: split\npub fn f() {}\n#[cfg(test)]\nmod t {\n    #[test]\n    fn a() {}\n    #[tokio::test(flavor = \"current_thread\")]\n    async fn b() {}\n    #[test] fn c() {}\n}\n",
        );
        write(tree, "web/hub/package.json", "{}\n");
        write(
            tree,
            "web/hub/app.test.ts",
            "it('x', () => {});\n  test.each([1])('y', () => {}); // FIXME later\ntest.skip('z', () => {});\ndescribe('d', () => {});\n",
        );
        write(tree, "tools/pyproject.toml", "[project]\n");
        write(
            tree,
            "tools/test_x.py",
            "def test_one():\n    pass\nasync def test_two():\n    pass\ndef helper(): pass  # XXX\n",
        );
        write(tree, "svc/go.mod", "module demo\n");
        write(
            tree,
            "svc/a_test.go",
            "package a\nfunc TestA(t *testing.T) {}\nfunc BenchmarkB(b *testing.B) {}\nfunc helper() {}\n",
        );
        // Dependencies and build output do not count.
        write(tree, "node_modules/dep/package.json", "{}\n");
        write(
            tree,
            "node_modules/dep/x.test.js",
            "it('n', () => {}); // TODO\n",
        );
        write(tree, "target/debug/gen.rs", "#[test] fn gen() {} // HACK\n");
        write(tree, "README.md", "TODO list for humans\n");
    }

    #[test]
    fn detects_stacks_up_to_two_directories_down_and_skips_dependencies() {
        let tree = temp_path();
        std::fs::create_dir_all(&tree).unwrap();
        assert!(detect_stacks(&tree).is_empty());
        polyglot(&tree);
        let names: Vec<&str> = detect_stacks(&tree).iter().map(|s| s.name).collect();
        assert_eq!(names, ["rust", "node", "python", "go"]);
        // Too deep to be the project's own stack.
        let deep = temp_path();
        write(&deep, "a/b/c/go.mod", "module deep\n");
        assert!(detect_stacks(&deep).is_empty());
        let vendored = temp_path();
        write(&vendored, "node_modules/x/package.json", "{}\n");
        assert!(detect_stacks(&vendored).is_empty());
    }

    #[test]
    fn counts_tests_and_markers_the_way_the_scorer_will() {
        let tree = temp_path();
        polyglot(&tree);
        let stacks = detect_stacks(&tree);
        let all = specs(&stacks);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "derived-tests");
        assert_eq!(all[0].direction, Direction::Higher);
        // 3 rust + 3 node + 2 python + 2 go; nothing from node_modules or target.
        assert_eq!(count(&tree, &all[0].script).unwrap(), 10.0);
        assert_eq!(all[1].name, "derived-todo-markers");
        assert_eq!(all[1].direction, Direction::Lower);
        // TODO in lib.rs, FIXME in app.test.ts, XXX in test_x.py; README is not code.
        assert_eq!(count(&tree, &all[1].script).unwrap(), 3.0);
        for spec in &all {
            assert!(!spec.script.contains('\''), "{}", spec.script);
            assert!(spec.script.contains("--exclude-dir=\".git\""));
        }
        let only_rust = specs(&detect_stacks(&tree)[..1]);
        assert!(only_rust[0].script.contains("*.rs") && !only_rust[0].script.contains("*.py"));
        assert!(specs(&[]).is_empty());
    }

    #[test]
    fn scales_and_scorer_commands_round_trip() {
        assert_eq!(scale_for(Direction::Higher, 0.0), 20.0);
        assert_eq!(scale_for(Direction::Higher, 1867.0), 3734.0);
        assert_eq!(scale_for(Direction::Lower, 3.0), 10.0);
        assert_eq!(scale_for(Direction::Lower, 48.0), 48.0);
        assert_eq!(scale_for(Direction::Higher, f64::NAN), 20.0);
        let spec = DerivedSpec {
            name: "derived-tests".into(),
            direction: Direction::Higher,
            script: "echo 7".into(),
            about: String::new(),
        };
        let scorer = scorer_command(&spec, 40.0);
        assert_eq!(
            scorer,
            "pulse evolve score metric --higher --scale 40 -- sh -c 'echo 7'"
        );
        assert_eq!(scorer_scale(&scorer), Some(40.0));
        assert_eq!(scorer_scale("pulse evolve score starter-x"), None);
        // The scorer scores what the count reads.
        assert_eq!(crate::metric::metric_score(7.0, true, 40.0, None), 0.175);
        assert_eq!(crate::metric::metric_score(1867.0, true, 3734.0, None), 0.5);
        assert_eq!(crate::metric::metric_score(10.0, false, 10.0, None), 0.5);
    }

    #[test]
    fn derive_adds_once_keeps_owner_choices_and_rearms_a_saturated_fixture() {
        let _serial = crate::test_helpers::serial();
        let s = test_store();
        let repo = test_project(&s, "demo");
        let tree = Path::new(&repo);
        // No stack: nothing derived, and that is said.
        let none = derive_fixtures(&s, "demo", "tester").unwrap();
        assert!(!none.changed());
        assert_eq!(none.line(), "no known stack in the tree, nothing derived");
        write(tree, "Cargo.toml", "[package]\nname = \"demo\"\n");
        write(
            tree,
            "src/lib.rs",
            "#[test]\nfn a() {}\n#[test]\nfn b() {}\n// TODO one\n",
        );
        let first = derive_fixtures(&s, "demo", "tester").unwrap();
        assert_eq!(first.stacks, ["rust"]);
        assert_eq!(
            first.added,
            ["derived-tests (2 now)", "derived-todo-markers (1 now)"]
        );
        assert_eq!(
            first.line(),
            "added derived-tests (2 now), derived-todo-markers (1 now); stacks: rust"
        );
        let fixtures = crate::fixtures::list_fixtures(&s, "demo", true).unwrap();
        let tests = fixtures.iter().find(|f| f.name == "derived-tests").unwrap();
        assert!(tests.metric && tests.enabled && tests.weight == 1.0);
        assert!(
            tests
                .scorer
                .starts_with("pulse evolve score metric --higher --scale 20 -- sh -c '"),
            "{}",
            tests.scorer
        );
        let todo = fixtures
            .iter()
            .find(|f| f.name == "derived-todo-markers")
            .unwrap();
        assert!(
            todo.scorer
                .starts_with("pulse evolve score metric --lower --scale 10 -- sh -c '"),
            "{}",
            todo.scorer
        );
        assert_eq!(events(&s, "evolution.fixture.derived"), 1);
        // Again: nothing to do.
        let again = derive_fixtures(&s, "demo", "tester").unwrap();
        assert!(!again.changed());
        assert_eq!(
            again.line(),
            "derived fixtures already present; stacks: rust"
        );
        // The count reaches the scale: the fixture could only tie from here,
        // so it is re-armed with room again and a fresh date.
        let many: String = (0..25)
            .map(|i| format!("#[test]\nfn t{i}() {{}}\n"))
            .collect();
        write(tree, "src/more.rs", &many);
        let before = tests.at.clone();
        s.conn()
            .unwrap()
            .execute(
                "UPDATE evolution_fixtures SET at = '2020-01-01T00:00:00Z' WHERE project = 'demo'",
                [],
            )
            .unwrap();
        let rearmed = derive_fixtures(&s, "demo", "tester").unwrap();
        assert_eq!(
            rearmed.rearmed,
            ["derived-tests (its count reached its scale; 27 now, scale 54)"]
        );
        let fixtures = crate::fixtures::list_fixtures(&s, "demo", true).unwrap();
        let tests = fixtures.iter().find(|f| f.name == "derived-tests").unwrap();
        assert!(tests.scorer.contains("--scale 54 --"), "{}", tests.scorer);
        assert!(tests.at.as_str() > "2020-01-01T00:00:00Z" && tests.at >= before);
        let todo = fixtures
            .iter()
            .find(|f| f.name == "derived-todo-markers")
            .unwrap();
        assert_eq!(
            todo.at, "2020-01-01T00:00:00Z",
            "a lower fixture never saturates"
        );
        // A disabled derived fixture is the owner's choice.
        crate::fixtures::set_fixture_enabled(&s, "demo", "derived-tests", false, "human").unwrap();
        write(tree, "src/even_more.rs", &many);
        let kept = derive_fixtures(&s, "demo", "tester").unwrap();
        assert!(!kept.changed());
        assert_eq!(
            kept.line(),
            "derived fixtures already present; stacks: rust; derived-tests stays disabled (the owner's choice)"
        );
        assert_eq!(events(&s, "evolution.fixture.rearmed"), 1);
    }

    fn events(s: &pulse_store::Store, action: &str) -> i64 {
        s.conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE action = ?",
                [action],
                |r| r.get(0),
            )
            .unwrap()
    }
}
