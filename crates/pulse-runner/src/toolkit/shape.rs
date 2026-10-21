//! What a checkout shows of itself, read once per look: the remote's host,
//! the files at its root, the packages its manifests depend on, and the
//! tools and skills it already has. Every suggestion reads this and
//! nothing else, so a look costs one directory listing and a few small
//! files.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub struct Shape {
    pub project: String,
    pub root: Option<PathBuf>,
    /// The host of the project's remote (github.com, gitlab.com), or empty.
    pub remote_host: String,
    /// Files and folders present, relative to the root, from `PROBE_FILES`,
    /// plus `*.tf` when a Terraform file sits at the root.
    pub files: BTreeSet<String>,
    /// Package names the manifests depend on, across ecosystems, lowercase.
    pub deps: BTreeSet<String>,
    /// The project's verify command, when one is on record.
    pub verify: String,
    /// The kind of user interface the project ships, when it shows one.
    pub ui: Option<String>,
    /// Whether the checkout has a tag reachable from HEAD.
    pub has_tag: bool,
    /// The last tag, when there is one.
    pub last_tag: String,
    /// Tool connections already registered for the project (own or shared),
    /// on or off: the owner decided about these.
    pub tools: BTreeSet<String>,
    /// Skills already reachable: the registry's (own or shared, on or off)
    /// and the clone's skill folders.
    pub skills: BTreeSet<String>,
    /// Every registered skill name, any project: names are unique across
    /// the registry, so a draft avoids one another project holds.
    pub skill_names_taken: BTreeSet<String>,
    /// Whether a compose file names a postgres image.
    pub compose_postgres: bool,
}

/// Files and folders a look probes for, relative to the root.
pub const PROBE_FILES: &[&str] = &[
    "package.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
    "bun.lock",
    "package-lock.json",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    "requirements.txt",
    "Pipfile",
    "uv.lock",
    "poetry.lock",
    "Gemfile",
    "composer.json",
    "pubspec.yaml",
    "mix.exs",
    "Dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
    ".github/workflows",
    ".gitlab-ci.yml",
    "prisma/schema.prisma",
    "supabase",
    "firebase.json",
    "vercel.json",
    "wrangler.toml",
    "wrangler.json",
    "wrangler.jsonc",
    "cdk.json",
    "serverless.yml",
    "template.yaml",
    "samconfig.toml",
    "terraform",
    "k8s",
    "kubernetes",
    "Chart.yaml",
    "charts",
    "helm",
    "migrations",
    "alembic",
    "db/migrate",
    "drizzle",
    "supabase/migrations",
    "prisma/migrations",
    "CHANGELOG.md",
    "CHANGELOG",
    "openapi.yaml",
    "openapi.yml",
    "openapi.json",
    "swagger.json",
    "swagger.yaml",
    "schema.graphql",
    "playwright.config.ts",
    "playwright.config.js",
    "playwright.config.mjs",
    "shopify.app.toml",
    "tests",
    "test",
    "spec",
    "__tests__",
    "e2e",
];

/// Where a clone's skill folders live, the layout Claude Code, Copilot and
/// the agents.md convention share (pulse-harness reads the same list).
const SKILL_DIRS: &[&str] = &[
    ".pulse/skills",
    ".claude/skills",
    ".github/skills",
    ".agents/skills",
];

