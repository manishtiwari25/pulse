//! The skills pulse drafts from a project's shape: short recipes an agent
//! follows for work the checkout shows it will have (setting up, checking
//! the UI, cutting a release, moving a migration). Each body is written
//! from what the shape says, so it names the real package manager, verify
//! command and folders, and ends by telling the reader to edit it.

use super::shape::Shape;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SkillPick {
    pub name: &'static str,
    /// Names a project may already use for the same recipe.
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    #[serde(skip)]
    pub signal: fn(&Shape) -> Option<String>,
    #[serde(skip)]
    pub body: fn(&Shape) -> String,
}

const FOOTER: &str = "Written by PULSE from the project's shape; edit it as the project changes.";

pub const SKILLS: &[SkillPick] = &[
    SkillPick {
        name: "dev-setup",
        aliases: &["setup", "dev-environment", "onboarding"],
        description: "Install, build and run the project's checks on a fresh clone, the way this project does it.",
        signal: |s| {
            (!s.manifests().is_empty()).then(|| {
                format!(
                    "It has {} and no skill says how a fresh clone is set up and checked",
                    s.manifests().join(", ")
                )
            })
        },
        body: dev_setup,
    },
    SkillPick {
        name: "ui-check",
        aliases: &["visual-check", "screenshot", "ui-review"],
        description: "Look at the UI before calling it done: serve it, open it, screenshot light and dark, read the console.",
        signal: |s| {
            s.ui.as_ref().map(|ui| format!("It ships a user interface ({ui}) and no skill says how to look at it before calling work done"))
        },
        body: ui_check,
    },
    SkillPick {
        name: "release-check",
        aliases: &["release", "changelog", "versioning"],
        description: "What a release needs here: the check green, the changelog written, the version bumped, the tag shaped like the last one.",
        signal: |s| {
            if s.has_tag {
                return Some(format!(
                    "It is released by tag (last {}) and no skill says what a release needs",
                    s.last_tag
                ));
            }
            s.first_of(&["CHANGELOG.md", "CHANGELOG"])
                .map(|f| format!("It keeps {f} and no skill says what a release needs"))
        },
        body: release_check,
    },
    SkillPick {
        name: "db-migrations",
        aliases: &["migrations", "database", "schema-changes"],
        description: "Add and apply a database migration with this project's tool; never edit one that has been applied.",
        signal: |s| {
            migration_tool(s).map(|(dir, tool)| {
                format!(
                    "It keeps migrations in {dir} ({tool}) and no skill says how to add one safely"
                )
            })
        },
        body: db_migrations,
    },
    SkillPick {
        name: "containers",
        aliases: &["docker", "compose", "container"],
        description: "Build the image and bring the services up and down the way this project's Dockerfile and compose file expect.",
        signal: |s| {
            s.first_of(&[
                "Dockerfile",
                "compose.yaml",
                "compose.yml",
                "docker-compose.yml",
                "docker-compose.yaml",
            ])
            .map(|f| format!("It has {f} and no skill says how its containers are built and run"))
        },
        body: containers,
    },
    SkillPick {
        name: "ci-failures",
        aliases: &["ci", "github-actions", "pipeline"],
        description: "Read a failed CI run from the terminal, reproduce it locally, fix the cause and not the symptom.",
        signal: |s| {
            s.first_of(&[".github/workflows", ".gitlab-ci.yml"])
                .map(|f| format!("It runs CI from {f} and no skill says how to read a red run"))
        },
        body: ci_failures,
    },
    SkillPick {
        name: "api-contract",
        aliases: &["openapi", "api-schema", "graphql-schema"],
        description: "Change the API through its schema first, regenerate what is generated, and keep the contract backward compatible.",
        signal: |s| {
            s.first_of(&[
                "openapi.yaml",
                "openapi.yml",
                "openapi.json",
                "swagger.json",
                "swagger.yaml",
                "schema.graphql",
            ])
            .map(|f| {
                format!(
                    "Its API has a contract ({f}) and no skill says how a change goes through it"
                )
            })
        },
        body: api_contract,
    },
    SkillPick {
        name: "infra-changes",
        aliases: &["infrastructure", "terraform", "iac"],
        description: "Plan infrastructure changes and show the plan; applying is the owner's, from a terminal.",
        signal: |s| {
            infra_tool(s).map(|(f, tool)| format!("It describes infrastructure in {f} ({tool}) and no skill says that agents plan and never apply"))
        },
        body: infra_changes,
    },
];

