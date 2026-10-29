//! The agent file a job's worker runs as. Of the project's agent specs -
//! its store `agent` records, the owner's `~/.pulse/agents`, the
//! checkout's `.pulse/agents`, `.claude/agents` and `.github/agents`
//! ([`pulse_harness::agents`]) - the one whose `pulse.kinds` names the
//! job's kind and whose `pulse.select` fits the card selects the job. The
//! worker then runs `pulse harness run --agent NAME` with the spec's model
//! (when `models.allowed` lets it), its step cap and its read-only flag;
//! the harness in the clone reads the same spec for the standing
//! instructions and the tool rules.

use crate::util::Row;
use pulse_harness::agents::{AgentDef, load_agent_specs};
use pulse_harness::permissions::glob;

/// What a card says about itself: the words of its `Tags:` lines, or of
/// its `Class:` line when it has no tags, and the paths of its `File:`
/// lines.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CardWords {
    pub(crate) tags: Vec<String>,
    pub(crate) files: Vec<String>,
}

/// Reads the `Tags:`, `Class:` and `File:` lines of a card's text (the
/// job's brief) the way the tally reads its fields: a leading list marker
/// and backticks do not count, keys match whatever their case.
pub(crate) fn card_words(text: &str) -> CardWords {
    let mut tags: Vec<String> = Vec::new();
    let mut class: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches(['-', '*', ' ']);
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim_matches(['*', ' ', '\t']).to_ascii_lowercase();
        let words = value
            .split([',', ' ', '\t'])
            .map(|w| w.trim_matches(['`', '*', '[', ']', '#']))
            .filter(|w| !w.is_empty());
        match key.as_str() {
            "tags" | "tag" => tags.extend(words.map(str::to_ascii_lowercase)),
            "class" => class.extend(words.map(str::to_ascii_lowercase)),
            "file" | "files" | "path" | "paths" => {
                files.extend(words.map(|w| w.trim_start_matches("./").to_string()));
            }
            _ => {}
        }
    }
    let mut out = CardWords {
        tags: if tags.is_empty() { class } else { tags },
        files,
    };
    out.tags.sort_unstable();
    out.tags.dedup();
    out.files.dedup();
    out
}

/// The spec that selects this job, if any: its `pulse.kinds` names the
/// job's kind, its `pulse.select.tags` are empty or share a word with the
/// card's, its `pulse.select.files` are empty or a glob of them matches a
/// `File:` path of the card. The most specific wins (more tags and files
/// matched), then the first by name. `checkout` is the directory whose
/// agent files count: the project's checkout, which the clone is made from.
pub(crate) fn spec_for_job(
    s: &pulse_store::Store,
    job: &Row,
    project_slug: &str,
    checkout: &str,
) -> Option<AgentDef> {
    let kind = job.str("kind").trim().to_ascii_lowercase();
    if kind.is_empty() {
        return None;
    }
    let card = card_words(&job.str("brief"));
    let specs = load_agent_specs(checkout, &s.home, Some(s), project_slug);
    pick(specs, &kind, &card)
}

pub(crate) fn pick(specs: Vec<AgentDef>, kind: &str, card: &CardWords) -> Option<AgentDef> {
    let mut best: Option<(usize, AgentDef)> = None;
    for a in specs.into_iter().filter(AgentDef::loads) {
        if !a.kinds.iter().any(|k| k == kind) {
            continue;
        }
        let tags = a
            .select_tags
            .iter()
            .filter(|t| card.tags.contains(t))
            .count();
        if !a.select_tags.is_empty() && tags == 0 {
            continue;
        }
        let files = a
            .select_files
            .iter()
            .filter(|g| card.files.iter().any(|f| glob(g, f)))
            .count();
        if !a.select_files.is_empty() && files == 0 {
            continue;
        }
        let score = tags + files;
        let better = match &best {
            None => true,
            Some((have, b)) => score > *have || (score == *have && a.name < b.name),
        };
        if better {
            best = Some((score, a));
        }
    }
    best.map(|(_, a)| a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_harness::agents::AgentLayer;

    fn spec(name: &str, kinds: &[&str], tags: &[&str], files: &[&str]) -> AgentDef {
        AgentDef {
            name: name.into(),
            kinds: kinds.iter().map(|s| (*s).to_string()).collect(),
            select_tags: tags.iter().map(|s| (*s).to_string()).collect(),
            select_files: files.iter().map(|s| (*s).to_string()).collect(),
            layer: AgentLayer::Repo,
            ..Default::default()
        }
    }

    #[test]
    fn card_words_read_tags_then_class_and_the_file_paths() {
        let w = card_words(
            "Some title\n- **Class:** `refactor`\nFile: `src/lib.rs`, crates/a/b.rs\nHypothesis: ok\n",
        );
        assert_eq!(w.tags, ["refactor"]);
        assert_eq!(w.files, ["src/lib.rs", "crates/a/b.rs"]);
        let w = card_words("Class: product\nTags: Rust, perf perf\nFiles: ./x.rs\n");
        assert_eq!(w.tags, ["perf", "rust"], "tags win over the class");
        assert_eq!(w.files, ["x.rs"]);
        assert_eq!(card_words("nothing here"), CardWords::default());
    }

    #[test]
    fn the_most_specific_spec_of_the_kind_wins_then_the_first_by_name() {
        let card = card_words("Class: refactor\nFile: crates/pulse-runner/src/run.rs\n");
        let specs = vec![
            spec("zed", &["task"], &[], &[]),
            spec("rust-refactorer", &["task"], &["refactor"], &[]),
            spec(
                "runner-hand",
                &["task"],
                &["refactor"],
                &["crates/pulse-runner/*"],
            ),
            spec("docs", &["task"], &["docs"], &[]),
            spec("answerer", &["answer"], &[], &[]),
            spec("web", &["task"], &[], &["web/*"]),
        ];
        assert_eq!(
            pick(specs.clone(), "task", &card).unwrap().name,
            "runner-hand"
        );
        let plain = card_words("Class: feature\n");
        assert_eq!(pick(specs.clone(), "task", &plain).unwrap().name, "zed");
        assert_eq!(
            pick(specs.clone(), "answer", &plain).unwrap().name,
            "answerer"
        );
        assert!(pick(specs.clone(), "ballot", &plain).is_none());
        let mut tie = vec![
            spec("b-agent", &["task"], &["refactor"], &[]),
            spec("a-agent", &["task"], &["refactor"], &[]),
        ];
        assert_eq!(pick(tie.clone(), "task", &card).unwrap().name, "a-agent");
        tie[1].rejected = Some("bypass".into());
        assert_eq!(pick(tie, "task", &card).unwrap().name, "b-agent");
    }
}
