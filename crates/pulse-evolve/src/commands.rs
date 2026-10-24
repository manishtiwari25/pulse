use anyhow::Result;
use std::io::{Read, Write};
use std::sync::{LazyLock, Mutex};

pub(crate) static STDIN: LazyLock<Mutex<Option<Box<dyn Read + Send>>>> =
    LazyLock::new(|| Mutex::new(None));
#[derive(Debug, Clone, Default)]
pub struct RepeatedStrings(pub Vec<String>);
impl std::fmt::Display for RepeatedStrings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.join(","))
    }
}
impl RepeatedStrings {
    pub fn set(&mut self, value: &str) -> Result<()> {
        self.0.push(value.to_string());
        Ok(())
    }
}
/// Command metadata for `pulse evolve`.
pub const EVOLVE_SUMMARY: &str = "measure, rehearse and run the voted self-improvement loop";
pub const EVOLVE_USAGE: &str = "pulse evolve status|report|fixture|seed|setup|measure|archive|lineage|cycle|score [flags]\n  fixture add --project P --name N --request FILE|- --scorer CMD [--metric] [--weight W] [--kind public|signal] [--answer-key PATH...]\n  fixture list|enable|disable [NAME] [--project P] [--json]\n  fixture seed [--project P] [--json]   (starter set; every registered project without --project)\n  seed [--project P] [--json]   (constitution and evaluators from the checkout's docs/evolution or the template, then the starter set; existing records kept)\n  setup DIR | status [--project P] [--json] | archive [--project P] [--json] | lineage --project P [--json]\n  report [--project P] [--days N] [--json]   (the window's counts from the store: posts, the suggestion funnel, card jobs, jobs by outcome, landings, experiments, the owner inbox)\n  measure --project P [--ref REF] [--tree DIR] [--fixture NAME...] [--shadow] [--json]   (--fixture NAME measures those public fixtures and, always, the held-out set)\n  cycle --project P [--once] [--agent A] [--json]\n  score NAME | score metric (--lower|--higher) --scale S [--floor F] -- CMD ARGS...";
pub const REFLECT_SUMMARY: &str =
    "read-only working state, board, ballot, hygiene and evolution gates";
pub const REFLECT_USAGE: &str = "pulse reflect [--project P] [--area A] [--agent R/M] [--json]";
pub const FLEET_SUMMARY: &str = "prepare, inspect and clean isolated worker clones";
pub const FLEET_USAGE: &str = "pulse fleet prepare --n N [--ref REV] [--project P] [--json]\n  pulse fleet verify|clean ID [--json]\n  pulse fleet list [--project P] [--json]";
pub const USAGE_SUMMARY: &str = "best-effort local runner token diagnostics; never a ledger";
pub const USAGE_USAGE: &str =
    "pulse usage [--runner copilot|claude|codex|opencode] [--days N] [--json]";
pub fn command_flags(
    c: &pulse_cli::Context,
    name: &str,
) -> Result<(pulse_cli::Context, pulse_cli::FlagSet)> {
    let copy = pulse_cli::Context {
        store: c.store.clone(),
        home: c.home.clone(),
        out: Box::new(std::io::sink()),
        err: Box::new(std::io::sink()),
        json: c.json,
        quiet: c.quiet,
        color: c.color,
        agent: c.agent.clone(),
        project: c.project.clone(),
        session: c.session.clone(),
        now: c.now.clone(),
        args: c.args.clone(),
    };
    let mut fs = pulse_cli::FlagSet::new(name);
    add_common_flags(&mut fs, &copy);
    Ok((copy, fs))
}
/// Parses a command's flags, forwarding whatever the flag set prints (usage,
/// parse errors) to the command's error writer.
pub(crate) fn parse_command_flags(
    c: &mut pulse_cli::Context,
    fs: &mut pulse_cli::FlagSet,
    args: &[String],
) -> Result<()> {
    fs.set_output(pulse_cli::flags::Output::Capture);
    let parsed = parse_command_flag_args(fs, args);
    let printed = fs.take_output();
    if !printed.is_empty() {
        let _ = c.err.write_all(&printed);
    }
    parsed
}