pub fn pick(name: &str) -> Option<&'static SkillPick> {
    SKILLS.iter().find(|k| k.name == name)
}

/// Whether the project already has this recipe under its name, an alias or
/// the project-prefixed name a draft falls back to.
pub fn present(k: &SkillPick, shape: &Shape) -> bool {
    shape.skills.contains(k.name)
        || k.aliases.iter().any(|a| shape.skills.contains(*a))
        || shape
            .skills
            .contains(&format!("{}-{}", shape.project, k.name))
}

/// The name the draft is registered under: the recipe's own, unless another
/// project's skill already holds it (names are unique across the registry),
/// then `<slug>-<name>`.
fn migration_tool(s: &Shape) -> Option<(&'static str, &'static str)> {
    if s.has("prisma/migrations") || s.has("prisma/schema.prisma") {
        Some(("prisma/migrations", "prisma"))
    } else if s.has("supabase/migrations") {
        Some(("supabase/migrations", "supabase"))
    } else if s.has("drizzle") {
        Some(("drizzle", "drizzle"))
    } else if s.has("alembic") {
        Some(("alembic", "alembic"))
    } else if s.has("db/migrate") {
        Some(("db/migrate", "rails"))
    } else if s.has("migrations") {
        let tool = if s.dep("sqlx") {
            "sqlx"
        } else if s.dep("diesel") {
            "diesel"
        } else if s.dep("sea-orm") || s.dep("sea-orm-migration") {
            "sea-orm"
        } else if s.dep("django") {
            "django"
        } else if s.dep("knex") {
            "knex"
        } else if s.dep("typeorm") {
            "typeorm"
        } else if s.dep("goose") || s.dep("github.com/pressly/goose/v3") {
            "goose"
        } else if s.dep("github.com/golang-migrate/migrate/v4") {
            "golang-migrate"
        } else {
            "plain SQL"
        };
        Some(("migrations", tool))
    } else {
        None
    }
}

fn infra_tool(s: &Shape) -> Option<(&'static str, &'static str)> {
    if s.has("*.tf") || s.has("terraform") {
        Some((
            if s.has("terraform") {
                "terraform/"
            } else {
                "*.tf"
            },
            "terraform",
        ))
    } else if s.has("cdk.json") {
        Some(("cdk.json", "cdk"))
    } else if s.has("serverless.yml") {
        Some(("serverless.yml", "serverless"))
    } else if s.has("template.yaml") || s.has("samconfig.toml") {
        Some(("template.yaml", "sam"))
    } else if s.has("Chart.yaml") || s.has("charts") || s.has("helm") {
        Some(("Chart.yaml", "helm"))
    } else if s.has("k8s") || s.has("kubernetes") {
        Some((if s.has("k8s") { "k8s/" } else { "kubernetes/" }, "kubectl"))
    } else {
        None
    }
}

fn verify_line(s: &Shape) -> String {
    if s.verify.is_empty() {
        "The project has no verify command on record yet; find the check in its README or CI and record it with `pulse init <path> --verify \"<command>\"` (re-registering keeps the project and replaces the command).".to_string()
    } else {
        format!("Run the project's check: `{}`.", s.verify)
    }
}

fn dev_setup(s: &Shape) -> String {
    let (pm, install) = s
        .package_manager()
        .unwrap_or(("the project's tool", "<install command>"));
    let mut b = String::new();
    b.push_str(&format!("# Dev setup for {}\n\n", s.project));
    b.push_str(
        "Use this on a fresh clone or when a build fails for reasons that look environmental.\n\n",
    );
    b.push_str("1. Read the recorded environment first: `pulse env show --project ");
    b.push_str(&s.project);
    b.push_str("` prints the toolchain versions and the install recipe the project keeps; prefer it to guessing.\n");
    b.push_str(&format!("2. Install with {pm}: `{install}`.\n"));
    let manifests = s.manifests();
    if manifests.len() > 1 {
        b.push_str(&format!(
            "   The checkout has more than one manifest ({}); install each ecosystem before the check.\n",
            manifests.join(", ")
        ));
    }
    b.push_str(&format!("3. {}\n", verify_line(s)));
    if let Some(f) = s.compose_file() {
        b.push_str(&format!("4. Services come from `{f}`: `docker compose up -d` before the check, `docker compose down` after.\n"));
    }
    b.push_str("\nWhen a step fails on the environment and not the code, fix the environment, then record what you learned with `pulse env` so the next agent starts further along.\n\n");
    b.push_str(FOOTER);
    b
}

