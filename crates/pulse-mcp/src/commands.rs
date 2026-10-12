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
struct SkillOut<'a> {
    skill: &'a Skill,
}

#[derive(Serialize)]
struct SkillsOut<'a> {
    skills: &'a [Skill],
}

pub fn register() {
    REGISTER.call_once(|| {
        pulse_cli::register(Command { name:"mcp".into(), group:"agents".into(), summary:"MCP servers agents can call: add, list, show, update, enable, disable, connect, tools, call, resources, read".into(), usage:"pulse mcp add --name N --transport stdio|http|sse (--command CMD [--arg A]... | --url U) [--auth none|bearer|basic|header|query|env] [--auth-header NAME] [--secret REF] [--env K=V]... [--project P] [--note TEXT]\npulse mcp list [--project P] [--json]\npulse mcp show N|MCP-NNN [--json]\npulse mcp update N|MCP-NNN [--name N] [--transport T] [--command CMD] [--arg A]... [--url U] [--auth A] [--auth-header NAME] [--secret REF] [--env K=V]... [--note TEXT] [--project P]\npulse mcp enable|disable N|MCP-NNN\npulse mcp connect N [--json]              (initialize + tools/list; records the check)\npulse mcp tools N [--json]\npulse mcp call N TOOL [--args JSON | stdin] [--json]   (exit 1 when the tool reports an error)\npulse mcp resources N [--json]\npulse mcp read N URI [--json]\n\nThe secret is a pulse_cred reference, never the value: op://VAULT/ITEM/FIELD, keychain://SERVICE/ACCOUNT or file:///ABSOLUTE/PATH. For basic auth it resolves to user:password. Env values may be references too.\n\nA server is for every project unless --project P names the one project that may use it; the project of your shell (PULSE_PROJECT) does not narrow it. A project's own server replaces a shared one with the same name.".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_mcp, flags:Some(mcp_flags) });
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
        "none, bearer, basic, header, query or env (add, update; default none)",
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
        match server.transport.as_str() {
            "stdio" => "check that the command is installed for this user and starts by hand; its output is in the why line",
            "sse" => "check the url and the credential; a Streamable HTTP server wants --transport http",
            _ => "check the url and the credential; an older server may want --transport sse",
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
                        k.created_by.clone(),
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