impl Shape {
    pub fn has(&self, file: &str) -> bool {
        self.files.contains(file)
    }
    pub fn has_any(&self, files: &[&str]) -> bool {
        files.iter().any(|f| self.has(f))
    }
    pub fn dep(&self, name: &str) -> bool {
        self.deps.contains(&name.to_lowercase())
    }
    pub fn dep_any(&self, names: &[&str]) -> bool {
        names.iter().any(|n| self.dep(n))
    }
    /// A dependency whose name starts with `prefix` (`@sentry/`, `awslabs.`).
    pub fn dep_prefix(&self, prefix: &str) -> bool {
        let p = prefix.to_lowercase();
        self.deps.iter().any(|d| d.starts_with(&p))
    }
    /// The first present file of `files`, for a "why" that names it.
    pub fn first_of<'a>(&self, files: &[&'a str]) -> Option<&'a str> {
        files.iter().copied().find(|f| self.has(f))
    }
    /// The dependency manifests present at the root.
    pub fn manifests(&self) -> Vec<&str> {
        [
            "package.json",
            "Cargo.toml",
            "go.mod",
            "pyproject.toml",
            "requirements.txt",
            "Pipfile",
            "Gemfile",
            "composer.json",
            "pubspec.yaml",
            "mix.exs",
        ]
        .into_iter()
        .filter(|f| self.has(f))
        .collect()
    }
    /// The package manager the lockfiles point at, and its install command.
    pub fn package_manager(&self) -> Option<(&'static str, &'static str)> {
        if self.has("pnpm-lock.yaml") {
            Some(("pnpm", "pnpm install --frozen-lockfile"))
        } else if self.has("yarn.lock") {
            Some(("yarn", "yarn install --immutable"))
        } else if self.has_any(&["bun.lockb", "bun.lock"]) {
            Some(("bun", "bun install --frozen-lockfile"))
        } else if self.has("package.json") {
            Some(("npm", "npm ci"))
        } else if self.has("Cargo.toml") {
            Some(("cargo", "cargo build"))
        } else if self.has("go.mod") {
            Some(("go", "go mod download"))
        } else if self.has("uv.lock") {
            Some(("uv", "uv sync"))
        } else if self.has("poetry.lock") {
            Some(("poetry", "poetry install"))
        } else if self.has("Pipfile") {
            Some(("pipenv", "pipenv install --dev"))
        } else if self.has("pyproject.toml") {
            Some((
                "pip",
                "python -m venv .venv && . .venv/bin/activate && pip install -e '.[dev]'",
            ))
        } else if self.has("requirements.txt") {
            Some((
                "pip",
                "python -m venv .venv && . .venv/bin/activate && pip install -r requirements.txt",
            ))
        } else if self.has("Gemfile") {
            Some(("bundler", "bundle install"))
        } else if self.has("composer.json") {
            Some(("composer", "composer install"))
        } else if self.has("pubspec.yaml") {
            Some(("flutter", "flutter pub get"))
        } else if self.has("mix.exs") {
            Some(("mix", "mix deps.get"))
        } else {
            None
        }
    }
    /// The test folders present at the root.
    pub fn test_dirs(&self) -> Vec<&str> {
        ["tests", "test", "spec", "__tests__", "e2e"]
            .into_iter()
            .filter(|f| self.has(f))
            .collect()
    }
    /// The compose file present, when one is.
    pub fn compose_file(&self) -> Option<&str> {
        self.first_of(&[
            "compose.yaml",
            "compose.yml",
            "docker-compose.yml",
            "docker-compose.yaml",
        ])
    }
}

/// Reads the shape of one project: its row in the store, its checkout on
/// this machine and the tools and skills registered for it.
pub fn read(s: &pulse_store::Store, slug: &str) -> Shape {
    let mut shape = Shape {
        project: slug.to_string(),
        ..Default::default()
    };
    let Ok(Some(p)) = s.project(slug) else {
        return shape;
    };
    shape.verify = p.verify_cmd.clone();
    shape.remote_host = remote_host(&p.remote);
    shape.tools = s
        .mcp_servers(slug)
        .unwrap_or_default()
        .into_iter()
        .map(|m| m.name)
        .collect();
    shape.skills = s
        .skills(slug)
        .unwrap_or_default()
        .into_iter()
        .map(|k| k.name)
        .collect();
    shape.skill_names_taken = s
        .skills("")
        .unwrap_or_default()
        .into_iter()
        .map(|k| k.name)
        .collect();
    if p.path.is_empty() {
        return shape;
    }
    let root = PathBuf::from(&p.path);
    if !root.is_dir() {
        return shape;
    }
    read_checkout(&mut shape, &root);
    if shape.remote_host.is_empty() {
        let r = crate::util::git(&p.path, &["remote", "get-url", "origin"]);
        if r.ok() {
            shape.remote_host = remote_host(r.stdout.trim());
        }
    }
    let tag = crate::util::git(&p.path, &["describe", "--tags", "--abbrev=0"]);
    if tag.ok() && !tag.stdout.trim().is_empty() {
        shape.has_tag = true;
        shape.last_tag = tag.stdout.trim().to_string();
    }
    shape.ui = crate::lead_roles::ui_kind(s, slug);
    shape.root = Some(root);
    shape
}