fn ui_check(s: &Shape) -> String {
    let ui = s.ui.clone().unwrap_or_else(|| "web".into());
    let serve = match ui.as_str() {
        "nextjs" | "next" => {
            "npm run dev (or `npm run build && npx serve out` for a static export)"
        }
        "vite" | "react" | "vue" | "svelte" | "sveltekit" | "astro" | "nuxt" => "npm run dev",
        "flutter" => "flutter run -d chrome",
        "tauri" | "electron" => "npm run dev (the window opens itself)",
        _ => "the project's dev server (see dev-setup)",
    };
    let screenshot = if s.tools.contains("playwright") {
        "Open the page with the playwright tool: `pulse mcp call playwright browser_navigate --args '{\"url\":\"http://127.0.0.1:PORT/\"}'`, then `pulse mcp call playwright browser_take_screenshot --args '{\"filename\":\"check.png\"}'` and read `browser_console_messages`.".to_string()
    } else {
        "Screenshot it headless: `npx -y playwright screenshot --viewport-size 1280,900 --wait-for-timeout 2000 http://127.0.0.1:PORT/ check.png` (a playwright tool connection makes this a tool call; `pulse toolkit` suggests it).".to_string()
    };
    format!(
        "# UI check for {p}\n\nUI work is not done until someone has looked at it. Do this before the last commit of any change that touches a page, a component or a stylesheet.\n\n1. Serve it: `{serve}`.\n2. {screenshot}\n3. Look at the screenshot: layout, spacing, text that overflows, states that are empty or loading. Take one in light and one in dark when the app has both.\n4. Read the browser console; a new error or warning is part of the change.\n5. Keep the screenshot out of the commit unless the project keeps them on purpose; mention what you saw in the handoff.\n\nUser interface: {ui}.\n\n{FOOTER}",
        p = s.project
    )
}

fn release_check(s: &Shape) -> String {
    let tag = if s.has_tag {
        format!(
            "Tags look like `{}`; the next one keeps that shape.",
            s.last_tag
        )
    } else {
        "The project has no tag yet; agree the shape of the first one with the owner before cutting it (v1.0.0 unless the project says otherwise).".to_string()
    };
    let bump = if s.has("package.json") {
        "the `version` in package.json"
    } else if s.has("Cargo.toml") {
        "the `version` in Cargo.toml (every crate that is published)"
    } else if s.has("pyproject.toml") {
        "the `version` in pyproject.toml"
    } else if s.has("pubspec.yaml") {
        "the `version` in pubspec.yaml"
    } else if s.has("composer.json") {
        "the `version` in composer.json"
    } else {
        "wherever the version lives (find the last bump with `git log -p -S<last version>`)"
    };
    let changelog = s
        .first_of(&["CHANGELOG.md", "CHANGELOG"])
        .map(|f| format!("Write the entry in `{f}` under the new version, in the style of the entries above it, from `git log <last tag>..HEAD`."))
        .unwrap_or_else(|| "The project keeps no changelog; the tag message carries the summary, written from `git log <last tag>..HEAD`.".to_string());
    format!(
        "# Release check for {p}\n\nA release is a commit the owner can tag and publish without reading the diff again.\n\n1. {verify}\n2. {changelog}\n3. Bump {bump}; the lockfile moves with it when the ecosystem records the version there.\n4. {tag}\n5. Commit the bump and the changelog together; stop there. Tagging, pushing and publishing are the owner's, from a terminal; say so in the handoff with the exact commands.\n\n{FOOTER}",
        p = s.project,
        verify = verify_line(s)
    )
}