fn parse_command_flag_args(fs: &mut pulse_cli::FlagSet, args: &[String]) -> Result<()> {
    let mut options = Vec::new();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            positional.extend_from_slice(&args[i + 1..]);
            break;
        }
        if arg == "-" || !arg.starts_with('-') {
            positional.push(arg.clone());
            i += 1;
            continue;
        }
        let name = arg
            .trim_start_matches('-')
            .split_once('=')
            .map_or(arg.trim_start_matches('-'), |x| x.0);
        if fs.lookup(name).is_none() {
            if name == "h" || name == "help" {
                // Prints "Usage of NAME:" and the defaults on the flag set's
                // output, then fails with FlagError::Help.
                return fs.parse(&["-h".to_string()]).map_err(Into::into);
            }
            return Err(crate::fixtures::input_error(
                "Unknown command flag",
                "that flag is not supported",
                &format!(
                    "pulse help {}",
                    fs.name().split_whitespace().next().unwrap_or(fs.name())
                ),
            ));
        }
        options.push(arg.clone());
        let is_bool = fs
            .lookup(name)
            .is_some_and(|f| matches!(f.default, pulse_cli::FlagValue::Bool(_)));
        if !arg.contains('=') && !is_bool {
            i += 1;
            if i == args.len() {
                return Err(crate::fixtures::input_error(
                    "Missing flag value",
                    "the flag requires a value",
                    &format!(
                        "pulse help {}",
                        fs.name().split_whitespace().next().unwrap_or(fs.name())
                    ),
                ));
            }
            options.push(args[i].clone());
        }
        i += 1;
    }
    options.push("--".to_string());
    options.extend(positional);
    fs.parse(&options).map_err(|e| match e {
        pulse_cli::FlagError::Help => e.into(),
        _ => crate::fixtures::input_error(
            "Invalid command flags",
            "a flag value is not valid",
            &format!(
                "pulse help {}",
                fs.name().split_whitespace().next().unwrap_or(fs.name())
            ),
        ),
    })
}
pub(crate) fn with_store<F>(c: &mut pulse_cli::Context, read_only: bool, f: F) -> Result<()>
where
    F: FnOnce(&pulse_store::Store) -> Result<()>,
{
    if let Some(s) = c.store.as_ref() {
        return f(s);
    }
    let s = pulse_store::Store::open(pulse_store::Options {
        home: c.home.clone(),
        create: false,
        read_only,
        skip_records: false,
        now: c.now.clone(),
    })?;
    let result = f(&s);
    let close = s.close();
    result.and(close)
}
pub(crate) fn command_error(err: anyhow::Error, command: &str) -> anyhow::Error {
    if err
        .chain()
        .any(|e| e.downcast_ref::<pulse_cli::FlagError>() == Some(&pulse_cli::FlagError::Help))
        || chain_has::<pulse_cli::Error>(&err)
    {
        return err;
    }
    if pulse_bus::ContextError::is(&err, pulse_bus::ContextError::Canceled) {
        return pulse_cli::errorf(
            130,
            &format!("{command} interrupted"),
            "the context was cancelled; owned temporary clones were cleaned",
            "pulse evolve status",
        )
        .into();
    }
    if let Some(violation) = err
        .chain()
        .find_map(|e| e.downcast_ref::<crate::clones::ViolationError>())
    {
        return pulse_cli::errorf(
            2,
            &format!("{command} stopped"),
            &violation.to_string(),
            "pulse reflect",
        )
        .into();
    }
    if chain_has::<pulse_store::NotFoundError>(&err) {
        return pulse_cli::errorf(
            2,
            &format!("{command} needs a store"),
            "no store exists in the selected home",
            "pulse init PATH",
        )
        .into();
    }
    pulse_cli::errorf(
        1,
        &format!("{command} failed"),
        &err.to_string(),
        &format!(
            "pulse help {}",
            command.split_whitespace().next().unwrap_or(command)
        ),
    )
    .into()
}
fn chain_has<T: std::error::Error + 'static>(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.downcast_ref::<T>().is_some())
}
pub(crate) fn current_project(
    s: &pulse_store::Store,
    c: &pulse_cli::Context,
) -> Result<pulse_store::Project> {
    let p = pulse_board::current_project(s, &c.project)?;
    if p.slug == pulse_store::COMMONS_SLUG {
        return Err(crate::fixtures::input_error(
            "Select a registered project",
            "evolution needs a project checkout",
            "pulse projects",
        ));
    }
    Ok(p)
}
pub(crate) fn calling_actor(s: &pulse_store::Store, c: &pulse_cli::Context) -> Result<String> {
    if c.agent.is_empty() {
        return Ok(pulse_store::HUMAN_AGENT.to_string());
    }
    Ok(pulse_board::resolve_agent(s, &c.agent, true)?
        .map(|a| a.name)
        .unwrap_or_default())
}
pub(crate) fn read_request(s: &pulse_store::Store, source: &str) -> Result<String> {
    let mut data = Vec::new();
    if source == "-" {
        let mut guard = STDIN.lock().expect("stdin hook poisoned");
        if let Some(r) = guard.as_mut() {
            r.read_to_end(&mut data)?;
        } else {
            std::io::stdin().read_to_end(&mut data)?;
        }
    } else {
        if source.is_empty() {
            return Err(crate::fixtures::input_error(
                "A fixture request is required",
                "use a file or - for stdin",
                "pulse evolve fixture add --help",
            ));
        }
        let file = crate::private::canonical_path(source).map_err(|_| {
            crate::fixtures::input_error(
                "Cannot read fixture request",
                "the request file cannot be resolved",
                "pulse evolve fixture add --request FILE --help",
            )
        })?;
        let private = s.setting("evolve.heldout_dir", "");
        if !private.is_empty()
            && crate::private::canonical_path(&private)
                .ok()
                .is_some_and(|p| crate::private::within(&file, &p))
        {
            return Err(crate::private::private_error(
                "a private request cannot be imported as a public fixture",
            ));
        }
        let meta = std::fs::metadata(&file).map_err(|_| {
            crate::fixtures::input_error(
                "Cannot read fixture request",
                "the request file cannot be opened",
                "pulse evolve fixture add --request FILE --help",
            )
        })?;
        if !meta.is_file() {
            return Err(crate::fixtures::input_error(
                "Cannot read fixture request",
                "use a regular text file or stdin",
                "pulse evolve fixture add --request - --help",
            ));
        }
        data = std::fs::read(&file)?;
    }
    if data.len() > crate::private::MAX_FIXTURE_BYTES {
        return Err(crate::fixtures::input_error(
            "Cannot read fixture request",
            "request must be readable and at most 1 MiB",
            "pulse evolve fixture add --request FILE --help",
        ));
    }
    Ok(String::from_utf8_lossy(&data).to_string())
}
pub fn run_evolve(
    c: &mut pulse_cli::Context,
    _fs: &pulse_cli::FlagSet,
    args: &[String],
) -> Result<()> {
    let mut args = args.to_vec();
    if args.is_empty() {
        args.push("status".to_string());
    }
    let sub = args[0].clone();
    let rest = args[1..].to_vec();
    if sub == "-h" || sub == "--help" {
        return Err(pulse_cli::FlagError::Help.into());
    }
    let result = match sub.as_str() {
        "fixture" => run_fixture(c, &rest),
        "seed" => run_seed(c, &rest),
        "score" => run_score(c, &rest),
        "report" | "measure" | "cycle" | "status" | "gates" | "archive" | "lineage" | "setup" => {
            run_evolve_sub(c, &sub, &rest)
        }
        _ => Err(crate::fixtures::input_error(
            "Unknown evolve subcommand",
            "choose status, report, fixture, seed, setup, measure, archive, lineage, cycle or score",
            "pulse help evolve",
        )),
    };
    result.map_err(|e| command_error(e, &format!("evolve {sub}")))
}
pub(crate) fn run_fixture(c: &mut pulse_cli::Context, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(crate::fixtures::input_error(
            "Fixture command required",
            "choose add, list, seed, disable or enable",
            "pulse help evolve",
        ));
    }
    let sub = args[0].clone();
    let rest = &args[1..];
    let mut fs = pulse_cli::FlagSet::new(format!("evolve fixture {sub}"));
    add_common_flags(&mut fs, c);
    match sub.as_str() {
        "add" => {
            fs.string_var("name", "", "public fixture name");
            fs.string_var("request", "", "request file or - for stdin");
            fs.string_var(
                "scorer",
                "",
                "owner-owned read-only command; prints a score in [0,1]",
            );
            fs.bool_var(
                "metric",
                false,
                "score the tree directly; --request may be empty",
            );
            fs.float64_var("weight", 1.0, "positive score weight");
            fs.string_var("kind", "public", "public or signal");
            fs.string_var(
                "answer-key",
                "",
                "repository-relative answer-key path to strip (repeatable)",
            );
        }
        "list" | "seed" | "disable" | "enable" => {}
        _ => {
            return Err(crate::fixtures::input_error(
                "Unknown fixture command",
                "choose add, list, seed, disable or enable",
                "pulse help evolve",
            ));
        }
    }
    let answer_keys = collect_repeated(rest, "answer-key");
    let filtered = filter_repeated(rest, "answer-key");
    parse_command_flags(c, &mut fs, &filtered)?;
    apply_common_flags(c, &fs);
    let pos = fs.args().to_vec();
    if ((sub == "enable" || sub == "disable") && pos.len() != 1)
        || ((sub == "add" || sub == "list" || sub == "seed") && !pos.is_empty())
    {
        return Err(crate::fixtures::input_error(
            "Wrong fixture arguments",
            "enable/disable take one name; add/list/seed take flags",
            "pulse help evolve",
        ));
    }
    let read_only = sub == "list";
    let mut sc = store_context(c);
    with_store(&mut sc, read_only, |s| {
        if sub == "list" {
            let fixtures = crate::fixtures::list_fixtures(s, &c.project, false)?;
            if c.json {
                return pulse_cli::write_json(
                    &mut *c.out,
                    &pulse_store::gomap! {"fixtures": fixtures},
                );
            }
            if !c.quiet {
                let rows: Vec<Vec<String>> = fixtures
                    .iter()
                    .map(|f| {
                        vec![
                            f.project.clone(),
                            f.name.clone(),
                            f.kind.clone(),
                            if f.metric { "metric" } else { "request" }.to_string(),
                            format_float(f.weight),
                            f.enabled.to_string(),
                        ]
                    })
                    .collect();
                pulse_cli::table(
                    &mut *c.out,
                    c.color,
                    Some(&["PROJECT", "NAME", "KIND", "MODE", "WEIGHT", "ENABLED"]),
                    &rows,
                )?;
            }
            return Ok(());
        }
        if sub == "seed" {
            return seed_command(s, c);
        }
        let p = current_project(s, c)?;
        let actor = calling_actor(s, c)?;
        if sub == "add" {
            let metric = fs.get_bool("metric");
            let body = if metric && fs.get_string("request").is_empty() {
                String::new()
            } else {
                read_request(s, &fs.get_string("request"))?
            };
            let f = crate::fixtures::add_fixture(
                s,
                crate::fixtures::FixtureInput {
                    project: p.slug,
                    name: fs.get_string("name"),
                    request: body,
                    scorer: fs.get_string("scorer"),
                    kind: fs.get_string("kind"),
                    metric,
                    weight: fs.get_f64("weight"),
                    actor,
                    answer_keys,
                },
            )?;
            if c.json {
                return pulse_cli::write_json(&mut *c.out, &f);
            }
            c.println(format!("Fixture added: {}", f.name))?;
            return Ok(());
        }
        crate::fixtures::set_fixture_enabled(s, &p.slug, &pos[0], sub == "enable", &actor)?;
        if c.json {
            return pulse_cli::write_json(
                &mut *c.out,
                &pulse_store::gomap! {"name": pos[0], "project": p.slug, "enabled": sub == "enable"},
            );
        }
        c.println(format!("Fixture {}: {}d", pos[0], sub))?;
        Ok(())
    })
}
pub(crate) fn seed_command(s: &pulse_store::Store, c: &mut pulse_cli::Context) -> Result<()> {
    let actor = calling_actor(s, c)?;
    let slugs = seed_targets(s, &c.project)?;
    #[derive(serde::Serialize)]
    struct Seeded {
        project: String,
        added: i64,
        derived: crate::derive::DeriveResult,
    }
    let mut results = Vec::new();
    let mut total = 0;
    for slug in slugs {
        let n = crate::starter::seed_starter_fixtures(s, &slug, &actor)?;
        let derived = crate::derive::derive_fixtures(s, &slug, &actor)?;
        results.push(Seeded {
            project: slug,
            added: n,
            derived,
        });
        total += n;
    }
    if c.json {
        return pulse_cli::write_json(
            &mut *c.out,
            &pulse_store::gomap! {"seeded": results, "added": total, "starter": crate::starter::STARTER_SET.len()},
        );
    }
    for r in &results {
        c.printf(format_args!(
            "{}: {} starter fixture(s) added; {} in the set (pulse evolve fixture list --project {}); derived: {}\n",
            r.project,
            r.added,
            crate::starter::STARTER_SET.len(),
            r.project,
            r.derived.line()
        ))?;
    }
    if results.is_empty() {
        c.println("No registered project to seed.")?;
    }
    Ok(())
}
pub(crate) fn seed_targets(s: &pulse_store::Store, project: &str) -> Result<Vec<String>> {
    if !project.is_empty() {
        return Ok(vec![crate::fixtures::valid_project(s, project)?.slug]);
    }
    Ok(s.projects(true)?
        .into_iter()
        .filter(|p| p.slug != pulse_store::COMMONS_SLUG)
        .map(|p| p.slug)
        .collect())
}
pub(crate) fn run_seed(c: &mut pulse_cli::Context, args: &[String]) -> Result<()> {
    let mut fs = pulse_cli::FlagSet::new("evolve seed");
    add_common_flags(&mut fs, c);
    parse_command_flags(c, &mut fs, args)?;
    apply_common_flags(c, &fs);
    if fs.narg() != 0 {
        return Err(crate::fixtures::input_error(
            "Wrong seed arguments",
            "seed takes flags only",
            "pulse help evolve",
        ));
    }
    let mut sc = store_context(c);
    with_store(&mut sc, false, |s| {
        let actor = calling_actor(s, c)?;
        let slugs = seed_targets(s, &c.project)?;
        pulse_records::ensure_framework(s, &actor)?;
        #[derive(serde::Serialize)]
        struct Seeded {
            #[serde(flatten)]
            summary: pulse_records::SeedSummary,
            added: i64,
            derived: crate::derive::DeriveResult,
        }
        let mut results = Vec::new();
        let mut total = 0;
        for slug in slugs {
            let summary = pulse_records::seed_loop_records(s, &slug, &actor)?;
            let n = crate::starter::seed_starter_fixtures(s, &slug, &actor)?;
            let derived = crate::derive::derive_fixtures(s, &slug, &actor)?;
            results.push(Seeded {
                summary,
                added: n,
                derived,
            });
            total += n;
        }
        if c.json {
            return pulse_cli::write_json(
                &mut *c.out,
                &pulse_store::gomap! {"seeded": results, "added": total, "starter": crate::starter::STARTER_SET.len()},
            );
        }
        for r in &results {
            c.printf(format_args!(
                "{}: {}, {}, {} starter fixture(s) added; {} in the set; derived: {}\n",
                r.summary.project,
                seed_words("constitution", r.summary.constitution.clone()),
                seed_words("evaluators", r.summary.evaluators.clone()),
                r.added,
                crate::starter::STARTER_SET.len(),
                r.derived.line()
            ))?;
        }
        if results.is_empty() {
            c.println("No registered project to seed.")?;
        }
        Ok(())
    })
}
pub(crate) fn seed_words(slug: &str, r: pulse_records::SeedResult) -> String {
    match r.action.as_str() {
        "kept" => format!("{slug} kept ({} r{})", r.id, r.rev),
        "absent" => format!(
            "{slug} absent ({}; the brain's records arrive by import)",
            r.id
        ),
        _ => format!("{slug} {} from {}", r.action, r.source),
    }
}
pub(crate) fn run_score(c: &mut pulse_cli::Context, args: &[String]) -> Result<()> {
    if args.first().is_some_and(|arg| arg == "metric") {
        return run_metric_score(c, &args[1..]);
    }
    let mut fs = pulse_cli::FlagSet::new("evolve score");
    add_common_flags(&mut fs, c);
    parse_command_flags(c, &mut fs, args)?;
    apply_common_flags(c, &fs);
    if fs.narg() != 1 {
        return Err(crate::fixtures::input_error(
            "Wrong score arguments",
            "score takes one starter fixture name",
            "pulse evolve score NAME",
        ));
    }
    let cancel = pulse_bus::CancellationToken::new();
    let _stop = crate::process::SignalStop::notify(&cancel)?;
    let cwd = pulse_store::cwd();
    let (score, reason) = crate::score::score_tree(
        &cancel,
        &cwd,
        fs.arg(0),
        &std::env::var("PULSE_MEASURE_COMMIT").unwrap_or_default(),
    )?;
    if c.json {
        return pulse_cli::write_json(
            &mut *c.out,
            &serde_json::json!({"name": fs.arg(0), "score": score, "reason": reason}),
        );
    }
    writeln!(c.out, "{score}")?;
    if !reason.is_empty() && !c.quiet {
        writeln!(c.err, "score 0: {reason}")?;
    }
    Ok(())
}