/// The files, folders, dependencies and skill folders of a checkout.
pub fn read_checkout(shape: &mut Shape, root: &Path) {
    for f in PROBE_FILES {
        if root.join(f).exists() {
            shape.files.insert((*f).to_string());
        }
    }
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".tf") {
                shape.files.insert("*.tf".into());
            }
            if name.starts_with("requirements") && name.ends_with(".txt") {
                shape.deps.extend(requirements_deps(&read_small(&e.path())));
            }
        }
    }
    if shape.has("package.json") {
        shape
            .deps
            .extend(package_json_deps(&read_small(&root.join("package.json"))));
    }
    if shape.has("Cargo.toml") {
        shape
            .deps
            .extend(cargo_deps(&read_small(&root.join("Cargo.toml"))));
    }
    if shape.has("pyproject.toml") {
        shape
            .deps
            .extend(pyproject_deps(&read_small(&root.join("pyproject.toml"))));
    }
    if shape.has("Pipfile") {
        shape
            .deps
            .extend(cargo_deps(&read_small(&root.join("Pipfile"))));
    }
    if shape.has("go.mod") {
        shape
            .deps
            .extend(gomod_deps(&read_small(&root.join("go.mod"))));
    }
    if shape.has("Gemfile") {
        shape
            .deps
            .extend(gemfile_deps(&read_small(&root.join("Gemfile"))));
    }
    if shape.has("composer.json") {
        shape
            .deps
            .extend(composer_deps(&read_small(&root.join("composer.json"))));
    }
    if shape.has("pubspec.yaml") {
        shape
            .deps
            .extend(pubspec_deps(&read_small(&root.join("pubspec.yaml"))));
    }
    if let Some(f) = shape.compose_file() {
        shape.compose_postgres = read_small(&root.join(f))
            .to_lowercase()
            .contains("postgres");
    }
    for dir in SKILL_DIRS {
        if let Ok(rd) = std::fs::read_dir(root.join(dir)) {
            for e in rd.flatten() {
                if e.path().join("SKILL.md").is_file() {
                    shape
                        .skills
                        .insert(e.file_name().to_string_lossy().to_string());
                }
            }
        }
    }
}

/// At most 256 KB of a manifest; a bigger one is not a manifest.
fn read_small(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(m) if m.len() <= 256 * 1024 => std::fs::read_to_string(path).unwrap_or_default(),
        _ => String::new(),
    }
}

/// The host of a git remote: `git@github.com:o/r.git`, `https://gitlab.com/o/r`,
/// `ssh://git@bitbucket.org/o/r`; empty when the remote is a path or blank.
pub fn remote_host(remote: &str) -> String {
    let r = remote.trim();
    if r.is_empty() {
        return String::new();
    }
    let rest = if let Some((_, after)) = r.split_once("://") {
        after
    } else if let Some((before, _)) = r.split_once(':')
        && !before.contains('/')
    {
        // scp-like: [user@]host:path
        before
    } else {
        return String::new();
    };
    let host = rest.split(['/', ':']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    host.to_lowercase()
}

fn package_json_deps(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return out;
    };
    for key in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(m) = v.get(key).and_then(|d| d.as_object()) {
            out.extend(m.keys().map(|k| k.to_lowercase()));
        }
    }
    out
}

