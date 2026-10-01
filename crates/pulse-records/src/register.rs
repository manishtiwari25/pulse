use crate::bundle::{bundle_path, ensure_framework, export_bundle, framework_status};
use crate::export::{ExportOptions, export};
use crate::frontmatter::render_markdown;
use crate::import::{ImportOptions, import_repo};
use crate::leadtime::{lead_days, lead_time};
use crate::records::{Filter, NewOptions, input_error};
use anyhow::Result;
use pulse_cli::flags::Output;
use pulse_cli::{Command, Context, FlagSet, table, write_json};
use serde::Serialize;
use std::io::{Read, Write};
use std::sync::Once;

static REGISTER: Once = Once::new();

pub fn register() {
    REGISTER.call_once(|| {
        pulse_cli::register(Command {
            name: "record".into(),
            group: "records".into(),
            raw: true,
            summary: "list, read, create, revise, accept, deliver, or export records".into(),
            usage: "pulse record list|show|new|amend|status|accept|deliver|draft|adopt|discard|drafts|export|lead-time|revisions [flags] [args]".into(),
            run: run_record,
            aliases: vec![],
            no_store: false,
            read_only: false,
            flags: None,
        });
        pulse_cli::register(Command {
            name: "guide".into(),
            group: "records".into(),
            raw: true,
            read_only: true,
            summary: "read the stored agent operating instructions".into(),
            usage: "pulse guide [--full] [--project P] [--json]".into(),
            run: run_guide,
            aliases: vec![],
            no_store: false,
            flags: None,
        });
        pulse_cli::register(Command {
            name: "migrate".into(),
            group: "records".into(),
            raw: true,
            no_store: true,
            summary: "import a repository's committed Markdown records into the store (the schema migrates itself on open)".into(),
            usage: "pulse migrate [--records] --repo PATH [--ref HEAD] [--project P] [--dry-run] [--force-rev]".into(),
            run: run_migrate,
            aliases: vec![],
            read_only: false,
            flags: None,
        });
        pulse_cli::register(Command {
            name: "seed".into(),
            group: "records".into(),
            raw: true,
            summary: "the framework's records the binary carries (ADR-038): apply them to this store, export them from the brain, or compare".into(),
            usage: "pulse seed apply|status [--json] | export [--out PATH] [--json]".into(),
            run: run_seed,
            aliases: vec![],
            no_store: false,
            read_only: false,
            flags: None,
        });
    });
}

fn store(c: &Context) -> Result<&pulse_store::Store> {
    c.store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("records command requires a store"))
}

pub fn run_seed(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(input_error(
            "Seed command required",
            "choose apply, export or status",
            "pulse help seed",
        ));
    }
    let sub = args[0].as_str();
    let mut fs = FlagSet::new(format!("seed {sub}"));
    fs.bool_var("json", c.json, "print JSON");
    if sub == "export" {
        fs.string_var(
            "out",
            "",
            &format!(
                "bundle path (default: the brain checkout's {})",
                crate::bundle::BUNDLE_FILE
            ),
        );
    } else if sub != "apply" && sub != "status" {
        return Err(input_error(
            &format!("Unknown seed command {sub}"),
            "choose apply, export or status",
            "pulse help seed",
        ));
    }
    parse_flags(c, &mut fs, &args[1..])?;
    if fs.narg() != 0 {
        return Err(input_error(
            "Unexpected seed arguments",
            &fs.args().join(" "),
            &format!("pulse seed {sub}"),
        ));
    }
    let json_out = fs.get_bool("json");
    match sub {
        "apply" => {
            let result = ensure_framework(store(c)?, &author(c))?;
            if json_out {
                return write_json(&mut c.out, &result);
            }
            if result.written.is_empty() {
                c.printf(format_args!(
                    "nothing to seed: the store has all {} framework records\n",
                    result.kept
                ))?;
            } else {
                c.printf(format_args!(
                    "seeded {} framework record(s) from {} ({} already present): {}\n",
                    result.written.len(),
                    result.from,
                    result.kept,
                    result.written.join(", ")
                ))?;
            }
        }
        "status" => {
            let status = framework_status(store(c)?)?;
            if json_out {
                return write_json(&mut c.out, &status);
            }
            c.printf(format_args!(
                "bundle: {} framework record(s); this store has {}\n",
                status.records, status.present
            ))?;
            if !status.missing.is_empty() {
                c.printf(format_args!(
                    "missing here (pulse seed apply writes them): {}\n",
                    status.missing.join(", ")
                ))?;
            }
            if !status.brain.is_empty() {
                if status.stale.is_empty() && status.unshipped.is_empty() {
                    c.printf(format_args!(
                        "brain {}: the bundle matches its framework records\n",
                        status.brain
                    ))?;
                } else {
                    c.printf(format_args!(
                        "brain {}: {} record(s) changed since the bundle shipped, {} not yet shipped; pulse seed export refreshes it\n",
                        status.brain,
                        status.stale.len(),
                        status.unshipped.len()
                    ))?;
                    if !status.stale.is_empty() {
                        c.printf(format_args!("  changed: {}\n", status.stale.join(", ")))?;
                    }
                    if !status.unshipped.is_empty() {
                        c.printf(format_args!(
                            "  unshipped: {}\n",
                            status.unshipped.join(", ")
                        ))?;
                    }
                }
            }
        }
        "export" => {
            let path = bundle_path(store(c)?, &fs.get_string("out"))?;
            let summary = export_bundle(store(c)?, &path)?;
            if json_out {
                return write_json(&mut c.out, &summary);
            }
            c.printf(format_args!(
                "wrote {} framework record(s) to {}\n",
                summary.records, summary.path
            ))?;
            if summary.same() {
                c.println("no change against the previous bundle")?;
            } else {
                c.printf(format_args!(
                    "added {}, changed {}, removed {}, unchanged {}\n",
                    summary.added.len(),
                    summary.changed.len(),
                    summary.removed.len(),
                    summary.unchanged
                ))?;
            }
            if !summary.skipped.is_empty() {
                c.printf(format_args!(
                    "left out (retired): {}\n",
                    summary.skipped.join(", ")
                ))?;
            }
            c.println("commit the file; the next build carries it")?;
        }
        _ => {}
    }
    Ok(())
}