fn run_metric_score(c: &mut pulse_cli::Context, args: &[String]) -> Result<()> {
    let mut fs = pulse_cli::FlagSet::new("evolve score metric");
    add_common_flags(&mut fs, c);
    fs.bool_var("higher", false, "higher raw values are better");
    fs.bool_var("lower", false, "lower raw values are better");
    fs.float64_var("scale", f64::NAN, "raw value scale");
    fs.float64_var("floor", f64::NAN, "minimum score in [0,1]");
    parse_command_flags(c, &mut fs, args)?;
    apply_common_flags(c, &fs);
    if fs.get_bool("higher") == fs.get_bool("lower") {
        return Err(crate::fixtures::input_error(
            "Wrong metric score arguments",
            "choose exactly one of --higher or --lower",
            "pulse evolve score metric --higher --scale S -- CMD ARGS...",
        ));
    }
    let scale = fs.get_f64("scale");
    let floor = fs
        .is_set("floor")
        .then(|| fs.get_f64("floor"))
        .filter(|v| !v.is_nan());
    let argv = fs.args().to_vec();
    crate::metric::run_metric_score(
        &mut *c.out,
        &mut *c.err,
        fs.get_bool("higher"),
        scale,
        floor,
        &argv,
    )
}
pub fn run_fleet(
    c: &mut pulse_cli::Context,
    _fs: &pulse_cli::FlagSet,
    args: &[String],
) -> Result<()> {
    let mut args = args.to_vec();
    if args.is_empty() {
        args.push("list".to_string());
    }
    let sub = args[0].clone();
    let rest = &args[1..];
    if sub == "-h" || sub == "--help" {
        return Err(pulse_cli::FlagError::Help.into());
    }
    let mut fs = pulse_cli::FlagSet::new(format!("fleet {sub}"));
    add_common_flags(&mut fs, c);
    if sub == "prepare" {
        fs.int_var("n", 1, "workers in this wave (1-256)");
        fs.string_var("ref", "HEAD", "local commit, branch or tag");
    } else if !matches!(sub.as_str(), "verify" | "clean" | "list") {
        return Err(crate::fixtures::input_error(
            "Unknown fleet command",
            "choose prepare, verify, clean or list",
            "pulse help fleet",
        ));
    }
    parse_command_flags(c, &mut fs, rest)?;
    apply_common_flags(c, &fs);
    let pos = fs.args().to_vec();
    if ((sub == "verify" || sub == "clean") && pos.len() != 1)
        || ((sub == "prepare" || sub == "list") && !pos.is_empty())
    {
        return Err(crate::fixtures::input_error(
            "Wrong fleet arguments",
            "verify and clean take a fleet id",
            "pulse help fleet",
        ));
    }
    let mut sc = store_context(c);
    let result = with_store(&mut sc, sub == "verify" || sub == "list", |s| {
        match sub.as_str() {
            "prepare" => {
                let p = current_project(s, c)?;
                let actor = calling_actor(s, c)?;
                let cancel = pulse_bus::CancellationToken::new();
                let _stop = crate::process::SignalStop::notify(&cancel)?;
                let fleet = crate::fleet::prepare_fleet(
                    &cancel,
                    s,
                    &p.slug,
                    &fs.get_string("ref"),
                    fs.get_int("n") as i64,
                    &actor,
                )?;
                if c.json {
                    return pulse_cli::write_json(&mut *c.out, &fleet);
                }
                c.println(&fleet.id)?;
                for path in fleet.paths {
                    c.println(path)?;
                }
                Ok(())
            }
            "verify" => {
                let cancel = pulse_bus::CancellationToken::new();
                let report = crate::fleet::verify_fleet(&cancel, s, &pos[0])?;
                if c.json {
                    pulse_cli::write_json(&mut *c.out, &report)?;
                } else if !c.quiet {
                    let rows: Vec<Vec<String>> = report
                        .workers
                        .iter()
                        .map(|w| {
                            let result = if w.problem.is_empty() {
                                "detached; no remote; no tags; answer keys absent".to_string()
                            } else {
                                w.problem.clone()
                            };
                            vec![w.worker.to_string(), w.path.clone(), result]
                        })
                        .collect();
                    pulse_cli::table(
                        &mut *c.out,
                        c.color,
                        Some(&["WORKER", "CLONE", "STATE"]),
                        &rows,
                    )?;
                    writeln!(
                        c.out,
                        "\nWORKER RESULTS (store jobs; no result files are read)"
                    )?;
                    let rows: Vec<Vec<String>> = report
                        .jobs
                        .iter()
                        .map(|j| {
                            vec![
                                j.id.clone(),
                                j.status.clone(),
                                j.result.clone(),
                                j.branch.clone(),
                            ]
                        })
                        .collect();
                    pulse_cli::table(
                        &mut *c.out,
                        c.color,
                        Some(&["JOB", "STATUS", "RESULT", "BRANCH"]),
                        &rows,
                    )?;
                }
                if !report.ok {
                    return Err(crate::fixtures::input_error(
                        "Fleet verification failed",
                        "one or more clones violated isolation constraints",
                        &format!("pulse fleet clean {}", pos[0]),
                    ));
                }
                Ok(())
            }
            "clean" => {
                let actor = calling_actor(s, c)?;
                crate::fleet::clean_fleet(s, &pos[0], &actor)?;
                if c.json {
                    return pulse_cli::write_json(
                        &mut *c.out,
                        &pulse_store::gomap! {"id": pos[0], "cleaned": true},
                    );
                }
                c.println(format!("Cleaned fleet: {}", pos[0]))?;
                Ok(())
            }
            "list" => {
                let fleets = crate::fleet::list_fleets(s, &c.project)?;
                if c.json {
                    return pulse_cli::write_json(
                        &mut *c.out,
                        &pulse_store::gomap! {"fleets": fleets},
                    );
                }
                if !c.quiet {
                    let rows: Vec<Vec<String>> = fleets
                        .iter()
                        .map(|f| {
                            vec![
                                f.id.clone(),
                                f.project.clone(),
                                f.workers.to_string(),
                                f.ref_.clone(),
                                if f.cleaned_at.is_empty() {
                                    "prepared"
                                } else {
                                    "cleaned"
                                }
                                .to_string(),
                            ]
                        })
                        .collect();
                    pulse_cli::table(
                        &mut *c.out,
                        c.color,
                        Some(&["ID", "PROJECT", "WORKERS", "REF", "STATE"]),
                        &rows,
                    )?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    });
    result.map_err(|e| command_error(e, &format!("fleet {sub}")))
}
pub fn run_reflect(
    c: &mut pulse_cli::Context,
    _fs: &pulse_cli::FlagSet,
    args: &[String],
) -> Result<()> {
    let mut fs = pulse_cli::FlagSet::new("reflect");
    add_common_flags(&mut fs, c);
    fs.string_var("area", "", "board area");
    parse_command_flags(c, &mut fs, args)?;
    apply_common_flags(c, &fs);
    if fs.narg() != 0 {
        return Err(crate::fixtures::input_error(
            "Wrong reflection arguments",
            "reflect takes flags, not file paths",
            "pulse help reflect",
        ));
    }
    let owned;
    let s_opt = if let Some(s) = c.store.as_ref() {
        Some(s)
    } else {
        match pulse_store::Store::open(pulse_store::Options {
            home: c.home.clone(),
            create: false,
            read_only: true,
            skip_records: false,
            now: c.now.clone(),
        }) {
            Ok(s) => {
                owned = s;
                Some(&owned)
            }
            Err(e) if e.downcast_ref::<pulse_store::NotFoundError>().is_some() => None,
            Err(e) => return Err(command_error(e, "reflect")),
        }
    };
    let cancel = pulse_bus::CancellationToken::new();
    let result = crate::reflect::reflect(
        &cancel,
        s_opt,
        &c.project,
        &fs.get_string("area"),
        &c.agent,
        &c.session,
    )
    .map_err(|e| command_error(e, "reflect"))?;
    if c.json {
        return pulse_cli::write_json(&mut *c.out, &result);
    }
    if !c.quiet {
        crate::reflect::print_reflection(&mut *c.out, c.color, &result);
    }
    Ok(())
}
pub fn run_usage(
    c: &mut pulse_cli::Context,
    _fs: &pulse_cli::FlagSet,
    args: &[String],
) -> Result<()> {
    let mut fs = pulse_cli::FlagSet::new("usage");
    add_common_flags(&mut fs, c);
    fs.string_var(
        "runner",
        "",
        "copilot, claude, codex or opencode (default: all)",
    );
    fs.int_var(
        "days",
        7,
        "sessions updated within this many days; 0 = all history",
    );
    parse_command_flags(c, &mut fs, args)?;
    apply_common_flags(c, &fs);
    if fs.narg() != 0 {
        return Err(crate::fixtures::input_error(
            "Wrong usage arguments",
            "select a runner with --runner",
            "pulse usage --help",
        ));
    }
    let now = if c.now.is_empty() {
        chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::UNIX_EPOCH)
    } else {
        chrono::DateTime::parse_from_rfc3339(&c.now)
            .map_err(|_| {
                crate::fixtures::input_error(
                    "Invalid usage clock",
                    "now must be UTC RFC3339",
                    "pulse usage --days 7",
                )
            })?
            .with_timezone(&chrono::Utc)
    };
    let reports = crate::usage::usage(crate::usage::UsageOptions {
        runner: fs.get_string("runner"),
        days: fs.get_int("days") as i64,
        now: if c.now.is_empty() { None } else { Some(now) },
        home: String::new(),
    })
    .map_err(|e| command_error(e, "usage"))?;
    let note = "Best-effort diagnostics, not billing. Days selects recently updated sessions; cumulative totals may include earlier turns.";
    if c.json {
        return pulse_cli::write_json(
            &mut *c.out,
            &pulse_store::gomap! {"runners": reports, "days": fs.get_int("days"), "note": note},
        );
    }
    if c.quiet {
        return Ok(());
    }
    for report in reports {
        if report.status == "not found" {
            writeln!(c.out, "{}: not found", report.runner)?;
        } else {
            writeln!(c.out, "{}", report.runner)?;
            let rows: Vec<Vec<String>> = report
                .sessions
                .iter()
                .map(|s| {
                    vec![
                        s.id.clone(),
                        opt_i64(s.input),
                        opt_i64(s.output),
                        opt_i64(s.cache),
                        opt_i64(s.subagent_total),
                        opt_i64(s.total),
                        s.mode.clone(),
                    ]
                })
                .collect();
            pulse_cli::table(
                &mut *c.out,
                c.color,
                Some(&[
                    "SESSION",
                    "IN",
                    "OUT",
                    "CACHE",
                    "SUBAGENT TOTAL",
                    "TOTAL",
                    "MODE",
                ]),
                &rows,
            )?;
        }
        for warning in report.warnings {
            writeln!(c.out, "  {warning}")?;
        }
    }
    writeln!(c.out, "{note}")?;
    Ok(())
}
pub(crate) fn print_scores(w: &mut dyn Write, color: bool, s: &crate::api::Scores) {
    let rows: Vec<Vec<String>> = s
        .fixtures
        .iter()
        .map(|f| {
            vec![
                f.name.clone(),
                f.kind.clone(),
                format!("{:.4}", f.score),
                format_float(f.weight),
            ]
        })
        .collect();
    let _ = pulse_cli::table(
        w,
        color,
        Some(&["FIXTURE", "KIND", "SCORE", "WEIGHT"]),
        &rows,
    );
    let _ = writeln!(
        w,
        "Weighted total: {:.4}; held-out count: {}",
        s.total, s.heldout
    );
    let _ = writeln!(
        w,
        "Boundary: detached local clones; empty worker PULSE_HOME; no OS sandbox. Clones removed."
    );
}
pub(crate) fn print_archive(w: &mut dyn Write, color: bool, archive: &[crate::archive::Archived]) {
    let rows: Vec<Vec<String>> = archive
        .iter()
        .map(|a| {
            let decision =
                if a.kept && crate::api::is_baseline_row(&a.hypothesis, &a.suggestion_post) {
                    "baseline"
                } else if a.kept {
                    "kept"
                } else {
                    "discarded"
                };
            vec![
                a.id.clone(),
                a.project.clone(),
                format!("{:.4}", a.baseline_score),
                format!("{:.4}", a.variant_score),
                format!("{:.4}", a.tolerance),
                decision.to_string(),
                a.suggestion_post.clone(),
                a.run_id.clone(),
            ]
        })
        .collect();
    let _ = pulse_cli::table(
        w,
        color,
        Some(&[
            "ID",
            "PROJECT",
            "BASELINE",
            "VARIANT",
            "TOLERANCE",
            "DECISION",
            "SUGGESTION",
            "RUN",
        ]),
        &rows,
    );
}

pub(crate) fn print_lineage(w: &mut dyn Write, tree: &[crate::lineage::LineageNode]) {
    for node in tree {
        let indent = "  ".repeat(node.depth);
        let decision = if node.kept { "kept" } else { "discarded" };
        let hypothesis = crate::card_job::truncate(&node.hypothesis.replace('\n', " "), 80);
        let _ = writeln!(
            w,
            "{}{} {} {:+.3} cmp {:.2} children {} \"{}\"",
            indent, node.id, decision, node.score_delta, node.cmp, node.children, hypothesis
        );
    }
}

fn add_common_flags(fs: &mut pulse_cli::FlagSet, c: &pulse_cli::Context) {
    fs.string_var("project", &c.project, "project slug");
    fs.string_var("agent", &c.agent, "calling agent");
    fs.string_var("session", &c.session, "calling session");
    fs.string_var("home", &c.home, "PULSE home");
    fs.string_var("now", &c.now, "fixed UTC clock");
    fs.bool_var("json", c.json, "machine-readable output");
    fs.bool_var("quiet", c.quiet, "suppress ordinary text");
}

fn apply_common_flags(c: &mut pulse_cli::Context, fs: &pulse_cli::FlagSet) {
    c.project = fs.get_string("project");
    c.agent = fs.get_string("agent");
    c.session = fs.get_string("session");
    c.home = fs.get_string("home");
    c.now = fs.get_string("now");
    c.json = fs.get_bool("json");
    c.quiet = fs.get_bool("quiet");
}

fn store_context(c: &pulse_cli::Context) -> pulse_cli::Context {
    pulse_cli::Context {
        store: c.store.clone(),
        home: c.home.clone(),
        out: Box::new(std::io::sink()),
        err: Box::new(std::io::sink()),
        json: c.json,
        quiet: c.quiet,
        color: c.color,
        agent: c.agent.clone(),
        project: c.project.clone(),
        session: c.session.clone(),
        now: c.now.clone(),
        args: c.args.clone(),
    }
}

fn format_float(v: f64) -> String {
    let s = format!("{v}");
    s
}

fn collect_repeated(args: &[String], name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let long = format!("--{name}");
    let prefix = format!("--{name}=");
    let mut i = 0;
    while i < args.len() {
        if args[i] == long {
            if i + 1 < args.len() {
                out.push(args[i + 1].clone());
                i += 2;
                continue;
            }
        } else if let Some(v) = args[i].strip_prefix(&prefix) {
            out.push(v.to_string());
        }
        i += 1;
    }
    out
}

fn filter_repeated(args: &[String], name: &str) -> Vec<String> {
    let long = format!("--{name}");
    let prefix = format!("--{name}=");
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == long {
            i += 2;
            continue;
        }
        if args[i].starts_with(&prefix) {
            i += 1;
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

fn opt_i64(v: Option<i64>) -> String {
    v.map(|n| n.to_string())
        .unwrap_or_else(|| "unavailable".to_string())
}

fn run_evolve_sub(c: &mut pulse_cli::Context, sub: &str, args: &[String]) -> Result<()> {
    let mut fs = pulse_cli::FlagSet::new(format!("evolve {sub}"));
    add_common_flags(&mut fs, c);
    let mut selected = Vec::new();
    let mut filtered = args.to_vec();
    match sub {
        "report" => fs.int_var("days", 7, "the window the counts cover"),
        "measure" => {
            fs.string_var("tree", "", "local source checkout (committed tree only)");
            fs.string_var("ref", "HEAD", "local commit, branch or tag");
            fs.string_var(
                "fixture",
                "",
                "enabled public fixture name (repeatable; private set is always included)",
            );
            fs.bool_var("shadow", false, "post scores as a finding; never archive");
            selected = collect_repeated(args, "fixture");
            filtered = filter_repeated(args, "fixture");
        }
        "cycle" => fs.bool_var("once", false, "finish at most one experiment"),
        "status" | "gates" | "archive" | "lineage" | "setup" => {}
        _ => {}
    }
    parse_command_flags(c, &mut fs, &filtered)?;
    apply_common_flags(c, &fs);
    let pos = fs.args().to_vec();
    if (sub == "setup" && pos.len() != 1) || (sub != "setup" && !pos.is_empty()) {
        return Err(crate::fixtures::input_error(
            "Wrong evolve arguments",
            "setup takes a directory; other commands take flags",
            "pulse help evolve",
        ));
    }
    let shadow = fs.lookup("shadow").is_some_and(|_| fs.get_bool("shadow"));
    let read_only = matches!(sub, "status" | "report" | "gates" | "archive" | "lineage")
        || (sub == "measure" && !shadow);
    let mut sc = store_context(c);
    with_store(&mut sc, read_only, |s| {
        if sub == "archive" {
            if !c.project.is_empty() {
                crate::fixtures::valid_project(s, &c.project)?;
            }
            let rows = crate::archive::list_archive(s, &c.project)?;
            if c.json {
                return pulse_cli::write_json(&mut *c.out, &pulse_store::gomap! {"archive": rows});
            }
            if !c.quiet {
                print_archive(&mut *c.out, c.color, &rows);
            }
            return Ok(());
        }
        if sub == "lineage" {
            if c.project.is_empty() {
                return Err(crate::fixtures::input_error(
                    "Project required",
                    "lineage is per project",
                    "pulse evolve lineage --project P",
                ));
            }
            crate::fixtures::valid_project(s, &c.project)?;
            let rows = crate::archive::list_archive(s, &c.project)?;
            let tree = crate::lineage::tree(&rows);
            let summary = crate::lineage::summary(&rows);
            let parent_rate = crate::lineage::parent_rate(s);
            if c.json {
                return pulse_cli::write_json(
                    &mut *c.out,
                    &pulse_store::gomap! {
                        "lineage": tree,
                        "summary": summary,
                        "parent_rate": parent_rate
                    },
                );
            }
            if !c.quiet {
                print_lineage(&mut *c.out, &tree);
                writeln!(
                    c.out,
                    "Parent rate: {:.3} - {} candidates with a saved patch",
                    parent_rate, summary.candidates
                )?;
            }
            return Ok(());
        }
        if sub == "setup" {
            let result = crate::private::setup(s, &pos[0])?;
            if c.json {
                return pulse_cli::write_json(&mut *c.out, &result);
            }
            c.printf(format_args!(
                "Private fixture setup: {} ready, {} pending; {} template file(s) created. Values withheld.\n",
                result.heldout_state.count, result.heldout_state.pending, result.created
            ))?;
            if result.heldout_state.pending > 0 {
                c.println("Owner: fill the private templates, then run pulse evolve setup DIR again. Loop remains unchanged.")?;
            }
            return Ok(());
        }
        let p = current_project(s, c)?;
        match sub {
            "report" => {
                let report = crate::report::build_report(s, &p.slug, fs.get_int("days") as i64)?;
                if c.json {
                    return pulse_cli::write_json(&mut *c.out, &report);
                }
                if !c.quiet {
                    writeln!(c.out, "SELF-IMPROVEMENT LOOP - {}", p.slug)?;
                    if let Some(status) = &report.status {
                        pulse_cli::table(
                            &mut *c.out,
                            c.color,
                            None,
                            &crate::status::status_rows(status),
                        )?;
                    }
                    writeln!(c.out)?;
                    writeln!(c.out, "SELF-IMPROVEMENT REPORT - {}", p.slug)?;
                    pulse_cli::table(
                        &mut *c.out,
                        c.color,
                        None,
                        &crate::report::report_rows(&report),
                    )?;
                }
            }
            "status" | "gates" => {
                let status = crate::status::status(s, &p.slug)?;
                if c.json {
                    return pulse_cli::write_json(&mut *c.out, &status);
                }
                if !c.quiet {
                    writeln!(c.out, "SELF-IMPROVEMENT LOOP - {}", p.slug)?;
                    pulse_cli::table(
                        &mut *c.out,
                        c.color,
                        None,
                        &crate::status::status_rows(&status),
                    )?;
                }
            }
            "measure" => {
                if shadow {
                    pulse_board::resolve_agent(s, &c.agent, true)?;
                }
                let tree = if fs.get_string("tree").is_empty() {
                    p.path.clone()
                } else {
                    fs.get_string("tree")
                };
                let mut progress_sink = std::io::sink();
                let mut progress_buf: Option<&mut dyn Write> = if c.quiet {
                    Some(&mut progress_sink)
                } else {
                    None
                };
                let out_ref: Option<&mut dyn Write> = if c.quiet {
                    progress_buf.take()
                } else if c.json {
                    Some(&mut *c.err)
                } else {
                    Some(&mut *c.out)
                };
                let cancel = pulse_bus::CancellationToken::new();
                let _stop = crate::process::SignalStop::notify(&cancel)?;
                let scores = crate::measure::measure(
                    &cancel,
                    s,
                    &p.slug,
                    &tree,
                    &fs.get_string("ref"),
                    crate::api::MeasureOptions {
                        fixtures: selected.clone(),
                        exact: false,
                        shadow,
                        out: out_ref,
                    },
                )?;
                let mut finding = String::new();
                if shadow {
                    let body = pulse_store::gojson::to_string(&scores)?;
                    let post = pulse_board::create_post(
                        s,
                        pulse_board::PostInput {
                            kind: "finding".into(),
                            project: p.slug.clone(),
                            area: "evolution".into(),
                            thread: "shadow".into(),
                            agent: c.agent.clone(),
                            session: c.session.clone(),
                            summary: format!(
                                "Shadow measurement {:.4}; {} held-out fixtures; not archived",
                                scores.total, scores.heldout
                            ),
                            body,
                            ..Default::default()
                        },
                    )?;
                    finding = post.id;
                }
                if c.json {
                    return pulse_cli::write_json(
                        &mut *c.out,
                        &pulse_store::gomap! {"scores": scores, "finding": finding},
                    );
                }
                if !c.quiet {
                    print_scores(&mut *c.out, c.color, &scores);
                    if !finding.is_empty() {
                        writeln!(c.out, "Finding: {finding} (shadow; no archive row)")?;
                    }
                }
            }
            "cycle" => {
                let cancel = pulse_bus::CancellationToken::new();
                let _stop = crate::process::SignalStop::notify(&cancel)?;
                crate::cycle::cycle(
                    &cancel,
                    s,
                    &p.slug,
                    crate::cycle::CycleOptions {
                        once: fs.get_bool("once"),
                        agent: c.agent.clone(),
                        session: c.session.clone(),
                        out: Some(Box::new(std::io::sink())),
                        json: c.json,
                        interval: std::time::Duration::ZERO,
                    },
                )?;
            }
            _ => {}
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_measure::*;
    use std::io::Read;

    #[test]
    fn go_command_registration_and_retired_legacy_names() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        crate::register::register();
        for (name, group) in [
            ("evolve", "agents"),
            ("reflect", "agents"),
            ("fleet", "agents"),
            ("usage", "inspect"),
        ] {
            let cmd = pulse_cli::lookup(name)
                .unwrap_or_else(|| panic!("{name} was not registered as a native {group} command"));
            assert_eq!(cmd.group, group);
            assert!(cmd.no_store);
        }
        for name in ["nudge", "links"] {
            assert!(pulse_cli::lookup(name).is_none());
        }
    }

    #[test]
    fn command_json_and_shadow_finding_never_archive() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        crate::register::register();
        let s = test_store();
        let _repo = test_project(&s, "demo");
        join_test_agent(&s, "tester");
        fake_harness(&s, "test ! -e \"$PULSE_HOME/pulse.sqlite\"; exit 0");
        let (_, forbidden) = private_set(&s);
        {
            let mut guard = STDIN.lock().unwrap();
            *guard = Some(Box::new(std::io::Cursor::new("one public request")));
        }
        let (out, _, err) = run_test_command(
            &s,
            "evolve",
            &[
                "fixture",
                "add",
                "--name",
                "F01",
                "--request",
                "-",
                "--scorer",
                "printf 0.5",
                "--answer-key",
                "keys/answer.txt",
                "--json",
            ],
        );
        assert!(err.is_ok(), "{err:?}");
        require_json(&out);
        let (out, _, err) = run_test_command(
            &s,
            "evolve",
            &[
                "fixture",
                "add",
                "--name",
                "metric-lines",
                "--metric",
                "--scorer",
                "printf 0.4",
                "--json",
            ],
        );
        assert!(err.is_ok(), "{err:?}");
        let metric_fixture: crate::fixtures::Fixture = serde_json::from_str(&out).unwrap();
        assert!(metric_fixture.metric);
        let (list, _, err) = run_test_command(&s, "evolve", &["fixture", "list"]);
        assert!(err.is_ok(), "{err:?}");
        assert!(list.contains("metric"));
        let (metric_out, metric_err, err) = run_test_command(
            &s,
            "evolve",
            &[
                "score",
                "metric",
                "--higher",
                "--scale",
                "10",
                "--",
                "sh",
                "-c",
                "echo raw 5",
            ],
        );
        assert!(err.is_ok(), "{err:?}");
        assert!(metric_out.contains("score: 0.5000"));
        assert!(metric_err.contains("raw: 5"));
        let (out, progress, err) =
            run_test_command(&s, "evolve", &["measure", "--shadow", "--json"]);
        assert!(err.is_ok(), "{err:?}");
        require_json(&out);
        assert_private_absent(&(out + &progress), &forbidden);
        assert_eq!(count_rows(&s, "evolution_archive"), 0);
        let (kind, actor, body): (String, String, String) = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT kind, agent, body FROM posts ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(kind, "finding");
        assert_eq!(actor, "tester");
        assert_private_absent(&body, &forbidden);
        for args in [
            &["status", "--json"][..],
            &["archive", "--json"][..],
            &["fixture", "list", "--json"][..],
        ] {
            let before = count_rows(&s, "events");
            let (out, _, err) = run_test_command(&s, "evolve", args);
            assert!(err.is_ok(), "{err:?}");
            require_json(&out);
            assert_eq!(count_rows(&s, "events"), before);
        }
        let (out, _, err) = run_test_command(&s, "fleet", &["prepare", "--n", "2", "--json"]);
        assert!(err.is_ok(), "{err:?}");
        let fleet: crate::fleet::Fleet = serde_json::from_str(&out).unwrap();
        assert_eq!(fleet.paths.len(), 2);
        for args in [
            vec!["verify", fleet.id.as_str(), "--json"],
            vec!["list", "--json"],
            vec!["clean", fleet.id.as_str(), "--json"],
        ] {
            let (out, _, err) = run_test_command(&s, "fleet", &args);
            assert!(err.is_ok(), "{err:?}");
            require_json(&out);
        }
        assert_no_clones(&s);
    }

    #[test]
    fn reflect_read_only_board_notices_ballot_hygiene_and_health() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        crate::register::register();
        let s = cycle_ready();
        let p = s.project("demo").unwrap().unwrap();
        let marker = crate::test_helpers::temp_path().join("health-was-run");
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET verify_cmd = ? WHERE slug='demo'",
                [format!("touch '{}'", marker.display())],
            )
            .unwrap();
        voted_suggestion(&s, "ballot-item", "product", "value.txt", 1);
        test_git(&p.path, &["branch", "evolve/stray"]);
        test_git(&p.path, &["branch", "pulse/stray"]);
        let before = count_rows(&s, "events");
        let (out, _, err) = run_test_command(&s, "reflect", &["--json", "--area", "evolution"]);
        assert!(err.is_ok(), "{err:?}");
        let reflected: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(reflected["level"], "D");
        assert!(
            reflected["board"]
                .as_str()
                .unwrap_or_default()
                .contains("NOTICES")
        );
        assert!(!reflected["ballot"].as_array().unwrap().is_empty());
        assert_eq!(
            reflected["hygiene"]["branches"].as_array().unwrap().len(),
            2
        );
        assert_eq!(
            reflected["hygiene"]["worktrees"].as_array().unwrap().len(),
            1
        );
        let commands = reflected["next_commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for command in ["pulse post ", "pulse vote "] {
            assert!(commands.contains(command));
        }
        assert!(std::fs::metadata(marker).is_err());
        assert_eq!(count_rows(&s, "events"), before);
        let shared = reflected["shared_board"].as_str().unwrap_or_default();
        assert!(
            shared.starts_with("shared board: 0 open questions, 0 unresolved pitfall/pattern/wish threads, no post yet"),
            "{shared}"
        );
        let problems = reflected["problems"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            problems.contains("the shared board has no post yet")
                && problems.contains("--origin-project demo"),
            "{problems}"
        );
        let (text, _, err) = run_test_command(&s, "reflect", &["--area", "evolution"]);
        assert!(err.is_ok(), "{err:?}");
        let health = text.split("HEALTH CHECKS").nth(1).unwrap_or_default();
        assert!(
            health.contains("shared board: 0 open questions"),
            "{health}"
        );
        assert!(text.contains("SHARED BOARD (commons"), "{text}");
    }

    #[test]
    fn reflect_health_is_quiet_about_a_shared_board_that_is_in_use() {
        let now = "2026-10-14T12:00:00Z";
        let fresh = pulse_tally::SharedBoard {
            posts: 3,
            last_post: "2026-10-10T12:00:00Z".into(),
            ..Default::default()
        };
        assert_eq!(
            crate::reflect::shared_board_problem(&fresh, now, "demo"),
            None
        );
        let quiet = pulse_tally::SharedBoard {
            posts: 3,
            last_post: "2026-09-10T12:00:00Z".into(),
            ..Default::default()
        };
        let problem = crate::reflect::shared_board_problem(&quiet, now, "demo").unwrap();
        assert!(
            problem.starts_with("the shared board has been quiet for 34 days")
                && problem.contains("`FINDING: shared: AREA/THREAD - ...`")
                && problem.contains("pulse post finding --project commons --origin-project demo"),
            "{problem}"
        );
    }

    #[test]
    fn reflect_missing_store_still_observes() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let _env = pulse_testutil::env_lock();
        let home = crate::test_helpers::temp_path();
        unsafe {
            std::env::set_var("PULSE_HOME", &home);
            std::env::set_var("PULSE_JOB", "");
        }
        let cwd = crate::test_helpers::temp_path();
        std::env::set_current_dir(&cwd).unwrap();
        let out = pulse_cli::Buffer::new();
        let mut ctx = pulse_cli::Context {
            store: None,
            home: home.display().to_string(),
            out: Box::new(out.clone()),
            err: Box::new(out.clone()),
            json: false,
            quiet: false,
            color: false,
            project: String::new(),
            agent: String::new(),
            session: String::new(),
            now: String::new(),
            args: Vec::new(),
        };
        let fs = pulse_cli::FlagSet::new("reflect");
        run_reflect(&mut ctx, &fs, &["--json".into()]).unwrap();
        let result: crate::reflect::Reflection = serde_json::from_str(&out.string()).unwrap();
        assert_eq!(result.level, "A");
        assert_eq!(result.next_human_step, "pulse init PATH");
        assert!(std::fs::metadata(home.join(pulse_store::STORE_NAME)).is_err());
    }

    #[test]
    fn usage_cli_without_store_and_stop_reminder() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let _env = pulse_testutil::env_lock();
        let home = crate::test_helpers::temp_path();
        unsafe {
            std::env::set_var("HOME", &home);
            for key in [
                "COPILOT_STATE_DIR",
                "CLAUDE_CONFIG_DIR",
                "CODEX_HOME",
                "OPENCODE_STORE",
            ] {
                std::env::set_var(key, "");
            }
        }
        let out = pulse_cli::Buffer::new();
        let mut ctx = pulse_cli::Context {
            store: None,
            home: crate::test_helpers::temp_path().display().to_string(),
            out: Box::new(out.clone()),
            err: Box::new(out.clone()),
            json: false,
            quiet: false,
            color: false,
            project: String::new(),
            agent: String::new(),
            session: String::new(),
            now: String::new(),
            args: Vec::new(),
        };
        let fs = pulse_cli::FlagSet::new("usage");
        run_usage(&mut ctx, &fs, &["--days".into(), "7".into()]).unwrap();
        for name in ["copilot", "claude", "codex", "opencode"] {
            assert!(out.string().contains(&format!("{name}: not found")));
        }
        let out2 = pulse_cli::Buffer::new();
        ctx.out = Box::new(out2.clone());
        ctx.err = Box::new(out2.clone());
        run_usage(&mut ctx, &fs, &["--json".into()]).unwrap();
        require_json(&out2.string());
        assert!(crate::status::stop_reminder(None).contains("pulse reflect"));
        let s = test_store();
        s.set_setting("human", "stop", "1").unwrap();
        let reminder = crate::status::stop_reminder(Some(&s));
        assert!(reminder.contains("STOP is engaged"));
        assert!(!reminder.contains('\n'));
    }

    #[test]
    fn scorer_refuses_escaping_symlink() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let clone = crate::test_helpers::temp_path();
        let outside = crate::test_helpers::temp_path().join("preserve");
        put_file(&outside, "keep", 0o600);
        std::os::unix::fs::symlink(&outside, clone.join("outside")).unwrap();
        let cancel = pulse_bus::CancellationToken::new();
        let err = crate::measure::score_clone(
            &cancel,
            &clone.display().to_string(),
            &crate::process::isolated_env(&crate::test_helpers::temp_path().display().to_string()),
            &["sh".into(), "-c".into(), "printf 1".into()],
        )
        .expect_err("read-only scoring accepted an escaping symlink");
        assert!(err.to_string().contains("symlink") || err.to_string().contains("constraint"));
        let mut body = String::new();
        std::fs::File::open(outside)
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert_eq!(body, "keep");
    }
}