fn db_migrations(s: &Shape) -> String {
    let (dir, tool) = migration_tool(s).unwrap_or(("migrations", "plain SQL"));
    let (add, apply) = match tool {
        "prisma" => (
            "npx prisma migrate dev --name <change>",
            "npx prisma migrate deploy",
        ),
        "supabase" => (
            "supabase migration new <change>",
            "supabase db push (or `supabase db reset` locally)",
        ),
        "drizzle" => ("npx drizzle-kit generate", "npx drizzle-kit migrate"),
        "alembic" => (
            "alembic revision --autogenerate -m \"<change>\"",
            "alembic upgrade head",
        ),
        "rails" => (
            "bin/rails generate migration <Change>",
            "bin/rails db:migrate",
        ),
        "sqlx" => ("sqlx migrate add -r <change>", "sqlx migrate run"),
        "diesel" => ("diesel migration generate <change>", "diesel migration run"),
        "sea-orm" => (
            "sea-orm-cli migrate generate <change>",
            "sea-orm-cli migrate up",
        ),
        "django" => (
            "python manage.py makemigrations",
            "python manage.py migrate",
        ),
        "knex" => ("npx knex migrate:make <change>", "npx knex migrate:latest"),
        "typeorm" => (
            "npx typeorm migration:generate -n <Change>",
            "npx typeorm migration:run",
        ),
        "goose" => ("goose create <change> sql", "goose up"),
        "golang-migrate" => (
            "migrate create -ext sql -dir migrations <change>",
            "migrate -path migrations -database $DATABASE_URL up",
        ),
        _ => (
            "a new numbered file in the folder, named after the change",
            "the project's own apply step (see its README)",
        ),
    };
    format!(
        "# Database migrations for {p}\n\nMigrations live in `{dir}` and are run by {tool}.\n\n1. Add one: `{add}`. One migration per change, named for what it does.\n2. Apply it locally: `{apply}`; then run the check.\n3. Never edit a migration that has been applied anywhere but your machine; add a new one that corrects it.\n4. A migration that drops or rewrites data is the owner's call: write it, show the plan in the handoff, do not apply it to a shared database.\n5. Keep the model, the migration and the code that reads the new shape in the same commit.\n\n{FOOTER}",
        p = s.project
    )
}

fn containers(s: &Shape) -> String {
    let compose = s.compose_file();
    let mut b = format!("# Containers for {}\n\n", s.project);
    if s.has("Dockerfile") {
        b.push_str("Build the image from the root: `docker build -t ");
        b.push_str(&s.project);
        b.push_str(
            ":dev .`; a change to the Dockerfile is checked by building it, not by reading it.\n",
        );
    }
    if let Some(f) = compose {
        b.push_str(&format!("Services are in `{f}`: `docker compose up -d` brings them up, `docker compose logs -f <service>` reads one, `docker compose down` stops them (add `-v` to drop their volumes; that is data).\n"));
    }
    b.push_str("\nRules:\n- Pin base images by tag; a `latest` that moved is a build that broke for no reason in the diff.\n- Secrets come from the environment or a mounted file, never from the image or the compose file in git.\n- Leave the machine as you found it: stop what you started before the handoff.\n\n");
    b.push_str(FOOTER);
    b
}

fn ci_failures(s: &Shape) -> String {
    let read = if s.has(".github/workflows") {
        "List the runs: `gh run list --limit 10`; read the failed one: `gh run view <id> --log-failed` (the failing step's log only).".to_string()
    } else {
        "Read the failed pipeline in GitLab: `glab ci status` and `glab ci trace <job>`, or the pipeline page.".to_string()
    };
    format!(
        "# CI failures for {p}\n\nA red run is a signal about the code or about the runner; find out which before changing anything.\n\n1. {read}\n2. Reproduce it locally with the same command the workflow runs ({verify}). A failure you cannot reproduce is usually the environment: a version the runner has and you do not, a missing service, a secret.\n3. Fix the cause. A retry that passes is not a fix; say so in the handoff if the failure was flaky, and open a task for the flake.\n4. Run the check again before the commit.\n\n{FOOTER}",
        p = s.project,
        verify = if s.verify.is_empty() {
            "see the workflow file for the command".to_string()
        } else {
            format!("`{}`", s.verify)
        }
    )
}