pub fn run_record(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(input_error(
            "Record command required",
            "choose list, show, new, amend, status, accept, deliver, draft, adopt, discard, drafts, export, lead-time, or revisions",
            "pulse help record",
        ));
    }
    let sub = args[0].as_str();
    let mut fs = FlagSet::new(format!("record {sub}"));
    fs.bool_var("json", c.json, "print JSON");
    let project_default = c.project.clone();
    match sub {
        "list" => {
            fs.string_var("project", &project_default, "project slug");
            fs.string_var("status", "", "status");
            fs.string_var("area", "", "area");
        }
        "show" => fs.int_var("rev", 0, "revision (0 = current)"),
        "new" => {
            fs.string_var("project", &project_default, "project slug");
            fs.string_var("title", "", "title");
            fs.string_var("status", "", "status");
            fs.string_var("areas", "", "comma-separated areas");
            fs.string_var("tags", "", "comma-separated tags");
            fs.int_var("tier", 0, "lifecycle tier");
            fs.string_var("body-file", "", "file, stdin, or -");
        }
        "amend" => {
            fs.string_var("body-file", "", "file, stdin, or -");
            fs.string_var("note", "", "revision note");
        }
        "accept" => fs.string_var("words", "", "the human's acceptance words"),
        "draft" => {
            fs.string_var("body-file", "", "file, stdin, or -");
            fs.string_var("note", "", "one line on what the draft is based on");
        }
        "adopt" | "discard" => {
            fs.string_var("words", "", "the owner's words for the revision note")
        }
        "drafts" => fs.string_var("project", &project_default, "project slug"),
        "deliver" => fs.string_var("date", "", "delivery date (YYYY-MM-DD)"),
        "export" => {
            fs.string_var("kind", "", "record kind");
            fs.string_var("project", &project_default, "project slug");
            fs.bool_var("force", false, "allow export inside a git checkout");
        }
        "lead-time" => fs.string_var("project", &project_default, "project slug"),
        "status" | "revisions" => {}
        _ => {
            return Err(input_error(
                &format!("Unknown record command {sub}"),
                "choose a supported record command",
                "pulse help record",
            ));
        }
    }
    parse_flags(c, &mut fs, &args[1..])?;
    let rest = fs.args().to_vec();
    let json_out = fs.get_bool("json");
    if sub == "list" {
        if rest.len() > 1 {
            return Err(usage_error(
                sub,
                "list [kind] [--project P] [--status S] [--area A] [--json]",
            ));
        }
        let kind = rest.first().cloned().unwrap_or_default();
        let rows = crate::records::list(
            store(c)?,
            Filter {
                project: fs.get_string("project"),
                kind,
                status: fs.get_string("status"),
                area: fs.get_string("area"),
                ..Filter::default()
            },
        )?;
        if json_out {
            return write_json(&mut c.out, &RecordsOut { records: rows });
        }
        let table_rows: Vec<_> = rows
            .iter()
            .map(|r| {
                vec![
                    r.id.clone(),
                    r.project.clone(),
                    r.status.clone(),
                    r.delivery.clone(),
                    r.title.clone(),
                ]
            })
            .collect();
        if !c.quiet {
            table(
                &mut c.out,
                c.color,
                Some(&["ID", "PROJECT", "STATUS", "DELIVERY", "TITLE"]),
                &table_rows,
            )?;
        }
        return Ok(());
    }
    if sub == "drafts" {
        if !rest.is_empty() {
            return Err(usage_error(sub, "drafts [--project P] [--json]"));
        }
        let rows = crate::drafts::drafts(store(c)?, &fs.get_string("project"))?;
        if json_out {
            return write_json(&mut c.out, &DraftsOut { drafts: rows });
        }
        let table_rows: Vec<_> = rows
            .iter()
            .map(|r| {
                vec![
                    r.record_id.clone(),
                    r.project.clone(),
                    format!("r{}", r.base_rev),
                    r.author.clone(),
                    r.at.clone(),
                    r.note.clone(),
                ]
            })
            .collect();
        if !c.quiet {
            table(
                &mut c.out,
                c.color,
                Some(&["RECORD", "PROJECT", "AGAINST", "AUTHOR", "AT", "NOTE"]),
                &table_rows,
            )?;
        }
        return Ok(());
    }
    if sub == "lead-time" {
        if !rest.is_empty() {
            return Err(usage_error(sub, "lead-time [--project P] [--json]"));
        }
        let rows = lead_time(store(c)?, &fs.get_string("project"))?;
        if json_out {
            return write_json(&mut c.out, &RecordsOut { records: rows });
        }
        let table_rows: Vec<_> = rows
            .iter()
            .map(|r| {
                vec![
                    r.kind.clone(),
                    r.id.clone(),
                    r.tier.to_string(),
                    r.status.clone(),
                    r.delivery.clone(),
                    r.proposed.clone(),
                    r.delivered.clone(),
                    lead_days(r),
                    r.target.clone(),
                ]
            })
            .collect();
        if !c.quiet {
            table(
                &mut c.out,
                c.color,
                Some(&[
                    "KIND",
                    "ID",
                    "TIER",
                    "STATUS",
                    "DELIVERY",
                    "PROPOSED",
                    "DELIVERED",
                    "DAYS",
                    "TARGET",
                ]),
                &table_rows,
            )?;
        }
        return Ok(());
    }
    let want = if sub == "status" || sub == "deliver" {
        2
    } else {
        1
    };
    if rest.len() != want {
        return Err(usage_error(
            sub,
            &format!("{sub} <ID or kind or directory> [flags]"),
        ));
    }
    let id = &rest[0];
    match sub {
        "show" => {
            let revision = fs.get_int("rev") as i64;
            if revision < 0 {
                return Err(input_error(
                    "Cannot show revision",
                    "--rev must be zero or a positive number",
                    &format!("pulse record revisions {id}"),
                ));
            }
            let (rec, rev) =
                crate::records::get(store(c)?, id, revision).map_err(|e| record_error(id, e))?;
            if json_out {
                let revisions = store(c)?.revisions(id)?;
                return write_json(
                    &mut c.out,
                    &ShowOut {
                        body: &rev.body,
                        frontmatter: &rev.frontmatter,
                        record: &rec,
                        rev: rev.rev,
                        revisions,
                    },
                );
            }
            if !c.quiet {
                write!(c.out, "{}", render_markdown(&rec, &rev))?;
            }
        }
        "new" => {
            let body_file = fs.get_string("body-file");
            let mut opts = NewOptions {
                areas: split_csv(&fs.get_string("areas")),
                tags: split_csv(&fs.get_string("tags")),
                tier: fs.get_int("tier") as i64,
                tier_set: fs.is_set("tier"),
                status: fs.get_string("status"),
                author: author(c),
                ..NewOptions::default()
            };
            if !body_file.is_empty() {
                opts.body = read_body(&body_file)?;
                opts.body_set = true;
            }
            let rec = crate::records::new_record(
                store(c)?,
                id,
                &fs.get_string("project"),
                &fs.get_string("title"),
                opts,
            )?;
            print_record_write(c, &rec, json_out)?;
        }
        "amend" => {
            let body_file = fs.get_string("body-file");
            if body_file.is_empty() {
                return Err(usage_error(
                    sub,
                    &format!("amend {id} --body-file F|stdin [--note TEXT]"),
                ));
            }
            let body = read_body(&body_file)?;
            if body.trim().is_empty() {
                return Err(input_error(
                    "Empty record body",
                    "the body file has no content",
                    "pulse record amend ID --body-file F with a non-empty file, or stdin",
                ));
            }
            let rec =
                crate::records::amend(store(c)?, id, &body, &author(c), &fs.get_string("note"))
                    .map_err(|e| record_error(id, e))?;
            print_record_write(c, &rec, json_out)?;
        }
        "status" | "accept" | "deliver" => {
            let result = match sub {
                "status" => crate::records::status(store(c)?, id, &rest[1], &author(c)),
                "accept" => crate::records::accept(store(c)?, id, &fs.get_string("words")),
                "deliver" => crate::records::deliver(
                    store(c)?,
                    id,
                    &rest[1],
                    &fs.get_string("date"),
                    &author(c),
                ),
                _ => unreachable!(),
            };
            result.map_err(|e| record_error(id, e))?;
            let rec = store(c)?.get_record(id, 0)?.0;
            print_record_write(c, &rec, json_out)?;
        }
        "draft" => {
            let body_file = fs.get_string("body-file");
            if body_file.is_empty() {
                return Err(usage_error(
                    sub,
                    &format!("draft {id} --body-file F|stdin [--note TEXT]"),
                ));
            }
            let body = read_body(&body_file)?;
            let draft = crate::drafts::draft(
                store(c)?,
                id,
                crate::drafts::DraftInput {
                    body,
                    author: author(c),
                    note: fs.get_string("note"),
                    job_id: std::env::var("PULSE_JOB").unwrap_or_default(),
                },
            )
            .map_err(|e| record_error(id, e))?
            .expect("draft stored");
            if json_out {
                return write_json(&mut c.out, &DraftOut { draft });
            }
            c.printf(format_args!("{} draft filed against r{} by {}; the owner adopts it in the hub or with pulse record adopt {}\n", id, draft.base_rev, draft.author, id))?;
        }
        "adopt" => {
            let rec = crate::drafts::adopt(store(c)?, id, &author(c), &fs.get_string("words"))
                .map_err(|e| record_error(id, e))?;
            print_record_write(c, &rec, json_out)?;
        }
        "discard" => {
            let gone = crate::drafts::discard(store(c)?, id, &author(c), &fs.get_string("words"))
                .map_err(|e| record_error(id, e))?;
            if json_out {
                return write_json(&mut c.out, &DiscardOut { discarded: gone });
            }
            if gone {
                c.printf(format_args!("{id} draft discarded\n"))?;
            } else {
                c.printf(format_args!("{id} has no draft\n"))?;
            }
        }
        "export" => {
            let n = export(
                store(c)?,
                id,
                ExportOptions {
                    kind: fs.get_string("kind"),
                    project: fs.get_string("project"),
                    force: fs.get_bool("force"),
                },
            )?;
            if json_out {
                return write_json(
                    &mut c.out,
                    &ExportOut {
                        directory: id,
                        exported: n,
                    },
                );
            }
            c.printf(format_args!("Exported {n} records to {id}\n"))?;
        }
        "revisions" => {
            store(c)?
                .get_record(id, 0)
                .map_err(|e| record_error(id, e))?;
            let rows = store(c)?.revisions(id)?;
            if json_out {
                return write_json(&mut c.out, &RevisionsOut { revisions: rows });
            }
            let table_rows: Vec<_> = rows
                .iter()
                .map(|r| {
                    vec![
                        r.rev.to_string(),
                        r.author.clone(),
                        r.at.clone(),
                        r.note.clone(),
                    ]
                })
                .collect();
            if !c.quiet {
                table(
                    &mut c.out,
                    c.color,
                    Some(&["REV", "AUTHOR", "AT", "NOTE"]),
                    &table_rows,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn run_migrate(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("migrate");
    fs.bool_var(
        "records",
        true,
        "accepted for compatibility: the import is the only migration left",
    );
    fs.string_var("repo", ".", "source repository");
    fs.string_var("ref", "HEAD", "source git ref");
    fs.string_var("project", &c.project, "project slug");
    fs.bool_var("dry-run", false, "preview without writes");
    fs.bool_var(
        "force-rev",
        false,
        "revise already imported paths whose body changed",
    );
    fs.bool_var("json", c.json, "print JSON");
    parse_flags(c, &mut fs, args)?;
    if fs.narg() != 0 {
        return Err(input_error(
            "Unexpected migrate arguments",
            &fs.args().join(" "),
            "pulse migrate --records --repo .",
        ));
    }
    let owned;
    let s = if let Some(s) = &c.store {
        s
    } else {
        owned = pulse_store::Store::open(pulse_store::Options {
            home: c.home.clone(),
            read_only: fs.get_bool("dry-run"),
            now: c.now.clone(),
            ..pulse_store::Options::default()
        })?;
        &owned
    };
    let summary = import_repo(
        s,
        &fs.get_string("repo"),
        &fs.get_string("ref"),
        &fs.get_string("project"),
        ImportOptions {
            dry_run: fs.get_bool("dry-run"),
            force_rev: fs.get_bool("force-rev"),
            author: author(c),
            paths: vec![],
        },
    )?;
    if fs.get_bool("json") {
        return write_json(&mut c.out, &summary);
    }
    if c.quiet {
        return Ok(());
    }
    if summary.dry_run {
        c.println("Dry run: no records or events written.")?;
    }
    let table_rows: Vec<_> = pulse_store::RECORD_KINDS
        .iter()
        .map(|kind| {
            let count = summary.kinds.get(*kind).cloned().unwrap_or_default();
            vec![
                (*kind).to_string(),
                count.imported.to_string(),
                count.skipped.to_string(),
                count.revised.to_string(),
            ]
        })
        .collect();
    table(
        &mut c.out,
        c.color,
        Some(&["KIND", "IMPORTED", "SKIPPED", "REVISED"]),
        &table_rows,
    )?;
    if !summary.skipped.is_empty() {
        c.println("\nSkipped files:")?;
        for skipped in summary.skipped {
            c.printf(format_args!("  {}: {}\n", skipped.path, skipped.reason))?;
        }
    }
    Ok(())
}

pub fn run_guide(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("guide");
    fs.bool_var("full", false, "include Claude and Copilot instructions");
    fs.string_var("project", &c.project, "project slug");
    fs.bool_var("json", c.json, "print JSON");
    parse_flags(c, &mut fs, args)?;
    if fs.narg() != 0 {
        return Err(input_error(
            "Unexpected guide arguments",
            &fs.args().join(" "),
            "pulse guide --full",
        ));
    }
    let p = if !fs.get_string("project").is_empty() {
        Some(crate::records::resolve_project(
            store(c)?,
            &fs.get_string("project"),
            "",
        )?)
    } else {
        match store(c)?.project_for_dir(&pulse_store::cwd())? {
            Some(p) => Some(p),
            None => store(c)?.brain()?,
        }
    };
    let mut slugs = vec!["agents"];
    if fs.get_bool("full") {
        slugs.extend(["claude", "copilot-instructions"]);
    }
    let mut output = Vec::new();
    let mut texts = Vec::new();
    for slug in slugs {
        let mut ids = Vec::new();
        if let Some(p) = &p {
            ids.push(crate::records::scoped_id("instruction", slug, p));
        }
        if p.as_ref().is_none_or(|p| !p.brain) {
            ids.push(format!("INSTR-{slug}"));
        }
        for id in ids {
            match store(c)?.get_record(&id, 0) {
                Ok((rec, Some(rev))) if rec.kind == "instruction" => {
                    texts.push(render_markdown(&rec, &rev));
                    output.push(GuideEntry {
                        record: rec,
                        frontmatter: rev.frontmatter,
                        body: rev.body,
                    });
                    break;
                }
                Ok(_) => {}
                Err(e) if e.downcast_ref::<pulse_store::ErrNoRecord>().is_some() => {}
                Err(e) => return Err(e),
            }
        }
    }
    let hint = "No instruction records; pulse seed apply writes the framework's, pulse migrate --records --repo PATH imports a checkout's.";
    if fs.get_bool("json") {
        return write_json(
            &mut c.out,
            &GuideOut {
                hint: if output.is_empty() { Some(hint) } else { None },
                records: output,
            },
        );
    }
    if texts.is_empty() {
        c.println(hint)?;
    } else if !c.quiet {
        write!(c.out, "{}", texts.join("\n\n"))?;
    }
    Ok(())
}

/// Go: `fs.SetOutput(c.Err)` — what Parse prints (usage on `-h`, bad flag
/// messages) goes to the command's error writer.
pub fn parse_flags(c: &mut Context, fs: &mut FlagSet, args: &[String]) -> Result<()> {
    fs.set_output(Output::Capture);
    let parsed = parse_flag_args(fs, args);
    let printed = fs.take_output();
    if !printed.is_empty() {
        let _ = c.err.write_all(&printed);
    }
    parsed
}

fn parse_flag_args(fs: &mut FlagSet, args: &[String]) -> Result<()> {
    let mut flags = Vec::new();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            positional.extend_from_slice(&args[i + 1..]);
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            positional.push(arg.clone());
            i += 1;
            continue;
        }
        let name = arg
            .trim_start_matches('-')
            .split_once('=')
            .map_or(arg.trim_start_matches('-'), |(n, _)| n);
        let equal = arg.contains('=');
        let Some(flag) = fs.lookup(name) else {
            if name == "h" || name == "help" {
                return fs
                    .parse(&["-h".to_string()])
                    .map_err(|e| anyhow::anyhow!(e));
            }
            return Err(input_error(
                &format!("Unknown flag {arg}"),
                "this command does not accept that flag",
                "pulse help record",
            ));
        };
        let is_bool = matches!(flag.value, pulse_cli::FlagValue::Bool(_));
        flags.push(arg.clone());
        if !equal && !is_bool {
            if i + 1 == args.len() {
                return Err(input_error(
                    &format!("Missing value for {arg}"),
                    "the flag requires a value",
                    "pulse help record",
                ));
            }
            i += 1;
            flags.push(args[i].clone());
        }
        i += 1;
    }
    flags.push("--".into());
    flags.extend(positional);
    fs.parse(&flags).map_err(|e| anyhow::anyhow!(e))
}

fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn read_body(file: &str) -> Result<String> {
    if file == "stdin" || file == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        return Ok(s);
    }
    std::fs::read_to_string(file).map_err(|e| {
        input_error(
            "Cannot read record body",
            &e.to_string(),
            "pulse record amend ID --body-file stdin",
        )
    })
}

fn print_record_write(c: &mut Context, rec: &pulse_store::Record, json_out: bool) -> Result<()> {
    if json_out {
        return write_json(&mut c.out, &RecordOut { record: rec });
    }
    c.printf(format_args!(
        "{} revision {}: {}\n",
        rec.id, rec.current_rev, rec.status
    ))?;
    Ok(())
}

fn record_error(id: &str, err: anyhow::Error) -> anyhow::Error {
    if err.downcast_ref::<pulse_store::ErrNoRecord>().is_some() {
        return input_error(
            &format!("Unknown record {id}"),
            "no record has this id",
            "pulse record list",
        );
    }
    err
}

fn usage_error(sub: &str, usage: &str) -> anyhow::Error {
    input_error(
        &format!("Invalid record {sub} arguments"),
        &format!("expected {usage}"),
        "pulse help record",
    )
}

fn author(c: &Context) -> String {
    if c.agent.is_empty() {
        pulse_store::HUMAN_AGENT.into()
    } else {
        c.agent.clone()
    }
}

#[derive(Serialize)]
struct RecordsOut<T> {
    records: T,
}

#[derive(Serialize)]
struct DraftsOut<T> {
    drafts: T,
}

#[derive(Serialize)]
struct DraftOut<T> {
    draft: T,
}

#[derive(Serialize)]
struct RecordOut<'a> {
    record: &'a pulse_store::Record,
}

