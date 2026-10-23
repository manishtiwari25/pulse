use std::io::{self, Write};
use tabwriter::TabWriter;

#[derive(Debug, Clone, Copy, Default)]
pub struct Style {
    pub on: bool,
}

impl Style {
    fn wrap(&self, code: &str, text: &str) -> String {
        if !self.on || text.is_empty() {
            text.to_string()
        } else {
            format!("\x1b[{code}m{text}\x1b[0m")
        }
    }

    pub fn bold(&self, t: &str) -> String {
        self.wrap("1", t)
    }
    pub fn dim(&self, t: &str) -> String {
        self.wrap("2", t)
    }
    pub fn underline(&self, t: &str) -> String {
        self.wrap("4", t)
    }
    pub fn green(&self, t: &str) -> String {
        self.wrap("32", t)
    }
    pub fn yellow(&self, t: &str) -> String {
        self.wrap("33", t)
    }
    pub fn red(&self, t: &str) -> String {
        self.wrap("31", t)
    }
    pub fn cyan(&self, t: &str) -> String {
        self.wrap("36", t)
    }
}

pub fn table(
    w: &mut dyn Write,
    color: bool,
    header: Option<&[&str]>,
    rows: &[Vec<String>],
) -> io::Result<()> {
    let mut buf = Vec::new();
    {
        let mut tw = TabWriter::new(&mut buf).minwidth(0).padding(2);
        if let Some(header) = header {
            writeln!(tw, "{}", header.join("\t"))?;
        }
        for row in rows {
            let cells: Vec<_> = row.iter().map(|c| c.replace(['\t', '\n'], " ")).collect();
            writeln!(tw, "{}", cells.join("\t"))?;
        }
        tw.flush()?;
    }
    if buf.is_empty() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&buf);
    let trimmed = text.trim_end_matches('\n');
    if trimmed.is_empty() {
        return Ok(());
    }
    let style = Style { on: color };
    for (i, line) in trimmed.split('\n').enumerate() {
        let mut line = line.trim_end_matches(' ').to_string();
        if i == 0 && header.is_some() {
            line = style.bold(&style.underline(&line));
        }
        writeln!(w, "{line}")?;
    }
    Ok(())
}
