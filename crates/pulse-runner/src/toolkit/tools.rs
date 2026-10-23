//! The tool connections pulse knows how to suggest: well-known MCP servers,
//! each with the signal in a checkout that calls for it. A pick with no
//! signal is in the catalog for the hub's "more tools" list but is never
//! suggested on its own.

use super::shape::Shape;
use serde::Serialize;

/// One MCP server pulse can suggest, as `pulse mcp add` would register it.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ToolPick {
    pub name: &'static str,
    pub title: &'static str,
    /// What agents get from it, in one line.
    pub about: &'static str,
    /// Where the server is documented.
    pub docs: &'static str,
    pub transport: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
    pub url: &'static str,
    pub auth: &'static str,
    /// The env variable or header the secret goes in, for `env` and `header`.
    pub auth_header: &'static str,
    /// What the secret is, for the owner; empty when the server needs none.
    pub secret: &'static str,
    /// For every project (the signal is about the owner's setup) or the one
    /// project the signal came from.
    pub shared: bool,
    /// What a checkout shows when this server would help, in one sentence.
    #[serde(skip)]
    pub signal: fn(&Shape) -> Option<String>,
}

fn none(_: &Shape) -> Option<String> {
    None
}

pub const TOOLS: &[ToolPick] = &[
    ToolPick {
        name: "github",
        title: "GitHub",
        about: "Issues, pull requests, reviews, Actions runs and code search on GitHub, read and written as you.",
        docs: "https://github.com/github/github-mcp-server",
        transport: "http",
        command: "",
        args: &[],
        url: "https://api.githubcopilot.com/mcp/",
        auth: "oauth",
        auth_header: "",
        secret: "",
        shared: true,
        signal: |s| {
            (s.remote_host == "github.com")
                .then(|| "The remote is on github.com and no tool reaches it yet".to_string())
        },
    },
    ToolPick {
        name: "gitlab",
        title: "GitLab",
        about: "Issues, merge requests and pipelines on GitLab (18.3 or later), as you.",
        docs: "https://docs.gitlab.com/user/gitlab_duo/model_context_protocol/mcp_server/",
        transport: "http",
        command: "",
        args: &[],
        url: "https://gitlab.com/api/v4/mcp",
        auth: "oauth",
        auth_header: "",
        secret: "",
        shared: true,
        signal: |s| {
            (s.remote_host == "gitlab.com")
                .then(|| "The remote is on gitlab.com and no tool reaches it yet".to_string())
        },
    },
    ToolPick {
        name: "playwright",
        title: "Playwright browser",
        about: "A real browser: open the app, click through it, read the page and take screenshots, so UI work is seen before it is called done.",
        docs: "https://github.com/microsoft/playwright-mcp",
        transport: "stdio",
        command: "npx",
        args: &["-y", "@playwright/mcp@latest"],
        url: "",
        auth: "none",
        auth_header: "",
        secret: "",
        shared: true,
        signal: |s| {
            if s.dep_any(&["@playwright/test", "playwright"])
                || s.has_any(&[
                    "playwright.config.ts",
                    "playwright.config.js",
                    "playwright.config.mjs",
                ])
            {
                return Some(
                    "Playwright is already a dependency, and no tool gives agents a browser".into(),
                );
            }
            s.ui.as_ref().map(|ui| format!("It ships a user interface ({ui}) and no tool gives agents a browser to look at it"))
        },
    },
    ToolPick {
        name: "context7",
        title: "Context7 docs",
        about: "Current documentation and code examples for the libraries this project depends on, so agents stop guessing at APIs. Works without a key; one raises the rate limit.",
        docs: "https://github.com/upstash/context7",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.context7.com/mcp",
        auth: "none",
        auth_header: "",
        secret: "",
        shared: true,
        signal: |s| {
            (s.deps.len() >= 3).then(|| format!("It depends on {} packages and no tool gives agents their current documentation", s.deps.len()))
        },
    },
    ToolPick {
        name: "postgres",
        title: "Postgres",
        about: "Reads the schema, explains query plans and runs read-only SQL against the project's database.",
        docs: "https://github.com/crystaldba/postgres-mcp",
        transport: "stdio",
        command: "uvx",
        args: &["postgres-mcp", "--access-mode=restricted"],
        url: "",
        auth: "env",
        auth_header: "DATABASE_URI",
        secret: "database URL, postgresql://user:password@host/db",
        shared: false,
        signal: |s| {
            const DEPS: &[&str] = &[
                "pg",
                "postgres",
                "tokio-postgres",
                "sqlx",
                "diesel",
                "sea-orm",
                "psycopg",
                "psycopg2",
                "psycopg2-binary",
                "asyncpg",
                "github.com/jackc/pgx/v5",
                "github.com/jackc/pgx/v4",
                "github.com/lib/pq",
            ];
            if let Some(d) = DEPS.iter().find(|d| s.dep(d)) {
                return Some(format!(
                    "It talks to Postgres ({d}) and no tool lets agents read the schema or a query plan"
                ));
            }
            s.compose_postgres.then(|| {
                format!(
                    "{} runs a postgres service and no tool lets agents read its schema",
                    s.compose_file().unwrap_or("compose")
                )
            })
        },
    },
    ToolPick {
        name: "prisma",
        title: "Prisma",
        about: "Migrations, schema changes and Prisma Postgres, from the project's own schema file.",
        docs: "https://www.prisma.io/docs/postgres/integrations/mcp-server",
        transport: "stdio",
        command: "npx",
        args: &["-y", "prisma", "mcp"],
        url: "",
        auth: "none",
        auth_header: "",
        secret: "",
        shared: false,
        signal: |s| {
            (s.has("prisma/schema.prisma") || s.dep_any(&["prisma", "@prisma/client"])).then(|| {
                "It uses Prisma and no tool lets agents work its schema and migrations".to_string()
            })
        },
    },
    ToolPick {
        name: "supabase",
        title: "Supabase",
        about: "Tables, SQL, migrations, edge functions and logs of the project's Supabase project, as you.",
        docs: "https://github.com/supabase-community/supabase-mcp",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.supabase.com/mcp",
        auth: "oauth",
        auth_header: "",
        secret: "",
        shared: false,
        signal: |s| {
            (s.has("supabase")
                || s.dep_any(&["@supabase/supabase-js", "@supabase/ssr", "supabase"]))
            .then(|| "It is built on Supabase and no tool reaches that project".to_string())
        },
    },
    ToolPick {
        name: "firebase",
        title: "Firebase",
        about: "Firestore, Auth, Hosting and the rest of the Firebase project, through the Firebase CLI's own server.",
        docs: "https://firebase.google.com/docs/cli/mcp-server",
        transport: "stdio",
        command: "npx",
        args: &["-y", "firebase-tools@latest", "mcp"],
        url: "",
        auth: "none",
        auth_header: "",
        secret: "",
        shared: false,
        signal: |s| {
            (s.has("firebase.json") || s.dep_any(&["firebase", "firebase-admin", "firebase_core"]))
                .then(|| "It is built on Firebase and no tool reaches that project".to_string())
        },
    },
    ToolPick {
        name: "stripe",
        title: "Stripe",
        about: "Customers, payments, subscriptions and the Stripe docs, from the account you sign into (test mode first).",
        docs: "https://docs.stripe.com/mcp",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.stripe.com",
        auth: "oauth",
        auth_header: "",
        secret: "",
        shared: false,
        signal: |s| {
            s.dep_any(&[
                "stripe",
                "@stripe/stripe-js",
                "@stripe/react-stripe-js",
                "async-stripe",
                "stripe/stripe-php",
                "github.com/stripe/stripe-go",
            ])
            .then(|| "It takes payments with Stripe and no tool reaches that account".to_string())
        },
    },
    ToolPick {
        name: "sentry",
        title: "Sentry",
        about: "Issues, stack traces and releases from Sentry, so a fix starts from the real error.",
        docs: "https://docs.sentry.io/product/sentry-mcp/",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.sentry.dev/mcp",
        auth: "oauth",
        auth_header: "",
        secret: "",
        shared: false,
        signal: |s| {
            (s.dep_prefix("@sentry/")
                || s.dep_any(&[
                    "sentry-sdk",
                    "sentry",
                    "sentry-ruby",
                    "sentry-rails",
                    "sentry/sentry",
                    "github.com/getsentry/sentry-go",
                ]))
            .then(|| "It reports errors to Sentry and no tool lets agents read them".to_string())
        },
    },
    ToolPick {
        name: "vercel",
        title: "Vercel",
        about: "Deployments, build logs and project settings on Vercel, as you.",
        docs: "https://vercel.com/docs/mcp/vercel-mcp",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.vercel.com",
        auth: "oauth",
        auth_header: "",
        secret: "",
        shared: false,
        signal: |s| {
            (s.has("vercel.json")
                || s.dep_any(&[
                    "vercel",
                    "@vercel/analytics",
                    "@vercel/blob",
                    "@vercel/kv",
                    "@vercel/postgres",
                ]))
            .then(|| {
                "It deploys to Vercel and no tool lets agents read its deployments".to_string()
            })
        },
    },
    ToolPick {
        name: "cloudflare",
        title: "Cloudflare",
        about: "Workers, KV, D1, R2, logs and the Cloudflare docs, as you.",
        docs: "https://github.com/cloudflare/mcp-server-cloudflare",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.cloudflare.com/mcp",
        auth: "oauth",
        auth_header: "",
        secret: "",
        shared: false,
        signal: |s| {
            (s.has_any(&["wrangler.toml", "wrangler.json", "wrangler.jsonc"])
                || s.dep_any(&["wrangler", "@cloudflare/workers-types"]))
            .then(|| "It runs on Cloudflare Workers and no tool reaches that account".to_string())
        },
    },
    ToolPick {
        name: "aws-knowledge",
        title: "AWS knowledge",
        about: "AWS documentation, API references and architecture guidance from AWS's own server; it never touches your account.",
        docs: "https://github.com/awslabs/mcp",
        transport: "http",
        command: "",
        args: &[],
        url: "https://knowledge-mcp.global.api.aws",
        auth: "none",
        auth_header: "",
        secret: "",
        shared: true,
        signal: |s| {
            let by_file = s.first_of(&["cdk.json", "serverless.yml", "samconfig.toml"]);
            let by_dep = s.dep_prefix("@aws-sdk/")
                || s.dep_prefix("github.com/aws/")
                || s.dep_any(&[
                    "aws-cdk-lib",
                    "boto3",
                    "aws-sdk",
                    "aws-config",
                    "aws-sdk-s3",
                ]);
            match (by_file, by_dep) {
                (Some(f), _) => Some(format!(
                    "{f} says it runs on AWS and no tool gives agents the AWS docs"
                )),
                (None, true) => {
                    Some("It uses the AWS SDK and no tool gives agents the AWS docs".into())
                }
                _ => None,
            }
        },
    },
    ToolPick {
        name: "terraform",
        title: "Terraform registry",
        about: "Provider and module documentation from the Terraform registry while writing HCL. Needs Docker on this machine.",
        docs: "https://github.com/hashicorp/terraform-mcp-server",
        transport: "stdio",
        command: "docker",
        args: &["run", "-i", "--rm", "hashicorp/terraform-mcp-server"],
        url: "",
        auth: "none",
        auth_header: "",
        secret: "",
        shared: false,
        signal: |s| {
            s.has_any(&["*.tf", "terraform"]).then(|| {
                "It has Terraform files and no tool gives agents the provider docs".to_string()
            })
        },
    },
    ToolPick {
        name: "kubernetes",
        title: "Kubernetes",
        about: "Pods, deployments, logs and events of the cluster kubectl already points at.",
        docs: "https://github.com/Flux159/mcp-server-kubernetes",
        transport: "stdio",
        command: "npx",
        args: &["-y", "mcp-server-kubernetes"],
        url: "",
        auth: "none",
        auth_header: "",
        secret: "",
        shared: false,
        signal: |s| {
            s.first_of(&["k8s", "kubernetes", "Chart.yaml", "charts", "helm"]).map(|f| format!("{f} describes a Kubernetes deployment and no tool lets agents read the cluster"))
        },
    },
    ToolPick {
        name: "huggingface",
        title: "Hugging Face",
        about: "Models, datasets, Spaces and papers on the Hugging Face Hub.",
        docs: "https://huggingface.co/settings/mcp",
        transport: "http",
        command: "",
        args: &[],
        url: "https://huggingface.co/mcp",
        auth: "none",
        auth_header: "",
        secret: "",
        shared: true,
        signal: |s| {
            (s.dep_prefix("@huggingface/")
                || s.dep_any(&[
                    "transformers",
                    "datasets",
                    "huggingface-hub",
                    "tokenizers",
                    "candle-core",
                    "sentence-transformers",
                ]))
            .then(|| "It works with Hugging Face models and no tool reaches the Hub".to_string())
        },
    },
    ToolPick {
        name: "shopify",
        title: "Shopify dev",
        about: "Shopify's API docs, GraphQL schemas and Polaris guidance while building the app or theme.",
        docs: "https://github.com/Shopify/dev-mcp",
        transport: "stdio",
        command: "npx",
        args: &["-y", "@shopify/dev-mcp@latest"],
        url: "",
        auth: "none",
        auth_header: "",
        secret: "",
        shared: false,
        signal: |s| {
            (s.has("shopify.app.toml") || s.dep_prefix("@shopify/")).then(|| {
                "It is a Shopify app and no tool gives agents Shopify's docs and schemas"
                    .to_string()
            })
        },
    },
    ToolPick {
        name: "chrome-devtools",
        title: "Chrome DevTools",
        about: "Chrome with DevTools open: performance traces, network and console, and the page itself.",
        docs: "https://github.com/ChromeDevTools/chrome-devtools-mcp",
        transport: "stdio",
        command: "npx",
        args: &["-y", "chrome-devtools-mcp@latest", "--headless"],
        url: "",
        auth: "none",
        auth_header: "",
        secret: "",
        shared: true,
        signal: none,
    },
    ToolPick {
        name: "linear",
        title: "Linear",
        about: "Issues, projects and cycles in Linear, as you.",
        docs: "https://linear.app/docs/mcp",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.linear.app/mcp",
        auth: "oauth",
        auth_header: "",
        secret: "",
        shared: true,
        signal: none,
    },
    ToolPick {
        name: "notion",
        title: "Notion",
        about: "Pages and databases in the Notion workspace you sign into.",
        docs: "https://developers.notion.com/docs/mcp",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.notion.com/mcp",
        auth: "oauth",
        auth_header: "",
        secret: "",
        shared: true,
        signal: none,
    },
    ToolPick {
        name: "figma",
        title: "Figma",
        about: "Design files and components from Figma, for building what was designed.",
        docs: "https://help.figma.com/hc/en-us/articles/32132100833559",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.figma.com/mcp",
        auth: "oauth",
        auth_header: "",
        secret: "",
        shared: true,
        signal: none,
    },
    ToolPick {
        name: "atlassian",
        title: "Jira and Confluence",
        about: "Jira issues and Confluence pages, as you.",
        docs: "https://www.atlassian.com/platform/remote-mcp-server",
        transport: "sse",
        command: "",
        args: &[],
        url: "https://mcp.atlassian.com/v1/sse",
        auth: "oauth",
        auth_header: "",
        secret: "",
        shared: true,
        signal: none,
    },
];