/// `[dependencies]`-style tables (Cargo.toml, Pipfile): the key before `=`
/// inside any table whose name ends with `dependencies` or `packages`.
fn cargo_deps(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_deps = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            let name = t.trim_matches(|c| c == '[' || c == ']').trim();
            // `[dependencies.x]` names one dependency; its body is x's own
            // keys (version, features), not more dependencies.
            if let Some((_, dep)) = name.rsplit_once("dependencies.") {
                out.insert(dep.trim_matches('"').to_lowercase());
                in_deps = false;
            } else {
                in_deps = name.ends_with("dependencies") || name.ends_with("packages");
            }
            continue;
        }
        if !in_deps || t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some((k, _)) = t.split_once('=') {
            let k = k.trim().trim_matches('"');
            if !k.is_empty() {
                out.insert(k.to_lowercase());
            }
        }
    }
    out
}

/// Requirement strings (`pyproject.toml`, requirements files): the name
/// before any version, extra or marker, with `_` read as `-`.
fn requirement_name(spec: &str) -> Option<String> {
    let s = spec
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == ',');
    if s.is_empty() || s.starts_with('#') || s.starts_with('-') {
        return None;
    }
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
        .unwrap_or(s.len());
    let name = &s[..end];
    if name.is_empty()
        || !name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(name.to_lowercase().replace('_', "-"))
}

fn requirements_deps(text: &str) -> BTreeSet<String> {
    text.lines().filter_map(requirement_name).collect()
}

/// `pyproject.toml`: the strings of `dependencies = [...]` arrays and the
/// keys of poetry's dependency tables.
fn pyproject_deps(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_array = false;
    let mut in_table = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            let name = t.trim_matches(|c| c == '[' || c == ']').trim();
            in_table = name.ends_with("dependencies") || name == "dependency-groups";
            in_array = false;
            continue;
        }
        if in_array {
            if t.starts_with(']') {
                in_array = false;
                continue;
            }
            out.extend(requirement_name(t));
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let k = k.trim();
            let v = v.trim();
            if v.starts_with('[') {
                in_array = !v.contains(']');
                for item in v.trim_matches(|c| c == '[' || c == ']').split(',') {
                    out.extend(requirement_name(item));
                }
            } else if in_table && k != "python" {
                out.insert(k.trim_matches('"').to_lowercase().replace('_', "-"));
            }
        }
    }
    out
}

fn gomod_deps(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_block = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("require (") {
            in_block = true;
            continue;
        }
        if in_block && t.starts_with(')') {
            in_block = false;
            continue;
        }
        let spec = if in_block {
            t
        } else if let Some(rest) = t.strip_prefix("require ") {
            rest
        } else {
            continue;
        };
        if let Some(module) = spec.split_whitespace().next()
            && module.contains('/')
        {
            out.insert(module.to_lowercase());
        }
    }
    out
}

fn gemfile_deps(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|l| {
            let t = l.trim();
            let rest = t.strip_prefix("gem ")?.trim_start();
            let q = rest.chars().next()?;
            if q != '\'' && q != '"' {
                return None;
            }
            rest[1..].split(q).next().map(|n| n.to_lowercase())
        })
        .collect()
}

fn composer_deps(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return out;
    };
    for key in ["require", "require-dev"] {
        if let Some(m) = v.get(key).and_then(|d| d.as_object()) {
            out.extend(m.keys().map(|k| k.to_lowercase()));
        }
    }
    out
}