fn api_contract(s: &Shape) -> String {
    let f = s
        .first_of(&[
            "openapi.yaml",
            "openapi.yml",
            "openapi.json",
            "swagger.json",
            "swagger.yaml",
            "schema.graphql",
        ])
        .unwrap_or("the schema file");
    let kind = if f.ends_with(".graphql") {
        "GraphQL schema"
    } else {
        "OpenAPI document"
    };
    format!(
        "# API contract for {p}\n\nThe API is defined by `{f}` ({kind}); the code serves it, not the other way round.\n\n1. Change the contract first, then the handler; regenerate anything generated from it (clients, types, docs) in the same commit.\n2. Keep it backward compatible: add fields, never remove or retype one a client may read; a breaking change gets a new version or a deprecation first and the owner's yes.\n3. Validate the document (an OpenAPI linter such as `npx -y @redocly/cli lint {f}`, or the GraphQL build) and run the check.\n4. Say what changed in the contract in the handoff, in the words a client author would search for.\n\n{FOOTER}",
        p = s.project
    )
}

fn infra_changes(s: &Shape) -> String {
    let (f, tool) = infra_tool(s).unwrap_or(("the infrastructure files", "the tool"));
    let plan = match tool {
        "terraform" => {
            "`terraform init -input=false && terraform plan -out=tfplan` (and `terraform validate`, `terraform fmt -check`)"
        }
        "cdk" => "`npx cdk synth && npx cdk diff`",
        "serverless" => "`npx serverless package` and read the generated template",
        "sam" => "`sam validate && sam build`",
        "helm" => {
            "`helm lint . && helm template .` (or `helm diff upgrade` when the plugin is there)"
        }
        _ => "`kubectl apply --dry-run=server -f <files>` and `kubectl diff -f <files>`",
    };
    format!(
        "# Infrastructure changes for {p}\n\nInfrastructure is described in `{f}` and applied with {tool}.\n\n1. Make the change in the files; never in the console or by hand on a resource.\n2. Plan, do not apply: {plan}. Read the plan; a destroy or a replace of anything stateful is the owner's call and goes in the handoff in bold.\n3. Applying is the owner's, from a terminal, with the credentials agents do not have. Put the exact apply command in the handoff.\n4. Secrets and account ids stay out of the files; they come from variables the owner sets.\n\n{FOOTER}",
        p = s.project
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn shape(files: &[&str], deps: &[&str]) -> Shape {
        Shape {
            project: "blog".into(),
            files: files.iter().map(|f| (*f).to_string()).collect(),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            verify: "npm test".into(),
            ..Default::default()
        }
    }

    #[test]
    fn catalog_names_are_slugs_and_bodies_are_within_limits() {
        let mut seen = BTreeSet::new();
        for k in SKILLS {
            assert!(pulse_store::is_slug(k.name), "{}", k.name);
            assert!(seen.insert(k.name), "duplicate {}", k.name);
            assert!(
                k.description.len() <= 200 && !k.description.contains('\n'),
                "{}",
                k.name
            );
            let full = shape(
                &[
                    "package.json",
                    "Cargo.toml",
                    "Dockerfile",
                    "compose.yaml",
                    ".github/workflows",
                    "prisma/migrations",
                    "CHANGELOG.md",
                    "openapi.yaml",
                    "*.tf",
                ],
                &["next", "sqlx"],
            );
            let body = (k.body)(&full);
            assert!(
                body.ends_with(FOOTER) && body.len() < 64 * 1024,
                "{}",
                k.name
            );
            assert!(body.contains("blog"), "{} names the project", k.name);
        }
    }

    #[test]
    fn signals_read_the_shape() {
        let s = shape(
            &["package.json", "pnpm-lock.yaml", "migrations"],
            &["next", "sqlx"],
        );
        assert!((pick("dev-setup").unwrap().signal)(&s).is_some());
        assert!(
            (pick("db-migrations").unwrap().signal)(&s)
                .unwrap()
                .contains("sqlx")
        );
        assert!(
            (pick("ui-check").unwrap().signal)(&s).is_none(),
            "ui is read by the store, not the files"
        );
        assert!((pick("containers").unwrap().signal)(&s).is_none());
        let body = (pick("dev-setup").unwrap().body)(&s);
        assert!(
            body.contains("pnpm install --frozen-lockfile") && body.contains("`npm test`"),
            "{body}"
        );
        let body = (pick("db-migrations").unwrap().body)(&s);
        assert!(body.contains("sqlx migrate add"), "{body}");
    }

    #[test]
    fn present_skills_match_by_name_or_alias() {
        let mut s = shape(&["Dockerfile"], &[]);
        let k = pick("containers").unwrap();
        assert!(!present(k, &s));
        s.skills.insert("docker".into());
        assert!(present(k, &s));
    }
}