pub fn pick(name: &str) -> Option<&'static ToolPick> {
    TOOLS.iter().find(|t| t.name == name)
}

/// What the owner runs to register a pick for `project` (empty: every
/// project), and what follows it.
pub fn add_command(t: &ToolPick, project: &str) -> (String, String) {
    let mut cmd = format!(
        "pulse mcp add --name {} --transport {}",
        t.name, t.transport
    );
    if t.transport == "stdio" {
        cmd.push_str(&format!(" --command {}", t.command));
        for a in t.args {
            cmd.push_str(&format!(" --arg {}", crate::util::shell_quote(a)));
        }
    } else {
        cmd.push_str(&format!(" --url {}", t.url));
    }
    if t.auth != "none" {
        cmd.push_str(&format!(" --auth {}", t.auth));
    }
    if !t.auth_header.is_empty() {
        cmd.push_str(&format!(" --auth-header {}", t.auth_header));
    }
    if !t.secret.is_empty() {
        cmd.push_str(" --secret keychain://pulse/");
        cmd.push_str(t.name);
    }
    if !t.shared && !project.is_empty() {
        cmd.push_str(&format!(" --project {project}"));
    }
    // The secret itself never goes through pulse: it rests in the login
    // Keychain (or 1Password, with an op:// reference instead) and the
    // server reads it from there at call time.
    let after = match t.auth {
        "oauth" => format!("pulse mcp login {}", t.name),
        _ if !t.secret.is_empty() => format!(
            "security add-generic-password -s pulse -a {} -U -w '<{}>'   (macOS Keychain; or pass --secret op://VAULT/ITEM/FIELD instead)",
            t.name, t.secret
        ),
        _ => String::new(),
    };
    (cmd, after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_are_slugs_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for t in TOOLS {
            assert!(pulse_store::is_slug(t.name), "{}", t.name);
            assert!(seen.insert(t.name), "duplicate {}", t.name);
            assert!(
                pulse_store::MCP_TRANSPORTS.contains(&t.transport),
                "{}",
                t.name
            );
            let auth = pulse_store::mcp_auth(t.auth).unwrap_or_else(|| panic!("{} auth", t.name));
            assert!(
                auth.transports.contains(&t.transport),
                "{} auth {} on {}",
                t.name,
                t.auth,
                t.transport
            );
            if t.transport == "stdio" {
                assert!(!t.command.is_empty() && t.url.is_empty(), "{}", t.name);
            } else {
                assert!(
                    t.url.starts_with("https://") && t.command.is_empty(),
                    "{}",
                    t.name
                );
            }
            assert!(
                !t.about.is_empty() && t.docs.starts_with("https://"),
                "{}",
                t.name
            );
        }
    }

    #[test]
    fn add_commands_follow_the_cli_usage() {
        let (cmd, after) = add_command(pick("github").unwrap(), "blog");
        assert_eq!(
            cmd,
            "pulse mcp add --name github --transport http --url https://api.githubcopilot.com/mcp/ --auth oauth"
        );
        assert_eq!(after, "pulse mcp login github");
        let (cmd, after) = add_command(pick("postgres").unwrap(), "blog");
        assert_eq!(
            cmd,
            "pulse mcp add --name postgres --transport stdio --command uvx --arg postgres-mcp --arg --access-mode=restricted --auth env --auth-header DATABASE_URI --secret keychain://pulse/postgres --project blog"
        );
        assert!(after.starts_with("security add-generic-password -s pulse -a postgres"));
        let (cmd, after) = add_command(pick("playwright").unwrap(), "blog");
        assert_eq!(
            cmd,
            "pulse mcp add --name playwright --transport stdio --command npx --arg -y --arg @playwright/mcp@latest"
        );
        assert!(after.is_empty());
    }
}
