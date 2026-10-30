use crate::client::{self, Client, ToolInfo};
use anyhow::{Result, anyhow};
use pulse_cli::{Command, Context, FlagSet};
use pulse_store::{
    McpServer, McpServerInput, McpServerPatch, Skill, SkillInput, SkillPatch, Store,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{IsTerminal, Read};
use std::sync::{Mutex, Once};
use std::time::Duration;

static REGISTER: Once = Once::new();
static STDIN_BUF: Mutex<Option<Vec<u8>>> = Mutex::new(None);
/// How long reading one credential may take (the hub uses the same).
pub const RESOLVER_TIMEOUT: Duration = Duration::from_secs(15);

pub fn set_stdin_for_tests(data: Option<Vec<u8>>) {
    *STDIN_BUF.lock().expect("stdin lock poisoned") = data;
}

#[derive(Serialize)]
struct ServerOut<'a> {
    server: &'a McpServer,
}

#[derive(Serialize)]
struct ServersOut<'a> {
    servers: &'a [McpServer],
}

#[derive(Serialize)]
struct ConnectOut<'a> {
    server: &'a McpServer,
    info: &'a client::ServerInfo,
    tools: &'a [ToolInfo],
}

#[derive(Serialize)]
struct ToolsOut<'a> {
    server: &'a str,
    tools: &'a [ToolInfo],
}

#[derive(Serialize)]
struct CallOut<'a> {
    server: &'a str,
    tool: &'a str,
    is_error: bool,
    text: &'a str,
    content: &'a [Value],
    #[serde(skip_serializing_if = "Value::is_null")]
    structured: &'a Value,
}

#[derive(Serialize)]
struct ResourcesOut<'a> {
    server: &'a str,
    resources: &'a [client::ResourceInfo],
}

#[derive(Serialize)]
struct ReadOut<'a> {
    server: &'a str,
    uri: &'a str,
    text: &'a str,
    contents: &'a [Value],
}

#[derive(Serialize)]
struct LoginOut<'a> {
    server: &'a str,
    issuer: &'a str,
    client_id: &'a str,
    registered: bool,
    /// The client is one PULSE carries; `client_label` says what the
    /// provider's consent page called it.
    built_in: bool,
    client_label: &'a str,
    scope: &'a str,
    expires_at: u64,
    has_refresh_token: bool,
    grant_file: &'a str,
}

#[derive(Serialize)]
struct LogoutOut<'a> {
    server: &'a str,
    had_grant: bool,
    revoked: bool,
    note: &'a str,
}

#[derive(Serialize)]
struct SkillOut<'a> {
    skill: &'a Skill,
}

#[derive(Serialize)]
struct SkillsOut<'a> {
    skills: &'a [Skill],
}

pub fn register() {
    REGISTER.call_once(|| {
        pulse_cli::register(Command { name:"mcp".into(), group:"agents".into(), summary:"MCP servers agents can call: add, list, show, update, enable, disable, login, logout, connect, tools, call, resources, read".into(), usage:"pulse mcp add --name N --transport stdio|http|sse (--command CMD [--arg A]... | --url U) [--auth none|bearer|basic|header|query|env|oauth] [--auth-header NAME] [--secret REF] [--env K=V]... [--project P] [--note TEXT]\npulse mcp list [--project P] [--json]\npulse mcp show N|MCP-NNN [--json]\npulse mcp update N|MCP-NNN [--name N] [--transport T] [--command CMD] [--arg A]... [--url U] [--auth A] [--auth-header NAME] [--secret REF] [--env K=V]... [--note TEXT] [--project P]\npulse mcp enable|disable N|MCP-NNN\npulse mcp login N [--client-id ID [--client-secret REF]] [--flow code|device] [--scope S] [--redirect-port N] [--no-browser] [--json]   (auth oauth: sign in once, in a browser or with a code)\npulse mcp logout N [--json]               (auth oauth: revoke and forget the tokens)\npulse mcp connect N [--json]              (initialize + tools/list; records the check)\npulse mcp tools N [--json]\npulse mcp call N TOOL [--args JSON | stdin] [--json]   (exit 1 when the tool reports an error)\npulse mcp resources N [--json]\npulse mcp read N URI [--json]\n\nThe secret is a pulse_cred reference, never the value: op://VAULT/ITEM/FIELD, keychain://SERVICE/ACCOUNT or file:///ABSOLUTE/PATH. For basic auth it resolves to user:password. Env values may be references too.\n\nauth oauth (http, sse) takes no secret: pulse mcp login N finds the server's authorization server, registers PULSE as a client when the server allows it, signs in through your browser (OAuth 2.1, PKCE) and keeps the tokens in ~/.pulse/secrets/mcp-oauth/, refreshing them itself. A provider that registers no clients (GitHub) signs in through a public client PULSE carries - the GitHub CLI's app, the one github-models uses - with the device flow: PULSE shows a code to type on github.com, where the page asks you to authorize GitHub CLI; nothing to register. --client-id and --client-secret are for a client you registered with the provider by hand instead; --redirect-port fixes the loopback port such a client was registered with. When the provider offers the device flow (RFC 8628) your client needs no secret and no port (--flow device, the default for such a client). The hub's Connections page runs the same sign-in from the browser.\n\nA server is for every project unless --project P names the one project that may use it; the project of your shell (PULSE_PROJECT) does not narrow it. A project's own server replaces a shared one with the same name.".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_mcp, flags:Some(mcp_flags) });
        pulse_cli::register(Command { name:"skill".into(), group:"agents".into(), summary:"skills shared between agents - a setup, a debugging recipe, a check: add, list, show, update, enable, disable".into(), usage:"pulse skill add --name N --description D (--file F | --body TEXT | stdin) [--project P]\npulse skill list [--project P] [--json]\npulse skill show N|SKILL-NNN [--json]      (prints the body)\npulse skill update N|SKILL-NNN [--name N] [--description D] [--file F | --body TEXT | stdin] [--project P]\npulse skill enable|disable N|SKILL-NNN\n\nA skill is for every project unless --project P names the one project that may use it; the project of your shell (PULSE_PROJECT) does not narrow it.".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_skill, flags:Some(skill_flags) });
    });
}

fn mcp_flags(fs: &mut FlagSet) {
    fs.string_var("name", "", "the server's name, a slug (add, update)");
    scope_flag(fs, "server");
    fs.string_var("transport", "", "stdio, http or sse (add, update)");
    fs.string_var("command", "", "stdio: the program to run (add, update)");
    fs.strings_var(
        "arg",
        "stdio: one `argument` of the command; repeat for more (update: --arg '' clears them)",
    );
    fs.string_var("url", "", "http, sse: the server's address (add, update)");
    fs.string_var(
        "auth",
        "",
        "none, bearer, basic, header, query, env or oauth (add, update; default none)",
    );
    fs.string_var(
        "auth-header",
        "",
        "the header, query parameter or env var that carries the secret (header, query, env)",
    );
    fs.string_var(
        "secret",
        "",
        "a pulse_cred `reference` to the secret (every auth but none)",
    );
    fs.strings_var(
        "env",
        "stdio: `K=V` for the command's environment; V may be a pulse_cred reference; repeat for more (update: --env '' clears them)",
    );
    fs.string_var(
        "note",
        "",
        "what the server is for; agents read it (add, update)",
    );
    fs.string_var(
        "args",
        "",
        "call: the tool's arguments as a JSON object (or on stdin)",
    );
    fs.string_var(
        "client-id",
        "",
        "login: a client `id` you registered with the provider by hand (default: PULSE registers itself, or signs in through its built-in client where the provider registers none)",
    );
    fs.string_var(
        "client-secret",
        "",
        "login: that client's secret as a pulse_cred `reference`, when it has one",
    );
    fs.string_var(
        "scope",
        "",
        "login: the `scopes` to ask for, space-separated (default: what the server suggests)",
    );
    fs.int_var(
        "redirect-port",
        0,
        "login: the loopback `port` the redirect URI uses (default: one the system picks)",
    );
    fs.bool_var(
        "no-browser",
        false,
        "login: print the sign-in address, do not open a browser",
    );
    fs.string_var(
        "flow",
        "",
        "login: `code` (a browser redirect) or `device` (a code typed on the provider's page; needs --client-id or a built-in client) (default: device when the client is yours or built in and the server offers it, else code)",
    );
}

fn skill_flags(fs: &mut FlagSet) {
    fs.string_var("name", "", "the skill's name, a slug (add, update)");
    scope_flag(fs, "skill");
    fs.string_var(
        "description",
        "",
        "one line saying what it covers (add, update)",
    );
    fs.string_var(
        "body",
        "",
        "the skill text, markdown (add, update); or --file, or stdin",
    );
    fs.string_var("file", "", "file holding the body; - for stdin");
}

/// `--project` is a global flag, but here it also chooses the scope of what
/// is added: given, the one project that may use the thing; absent, every
/// project. Binding it here keeps that reading and its help text with the
/// command.
fn scope_flag(fs: &mut FlagSet, what: &str) {
    fs.string_var(
        "project",
        "",
        &format!(
            "the one project that may use the {what} (add, update; default: every project, whatever PULSE_PROJECT says)"
        ),
    );
}

/// The scope of a new skill or server: the project `--project` names, else
/// every project. The ambient project (`PULSE_PROJECT`, set in every
/// worker's shell) never narrows it - what one project's worker adds is
/// meant for the fleet unless it says otherwise.
fn scope(fs: &FlagSet) -> String {
    if fs.is_set("project") {
        fs.get_string("project")
    } else {
        String::new()
    }
}

fn store(c: &Context) -> Result<&Store> {
    c.store
        .as_ref()
        .ok_or_else(|| anyhow!("command requires a store"))
}

fn actor(c: &Context) -> String {
    if c.agent.is_empty() {
        pulse_store::HUMAN_AGENT.into()
    } else {
        c.agent.clone()
    }
}

fn read_body(inline: &str, file: &str) -> Result<String> {
    if !inline.is_empty() {
        return Ok(inline.to_string());
    }
    if !file.is_empty() && file != "-" {
        return std::fs::read_to_string(file).map_err(|e| {
            pulse_cli::errorf(
                2,
                &format!("Cannot read {file}"),
                &e.to_string(),
                "check the path",
            )
            .into()
        });
    }
    if let Some(data) = STDIN_BUF.lock().expect("stdin lock poisoned").clone() {
        return Ok(String::from_utf8_lossy(&data).to_string());
    }
    if file == "-" || !std::io::stdin().is_terminal() {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        return Ok(s);
    }
    Ok(String::new())
}

fn usage(cmd: &str, sub: &str) -> anyhow::Error {
    let what = if sub.is_empty() {
        format!("pulse {cmd} needs a subcommand")
    } else {
        format!("pulse {cmd}: unknown subcommand {sub:?}")
    };
    pulse_cli::errorf(2, &what, "see the usage", &format!("pulse help {cmd}")).into()
}

fn need(cmd: &str, args: &[String], n: usize, fix: &str) -> Result<()> {
    if args.len() < n {
        Err(pulse_cli::errorf(
            2,
            "Missing argument",
            &format!("the command needs {fix}"),
            &format!("pulse help {cmd}"),
        )
        .into())
    } else {
        Ok(())
    }
}

fn printf(c: &mut Context, args: std::fmt::Arguments<'_>) -> Result<()> {
    c.printf(args)?;
    Ok(())
}

/// `--env K=V` pairs into a map; `--env ''` alone means "none".
fn parse_env(pairs: &[String]) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for p in pairs {
        if p.is_empty() {
            continue;
        }
        let Some((k, v)) = p.split_once('=') else {
            return Err(pulse_cli::errorf(
                2,
                &format!("Invalid --env {p:?}"),
                "an env entry is NAME=VALUE",
                "pass --env GITHUB_HOST=github.example.com (the value may be a pulse_cred reference)",
            )
            .into());
        };
        env.insert(k.trim().to_string(), v.to_string());
    }
    Ok(env)
}

fn clean_args(args: Vec<String>) -> Vec<String> {
    if args.len() == 1 && args[0].is_empty() {
        Vec::new()
    } else {
        args
    }
}

pub fn resolver(home: &str) -> pulse_cred::Resolver {
    let mut r = pulse_cred::new(home);
    r.timeout = RESOLVER_TIMEOUT;
    r
}

