use std::io::{self, Write};

use crate::cli::{Command, GROUP_TITLES, GROUPS, all, names};
use crate::flags::FlagSet;
use crate::table::{Style, table};

pub const TITLE: &str = "pulse - the PULSE store, board, records and runner in one command";
pub const GLOBAL_USAGE: &str = "usage: pulse [--home DIR] [--project SLUG] [--agent NAME] [--session ID] [--json] [--quiet] [--no-color] [--now TIME] <command> [flags] [args]";

pub fn help(w: &mut dyn Write, color: bool) -> io::Result<()> {
    let style = Style { on: color };
    writeln!(w, "{}", style.bold(TITLE))?;
    writeln!(w)?;
    writeln!(w, "{GLOBAL_USAGE}")?;
    writeln!(
        w,
        "       pulse help [command]     flags and usage of one command"
    )?;
    let all = all();
    for g in GROUPS {
        let cmds: Vec<_> = all.iter().filter(|cmd| cmd.group == *g).collect();
        if cmds.is_empty() {
            continue;
        }
        writeln!(w)?;
        writeln!(w, "{}", style.cyan(GROUP_TITLES[*g]))?;
        let rows: Vec<_> = cmds
            .into_iter()
            .map(|cmd| {
                let mut name = cmd.name.clone();
                if !cmd.aliases.is_empty() {
                    name.push_str(" (");
                    name.push_str(&cmd.aliases.join(", "));
                    name.push(')');
                }
                vec![format!("  {name}"), cmd.summary.clone()]
            })
            .collect();
        table(w, false, None, &rows)?;
    }
    writeln!(w)?;
    writeln!(
        w,
        "Global flags default to $PULSE_HOME, $PULSE_PROJECT, $PULSE_AGENT, $PULSE_SESSION and $PULSE_NOW."
    )
}

pub fn help_command(w: &mut dyn Write, color: bool, cmd: &Command) -> io::Result<()> {
    let style = Style { on: color };
    writeln!(
        w,
        "{}  {}",
        style.bold(&format!("pulse {}", cmd.name)),
        cmd.summary
    )?;
    if !cmd.aliases.is_empty() {
        writeln!(w, "aliases: {}", cmd.aliases.join(", "))?;
    }
    writeln!(w)?;
    let usage = if cmd.usage.is_empty() {
        format!("pulse {}", cmd.name)
    } else {
        cmd.usage.clone()
    };
    writeln!(w, "Usage: {usage}")?;
    let Some(bind) = cmd.flags else {
        return Ok(());
    };
    let mut fs = FlagSet::new(&cmd.name);
    bind(&mut fs);
    let mut n = 0;
    fs.visit_all(|_| n += 1);
    if n == 0 {
        return Ok(());
    }
    writeln!(w)?;
    writeln!(w, "Flags:")?;
    fs.print_defaults(w)
}

pub fn suggest(name: &str, n: usize) -> Vec<String> {
    let mut cands: Vec<_> = names()
        .into_iter()
        .map(|other| {
            let d = edit_distance(name, &other);
            (d, other)
        })
        .collect();
    cands.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    cands.into_iter().take(n).map(|(_, name)| name).collect()
}

pub fn edit_distance(a: &str, b: &str) -> usize {
    let ra: Vec<char> = a.chars().collect();
    let rb: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=rb.len()).collect();
    let mut cur = vec![0; rb.len() + 1];
    for i in 1..=ra.len() {
        cur[0] = i;
        for j in 1..=rb.len() {
            let cost = usize::from(ra[i - 1] != rb[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[rb.len()]
}