/// `pubspec.yaml`: the keys two spaces under `dependencies:` and
/// `dev_dependencies:`.
fn pubspec_deps(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_deps = false;
    for line in text.lines() {
        if !line.starts_with(' ') && !line.trim().is_empty() {
            in_deps = line.starts_with("dependencies:") || line.starts_with("dev_dependencies:");
            continue;
        }
        if in_deps
            && let Some(rest) = line.strip_prefix("  ")
            && !rest.starts_with(' ')
            && let Some((k, _)) = rest.split_once(':')
        {
            out.insert(k.trim().to_lowercase());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_hosts_are_read_from_every_remote_form() {
        assert_eq!(remote_host("git@github.com:o/r.git"), "github.com");
        assert_eq!(remote_host("https://gitlab.com/o/r"), "gitlab.com");
        assert_eq!(
            remote_host("ssh://git@bitbucket.org:22/o/r"),
            "bitbucket.org"
        );
        assert_eq!(remote_host("https://user@GitHub.com/o/r.git"), "github.com");
        assert_eq!(remote_host("/srv/git/r.git"), "");
        assert_eq!(remote_host(""), "");
    }

    #[test]
    fn manifests_yield_lowercase_package_names() {
        let pkg = r#"{"dependencies":{"Next":"16","@sentry/nextjs":"8"},"devDependencies":{"@playwright/test":"1"}}"#;
        let d = package_json_deps(pkg);
        assert!(
            d.contains("next") && d.contains("@sentry/nextjs") && d.contains("@playwright/test")
        );
        let cargo = "[package]\nname = \"x\"\n\n[dependencies]\nsqlx = { version = \"0.8\" }\n\"tokio-postgres\" = \"0.7\"\n\n[dependencies.sentry]\nversion = \"0.34\"\n\n[dev-dependencies]\ninsta = \"1\"\n";
        let d = cargo_deps(cargo);
        assert!(
            d.contains("sqlx")
                && d.contains("tokio-postgres")
                && d.contains("sentry")
                && d.contains("insta")
        );
        assert!(!d.contains("name"));
        let py = "[project]\nname = \"x\"\ndependencies = [\n  \"Django>=5\",\n  \"psycopg[binary]\",\n]\n\n[project.optional-dependencies]\ndev = [\"pytest\", \"sentry_sdk\"]\n\n[tool.poetry.dependencies]\npython = \"^3.12\"\nboto3 = \"*\"\n";
        let d = pyproject_deps(py);
        assert!(
            d.contains("django")
                && d.contains("psycopg")
                && d.contains("pytest")
                && d.contains("sentry-sdk")
                && d.contains("boto3"),
            "{d:?}"
        );
        assert!(!d.contains("python"));
        let d = requirements_deps(
            "# tools\nstripe==9.0\n-r base.txt\nasyncpg>=0.29 ; python_version > '3.8'\n",
        );
        assert!(
            d.contains("stripe") && d.contains("asyncpg") && d.len() == 2,
            "{d:?}"
        );
        let d = gomod_deps(
            "module x\n\ngo 1.22\n\nrequire github.com/jackc/pgx/v5 v5.6.0\n\nrequire (\n\tgithub.com/aws/aws-sdk-go-v2 v1.30.0 // indirect\n)\n",
        );
        assert!(
            d.contains("github.com/jackc/pgx/v5") && d.contains("github.com/aws/aws-sdk-go-v2")
        );
        let d = gemfile_deps("source 'https://rubygems.org'\ngem 'rails', '~> 7.1'\ngem \"pg\"\n");
        assert!(d.contains("rails") && d.contains("pg"));
        let d = composer_deps(r#"{"require":{"stripe/stripe-php":"^13"}}"#);
        assert!(d.contains("stripe/stripe-php"));
        let d = pubspec_deps(
            "name: app\ndependencies:\n  flutter:\n    sdk: flutter\n  firebase_core: ^3.0.0\ndev_dependencies:\n  flutter_test:\n    sdk: flutter\n",
        );
        assert!(
            d.contains("firebase_core") && d.contains("flutter_test") && !d.contains("sdk"),
            "{d:?}"
        );
    }
}