/// What `connect` found.
pub struct Checked {
    pub server: McpServer,
    pub info: client::ServerInfo,
    pub tools: Vec<ToolInfo>,
}

/// Connects, lists the tools and records the outcome on the row - the one
/// place a server's last check is written. The error is already redacted.
pub fn connect_and_record(
    s: &Store,
    actor: &str,
    server: &McpServer,
    resolver: &pulse_cred::Resolver,
    connect_timeout: Duration,
) -> Result<Checked> {
    let outcome = client::prepare(server, resolver)
        .and_then(|wire| Client::connect_with(server, wire, connect_timeout))
        .and_then(|mut cl| {
            let tools = cl.tools()?;
            Ok((cl.server.clone(), tools))
        });
    match outcome {
        Ok((info, tools)) => {
            let note = client::check_note(&info, &tools);
            let server = s.record_mcp_check(actor, &server.id, true, &note)?;
            Ok(Checked {
                server,
                info,
                tools,
            })
        }
        Err(e) => {
            let text = pulse_cred::redact(&e.to_string(), &[]);
            let _ = s.record_mcp_check(actor, &server.id, false, &text);
            Err(e)
        }
    }
}

/// The user-facing error for a failed connect: what, why (redacted) and a fix.
pub fn connect_error(server: &McpServer, e: anyhow::Error) -> anyhow::Error {
    if let Some(ln) = e.downcast_ref::<crate::oauth::LoginNeeded>() {
        return pulse_cli::errorf(
            1,
            &format!("{} needs a sign-in", server.name),
            &ln.why,
            &format!("pulse mcp login {}", server.name),
        )
        .into();
    }
    if let Some(ce) = e.downcast_ref::<client::CredentialError>() {
        return pulse_cli::errorf(
            1,
            &format!("Cannot read the secret of {}", server.name),
            &ce.to_string(),
            &format!(
                "check the reference ({}) and that this machine can read it; pulse mcp update {} --secret REF changes it",
                pulse_cred::SHAPES,
                server.name
            ),
        )
        .into();
    }
    pulse_cli::errorf(
        1,
        &format!("Cannot reach {} ({})", server.name, server.endpoint()),
        &pulse_cred::redact(&e.to_string(), &[]),
        &match (server.transport.as_str(), server.auth.as_str()) {
            ("stdio", _) => "check that the command is installed for this user and starts by hand; its output is in the why line".to_string(),
            (_, "oauth") => format!(
                "check the url; when the server turned the token down, sign in again: pulse mcp login {}",
                server.name
            ),
            ("sse", _) => "check the url and the credential; a Streamable HTTP server wants --transport http".to_string(),
            _ => "check the url and the credential; an older server may want --transport sse".to_string(),
        },
    )
    .into()
}

fn open(c: &Context, key: &str) -> Result<(McpServer, Client)> {
    let s = store(c)?;
    let server = s.mcp_server(&c.project, key)?;
    if !server.enabled() {
        return Err(pulse_cli::errorf(
            1,
            &format!("{} is disabled", server.name),
            "a disabled server is not called",
            &format!("pulse mcp enable {}", server.name),
        )
        .into());
    }
    let r = resolver(&c.home);
    let cl = Client::connect(&server, &r).map_err(|e| connect_error(&server, e))?;
    Ok((server, cl))
}

fn last_check(m: &McpServer) -> String {
    match m.last_check_ok {
        None => "never".to_string(),
        Some(true) => format!("ok {}", m.last_check_at),
        Some(false) => format!("failed {}", m.last_check_at),
    }
}

fn print_server(c: &mut Context, m: &McpServer) -> Result<()> {
    printf(c, format_args!("{} {}\n", m.id, m.name))?;
    printf(
        c,
        format_args!(
            "  project:    {}\n",
            if m.project.is_empty() {
                "(every project)"
            } else {
                &m.project
            }
        ),
    )?;
    printf(c, format_args!("  transport:  {}\n", m.transport))?;
    if m.transport == "stdio" {
        printf(c, format_args!("  command:    {}\n", m.endpoint()))?;
        if !m.env.is_empty() {
            let env = m
                .env
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ");
            printf(c, format_args!("  env:        {env}\n"))?;
        }
    } else {
        printf(c, format_args!("  url:        {}\n", m.url))?;
    }
    let auth = match m.auth.as_str() {
        "none" => "none".to_string(),
        "oauth" => format!("oauth ({})", crate::oauth::describe(m, &c.home)),
        "bearer" | "basic" => format!("{} ({})", m.auth, m.secret_ref),
        _ => format!("{} {} ({})", m.auth, m.auth_header, m.secret_ref),
    };
    printf(c, format_args!("  auth:       {auth}\n"))?;
    printf(c, format_args!("  status:     {}\n", m.status))?;
    printf(c, format_args!("  last check: {}\n", last_check(m)))?;
    if !m.last_check_note.is_empty() {
        printf(c, format_args!("              {}\n", m.last_check_note))?;
    }
    if !m.note.is_empty() {
        printf(c, format_args!("  note:       {}\n", m.note))?;
    }
    printf(
        c,
        format_args!("  added:      {} by {}\n", m.created_at, m.created_by),
    )?;
    Ok(())
}

fn print_tools(c: &mut Context, tools: &[ToolInfo]) -> Result<()> {
    if tools.is_empty() {
        c.println("No tools.")?;
        return Ok(());
    }
    let rows = tools
        .iter()
        .map(|t| {
            vec![
                t.name.clone(),
                t.description
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(100)
                    .collect(),
            ]
        })
        .collect::<Vec<_>>();
    pulse_cli::table(&mut c.out, c.color, Some(&["TOOL", "DESCRIPTION"]), &rows)?;
    Ok(())
}

pub fn run_mcp(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(usage("mcp", ""));
    }
    let (sub, rest) = (&args[0], &args[1..]);
    match sub.as_str() {
        "add" => {
            let m = store(c)?.add_mcp_server(
                &actor(c),
                McpServerInput {
                    name: fs.get_string("name"),
                    project: scope(fs),
                    transport: fs.get_string("transport"),
                    command: fs.get_string("command"),
                    args: clean_args(fs.get_strings("arg")),
                    url: fs.get_string("url"),
                    auth: fs.get_string("auth"),
                    auth_header: fs.get_string("auth-header"),
                    secret_ref: fs.get_string("secret"),
                    env: parse_env(&fs.get_strings("env"))?,
                    note: fs.get_string("note"),
                },
            )?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ServerOut { server: &m });
            }
            printf(
                c,
                format_args!(
                    "{} added: {} ({} {}){}; try pulse mcp connect {}\n",
                    m.id,
                    m.name,
                    m.transport,
                    m.endpoint(),
                    if m.project.is_empty() {
                        " for every project".to_string()
                    } else {
                        format!(" in {}", m.project)
                    },
                    m.name
                ),
            )
        }
        "list" => {
            let list = store(c)?.mcp_servers(&c.project)?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ServersOut { servers: &list });
            }
            if list.is_empty() {
                c.println("No MCP servers. Add one: pulse mcp add --name N --transport stdio --command CMD")?;
                return Ok(());
            }
            let rows = list
                .iter()
                .map(|m| {
                    vec![
                        m.id.clone(),
                        m.name.clone(),
                        if m.project.is_empty() {
                            "*".to_string()
                        } else {
                            m.project.clone()
                        },
                        m.transport.clone(),
                        m.auth.clone(),
                        m.status.clone(),
                        last_check(m),
                        m.note
                            .lines()
                            .next()
                            .unwrap_or("")
                            .chars()
                            .take(60)
                            .collect(),
                    ]
                })
                .collect::<Vec<_>>();
            pulse_cli::table(
                &mut c.out,
                c.color,
                Some(&[
                    "ID",
                    "NAME",
                    "PROJECT",
                    "TRANSPORT",
                    "AUTH",
                    "STATUS",
                    "LAST CHECK",
                    "NOTE",
                ]),
                &rows,
            )?;
            Ok(())
        }
        "show" => {
            need("mcp", rest, 1, "a server name or id")?;
            let m = store(c)?.mcp_server(&c.project, &rest[0])?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ServerOut { server: &m });
            }
            print_server(c, &m)
        }
        "update" => {
            need("mcp", rest, 1, "a server name or id")?;
            let s = store(c)?;
            let m = s.mcp_server(&c.project, &rest[0])?;
            let set = |name: &str| fs.is_set(name).then(|| fs.get_string(name));
            let patch = McpServerPatch {
                name: set("name"),
                project: set("project"),
                transport: set("transport"),
                command: set("command"),
                args: fs.is_set("arg").then(|| clean_args(fs.get_strings("arg"))),
                url: set("url"),
                auth: set("auth"),
                auth_header: set("auth-header"),
                secret_ref: set("secret"),
                env: if fs.is_set("env") {
                    Some(parse_env(&fs.get_strings("env"))?)
                } else {
                    None
                },
                note: set("note"),
            };
            if patch.is_empty() {
                return Err(pulse_cli::errorf(
                    2,
                    "Nothing to update",
                    "no flag was given",
                    "pass the fields to change, e.g. --note TEXT or --secret REF",
                )
                .into());
            }
            let m = s.update_mcp_server(&actor(c), &m.id, patch)?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ServerOut { server: &m });
            }
            printf(c, format_args!("{} updated: {}\n", m.id, m.name))
        }
        "enable" | "disable" => {
            need("mcp", rest, 1, "a server name or id")?;
            let s = store(c)?;
            let m = s.mcp_server(&c.project, &rest[0])?;
            let status = if sub == "enable" {
                "enabled"
            } else {
                "disabled"
            };
            let m = s.set_mcp_server_status(&actor(c), &m.id, status)?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ServerOut { server: &m });
            }
            printf(c, format_args!("{} {}: {}\n", m.id, m.status, m.name))
        }
        "login" => {
            need("mcp", rest, 1, "a server name or id")?;
            let s = store(c)?;
            let m = s.mcp_server(&c.project, &rest[0])?;
            if m.auth != "oauth" {
                return Err(pulse_cli::errorf(
                    2,
                    &format!("{} does not sign in with OAuth", m.name),
                    &format!("its auth is {}", m.auth),
                    &format!(
                        "pulse mcp update {} --auth oauth --secret '' switches it, then pulse mcp login {}",
                        m.name, m.name
                    ),
                )
                .into());
            }
            let port = fs.get_int("redirect-port");
            if !(0..=65535).contains(&port) {
                return Err(pulse_cli::errorf(
                    2,
                    "Invalid --redirect-port",
                    &format!("{port} is not a port"),
                    "pass a number between 1 and 65535",
                )
                .into());
            }
            let flow = fs.get_string("flow");
            if !matches!(flow.as_str(), "" | "code" | "device") {
                return Err(pulse_cli::errorf(
                    2,
                    "Invalid --flow",
                    &format!("{flow} is not a sign-in flow PULSE knows"),
                    "pass code (a browser redirect) or device (a code typed on the provider's page)",
                )
                .into());
            }
            let opts = crate::oauth::LoginOptions {
                client_id: fs.get_string("client-id"),
                client_secret_ref: fs.get_string("client-secret"),
                scope: fs.get_string("scope"),
                redirect_port: port as u16,
                open_browser: !fs.get_bool("no-browser"),
                flow,
            };
            let r = resolver(&c.home);
            let quiet = c.json || c.quiet;
            let mut report = |line: &str| {
                if !quiet {
                    let _ = c.println(format!("  {line}"));
                }
            };
            let login = crate::oauth::login(&m, &r, &opts, &mut report).map_err(|e| {
                pulse_cli::errorf(
                    1,
                    &format!("Cannot sign in to {}", m.name),
                    &pulse_cred::redact(&e.to_string(), &[]),
                    "fix what the why line names and run the login again; pulse mcp show NAME shows the server",
                )
            })?;
            let g = &login.grant;
            let label = crate::oauth::built_in_label(&g.client_id).unwrap_or_default();
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &LoginOut {
                        server: &m.name,
                        issuer: &g.issuer,
                        client_id: &g.client_id,
                        registered: g.registered,
                        built_in: !label.is_empty(),
                        client_label: &label,
                        scope: &g.scope,
                        expires_at: g.expires_at,
                        has_refresh_token: !g.refresh_token.is_empty(),
                        grant_file: &login.path.display().to_string(),
                    },
                );
            }
            let life = match g.expires_in(crate::oauth::now_unix()) {
                Some(secs) => format!(
                    "; the token expires in {}{}",
                    crate::oauth::humanize(secs),
                    if g.refresh_token.is_empty() {
                        " and the server gave no refresh token"
                    } else {
                        " and refreshes itself"
                    }
                ),
                None => String::new(),
            };
            printf(
                c,
                format_args!(
                    "{}: signed in at {} {}{}{life}; kept in {}\nnext: pulse mcp connect {}\n",
                    m.name,
                    g.issuer,
                    if label.is_empty() {
                        format!("as client {}", g.client_id)
                    } else {
                        format!(
                            "through the built-in client {} ({label}'s public app)",
                            g.client_id
                        )
                    },
                    if g.scope.is_empty() {
                        String::new()
                    } else {
                        format!(" (scope {})", g.scope)
                    },
                    login.path.display(),
                    m.name
                ),
            )
        }
        "logout" => {
            need("mcp", rest, 1, "a server name or id")?;
            let s = store(c)?;
            let m = s.mcp_server(&c.project, &rest[0])?;
            let r = resolver(&c.home);
            let out = crate::oauth::logout(&m, &r)?;
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &LogoutOut {
                        server: &m.name,
                        had_grant: out.had_grant,
                        revoked: out.revoked,
                        note: &out.revoke_note,
                    },
                );
            }
            if !out.had_grant {
                return printf(
                    c,
                    format_args!("{}: nothing to forget, it was not signed in\n", m.name),
                );
            }
            printf(
                c,
                format_args!(
                    "{}: signed out; the tokens are {} and forgotten here\n",
                    m.name,
                    if out.revoked {
                        "revoked at the authorization server".to_string()
                    } else {
                        format!("not revoked ({})", out.revoke_note)
                    }
                ),
            )
        }
        "connect" => {
            need("mcp", rest, 1, "a server name or id")?;
            let s = store(c)?;
            let m = s.mcp_server(&c.project, &rest[0])?;
            let r = resolver(&c.home);
            let checked = connect_and_record(s, &actor(c), &m, &r, client::CONNECT_TIMEOUT)
                .map_err(|e| connect_error(&m, e))?;
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &ConnectOut {
                        server: &checked.server,
                        info: &checked.info,
                        tools: &checked.tools,
                    },
                );
            }
            printf(
                c,
                format_args!(
                    "{}: connected to {}{} (protocol {})\n",
                    m.name,
                    if checked.info.name.is_empty() {
                        "the server".to_string()
                    } else {
                        checked.info.name.clone()
                    },
                    if checked.info.version.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", checked.info.version)
                    },
                    checked.info.protocol_version
                ),
            )?;
            if !checked.info.instructions.is_empty() {
                let text: String = checked.info.instructions.chars().take(600).collect();
                printf(c, format_args!("{text}\n"))?;
            }
            print_tools(c, &checked.tools)
        }
        "tools" => {
            need("mcp", rest, 1, "a server name or id")?;
            let (m, mut cl) = open(c, &rest[0])?;
            let tools = cl.tools()?;
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &ToolsOut {
                        server: &m.name,
                        tools: &tools,
                    },
                );
            }
            print_tools(c, &tools)
        }
        "call" => {
            need("mcp", rest, 2, "a server name and a tool name")?;
            let raw = read_body(&fs.get_string("args"), "")?;
            let arguments = if raw.trim().is_empty() {
                Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str::<Value>(raw.trim()).map_err(|e| {
                    pulse_cli::errorf(
                        2,
                        "Invalid --args",
                        &format!("not JSON: {e}"),
                        "pass the tool's arguments as a JSON object, e.g. --args '{\"owner\":\"x\",\"repo\":\"y\"}'",
                    )
                })?
            };
            if !arguments.is_object() {
                return Err(pulse_cli::errorf(
                    2,
                    "Invalid --args",
                    "the arguments must be a JSON object",
                    "pass --args '{...}'",
                )
                .into());
            }
            let (m, mut cl) = open(c, &rest[0])?;
            let tool = &rest[1];
            let result = cl.call_tool(tool, arguments)?;
            if c.json {
                pulse_cli::write_json(
                    &mut c.out,
                    &CallOut {
                        server: &m.name,
                        tool,
                        is_error: result.is_error,
                        text: &result.text,
                        content: &result.content,
                        structured: &result.structured,
                    },
                )?;
            } else if !result.text.is_empty() {
                printf(c, format_args!("{}\n", result.text))?;
            }
            if result.is_error {
                return Err(
                    pulse_cli::errorf(1, &format!("{tool} reported an error"), "", "").into(),
                );
            }
            Ok(())
        }
        "resources" => {
            need("mcp", rest, 1, "a server name or id")?;
            let (m, mut cl) = open(c, &rest[0])?;
            let resources = cl.resources()?;
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &ResourcesOut {
                        server: &m.name,
                        resources: &resources,
                    },
                );
            }
            if resources.is_empty() {
                c.println("No resources.")?;
                return Ok(());
            }
            let rows = resources
                .iter()
                .map(|r| {
                    vec![
                        r.uri.clone(),
                        r.name.clone(),
                        r.mime_type.clone(),
                        r.description
                            .lines()
                            .next()
                            .unwrap_or("")
                            .chars()
                            .take(80)
                            .collect(),
                    ]
                })
                .collect::<Vec<_>>();
            pulse_cli::table(
                &mut c.out,
                c.color,
                Some(&["URI", "NAME", "TYPE", "DESCRIPTION"]),
                &rows,
            )?;
            Ok(())
        }
        "read" => {
            need("mcp", rest, 2, "a server name and a resource uri")?;
            let (m, mut cl) = open(c, &rest[0])?;
            let result = cl.read_resource(&rest[1])?;
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &ReadOut {
                        server: &m.name,
                        uri: &rest[1],
                        text: &result.text,
                        contents: &result.contents,
                    },
                );
            }
            printf(c, format_args!("{}\n", result.text))
        }
        other => Err(usage("mcp", other)),
    }
}