#[derive(Serialize)]
struct DiscardOut {
    discarded: bool,
}

#[derive(Serialize)]
struct ExportOut<'a> {
    directory: &'a str,
    exported: usize,
}

#[derive(Serialize)]
struct RevisionsOut<T> {
    revisions: T,
}

#[derive(Serialize)]
struct ShowOut<'a> {
    body: &'a str,
    frontmatter: &'a std::collections::BTreeMap<String, serde_json::Value>,
    record: &'a pulse_store::Record,
    rev: i64,
    revisions: Vec<pulse_store::Revision>,
}

#[derive(Serialize)]
struct GuideEntry {
    body: String,
    frontmatter: std::collections::BTreeMap<String, serde_json::Value>,
    record: pulse_store::Record,
}

#[derive(Serialize)]
struct GuideOut<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'a str>,
    records: Vec<GuideEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    struct TestStore {
        _home: tempfile::TempDir,
        store: pulse_store::Store,
    }
    impl std::ops::Deref for TestStore {
        type Target = pulse_store::Store;
        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }
    fn temp_store() -> TestStore {
        let home = tempfile::tempdir().unwrap();
        pulse_testutil::neutralize_runner_environment();
        let store = pulse_store::Store::open(pulse_store::Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        TestStore { _home: home, store }
    }

    fn add_project(s: &pulse_store::Store, slug: &str, brain: bool) {
        let dir = std::path::Path::new(&s.home).join(format!("project-{slug}"));
        std::fs::create_dir_all(&dir).unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug,name,path,brain,added_at) VALUES (?,?,?,?,?)",
                rusqlite::params![slug, slug, dir.display().to_string(), brain, s.now()],
            )
            .unwrap();
    }

    fn context(s: pulse_store::Store) -> (Context, pulse_cli::Buffer) {
        let out = pulse_cli::Buffer::new();
        let err = pulse_cli::Buffer::new();
        (
            Context {
                store: Some(s.clone()),
                home: s.home.clone(),
                out: Box::new(out.clone()),
                err: Box::new(err),
                json: false,
                quiet: false,
                color: false,
                agent: "agent".into(),
                project: "brain".into(),
                session: String::new(),
                now: "2026-09-06T21:00:00Z".into(),
                args: vec![],
            },
            out,
        )
    }

    #[test]
    fn cli_commands_and_guide() {
        let _serial = crate::test_helpers::serial();
        register();
        register();
        assert!(pulse_cli::lookup("record").is_some());
        assert!(pulse_cli::lookup("guide").is_some());

        let s = temp_store();
        add_project(&s, "brain", true);
        let (mut c, out) = context(s.clone());
        run_guide(&mut c, &FlagSet::new(""), &[]).unwrap();
        assert!(
            out.string().lines().count() == 1 && out.string().contains("pulse migrate --records"),
            "empty guide: {:?}",
            out.string()
        );
        for slug in ["agents", "claude", "copilot-instructions"] {
            s.put_record(
                pulse_store::Record {
                    id: format!("INSTR-{slug}"),
                    project: "brain".into(),
                    kind: "instruction".into(),
                    slug: slug.into(),
                    ..Default::default()
                },
                Default::default(),
                &format!("# {slug}\n"),
                "tester",
                "",
            )
            .unwrap();
        }
        out.clear();
        run_guide(
            &mut c,
            &FlagSet::new(""),
            &["--full".into(), "--project".into(), "brain".into()],
        )
        .unwrap();
        assert_eq!(
            out.string(),
            "# agents\n\n\n# claude\n\n\n# copilot-instructions\n",
            "full guide order"
        );
        out.clear();
        let body = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(body.path(), "# New body\n").unwrap();
        let calls: Vec<Vec<String>> = vec![
            vec![
                "new".into(),
                "adr".into(),
                "--title".into(),
                "A title with spaces".into(),
                "--areas".into(),
                "cli,data".into(),
                "--tags".into(),
                "one, two".into(),
                "--body-file".into(),
                body.path().display().to_string(),
            ],
            vec![
                "list".into(),
                "ADR".into(),
                "--status".into(),
                "Proposed".into(),
                "--area".into(),
                "cli".into(),
                "--json".into(),
            ],
            vec![
                "show".into(),
                "ADR-001".into(),
                "--rev".into(),
                "1".into(),
                "--json".into(),
            ],
            vec![
                "status".into(),
                "ADR-001".into(),
                "Draft".into(),
                "--json".into(),
            ],
            vec![
                "accept".into(),
                "ADR-001".into(),
                "--words".into(),
                "The owner approves".into(),
            ],
            vec![
                "deliver".into(),
                "ADR-001".into(),
                "partially delivered".into(),
                "--date".into(),
                "2026-09-06".into(),
            ],
            vec![
                "amend".into(),
                "ADR-001".into(),
                "--body-file".into(),
                body.path().display().to_string(),
                "--note".into(),
                "Clarified".into(),
            ],
            vec!["revisions".into(), "ADR-001".into(), "--json".into()],
            vec![
                "lead-time".into(),
                "--project".into(),
                "brain".into(),
                "--json".into(),
            ],
        ];
        for args in calls {
            out.clear();
            run_record(&mut c, &FlagSet::new(""), &args)
                .unwrap_or_else(|err| panic!("record {args:?}: {err}"));
            if args.last().is_some_and(|arg| arg == "--json") {
                assert!(
                    serde_json::from_str::<Value>(&out.string()).is_ok(),
                    "invalid JSON for {args:?}: {}",
                    out.string()
                );
            }
        }
        assert!(
            run_record(
                &mut c,
                &FlagSet::new(""),
                &[
                    "new".into(),
                    "adr".into(),
                    "--title".into(),
                    "Wrong".into(),
                    "--status".into(),
                    "Accepted".into()
                ]
            )
            .is_err()
        );
        for args in [
            vec!["show".into(), "MISSING".into()],
            vec!["show".into(), "ADR-001".into(), "--rev".into(), "-1".into()],
            vec!["new".into(), "wu".into()],
            vec!["list".into(), "no-kind".into()],
            vec!["list".into(), "--bogus".into()],
            vec!["amend".into(), "ADR-001".into()],
            vec!["new".into(), "wu".into(), "--title".into()],
            vec!["status".into(), "ADR-001".into()],
        ] {
            assert!(
                run_record(&mut c, &FlagSet::new(""), &args).is_err(),
                "accepted invalid CLI arguments {args:?}"
            );
        }
        let stdin_body = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(stdin_body.path(), "From stdin\n").unwrap();
        run_record(
            &mut c,
            &FlagSet::new(""),
            &[
                "amend".into(),
                "ADR-001".into(),
                "--body-file".into(),
                stdin_body.path().display().to_string(),
            ],
        )
        .unwrap();
        let (_, rev) = crate::records::get(&s, "ADR-001", 0).unwrap();
        assert_eq!(rev.body, "From stdin\n", "stdin body not stored");
    }

    #[test]
    fn amend_refuses_empty_body() {
        let _serial = crate::test_helpers::serial();
        let cases = [
            ("empty", "", true),
            ("ascii-whitespace", " \t\r\n\u{000b}\u{000c}", true),
            (
                "unicode-whitespace",
                "\u{0085}\u{00a0}\u{2003}\u{2028}\u{2029}\u{3000}",
                true,
            ),
            ("one-character", "x", false),
            (
                "padded-content",
                " \t\u{2003}# Revised body\r\n\u{00a0} \t",
                false,
            ),
        ];
        for (name, body, blank) in cases {
            let s = temp_store();
            add_project(&s, "brain", true);
            let rec = crate::records::new_record(
                &s,
                "memory",
                "brain",
                "Keep this body",
                crate::records::NewOptions {
                    author: "agent".into(),
                    body: "Original body\n".into(),
                    body_set: true,
                    ..crate::records::NewOptions::default()
                },
            )
            .unwrap();
            let body_file = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(body_file.path(), body).unwrap();
            let out = pulse_cli::Buffer::new();
            let err = pulse_cli::Buffer::new();
            let mut c = Context {
                store: Some(s.clone()),
                home: s.home.clone(),
                out: Box::new(out.clone()),
                err: Box::new(err),
                json: false,
                quiet: false,
                color: false,
                agent: "agent".into(),
                project: "brain".into(),
                session: String::new(),
                now: "2026-09-06T21:00:00Z".into(),
                args: vec![],
            };
            let err = run_record(
                &mut c,
                &FlagSet::new(""),
                &[
                    "amend".into(),
                    rec.id.clone(),
                    "--body-file".into(),
                    body_file.path().display().to_string(),
                    "--note".into(),
                    "Replace body".into(),
                ],
            )
            .err();
            let want_rev = if blank { 1 } else { 2 };
            if blank {
                let err = err.unwrap_or_else(|| panic!("{name}: blank body accepted"));
                let ce = err.downcast_ref::<pulse_cli::Error>().unwrap();
                assert_eq!(ce.what, "Empty record body");
                assert_eq!(ce.why, "the body file has no content");
                assert_eq!(
                    ce.fix,
                    "pulse record amend ID --body-file F with a non-empty file, or stdin"
                );
                assert_eq!(out.string(), "");
            } else {
                assert!(err.is_none(), "{name}: {err:?}");
            }
            let (head, rev) = crate::records::get(&s, &rec.id, 0).unwrap();
            assert_eq!(head.current_rev, want_rev);
            assert_eq!(rev.rev, want_rev);
            assert_eq!(
                rev.body,
                if blank { "Original body\n" } else { body },
                "{name}"
            );
            assert_eq!(s.revisions(&rec.id).unwrap().len() as i64, want_rev);
        }
    }
}