pub fn run_skill(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(usage("skill", ""));
    }
    let (sub, rest) = (&args[0], &args[1..]);
    match sub.as_str() {
        "add" => {
            let body = read_body(&fs.get_string("body"), &fs.get_string("file"))?;
            let k = store(c)?.add_skill(
                &actor(c),
                SkillInput {
                    name: fs.get_string("name"),
                    project: scope(fs),
                    description: fs.get_string("description"),
                    body,
                    origin: String::new(),
                },
            )?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &SkillOut { skill: &k });
            }
            printf(
                c,
                format_args!(
                    "{} added: {}{} - {}\n",
                    k.id,
                    k.name,
                    if k.project.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", k.project)
                    },
                    k.description
                ),
            )
        }
        "list" => {
            let list = store(c)?.skills(&c.project)?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &SkillsOut { skills: &list });
            }
            if list.is_empty() {
                c.println("No skills. Add one: pulse skill add --name N --description D --file F")?;
                return Ok(());
            }
            let rows = list
                .iter()
                .map(|k| {
                    vec![
                        k.id.clone(),
                        k.name.clone(),
                        if k.project.is_empty() {
                            "*".to_string()
                        } else {
                            k.project.clone()
                        },
                        k.status.clone(),
                        k.uses.to_string(),
                        if k.from_repo() {
                            "repository".to_string()
                        } else {
                            k.created_by.clone()
                        },
                        k.description.chars().take(80).collect(),
                    ]
                })
                .collect::<Vec<_>>();
            pulse_cli::table(
                &mut c.out,
                c.color,
                Some(&[
                    "ID",
                    "NAME",
                    "PROJECT",
                    "STATUS",
                    "USES",
                    "BY",
                    "DESCRIPTION",
                ]),
                &rows,
            )?;
            Ok(())
        }
        "show" => {
            need("skill", rest, 1, "a skill name or id")?;
            let s = store(c)?;
            let k = s.skill(&rest[0])?;
            s.bump_skill_uses(&k.id)?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &SkillOut { skill: &k });
            }
            printf(c, format_args!("{}\n", k.body))
        }
        "update" => {
            need("skill", rest, 1, "a skill name or id")?;
            let s = store(c)?;
            let k = s.skill(&rest[0])?;
            let body = read_body(&fs.get_string("body"), &fs.get_string("file"))?;
            let patch = SkillPatch {
                name: fs.is_set("name").then(|| fs.get_string("name")),
                project: fs.is_set("project").then(|| fs.get_string("project")),
                description: fs
                    .is_set("description")
                    .then(|| fs.get_string("description")),
                body: (!body.is_empty()).then_some(body),
                origin: None,
            };
            if patch == SkillPatch::default() {
                return Err(pulse_cli::errorf(
                    2,
                    "Nothing to update",
                    "no flag was given",
                    "pass the fields to change, e.g. --description D or --file F",
                )
                .into());
            }
            let k = s.update_skill(&actor(c), &k.id, patch)?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &SkillOut { skill: &k });
            }
            printf(c, format_args!("{} updated: {}\n", k.id, k.name))
        }
        "enable" | "disable" => {
            need("skill", rest, 1, "a skill name or id")?;
            let s = store(c)?;
            let k = s.skill(&rest[0])?;
            let status = if sub == "enable" {
                "enabled"
            } else {
                "disabled"
            };
            let k = s.set_skill_status(&actor(c), &k.id, status)?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &SkillOut { skill: &k });
            }
            printf(c, format_args!("{} {}: {}\n", k.id, k.status, k.name))
        }
        other => Err(usage("skill", other)),
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use std::path::{Path, PathBuf};

    /// A stdio MCP server in POSIX sh: answers initialize, tools/list,
    /// tools/call (echo: returns its arguments; fail: isError), resources/list
    /// and resources/read. It echoes `$FAKE_TOKEN` in the `whoami` tool so
    /// tests can see env auth arrive - and that it is redacted on the way out.
    pub(crate) const FAKE_SERVER: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"fake-mcp\",\"version\":\"0.1.0\"},\"instructions\":\"Be nice.\"}}" ;;
    *'"method":"notifications/initialized"'*) ;;
    *'"method":"tools/list"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":"listing"}}'
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"tools\":[{\"name\":\"echo\",\"description\":\"Echoes its arguments\",\"inputSchema\":{\"type\":\"object\"}},{\"name\":\"fail\",\"description\":\"Always fails\"},{\"name\":\"whoami\",\"description\":\"Shows the token\"}]}}" ;;
    *'"method":"tools/call"'*'"name":"echo"'*)
      args=$(printf '%s' "$line" | sed -n 's/.*"arguments":\(.*\),"name":"echo"}}$/\1/p')
      esc=$(printf '%s' "$args" | sed 's/\\/\\\\/g; s/"/\\"/g')
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"echo: $esc\"}],\"isError\":false}}" ;;
    *'"method":"tools/call"'*'"name":"fail"'*)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"it broke\"}],\"isError\":true}}" ;;
    *'"method":"tools/call"'*'"name":"whoami"'*)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"token=$FAKE_TOKEN other=$OTHER\"}]}}" ;;
    *'"method":"tools/call"'*)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{\"code\":-32602,\"message\":\"Unknown tool\"}}" ;;
    *'"method":"resources/list"'*)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"resources\":[{\"uri\":\"fake://readme\",\"name\":\"README\",\"mimeType\":\"text/plain\"}]}}" ;;
    *'"method":"resources/read"'*)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"contents\":[{\"uri\":\"fake://readme\",\"mimeType\":\"text/plain\",\"text\":\"hello from the fake\"}]}}" ;;
    *)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{\"code\":-32601,\"message\":\"Method not found\"}}" ;;
  esac
done
"#;

    pub(crate) fn write_script(dir: &Path, name: &str, text: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    pub(crate) fn fake_server(dir: &Path) -> PathBuf {
        write_script(dir, "fake-mcp.sh", FAKE_SERVER)
    }

    /// The initialize/tools/call answers a Streamable HTTP fake gives; `sse`
    /// wraps them as an event stream.
    pub(crate) fn http_answer(
        req: &serde_json::Value,
        sse: bool,
    ) -> pulse_testutil::httptest::Response {
        use pulse_testutil::httptest::Response;
        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if method.starts_with("notifications/") {
            return Response::new(202);
        }
        let result = match method {
            "initialize" => {
                serde_json::json!({"protocolVersion": "2025-03-26", "capabilities": {}, "serverInfo": {"name": "http-fake", "version": "2.0"}})
            }
            "tools/list" => serde_json::json!({"tools": [{"name": "ping", "description": "pong"}]}),
            "tools/call" => {
                serde_json::json!({"content": [{"type": "text", "text": format!("called {}", req["params"]["name"].as_str().unwrap_or(""))}]})
            }
            _ => {
                return Response::json(
                    200,
                    &serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}}),
                );
            }
        };
        let body = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result});
        if sse {
            let text = format!(": keepalive\n\nevent: message\ndata: {body}\n\n");
            Response::new(200)
                .header("Content-Type", "text/event-stream")
                .header("Mcp-Session-Id", "sess-1")
                .body(text.into_bytes())
        } else {
            Response::json(200, &body).header("Mcp-Session-Id", "sess-1")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::*;
    use super::*;
    use std::sync::Arc;

    struct TestStore {
        _home: tempfile::TempDir,
        store: Store,
    }
    impl std::ops::Deref for TestStore {
        type Target = Store;
        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }

    fn new_store() -> TestStore {
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let home = tempfile::tempdir().unwrap();
        let s = Store::open(pulse_store::Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES ('demo', 'Demo', ?, ?)",
                rusqlite::params![home.path().join("demo").display().to_string(), s.now()],
            )
            .unwrap();
        TestStore {
            _home: home,
            store: s,
        }
    }

    fn ctx_for(s: &TestStore, json: bool) -> (Context, pulse_cli::Buffer) {
        let out = pulse_cli::Buffer::new();
        (
            Context {
                store: Some(s.store.clone()),
                home: s.home.clone(),
                out: Box::new(out.clone()),
                err: Box::new(out.clone()),
                json,
                quiet: false,
                color: false,
                agent: String::new(),
                project: String::new(),
                session: String::new(),
                now: "2026-09-06T21:00:00Z".into(),
                args: vec![],
            },
            out,
        )
    }

    fn run_cmd(c: &mut Context, name: &str, args: &[&str]) -> Result<()> {
        register();
        let cmd = pulse_cli::lookup(name).expect("command");
        let mut fs = FlagSet::new(name);
        if let Some(flags) = cmd.flags {
            flags(&mut fs);
        }
        let raw = args.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        let positional = pulse_cli::flags::parse_interleaved(&mut fs, &raw)?;
        (cmd.run)(c, &fs, &positional)
    }

    fn secret_file(dir: &std::path::Path, name: &str, value: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("{value}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        format!("file://{}", p.display())
    }

    #[test]
    fn mcp_registry_commands() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (mut c, out) = ctx_for(&s, false);
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "github",
                "--transport",
                "stdio",
                "--command",
                "npx",
                "--arg",
                "-y",
                "--arg",
                "@modelcontextprotocol/server-github",
                "--env",
                "GITHUB_HOST=github.example.com",
                "--note",
                "Issues and PRs",
            ],
        )
        .unwrap();
        let text = out.string();
        assert!(text.contains("MCP-001 added: github (stdio npx -y @modelcontextprotocol/server-github) for every project"), "{text}");
        out.clear();

        // the shell's project (PULSE_PROJECT in a worker's shell) does not
        // narrow a new server; only an explicit --project does
        c.project = "demo".into();
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "docs",
                "--transport",
                "http",
                "--url",
                "https://docs.example/mcp",
                "--auth",
                "bearer",
                "--secret",
                "keychain://pulse/docs",
            ],
        )
        .unwrap();
        assert!(
            out.string()
                .contains("MCP-002 added: docs (http https://docs.example/mcp) for every project"),
            "{}",
            out.string()
        );
        out.clear();
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "docs",
                "--project",
                "demo",
                "--transport",
                "http",
                "--url",
                "https://docs.demo.example/mcp",
                "--auth",
                "bearer",
                "--secret",
                "keychain://pulse/docs-demo",
            ],
        )
        .unwrap();
        assert!(
            out.string()
                .contains("MCP-003 added: docs (http https://docs.demo.example/mcp) in demo"),
            "{}",
            out.string()
        );
        out.clear();
        // --project on update re-scopes; nothing else does
        run_cmd(&mut c, "mcp", &["update", "github", "--project", "demo"]).unwrap();
        assert_eq!(s.store.mcp_server_by_id("MCP-001").unwrap().project, "demo");
        run_cmd(
            &mut c,
            "mcp",
            &["update", "github", "--note", "Issues and PRs"],
        )
        .unwrap();
        assert_eq!(s.store.mcp_server_by_id("MCP-001").unwrap().project, "demo");
        run_cmd(&mut c, "mcp", &["update", "github", "--project", ""]).unwrap();
        assert_eq!(s.store.mcp_server_by_id("MCP-001").unwrap().project, "");
        out.clear();
        c.project.clear();

        let err = run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "bad",
                "--transport",
                "http",
                "--url",
                "https://x.example",
                "--auth",
                "bearer",
            ],
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("secret reference is needed"),
            "{err}"
        );
        let err = run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "bad",
                "--transport",
                "stdio",
                "--command",
                "x",
                "--env",
                "NOEQUALS",
            ],
        )
        .unwrap_err();
        assert!(err.to_string().contains("Invalid --env"), "{err}");

        run_cmd(&mut c, "mcp", &["list"]).unwrap();
        let text = out.string();
        assert!(
            text.contains("MCP-001") && text.contains("github") && text.contains("never"),
            "{text}"
        );
        assert!(
            text.contains("MCP-002") && text.contains("demo") && text.contains("bearer"),
            "{text}"
        );
        out.clear();

        run_cmd(&mut c, "mcp", &["show", "github"]).unwrap();
        let text = out.string();
        assert!(
            text.contains("command:    npx -y @modelcontextprotocol/server-github"),
            "{text}"
        );
        assert!(
            text.contains("env:        GITHUB_HOST=github.example.com"),
            "{text}"
        );
        assert!(text.contains("note:       Issues and PRs"), "{text}");
        out.clear();

        run_cmd(
            &mut c,
            "mcp",
            &[
                "update",
                "github",
                "--note",
                "GitHub issues",
                "--arg",
                "",
                "--env",
                "",
            ],
        )
        .unwrap();
        assert!(
            out.string().contains("MCP-001 updated: github"),
            "{}",
            out.string()
        );
        let m = s.mcp_server_by_id("MCP-001").unwrap();
        assert!(m.args.is_empty() && m.env.is_empty());
        assert_eq!(m.note, "GitHub issues");
        out.clear();
        let err = run_cmd(&mut c, "mcp", &["update", "github"]).unwrap_err();
        assert!(err.to_string().contains("Nothing to update"), "{err}");

        run_cmd(&mut c, "mcp", &["disable", "github"]).unwrap();
        assert!(
            out.string().contains("MCP-001 disabled: github"),
            "{}",
            out.string()
        );
        assert_eq!(s.mcp_server_by_id("MCP-001").unwrap().status, "disabled");
        out.clear();
        let err = run_cmd(&mut c, "mcp", &["tools", "github"]).unwrap_err();
        assert!(err.to_string().contains("github is disabled"), "{err}");
        run_cmd(&mut c, "mcp", &["enable", "MCP-001"]).unwrap();
        assert!(
            out.string().contains("MCP-001 enabled: github"),
            "{}",
            out.string()
        );
        out.clear();

        let (mut j, jout) = ctx_for(&s, true);
        run_cmd(&mut j, "mcp", &["list"]).unwrap();
        let v: Value = serde_json::from_str(&jout.string()).unwrap();
        assert_eq!(v["servers"].as_array().unwrap().len(), 3);
        assert_eq!(v["servers"][0]["name"], "docs");
        assert_eq!(v["servers"][0]["project"], "");
        assert_eq!(v["servers"][1]["name"], "github");
        assert_eq!(v["servers"][2]["project"], "demo");
        assert_eq!(v["servers"][2]["secret_ref"], "keychain://pulse/docs-demo");

        let err = run_cmd(&mut c, "mcp", &["show", "nothing"]).unwrap_err();
        assert!(
            err.to_string().starts_with("No MCP server named nothing"),
            "{err}"
        );
        let err = run_cmd(&mut c, "mcp", &["bogus"]).unwrap_err();
        assert!(err.to_string().contains("unknown subcommand"), "{err}");
        let err = run_cmd(&mut c, "mcp", &[]).unwrap_err();
        assert!(err.to_string().contains("needs a subcommand"), "{err}");
    }

    #[test]
    fn connect_tools_call_and_read_through_a_stdio_server() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let dir = tempfile::tempdir().unwrap();
        let script = fake_server(dir.path());
        let token_ref = secret_file(dir.path(), "token", "tok-very-secret-123");
        let other_ref = secret_file(dir.path(), "other", "other-secret-456");
        let (mut c, out) = ctx_for(&s, false);
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "fake",
                "--transport",
                "stdio",
                "--command",
                script.to_str().unwrap(),
                "--auth",
                "env",
                "--auth-header",
                "FAKE_TOKEN",
                "--secret",
                &token_ref,
                "--env",
                &format!("OTHER={other_ref}"),
                "--note",
                "A fake",
            ],
        )
        .unwrap();
        out.clear();

        run_cmd(&mut c, "mcp", &["connect", "fake"]).unwrap();
        let text = out.string();
        assert!(
            text.contains("fake: connected to fake-mcp 0.1.0 (protocol 2025-06-18)"),
            "{text}"
        );
        assert!(text.contains("Be nice."), "{text}");
        assert!(
            text.contains("echo")
                && text.contains("Echoes its arguments")
                && text.contains("whoami"),
            "{text}"
        );
        let m = s.mcp_server_by_id("MCP-001").unwrap();
        assert_eq!(m.last_check_ok, Some(true));
        assert_eq!(
            m.last_check_note,
            "fake-mcp 0.1.0: 3 tools: echo, fail, whoami"
        );
        out.clear();

        run_cmd(&mut c, "mcp", &["tools", "fake"]).unwrap();
        assert!(out.string().contains("Always fails"), "{}", out.string());
        out.clear();

        run_cmd(
            &mut c,
            "mcp",
            &["call", "fake", "echo", "--args", r#"{"a":1,"b":"two"}"#],
        )
        .unwrap();
        assert_eq!(out.string().trim(), r#"echo: {"a":1,"b":"two"}"#);
        out.clear();

        // The secret reached the child's environment and never reaches the output.
        run_cmd(&mut c, "mcp", &["call", "fake", "whoami"]).unwrap();
        let text = out.string();
        assert_eq!(text.trim(), "token=[redacted] other=[redacted]", "{text}");
        out.clear();

        let err = run_cmd(&mut c, "mcp", &["call", "fake", "fail"]).unwrap_err();
        let ce = err.downcast_ref::<pulse_cli::Error>().expect("cli error");
        assert_eq!(ce.exit_code(), 1);
        assert!(ce.what.contains("fail reported an error"), "{}", ce.what);
        assert_eq!(out.string().trim(), "it broke");
        out.clear();

        let err = run_cmd(&mut c, "mcp", &["call", "fake", "nope"]).unwrap_err();
        assert!(
            err.to_string()
                .contains("tools/call: Unknown tool (code -32602)"),
            "{err}"
        );
        let err = run_cmd(&mut c, "mcp", &["call", "fake", "echo", "--args", "[1]"]).unwrap_err();
        assert!(err.to_string().contains("must be a JSON object"), "{err}");
        let err = run_cmd(&mut c, "mcp", &["call", "fake", "echo", "--args", "{bad"]).unwrap_err();
        assert!(err.to_string().contains("not JSON"), "{err}");

        set_stdin_for_tests(Some(br#"{"from":"stdin"}"#.to_vec()));
        run_cmd(&mut c, "mcp", &["call", "fake", "echo"]).unwrap();
        set_stdin_for_tests(None);
        assert_eq!(out.string().trim(), r#"echo: {"from":"stdin"}"#);
        out.clear();

        run_cmd(&mut c, "mcp", &["resources", "fake"]).unwrap();
        assert!(
            out.string().contains("fake://readme") && out.string().contains("text/plain"),
            "{}",
            out.string()
        );
        out.clear();
        run_cmd(&mut c, "mcp", &["read", "fake", "fake://readme"]).unwrap();
        assert_eq!(out.string().trim(), "hello from the fake");
        out.clear();

        let (mut j, jout) = ctx_for(&s, true);
        run_cmd(&mut j, "mcp", &["call", "fake", "echo", "--args", "{}"]).unwrap();
        let v: Value = serde_json::from_str(&jout.string()).unwrap();
        assert_eq!(v["server"], "fake");
        assert_eq!(v["is_error"], false);
        assert_eq!(v["content"][0]["type"], "text");
        jout.clear();
        run_cmd(&mut j, "mcp", &["connect", "fake"]).unwrap();
        let v: Value = serde_json::from_str(&jout.string()).unwrap();
        assert_eq!(v["info"]["name"], "fake-mcp");
        assert_eq!(v["tools"].as_array().unwrap().len(), 3);
        assert_eq!(v["server"]["last_check_ok"], true);
    }

    #[test]
    fn connect_failures_are_recorded_with_a_fix() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let dir = tempfile::tempdir().unwrap();
        let (mut c, out) = ctx_for(&s, false);
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "gone",
                "--transport",
                "stdio",
                "--command",
                "/nonexistent/mcp-server",
            ],
        )
        .unwrap();
        let err = run_cmd(&mut c, "mcp", &["connect", "gone"]).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("Cannot reach gone (/nonexistent/mcp-server)"),
            "{text}"
        );
        assert!(text.contains("spawn /nonexistent/mcp-server"), "{text}");
        assert!(
            text.contains("fix: check that the command is installed"),
            "{text}"
        );
        let m = s.mcp_server_by_id("MCP-001").unwrap();
        assert_eq!(m.last_check_ok, Some(false));
        assert!(
            m.last_check_note.contains("spawn /nonexistent/mcp-server"),
            "{}",
            m.last_check_note
        );

        // A server that dies before answering: its stderr is in the error.
        let dying = write_script(
            dir.path(),
            "dying.sh",
            "#!/bin/sh\necho 'cannot start: no token' >&2\nexit 3\n",
        );
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "dying",
                "--transport",
                "stdio",
                "--command",
                dying.to_str().unwrap(),
            ],
        )
        .unwrap();
        let err = run_cmd(&mut c, "mcp", &["connect", "dying"])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("exited") || err.contains("closed its"),
            "{err}"
        );
        assert!(err.contains("cannot start: no token"), "{err}");

        // A missing credential names the reference, never a value.
        let missing = dir.path().join("missing-token");
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "needs-token",
                "--transport",
                "stdio",
                "--command",
                "/bin/cat",
                "--auth",
                "env",
                "--auth-header",
                "TOKEN",
                "--secret",
                &format!("file://{}", missing.display()),
            ],
        )
        .unwrap();
        let err = run_cmd(&mut c, "mcp", &["connect", "needs-token"])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Cannot read the secret of needs-token"),
            "{err}"
        );
        assert!(err.contains("missing-token"), "{err}");
        assert!(
            err.contains("pulse mcp update needs-token --secret REF"),
            "{err}"
        );
        let m = s.mcp_server_by_id("MCP-003").unwrap();
        assert_eq!(m.last_check_ok, Some(false));
        assert!(
            m.last_check_note.starts_with("credential file://"),
            "{}",
            m.last_check_note
        );
        let _ = out;
    }

    #[test]
    fn http_transport_sends_auth_and_reads_json_or_sse_answers() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let dir = tempfile::tempdir().unwrap();
        // (uri, authorization, mcp-session-id, x-api-key) per request
        type Seen = Vec<(String, String, String, String)>;
        let seen: Arc<Mutex<Seen>> = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let srv = pulse_testutil::httptest::Server::new(move |req| {
            let body: Value = req.json().unwrap_or(Value::Null);
            log.lock().unwrap().push((
                req.uri.clone(),
                req.header("authorization").unwrap_or("").to_string(),
                req.header("mcp-session-id").unwrap_or("").to_string(),
                req.header("x-api-key").unwrap_or("").to_string(),
            ));
            let sse = req.path().ends_with("/sse-style");
            http_answer(&body, sse)
        });
        let token_ref = secret_file(dir.path(), "token", "bearer-token-xyz");
        let (mut c, out) = ctx_for(&s, false);
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "plain",
                "--transport",
                "http",
                "--url",
                &format!("{}/mcp", srv.url()),
                "--auth",
                "bearer",
                "--secret",
                &token_ref,
            ],
        )
        .unwrap();
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "streamed",
                "--transport",
                "http",
                "--url",
                &format!("{}/sse-style", srv.url()),
                "--auth",
                "header",
                "--auth-header",
                "X-Api-Key",
                "--secret",
                &token_ref,
            ],
        )
        .unwrap();
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "queried",
                "--transport",
                "http",
                "--url",
                &format!("{}/mcp?v=1", srv.url()),
                "--auth",
                "query",
                "--auth-header",
                "key",
                "--secret",
                &token_ref,
            ],
        )
        .unwrap();
        let basic_ref = secret_file(dir.path(), "basic", "alice:s3cret");
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "basic",
                "--transport",
                "http",
                "--url",
                &format!("{}/mcp", srv.url()),
                "--auth",
                "basic",
                "--secret",
                &basic_ref,
            ],
        )
        .unwrap();
        out.clear();

        run_cmd(&mut c, "mcp", &["connect", "plain"]).unwrap();
        assert!(
            out.string()
                .contains("plain: connected to http-fake 2.0 (protocol 2025-03-26)"),
            "{}",
            out.string()
        );
        assert!(out.string().contains("ping"), "{}", out.string());
        out.clear();
        {
            let calls = seen.lock().unwrap();
            assert_eq!(calls.len(), 3, "{calls:?}");
            assert!(
                calls.iter().all(|c| c.1 == "Bearer bearer-token-xyz"),
                "{calls:?}"
            );
            assert_eq!(calls[0].2, "", "initialize carries no session yet");
            assert_eq!(calls[1].2, "sess-1", "initialized carries the session");
            assert_eq!(calls[2].2, "sess-1");
        }
        seen.lock().unwrap().clear();

        run_cmd(&mut c, "mcp", &["call", "streamed", "ping"]).unwrap();
        assert_eq!(out.string().trim(), "called ping");
        out.clear();
        {
            let calls = seen.lock().unwrap();
            assert!(
                calls
                    .iter()
                    .all(|c| c.3 == "bearer-token-xyz" && c.1.is_empty()),
                "{calls:?}"
            );
        }
        seen.lock().unwrap().clear();

        run_cmd(&mut c, "mcp", &["tools", "queried"]).unwrap();
        {
            let calls = seen.lock().unwrap();
            assert!(
                calls.iter().all(|c| c.0 == "/mcp?v=1&key=bearer-token-xyz"),
                "{calls:?}"
            );
        }
        seen.lock().unwrap().clear();
        out.clear();

        run_cmd(&mut c, "mcp", &["connect", "basic"]).unwrap();
        {
            let calls = seen.lock().unwrap();
            assert!(
                calls.iter().all(|c| c.1 == "Basic YWxpY2U6czNjcmV0"),
                "{calls:?}"
            );
        }
        let m = s.mcp_server("", "basic").unwrap();
        assert_eq!(m.last_check_note, "http-fake 2.0: 1 tool: ping");
    }

    #[test]
    fn http_errors_name_the_status_and_hide_secrets() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let dir = tempfile::tempdir().unwrap();
        let srv = pulse_testutil::httptest::Server::new(|_req| {
            pulse_testutil::httptest::Response::text(401, "unauthorized for key=bearer-token-xyz")
        });
        let token_ref = secret_file(dir.path(), "token", "bearer-token-xyz");
        let (mut c, _out) = ctx_for(&s, false);
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "locked",
                "--transport",
                "http",
                "--url",
                &format!("{}/mcp", srv.url()),
                "--auth",
                "query",
                "--auth-header",
                "key",
                "--secret",
                &token_ref,
            ],
        )
        .unwrap();
        let err = run_cmd(&mut c, "mcp", &["connect", "locked"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("HTTP 401"), "{err}");
        assert!(!err.contains("bearer-token-xyz"), "{err}");
        assert!(err.contains("[redacted]"), "{err}");
        let m = s.mcp_server("", "locked").unwrap();
        assert_eq!(m.last_check_ok, Some(false));
        assert!(
            !m.last_check_note.contains("bearer-token-xyz"),
            "{}",
            m.last_check_note
        );
    }

    #[test]
    fn legacy_sse_transport_round_trips() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let auth_seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&auth_seen);
        // One GET keeps the stream; each POST is answered 202 and its
        // response is written onto the stream.
        std::thread::spawn(move || {
            let stream: Arc<Mutex<Option<std::net::TcpStream>>> = Arc::new(Mutex::new(None));
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { break };
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut headers = std::collections::BTreeMap::new();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let h = h.trim_end().to_string();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                    }
                }
                log.lock()
                    .unwrap()
                    .push(headers.get("authorization").cloned().unwrap_or_default());
                if line.starts_with("GET") {
                    conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n").unwrap();
                    conn.write_all(b"event: endpoint\ndata: /messages?session=abc\n\n")
                        .unwrap();
                    conn.flush().unwrap();
                    *stream.lock().unwrap() = Some(conn);
                    continue;
                }
                let len: usize = headers
                    .get("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0u8; len];
                std::io::Read::read_exact(&mut reader, &mut body).unwrap();
                let req: Value = serde_json::from_slice(&body).unwrap();
                conn.write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
                conn.flush().unwrap();
                if req.get("id").is_some() {
                    let resp = match req["method"].as_str().unwrap_or("") {
                        "initialize" => {
                            serde_json::json!({"jsonrpc": "2.0", "id": req["id"], "result": {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "sse-fake", "version": "1"}}})
                        }
                        "tools/list" => {
                            serde_json::json!({"jsonrpc": "2.0", "id": req["id"], "result": {"tools": [{"name": "legacy", "description": "old style"}]}})
                        }
                        _ => {
                            serde_json::json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32601, "message": "nope"}})
                        }
                    };
                    if let Some(st) = stream.lock().unwrap().as_mut() {
                        let _ =
                            st.write_all(format!("event: message\ndata: {resp}\n\n").as_bytes());
                        let _ = st.flush();
                    }
                }
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let token_ref = secret_file(dir.path(), "token", "legacy-token");
        let (mut c, out) = ctx_for(&s, false);
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "old",
                "--transport",
                "sse",
                "--url",
                &format!("http://{addr}/sse"),
                "--auth",
                "bearer",
                "--secret",
                &token_ref,
            ],
        )
        .unwrap();
        out.clear();
        run_cmd(&mut c, "mcp", &["connect", "old"]).unwrap();
        let text = out.string();
        assert!(
            text.contains("old: connected to sse-fake 1 (protocol 2024-11-05)"),
            "{text}"
        );
        assert!(
            text.contains("legacy") && text.contains("old style"),
            "{text}"
        );
        let seen = auth_seen.lock().unwrap();
        assert!(
            seen.len() >= 3 && seen.iter().all(|a| a == "Bearer legacy-token"),
            "{seen:?}"
        );
    }

    /// What the fake authorization server saw and handed out.
    #[derive(Default, Debug)]
    struct AsState {
        registered: Vec<Value>,
        authorize_params: Vec<BTreeMap<String, String>>,
        token_forms: Vec<BTreeMap<String, String>>,
        revoked: Vec<BTreeMap<String, String>>,
        mcp_auth_seen: Vec<String>,
        code_challenge: String,
        /// The access token the MCP server accepts right now.
        current_token: String,
        /// GitHub-like: no registration endpoint, a device endpoint, and
        /// device-flow errors sent with HTTP 200.
        hand_clients_only: bool,
        device_forms: Vec<BTreeMap<String, String>>,
        /// How many device-grant polls answer authorization_pending first.
        device_pending: u32,
        device_polls: u32,
    }

    fn form_map(body: &str) -> BTreeMap<String, String> {
        url::form_urlencoded::parse(body.as_bytes())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// One server playing both parts: the MCP server at /mcp (401 with a
    /// resource_metadata pointer until the right token comes) and the
    /// authorization server under /as with metadata, registration,
    /// authorize, token and revoke endpoints.
    fn oauth_server(state: Arc<Mutex<AsState>>) -> pulse_testutil::httptest::Server {
        use pulse_testutil::httptest::Response;
        let base: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let base_for_handler = Arc::clone(&base);
        let srv = pulse_testutil::httptest::Server::new(move |req| {
            let base = base_for_handler.lock().unwrap().clone();
            let mut st = state.lock().unwrap();
            match (req.method.as_str(), req.path()) {
                ("POST", "/mcp") => {
                    let auth = req.header("authorization").unwrap_or("").to_string();
                    st.mcp_auth_seen.push(auth.clone());
                    if auth != format!("Bearer {}", st.current_token) {
                        return Response::text(401, "sign in first").header(
                            "WWW-Authenticate",
                            &format!(
                                "Bearer realm=\"mcp\", resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\", scope=\"mcp:tools\""
                            ),
                        );
                    }
                    let body: Value = req.json().unwrap_or(Value::Null);
                    http_answer(&body, false)
                }
                ("GET", "/.well-known/oauth-protected-resource/mcp") => Response::json(
                    200,
                    &serde_json::json!({
                        "resource": format!("{base}/mcp"),
                        "authorization_servers": [format!("{base}/as")],
                        "scopes_supported": ["mcp:tools", "mcp:resources"],
                    }),
                ),
                ("GET", "/.well-known/oauth-authorization-server/as") => {
                    let mut meta = serde_json::json!({
                        "issuer": format!("{base}/as"),
                        "authorization_endpoint": format!("{base}/as/authorize"),
                        "token_endpoint": format!("{base}/as/token"),
                        "registration_endpoint": format!("{base}/as/register"),
                        "revocation_endpoint": format!("{base}/as/revoke"),
                        "code_challenge_methods_supported": ["S256"],
                        "response_types_supported": ["code"],
                        "grant_types_supported": ["authorization_code", "refresh_token"],
                    });
                    if st.hand_clients_only {
                        let o = meta.as_object_mut().unwrap();
                        o.remove("registration_endpoint");
                        o.insert(
                            "device_authorization_endpoint".into(),
                            Value::String(format!("{base}/as/device")),
                        );
                        o.insert(
                            "service_documentation".into(),
                            Value::String(format!("{base}/docs/apps")),
                        );
                        o.insert(
                            "grant_types_supported".into(),
                            serde_json::json!([
                                "authorization_code",
                                "refresh_token",
                                "urn:ietf:params:oauth:grant-type:device_code"
                            ]),
                        );
                    }
                    Response::json(200, &meta)
                }
                ("POST", "/as/device") => {
                    let form = form_map(&req.body_str());
                    st.device_forms.push(form);
                    let n = st.device_forms.len();
                    Response::json(
                        200,
                        &serde_json::json!({
                            "device_code": format!("dev-{n}"),
                            "user_code": "WDJB-MJHT",
                            "verification_uri": format!("{base}/as/device-page"),
                            "verification_uri_complete": format!("{base}/as/device-page?user_code=WDJB-MJHT"),
                            "expires_in": 900,
                            "interval": 1,
                        }),
                    )
                }
                ("POST", "/as/register") => {
                    let body: Value = req.json().unwrap();
                    st.registered.push(body.clone());
                    Response::json(
                        201,
                        &serde_json::json!({
                            "client_id": format!("dyn-{}", st.registered.len()),
                            "client_id_issued_at": 1_700_000_000,
                            "redirect_uris": body["redirect_uris"],
                            "token_endpoint_auth_method": "none",
                        }),
                    )
                }
                ("GET", "/as/authorize") => {
                    let q = form_map(req.query());
                    st.code_challenge = q.get("code_challenge").cloned().unwrap_or_default();
                    let redirect = q.get("redirect_uri").cloned().unwrap_or_default();
                    let state_param = q.get("state").cloned().unwrap_or_default();
                    st.authorize_params.push(q);
                    let n = st.authorize_params.len();
                    Response::new(302).header(
                        "Location",
                        &format!("{redirect}?code=code-{n}&state={state_param}"),
                    )
                }
                ("POST", "/as/token") => {
                    let form = form_map(&req.body_str());
                    st.token_forms.push(form.clone());
                    let n = st.token_forms.len();
                    match form.get("grant_type").map(String::as_str) {
                        Some("authorization_code") => {
                            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                            if crate::oauth::pkce_challenge(&verifier) != st.code_challenge {
                                return Response::json(
                                    400,
                                    &serde_json::json!({"error": "invalid_grant", "error_description": "PKCE verifier does not match"}),
                                );
                            }
                            if form.get("resource").map(String::as_str)
                                != Some(&format!("{base}/mcp"))
                            {
                                return Response::json(
                                    400,
                                    &serde_json::json!({"error": "invalid_target"}),
                                );
                            }
                            if !form.get("code").is_some_and(|c| c.starts_with("code-")) {
                                return Response::json(
                                    400,
                                    &serde_json::json!({"error": "invalid_grant"}),
                                );
                            }
                        }
                        Some("refresh_token") => {
                            if form.get("refresh_token").map(String::as_str)
                                != Some(&format!("rt-{}", n - 1))
                            {
                                return Response::json(
                                    400,
                                    &serde_json::json!({"error": "invalid_grant", "error_description": "unknown refresh token"}),
                                );
                            }
                        }
                        Some("urn:ietf:params:oauth:grant-type:device_code") => {
                            if !form
                                .get("device_code")
                                .is_some_and(|c| c.starts_with("dev-"))
                                || form.get("client_id").is_none_or(String::is_empty)
                            {
                                // GitHub answers device-flow errors with HTTP 200.
                                return Response::json(
                                    200,
                                    &serde_json::json!({"error": "incorrect_device_code"}),
                                );
                            }
                            st.device_polls += 1;
                            if st.device_polls <= st.device_pending {
                                st.token_forms.pop();
                                return Response::json(
                                    200,
                                    &serde_json::json!({"error": "authorization_pending", "error_description": "The authorization request is still pending."}),
                                );
                            }
                        }
                        _ => {
                            return Response::json(
                                400,
                                &serde_json::json!({"error": "unsupported_grant_type"}),
                            );
                        }
                    }
                    st.current_token = format!("at-{n}");
                    Response::json(
                        200,
                        &serde_json::json!({
                            "access_token": format!("at-{n}"),
                            "token_type": "Bearer",
                            "expires_in": 3600,
                            "refresh_token": format!("rt-{n}"),
                            "scope": form.get("scope").cloned().unwrap_or_else(|| "mcp:tools".into()),
                        }),
                    )
                }
                ("POST", "/as/revoke") => {
                    st.revoked.push(form_map(&req.body_str()));
                    Response::new(200)
                }
                _ => Response::text(404, "nothing here"),
            }
        });
        *base.lock().unwrap() = srv.url();
        srv
    }

    /// Stands in for the browser: follows the authorization URL (the fake
    /// server redirects to the loopback callback) from another thread.
    fn browser_for_tests() {
        crate::oauth::set_opener_for_tests(Some(Box::new(|url: &str| {
            let url = url.to_string();
            std::thread::spawn(move || {
                let _ = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(20))
                    .build()
                    .unwrap()
                    .get(&url)
                    .send();
            });
            Ok(())
        })));
    }

    #[test]
    fn oauth_login_connect_refresh_and_logout() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let state = Arc::new(Mutex::new(AsState::default()));
        let srv = oauth_server(Arc::clone(&state));
        browser_for_tests();
        let (mut c, out) = ctx_for(&s, false);
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "docs",
                "--transport",
                "http",
                "--url",
                &format!("{}/mcp", srv.url()),
                "--auth",
                "oauth",
            ],
        )
        .unwrap();
        let e = run_cmd(
            &mut c,
            "mcp",
            &["update", "docs", "--secret", "keychain://pulse/x"],
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("pulse mcp login docs"), "{e}");
        out.clear();

        // Before the sign-in: connect says so, and records it.
        let e = run_cmd(&mut c, "mcp", &["connect", "docs"])
            .unwrap_err()
            .to_string();
        assert!(e.contains("docs needs a sign-in"), "{e}");
        assert!(e.contains("no sign-in on this machine yet"), "{e}");
        assert!(e.contains("fix: pulse mcp login docs"), "{e}");
        let m = s.mcp_server("", "docs").unwrap();
        assert_eq!(m.last_check_ok, Some(false));
        assert!(
            m.last_check_note.contains("needs a sign-in"),
            "{}",
            m.last_check_note
        );
        assert!(
            state.lock().unwrap().mcp_auth_seen.is_empty(),
            "no request without a token"
        );
        run_cmd(&mut c, "mcp", &["show", "docs"]).unwrap();
        assert!(
            out.string()
                .contains("auth:       oauth (not signed in: pulse mcp login docs)"),
            "{}",
            out.string()
        );
        out.clear();
        let e = run_cmd(&mut c, "mcp", &["login", "nope"])
            .unwrap_err()
            .to_string();
        assert!(e.contains("No MCP server named nope"), "{e}");

        // The sign-in: discovery, registration, PKCE, loopback redirect, exchange.
        run_cmd(&mut c, "mcp", &["login", "docs"]).unwrap();
        let text = out.string();
        assert!(text.contains("asks for a sign-in (HTTP 401)"), "{text}");
        assert!(
            text.contains(&format!("authorization server {}/as", srv.url())),
            "{text}"
        );
        assert!(
            text.contains("registered as client dyn-1 (token endpoint auth: none)"),
            "{text}"
        );
        assert!(text.contains("sign in here:\n"), "{text}");
        assert!(text.contains("/as/authorize?response_type=code&client_id=dyn-1&redirect_uri=http%3A%2F%2F127.0.0.1%3A"), "{text}");
        assert!(text.contains("(a browser was opened)"), "{text}");
        assert!(
            text.contains(&format!(
                "docs: signed in at {}/as as client dyn-1 (scope mcp:tools); the token expires in ",
                srv.url()
            )),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                " and refreshes itself; kept in {}\n",
                crate::oauth::grant_path(&s.home, "MCP-001").display()
            )),
            "{text}"
        );
        assert!(text.contains("next: pulse mcp connect docs"), "{text}");
        assert!(
            !text.contains("at-1") && !text.contains("rt-1"),
            "tokens never printed: {text}"
        );
        out.clear();
        {
            let st = state.lock().unwrap();
            assert_eq!(st.registered.len(), 1);
            let reg = &st.registered[0];
            assert_eq!(reg["client_name"], "PULSE");
            assert_eq!(reg["token_endpoint_auth_method"], "none");
            assert_eq!(
                reg["grant_types"],
                serde_json::json!(["authorization_code", "refresh_token"])
            );
            assert_eq!(
                reg["scope"], "mcp:tools",
                "the 401's scope hint wins over scopes_supported"
            );
            let redirect = reg["redirect_uris"][0].as_str().unwrap().to_string();
            assert!(
                redirect.starts_with("http://127.0.0.1:") && redirect.ends_with("/callback"),
                "{redirect}"
            );
            let auth = &st.authorize_params[0];
            assert_eq!(auth["code_challenge_method"], "S256");
            assert_eq!(auth["resource"], format!("{}/mcp", srv.url()));
            assert_eq!(auth["redirect_uri"], redirect);
            assert_eq!(auth["scope"], "mcp:tools");
            assert_eq!(auth["state"].len(), 43);
            let tok = &st.token_forms[0];
            assert_eq!(tok["grant_type"], "authorization_code");
            assert_eq!(tok["code"], "code-1");
            assert_eq!(tok["redirect_uri"], redirect);
            assert_eq!(tok["client_id"], "dyn-1");
            assert!(!tok.contains_key("client_secret"));
        }
        let grant = crate::oauth::load_grant(&s.home, "MCP-001")
            .unwrap()
            .unwrap();
        assert_eq!(grant.access_token, "at-1");
        assert_eq!(grant.refresh_token, "rt-1");
        assert!(grant.registered);
        assert_eq!(grant.client_id, "dyn-1");
        assert!(grant.expires_at > crate::oauth::now_unix() + 3500);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(crate::oauth::grant_path(&s.home, "MCP-001"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Connected with the token; show knows the state. (The login's own
        // probe was the one request without a token.)
        {
            let mut st = state.lock().unwrap();
            assert_eq!(
                st.mcp_auth_seen,
                vec![String::new()],
                "{:?}",
                st.mcp_auth_seen
            );
            st.mcp_auth_seen.clear();
        }
        run_cmd(&mut c, "mcp", &["connect", "docs"]).unwrap();
        assert!(
            out.string().contains("docs: connected to http-fake 2.0"),
            "{}",
            out.string()
        );
        out.clear();
        {
            let st = state.lock().unwrap();
            assert!(!st.mcp_auth_seen.is_empty());
            assert!(
                st.mcp_auth_seen.iter().all(|a| a == "Bearer at-1"),
                "{:?}",
                st.mcp_auth_seen
            );
            assert_eq!(st.token_forms.len(), 1, "a fresh token is not refreshed");
        }
        run_cmd(&mut c, "mcp", &["show", "docs"]).unwrap();
        assert!(
            out.string().contains(&format!(
                "auth:       oauth (signed in at {}/as as client dyn-1; the token expires in ",
                srv.url()
            )),
            "{}",
            out.string()
        );
        out.clear();

        // An expiring token is refreshed on the way in, and the rotation kept.
        let mut stale = grant.clone();
        stale.expires_at = crate::oauth::now_unix() + 10;
        crate::oauth::save_grant(&s.home, "MCP-001", &stale).unwrap();
        state.lock().unwrap().mcp_auth_seen.clear();
        run_cmd(&mut c, "mcp", &["tools", "docs"]).unwrap();
        assert!(out.string().contains("ping"), "{}", out.string());
        out.clear();
        {
            let st = state.lock().unwrap();
            assert_eq!(st.token_forms.len(), 2);
            let r = &st.token_forms[1];
            assert_eq!(r["grant_type"], "refresh_token");
            assert_eq!(r["refresh_token"], "rt-1");
            assert_eq!(r["client_id"], "dyn-1");
            assert_eq!(r["resource"], format!("{}/mcp", srv.url()));
            assert!(
                st.mcp_auth_seen.iter().all(|a| a == "Bearer at-2"),
                "{:?}",
                st.mcp_auth_seen
            );
        }
        let grant = crate::oauth::load_grant(&s.home, "MCP-001")
            .unwrap()
            .unwrap();
        assert_eq!(
            (grant.access_token.as_str(), grant.refresh_token.as_str()),
            ("at-2", "rt-2")
        );
        assert!(grant.expires_at > crate::oauth::now_unix() + 3500);

        // A refresh the server turns down asks for a sign-in, with the reason.
        let mut broken = grant.clone();
        broken.expires_at = 1;
        broken.refresh_token = "rt-stale".into();
        crate::oauth::save_grant(&s.home, "MCP-001", &broken).unwrap();
        let e = run_cmd(&mut c, "mcp", &["connect", "docs"])
            .unwrap_err()
            .to_string();
        assert!(e.contains("docs needs a sign-in"), "{e}");
        assert!(e.contains("refreshing it failed"), "{e}");
        assert!(e.contains("unknown refresh token"), "{e}");
        assert!(!e.contains("rt-stale"), "{e}");
        assert!(e.contains("fix: pulse mcp login docs"), "{e}");
        crate::oauth::save_grant(&s.home, "MCP-001", &grant).unwrap();

        // Signing in again keeps the registration (same client, same port).
        // With --no-browser the address is printed before the wait, for a
        // person to follow by hand - here, this thread.
        let again = pulse_cli::Buffer::new();
        let worker = {
            let (store, home, again) = (s.store.clone(), s.home.clone(), again.clone());
            std::thread::spawn(move || {
                let mut c = Context {
                    store: Some(store),
                    home,
                    out: Box::new(again.clone()),
                    err: Box::new(again),
                    json: false,
                    quiet: false,
                    color: false,
                    agent: String::new(),
                    project: String::new(),
                    session: String::new(),
                    now: "2026-09-06T21:00:00Z".into(),
                    args: vec![],
                };
                run_cmd(&mut c, "mcp", &["login", "docs", "--no-browser"])
                    .map_err(|e| e.to_string())
            })
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let url = loop {
            let text = again.string();
            if let Some(rest) = text.split("sign in here:\n").nth(1)
                && let Some(line) = rest.lines().next()
                && text.contains("waiting for the sign-in on")
            {
                break line.trim().to_string();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no address printed: {text}"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap()
            .get(&url)
            .send()
            .unwrap();
        worker.join().unwrap().unwrap();
        let text = again.string();
        assert!(
            text.contains("still client dyn-1 from the last sign-in"),
            "{text}"
        );
        assert!(!text.contains("a browser was opened"), "{text}");
        assert_eq!(
            state.lock().unwrap().registered.len(),
            1,
            "no second registration"
        );

        // Logout revokes and forgets; connect then asks for a sign-in again.
        run_cmd(&mut c, "mcp", &["logout", "docs"]).unwrap();
        assert!(
            out.string().contains("docs: signed out; the tokens are revoked at the authorization server and forgotten here"),
            "{}",
            out.string()
        );
        out.clear();
        {
            let st = state.lock().unwrap();
            assert_eq!(st.revoked.len(), 1);
            assert_eq!(st.revoked[0]["token_type_hint"], "refresh_token");
            assert!(st.revoked[0]["token"].starts_with("rt-"));
        }
        assert!(
            crate::oauth::load_grant(&s.home, "MCP-001")
                .unwrap()
                .is_none()
        );
        let e = run_cmd(&mut c, "mcp", &["connect", "docs"])
            .unwrap_err()
            .to_string();
        assert!(e.contains("docs needs a sign-in"), "{e}");
        run_cmd(&mut c, "mcp", &["logout", "docs"]).unwrap();
        assert!(
            out.string()
                .contains("docs: nothing to forget, it was not signed in"),
            "{}",
            out.string()
        );
        out.clear();

        // JSON shapes.
        let (mut cj, outj) = ctx_for(&s, true);
        run_cmd(&mut cj, "mcp", &["login", "docs"]).unwrap();
        let v: Value = serde_json::from_str(&outj.string()).unwrap();
        assert_eq!(v["server"], "docs");
        assert_eq!(v["issuer"], format!("{}/as", srv.url()));
        assert_eq!(v["registered"], true);
        assert_eq!(v["has_refresh_token"], true);
        assert!(v["expires_at"].as_u64().unwrap() > 0);
        assert!(v.get("access_token").is_none());
        outj.clear();
        run_cmd(&mut cj, "mcp", &["logout", "docs"]).unwrap();
        let v: Value = serde_json::from_str(&outj.string()).unwrap();
        assert_eq!(v["had_grant"], true);
        assert_eq!(v["revoked"], true);

        // login on a server without oauth points at update.
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "plain",
                "--transport",
                "http",
                "--url",
                &format!("{}/mcp", srv.url()),
            ],
        )
        .unwrap();
        let e = run_cmd(&mut c, "mcp", &["login", "plain"])
            .unwrap_err()
            .to_string();
        assert!(e.contains("plain does not sign in with OAuth"), "{e}");
        assert!(e.contains("--auth oauth"), "{e}");
        crate::oauth::set_opener_for_tests(None);
    }

    /// A GitHub-like authorization server: no client registration, a
    /// device endpoint. The first login says what to register; the second,
    /// with a client id, signs in with a typed code; the third reuses that
    /// client without being told again.
    #[test]
    fn oauth_login_with_a_hand_client_and_the_device_flow() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let state = Arc::new(Mutex::new(AsState {
            hand_clients_only: true,
            device_pending: 1,
            ..AsState::default()
        }));
        let srv = oauth_server(Arc::clone(&state));
        let opened: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let opened_for_hook = Arc::clone(&opened);
        crate::oauth::set_opener_for_tests(Some(Box::new(move |url: &str| {
            opened_for_hook.lock().unwrap().push(url.to_string());
            Ok(())
        })));
        let (mut c, out) = ctx_for(&s, false);
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "github",
                "--transport",
                "http",
                "--url",
                &format!("{}/mcp", srv.url()),
                "--auth",
                "oauth",
            ],
        )
        .unwrap();
        out.clear();

        // No client yet: the error names what to register and how to come back.
        let e = run_cmd(&mut c, "mcp", &["login", "github"])
            .unwrap_err()
            .to_string();
        assert!(e.contains("does not register clients by itself"), "{e}");
        assert!(
            e.contains(&format!(
                "pulse mcp login github --client-id ID (register the client with {}/docs/apps first; with the device flow it needs no secret and no port)",
                srv.url()
            )),
            "{e}"
        );
        assert!(state.lock().unwrap().device_forms.is_empty());
        out.clear();

        // The flow nobody offers is refused before anything is sent.
        let e = run_cmd(
            &mut c,
            "mcp",
            &["login", "github", "--client-id", "Iv1.abc", "--flow", "sms"],
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("Invalid --flow"), "{e}");
        out.clear();

        // With a client id the device flow runs: code shown, browser opened
        // on the complete URI, one pending poll, then tokens.
        run_cmd(
            &mut c,
            "mcp",
            &["login", "github", "--client-id", "Iv1.abc"],
        )
        .unwrap();
        let text = out.string();
        assert!(
            text.contains("using client Iv1.abc registered by hand, with the device flow"),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "sign in here:\n{}/as/device-page\nand enter the code WDJB-MJHT",
                srv.url()
            )),
            "{text}"
        );
        assert!(text.contains("(a browser was opened)"), "{text}");
        assert!(text.contains("waiting for the sign-in at"), "{text}");
        assert!(
            text.contains(&format!(
                "github: signed in at {}/as as client Iv1.abc (scope mcp:tools); the token expires in ",
                srv.url()
            )),
            "{text}"
        );
        assert!(!text.contains("at-1"), "tokens never printed: {text}");
        assert_eq!(
            opened.lock().unwrap().as_slice(),
            [format!("{}/as/device-page?user_code=WDJB-MJHT", srv.url())]
        );
        {
            let st = state.lock().unwrap();
            assert!(st.registered.is_empty(), "nothing registered");
            assert!(st.authorize_params.is_empty(), "no browser redirect");
            assert_eq!(st.device_forms.len(), 1);
            assert_eq!(st.device_forms[0]["client_id"], "Iv1.abc");
            assert_eq!(st.device_forms[0]["scope"], "mcp:tools");
            assert_eq!(st.device_polls, 2, "one pending answer, then the token");
            let tok = &st.token_forms[0];
            assert_eq!(
                tok["grant_type"],
                "urn:ietf:params:oauth:grant-type:device_code"
            );
            assert_eq!(tok["device_code"], "dev-1");
            assert_eq!(tok["client_id"], "Iv1.abc");
            assert!(!tok.contains_key("client_secret"));
        }
        let grant = crate::oauth::load_grant(&s.home, "MCP-001")
            .unwrap()
            .unwrap();
        assert_eq!(grant.flow, "device");
        assert_eq!(grant.client_id, "Iv1.abc");
        assert!(!grant.registered);
        assert_eq!(grant.redirect_uri, "");
        assert_eq!(grant.token_endpoint_auth_method, "none");
        out.clear();

        // The token works against the MCP server.
        run_cmd(&mut c, "mcp", &["connect", "github"]).unwrap();
        assert!(
            out.string().contains("github: connected to"),
            "{}",
            out.string()
        );
        out.clear();
        run_cmd(&mut c, "mcp", &["show", "github"]).unwrap();
        assert!(
            out.string()
                .contains("as client Iv1.abc (device flow); the token expires in"),
            "{}",
            out.string()
        );
        out.clear();

        // Signing in again needs no --client-id: the hand client is kept.
        run_cmd(&mut c, "mcp", &["login", "github"]).unwrap();
        let text = out.string();
        assert!(
            text.contains("using client Iv1.abc registered by hand, with the device flow"),
            "{text}"
        );
        assert_eq!(state.lock().unwrap().device_forms.len(), 2);
        out.clear();

        // Asking for the code flow with a hand client needs a redirect port,
        // which the fake server redirects to; the client id is still kept.
        let e = run_cmd(
            &mut c,
            "mcp",
            &["login", "github", "--flow", "code", "--redirect-port", "1"],
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("cannot listen on 127.0.0.1:1"), "{e}");
        crate::oauth::set_opener_for_tests(None);
    }

    /// An issuer that registers no clients but has a client PULSE carries
    /// (GitHub, in the real table) signs in through it with the device flow
    /// and nothing registered; the person is told what the consent page
    /// calls it. The code flow is refused with it, since it has no secret,
    /// and a client given by hand still wins.
    #[test]
    fn oauth_login_through_the_built_in_client_registers_nothing() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let state = Arc::new(Mutex::new(AsState {
            hand_clients_only: true,
            device_pending: 1,
            ..AsState::default()
        }));
        let srv = oauth_server(Arc::clone(&state));
        let _built_in = crate::oauth::add_built_in_client_for_tests(crate::oauth::BuiltInClient {
            issuer: format!("{}/as", srv.url()),
            client_id: "pulse-carries-1".into(),
            label: "Fake CLI".into(),
        });
        let opened: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let opened_for_hook = Arc::clone(&opened);
        crate::oauth::set_opener_for_tests(Some(Box::new(move |url: &str| {
            opened_for_hook.lock().unwrap().push(url.to_string());
            Ok(())
        })));
        let (mut c, out) = ctx_for(&s, false);
        run_cmd(
            &mut c,
            "mcp",
            &[
                "add",
                "--name",
                "github",
                "--transport",
                "http",
                "--url",
                &format!("{}/mcp", srv.url()),
                "--auth",
                "oauth",
            ],
        )
        .unwrap();
        out.clear();

        // The code flow cannot run through a client without a secret.
        let e = run_cmd(&mut c, "mcp", &["login", "github", "--flow", "code"])
            .unwrap_err()
            .to_string();
        assert!(
            e.contains(
                "the built-in client pulse-carries-1 (Fake CLI) signs in with the device flow only, since it has no secret"
            ),
            "{e}"
        );
        assert!(
            e.contains("--client-id ID [--client-secret REF] --redirect-port PORT"),
            "{e}"
        );
        assert!(state.lock().unwrap().device_forms.is_empty());
        out.clear();

        // No --client-id, nothing registered: the device flow runs.
        run_cmd(&mut c, "mcp", &["login", "github"]).unwrap();
        let text = out.string();
        assert!(
            text.contains(
                "using the built-in client pulse-carries-1 (Fake CLI's public app), with the device flow; the sign-in page asks you to authorize Fake CLI"
            ),
            "{text}"
        );
        assert!(text.contains("and enter the code WDJB-MJHT"), "{text}");
        assert!(
            text.contains(&format!(
                "github: signed in at {}/as through the built-in client pulse-carries-1 (Fake CLI's public app) (scope mcp:tools); the token expires in ",
                srv.url()
            )),
            "{text}"
        );
        assert!(!text.contains("at-1"), "tokens never printed: {text}");
        assert_eq!(
            opened.lock().unwrap().as_slice(),
            [format!("{}/as/device-page?user_code=WDJB-MJHT", srv.url())]
        );
        {
            let st = state.lock().unwrap();
            assert!(st.registered.is_empty(), "nothing registered");
            assert!(st.authorize_params.is_empty(), "no browser redirect");
            assert_eq!(st.device_forms.len(), 1);
            assert_eq!(st.device_forms[0]["client_id"], "pulse-carries-1");
            assert_eq!(st.device_forms[0]["scope"], "mcp:tools");
            let tok = &st.token_forms[0];
            assert_eq!(
                tok["grant_type"],
                "urn:ietf:params:oauth:grant-type:device_code"
            );
            assert_eq!(tok["client_id"], "pulse-carries-1");
            assert!(!tok.contains_key("client_secret"));
        }
        let grant = crate::oauth::load_grant(&s.home, "MCP-001")
            .unwrap()
            .unwrap();
        assert_eq!(grant.flow, "device");
        assert_eq!(grant.client_id, "pulse-carries-1");
        assert!(!grant.registered);
        assert_eq!(grant.token_endpoint_auth_method, "none");
        assert_eq!(grant.client_secret_ref, "");
        out.clear();

        // The token works; show says whose app signed in.
        run_cmd(&mut c, "mcp", &["connect", "github"]).unwrap();
        assert!(
            out.string().contains("github: connected to"),
            "{}",
            out.string()
        );
        out.clear();
        run_cmd(&mut c, "mcp", &["show", "github"]).unwrap();
        assert!(
            out.string().contains(
                "through the built-in client pulse-carries-1 (Fake CLI's public app, device flow); the token expires in"
            ),
            "{}",
            out.string()
        );
        out.clear();

        // Signing in again keeps the built-in client; --json says which.
        let (mut cj, outj) = ctx_for(&s, true);
        run_cmd(&mut cj, "mcp", &["login", "github"]).unwrap();
        let v: Value = serde_json::from_str(&outj.string()).unwrap();
        assert_eq!(v["client_id"], "pulse-carries-1");
        assert_eq!(v["built_in"], true);
        assert_eq!(v["client_label"], "Fake CLI");
        assert_eq!(v["registered"], false);
        assert_eq!(state.lock().unwrap().device_forms.len(), 2);

        // A client given by hand wins over the built-in one.
        run_cmd(
            &mut c,
            "mcp",
            &["login", "github", "--client-id", "Iv1.mine"],
        )
        .unwrap();
        assert!(
            out.string()
                .contains("using client Iv1.mine registered by hand, with the device flow"),
            "{}",
            out.string()
        );
        {
            let st = state.lock().unwrap();
            assert_eq!(st.device_forms.len(), 3);
            assert_eq!(st.device_forms[2]["client_id"], "Iv1.mine");
        }
        let grant = crate::oauth::load_grant(&s.home, "MCP-001")
            .unwrap()
            .unwrap();
        assert_eq!(grant.client_id, "Iv1.mine");
        crate::oauth::set_opener_for_tests(None);
    }

    /// The hub starts a sign-in and waits for it on a thread of its own; a
    /// raised cancel flag ends that wait, nothing is saved.
    #[test]
    fn oauth_begin_then_cancelled_finish() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let state = Arc::new(Mutex::new(AsState::default()));
        let srv = oauth_server(Arc::clone(&state));
        let m = s
            .add_mcp_server(
                "test",
                pulse_store::McpServerInput {
                    name: "docs".into(),
                    transport: "http".into(),
                    url: format!("{}/mcp", srv.url()),
                    auth: "oauth".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let r = resolver(&s.home);
        let mut lines = Vec::new();
        let opts = crate::oauth::LoginOptions::default();
        let begun =
            crate::oauth::begin(&m, &r, &opts, &mut |l: &str| lines.push(l.to_string())).unwrap();
        let crate::oauth::Begun::Pending(p) = begun else {
            panic!("expected a pending sign-in, got {begun:?}");
        };
        assert_eq!(p.flow, "code");
        assert!(
            p.url
                .contains("/as/authorize?response_type=code&client_id=dyn-1"),
            "{}",
            p.url
        );
        assert!(
            p.redirect_uri.starts_with("http://127.0.0.1:"),
            "{}",
            p.redirect_uri
        );
        assert!(
            lines.iter().any(|l| l.starts_with("sign in here:\n")),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("a browser was opened")),
            "open_browser was false: {lines:?}"
        );
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let started = std::time::Instant::now();
        let e = crate::oauth::finish(*p, &r, Duration::from_secs(60), &cancel)
            .unwrap_err()
            .to_string();
        assert_eq!(e, crate::oauth::CANCELLED);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(crate::oauth::load_grant(&s.home, &m.id).unwrap().is_none());
        assert!(state.lock().unwrap().token_forms.is_empty());
    }

    #[test]
    fn skill_commands() {
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("skill.md");
        std::fs::write(&file, "# Release\n\n1. Tag.\n2. Push.\n").unwrap();
        let (mut c, out) = ctx_for(&s, false);
        run_cmd(
            &mut c,
            "skill",
            &[
                "add",
                "--name",
                "release",
                "--description",
                "How to cut a release",
                "--file",
                file.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert!(
            out.string()
                .contains("SKILL-001 added: release - How to cut a release"),
            "{}",
            out.string()
        );
        out.clear();
        // the shell's project (PULSE_PROJECT in a worker's shell) does not
        // narrow a new skill; only an explicit --project does
        c.project = "demo".into();
        c.agent = "cinder".into();
        set_stdin_for_tests(Some(b"Shared by every project.\n".to_vec()));
        run_cmd(
            &mut c,
            "skill",
            &[
                "add",
                "--name",
                "shared-by-default",
                "--description",
                "Added from demo's shell, for everyone",
            ],
        )
        .unwrap();
        set_stdin_for_tests(None);
        assert!(
            out.string().contains(
                "SKILL-002 added: shared-by-default - Added from demo's shell, for everyone"
            ),
            "{}",
            out.string()
        );
        assert_eq!(s.skill("shared-by-default").unwrap().project, "");
        out.clear();
        set_stdin_for_tests(Some(
            b"Run `cargo test -- --nocapture` and look for the flake.\n".to_vec(),
        ));
        run_cmd(
            &mut c,
            "skill",
            &[
                "add",
                "--name",
                "flaky-tests",
                "--project",
                "demo",
                "--description",
                "Finding a flaky test",
            ],
        )
        .unwrap();
        set_stdin_for_tests(None);
        assert!(
            out.string().contains("SKILL-003 added: flaky-tests (demo)"),
            "{}",
            out.string()
        );
        out.clear();
        // an update from demo's shell leaves a shared skill shared
        run_cmd(
            &mut c,
            "skill",
            &[
                "update",
                "shared-by-default",
                "--description",
                "Still for everyone",
            ],
        )
        .unwrap();
        assert_eq!(s.skill("shared-by-default").unwrap().project, "");
        run_cmd(&mut c, "skill", &["update", "flaky-tests", "--project", ""]).unwrap();
        assert_eq!(s.skill("flaky-tests").unwrap().project, "");
        run_cmd(
            &mut c,
            "skill",
            &["update", "flaky-tests", "--project", "demo"],
        )
        .unwrap();
        assert_eq!(s.skill("flaky-tests").unwrap().project, "demo");
        out.clear();
        c.project.clear();
        c.agent.clear();

        let err = run_cmd(
            &mut c,
            "skill",
            &["add", "--name", "empty", "--description", "x"],
        )
        .unwrap_err();
        assert!(err.to_string().contains("Invalid body"), "{err}");

        run_cmd(&mut c, "skill", &["list"]).unwrap();
        let text = out.string();
        assert!(
            text.contains("SKILL-001")
                && text.contains("release")
                && text.contains("SKILL-002")
                && text.contains("cinder"),
            "{text}"
        );
        out.clear();

        run_cmd(&mut c, "skill", &["show", "release"]).unwrap();
        assert_eq!(out.string(), "# Release\n\n1. Tag.\n2. Push.\n");
        out.clear();
        run_cmd(&mut c, "skill", &["show", "SKILL-001"]).unwrap();
        out.clear();
        assert_eq!(s.skill_by_id("SKILL-001").unwrap().uses, 2);

        run_cmd(
            &mut c,
            "skill",
            &[
                "update",
                "release",
                "--description",
                "Cutting a release",
                "--body",
                "# Release\n\nTag, push, announce.\n",
            ],
        )
        .unwrap();
        assert!(
            out.string().contains("SKILL-001 updated: release"),
            "{}",
            out.string()
        );
        let k = s.skill("release").unwrap();
        assert_eq!(k.description, "Cutting a release");
        assert_eq!(k.body, "# Release\n\nTag, push, announce.");
        out.clear();
        let err = run_cmd(&mut c, "skill", &["update", "release"]).unwrap_err();
        assert!(err.to_string().contains("Nothing to update"), "{err}");

        run_cmd(&mut c, "skill", &["disable", "release"]).unwrap();
        assert!(
            out.string().contains("SKILL-001 disabled: release"),
            "{}",
            out.string()
        );
        assert_eq!(s.skills_for("demo").unwrap().len(), 2);
        out.clear();
        run_cmd(&mut c, "skill", &["enable", "SKILL-001"]).unwrap();
        assert_eq!(s.skills_for("demo").unwrap().len(), 3);
        out.clear();

        let (mut j, jout) = ctx_for(&s, true);
        run_cmd(&mut j, "skill", &["show", "flaky-tests"]).unwrap();
        let v: Value = serde_json::from_str(&jout.string()).unwrap();
        assert_eq!(v["skill"]["project"], "demo");
        assert_eq!(v["skill"]["uses"], 0);
        jout.clear();
        run_cmd(&mut j, "skill", &["list"]).unwrap();
        let v: Value = serde_json::from_str(&jout.string()).unwrap();
        assert_eq!(v["skills"].as_array().unwrap().len(), 3);

        let err = run_cmd(&mut c, "skill", &["show", "nothing"]).unwrap_err();
        assert!(
            err.to_string().starts_with("No skill named nothing"),
            "{err}"
        );
        let err = run_cmd(&mut c, "skill", &["nope"]).unwrap_err();
        assert!(err.to_string().contains("unknown subcommand"), "{err}");
    }
}
