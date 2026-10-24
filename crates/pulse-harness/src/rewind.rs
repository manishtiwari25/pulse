//! Rewind: what a session needs to put the checkout and the conversation
//! back to before one of its turns.
//!
//! Each turn of `pulse harness chat` records the files its `edit` and
//! `write` calls touch, as they were before the first change of the turn
//! (or that they were absent). `/rewind N` writes those pre-images back,
//! latest turn first so the earliest state wins, and drops the turn's
//! messages from the conversation. What a shell command changed is not
//! recorded - git is the rewind for that.
//!
//! The records live in memory and, when the session has a directory for
//! them (`~/.pulse/harness/<session>.rewind/`), on disk as an `index.json`
//! and one blob per pre-image, so a resumed session can still rewind the
//! turns of the one it continues.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One turn of a session as rewind sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnRecord {
    /// The user's message that started the turn.
    pub text: String,
    /// Index of that message in the conversation; None once compaction
    /// folded it into a summary (the files can still go back).
    pub user_index: Option<usize>,
    /// Relative path to the bytes before the turn; None when the file
    /// was absent.
    pub files: BTreeMap<String, Option<Vec<u8>>>,
}

/// What `/rewind N` puts back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Both,
    Files,
    Conversation,
}

impl What {
    pub fn parse(s: &str) -> Option<What> {
        match s.trim() {
            "" | "both" | "all" => Some(What::Both),
            "files" | "file" | "code" => Some(What::Files),
            "conversation" | "chat" | "messages" => Some(What::Conversation),
            _ => None,
        }
    }

    pub fn files(self) -> bool {
        matches!(self, What::Both | What::Files)
    }

    pub fn conversation(self) -> bool {
        matches!(self, What::Both | What::Conversation)
    }
}

/// What a rewind did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Files written back to their earlier content.
    pub restored: Vec<String>,
    /// Files removed because the turns created them.
    pub removed: Vec<String>,
    /// Files that could not be put back, with the reason.
    pub failed: Vec<String>,
    /// Messages dropped from the conversation (0 for a files-only rewind).
    pub dropped_messages: usize,
    /// Turns forgotten.
    pub turns: usize,
    /// The conversation index to truncate to, when the conversation goes
    /// back; the agent applies it.
    pub truncate_to: Option<usize>,
}

impl Report {
    /// One line for the user.
    pub fn describe(&self, to_turn: usize, what: What) -> String {
        let mut parts = Vec::new();
        if what.files() {
            let n = self.restored.len() + self.removed.len();
            if n == 0 {
                parts.push("no files to put back".to_string());
            } else {
                let mut names: Vec<String> = self.restored.clone();
                names.extend(self.removed.iter().map(|p| format!("{p} (removed)")));
                parts.push(format!(
                    "put {n} file{} back ({})",
                    if n == 1 { "" } else { "s" },
                    names.join(", ")
                ));
            }
            if !self.failed.is_empty() {
                parts.push(format!("could not put back: {}", self.failed.join("; ")));
            }
        }
        if what.conversation() {
            parts.push(format!(
                "dropped {} message{} of {} turn{} from the conversation",
                self.dropped_messages,
                if self.dropped_messages == 1 { "" } else { "s" },
                self.turns,
                if self.turns == 1 { "" } else { "s" }
            ));
        }
        format!("rewound to before turn {to_turn}: {}", parts.join("; "))
    }
}

#[derive(Serialize, Deserialize)]
struct Index {
    turns: Vec<IndexTurn>,
}

#[derive(Serialize, Deserialize)]
struct IndexTurn {
    text: String,
    files: Vec<IndexFile>,
}

#[derive(Serialize, Deserialize)]
struct IndexFile {
    path: String,
    /// Blob relative to the rewind directory; None for an absent file.
    blob: Option<String>,
}

/// The turn records of one session and where they are kept.
#[derive(Debug, Default)]
pub struct Rewind {
    cwd: PathBuf,
    dir: Option<PathBuf>,
    turns: Vec<TurnRecord>,
    /// Whether a turn is open (between `begin_turn` and `end_turn`).
    open: bool,
}

impl Rewind {
    pub fn new(cwd: &str) -> Rewind {
        Rewind {
            cwd: PathBuf::from(cwd),
            ..Rewind::default()
        }
    }

    /// Where this session saves its records; created on the first save.
    pub fn set_dir(&mut self, dir: PathBuf) {
        self.dir = Some(dir);
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn turns(&self) -> &[TurnRecord] {
        &self.turns
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Adds the records saved under `dir` by an earlier session, before
    /// its own turns; their conversation indices come from `match_messages`.
    pub fn load(&mut self, dir: &Path) -> std::io::Result<usize> {
        let index: Index = match fs::read(dir.join("index.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let mut loaded = Vec::with_capacity(index.turns.len());
        for t in index.turns {
            let mut files = BTreeMap::new();
            for f in t.files {
                let bytes = match f.blob {
                    Some(rel) => Some(fs::read(dir.join(rel))?),
                    None => None,
                };
                files.insert(f.path, bytes);
            }
            loaded.push(TurnRecord {
                text: t.text,
                user_index: None,
                files,
            });
        }
        let n = loaded.len();
        loaded.append(&mut self.turns);
        self.turns = loaded;
        Ok(n)
    }

    /// Gives loaded records their place in the conversation: `starts` are
    /// the (index, text) of the user messages that begin a turn, in order.
    /// A record whose text is not among them keeps None.
    pub fn match_messages(&mut self, starts: &[(usize, String)]) {
        let mut next = 0;
        for t in &mut self.turns {
            if t.user_index.is_some() {
                continue;
            }
            if let Some(pos) = starts[next.min(starts.len())..]
                .iter()
                .position(|(_, text)| *text == t.text)
            {
                t.user_index = Some(starts[next + pos].0);
                next += pos + 1;
            }
        }
    }

    /// Opens the record of a turn; `user_index` is where its message sits.
    pub fn begin_turn(&mut self, text: &str, user_index: usize) {
        self.turns.push(TurnRecord {
            text: text.to_string(),
            user_index: Some(user_index),
            files: BTreeMap::new(),
        });
        self.open = true;
    }

    /// Records `rel` as it is now, the first time a turn touches it.
    pub fn capture(&mut self, rel: &str) {
        if !self.open {
            return;
        }
        let Some(turn) = self.turns.last_mut() else {
            return;
        };
        if turn.files.contains_key(rel) {
            return;
        }
        let path = self.cwd.join(rel);
        if path.is_dir() {
            return;
        }
        let before = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return,
        };
        turn.files.insert(rel.to_string(), before);
    }

    /// Closes the turn: files that ended as they began are forgotten and
    /// the records are saved.
    pub fn end_turn(&mut self) -> std::io::Result<()> {
        self.open = false;
        if let Some(turn) = self.turns.last_mut() {
            let cwd = self.cwd.clone();
            turn.files.retain(|rel, before| {
                let now = fs::read(cwd.join(rel)).ok();
                now != *before
            });
        }
        self.save()
    }

    /// Compaction replaced `msgs[2..cut]` by one summary: indices move.
    pub fn on_compaction(&mut self, cut: usize) {
        for t in &mut self.turns {
            t.user_index = match t.user_index {
                Some(i) if i >= cut => Some(i - cut + 3),
                Some(i) if i < 2 => Some(i),
                _ => None,
            };
        }
    }

    /// The turns from `to` (0-based) on, latest first, as the user sees
    /// them: number, first line, files.
    pub fn describe(&self) -> Vec<String> {
        self.turns
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let files = if t.files.is_empty() {
                    "no file changes".to_string()
                } else {
                    format!(
                        "{} file{}: {}",
                        t.files.len(),
                        if t.files.len() == 1 { "" } else { "s" },
                        t.files.keys().cloned().collect::<Vec<_>>().join(", ")
                    )
                };
                let folded = if t.user_index.is_none() {
                    " (conversation folded by compaction; files only)"
                } else {
                    ""
                };
                format!("{}. {} - {files}{folded}", i + 1, first_line(&t.text, 60))
            })
            .collect()
    }

    /// Puts the checkout and/or the conversation back to before turn `to`
    /// (0-based) and forgets the turns from it on. `messages` is the
    /// conversation's length, for the count of dropped messages.
    pub fn rewind(
        &mut self,
        to: usize,
        what: What,
        messages: usize,
    ) -> std::result::Result<Report, String> {
        if to >= self.turns.len() {
            return Err(format!(
                "there {} {} turn{} to rewind; /rewind lists them",
                if self.turns.len() == 1 { "is" } else { "are" },
                self.turns.len(),
                if self.turns.len() == 1 { "" } else { "s" }
            ));
        }
        let mut report = Report::default();
        if what.conversation() {
            let Some(index) = self.turns[to].user_index else {
                let later = self.turns[to..]
                    .iter()
                    .position(|t| t.user_index.is_some())
                    .map(|p| p + to + 1);
                return Err(match later {
                    Some(n) => format!(
                        "turn {} was folded into a compaction summary, so the conversation cannot go back to it; `/rewind {} files` puts the files back, or rewind to turn {n} or later",
                        to + 1,
                        to + 1
                    ),
                    None => format!(
                        "turn {} was folded into a compaction summary, so the conversation cannot go back to it; `/rewind {} files` puts the files back",
                        to + 1,
                        to + 1
                    ),
                });
            };
            report.truncate_to = Some(index);
            report.dropped_messages = messages.saturating_sub(index);
        }
        if what.files() {
            // Latest turn first, so the earliest pre-image of a path is
            // the one that stays.
            let mut outcomes: Vec<(String, Outcome)> = Vec::new();
            for t in self.turns[to..].iter().rev() {
                for (rel, before) in &t.files {
                    let path = self.cwd.join(rel);
                    let outcome = match before {
                        Some(bytes) => {
                            match create_parent(&path).and_then(|()| fs::write(&path, bytes)) {
                                Ok(()) => Outcome::Restored,
                                Err(e) => Outcome::Failed(e.to_string()),
                            }
                        }
                        None => match fs::remove_file(&path) {
                            Ok(()) => {
                                remove_empty_parents(&path, &self.cwd);
                                Outcome::Removed
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Outcome::Removed,
                            Err(e) => Outcome::Failed(e.to_string()),
                        },
                    };
                    // The last write of a path (its earliest turn) decides.
                    outcomes.retain(|(p, _)| p != rel);
                    outcomes.push((rel.clone(), outcome));
                }
            }
            outcomes.sort_by(|a, b| a.0.cmp(&b.0));
            for (rel, outcome) in outcomes {
                match outcome {
                    Outcome::Restored => report.restored.push(rel),
                    Outcome::Removed => report.removed.push(rel),
                    Outcome::Failed(e) => report.failed.push(format!("{rel}: {e}")),
                }
            }
        }
        report.turns = self.turns.len() - to;
        if what.conversation() {
            self.turns.truncate(to);
        } else {
            // The files are back but the conversation goes on: the turns
            // stay, with nothing left to put back.
            for t in &mut self.turns[to..] {
                t.files.clear();
            }
        }
        self.open = false;
        if let Err(e) = self.save() {
            report
                .failed
                .push(format!("saving the rewind records: {e}"));
        }
        Ok(report)
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        if self.turns.is_empty() && !dir.exists() {
            return Ok(());
        }
        let fresh = dir.with_extension("rewind.tmp");
        if fresh.exists() {
            fs::remove_dir_all(&fresh)?;
        }
        fs::create_dir_all(&fresh)?;
        let mut index = Index { turns: Vec::new() };
        for (n, t) in self.turns.iter().enumerate() {
            let mut files = Vec::new();
            for (i, (rel, before)) in t.files.iter().enumerate() {
                let blob = match before {
                    Some(bytes) => {
                        let name = format!("{}/{i}", n + 1);
                        let p = fresh.join(&name);
                        create_parent(&p)?;
                        fs::write(&p, bytes)?;
                        Some(name)
                    }
                    None => None,
                };
                files.push(IndexFile {
                    path: rel.clone(),
                    blob,
                });
            }
            index.turns.push(IndexTurn {
                text: t.text.clone(),
                files,
            });
        }
        fs::write(
            fresh.join("index.json"),
            serde_json::to_vec_pretty(&index).map_err(std::io::Error::other)?,
        )?;
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        fs::rename(&fresh, dir)
    }
}

enum Outcome {
    Restored,
    Removed,
    Failed(String),
}

fn create_parent(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() && !p.exists() => fs::create_dir_all(p),
        _ => Ok(()),
    }
}

/// Removes the directories a created file left behind, up to the checkout
/// root, while they are empty.
fn remove_empty_parents(path: &Path, root: &Path) {
    let mut cur = path.parent();
    while let Some(dir) = cur {
        if dir == root || !dir.starts_with(root) {
            break;
        }
        if fs::remove_dir(dir).is_err() {
            break;
        }
        cur = dir.parent();
    }
}

/// The user's words in a turn's message: a session puts shell output and
/// mode notes before them, so the text after the last such block counts.
/// A line of the context a chat prepends to a message rather than of
/// the message: blank, a `Note:`, an attached file's tag, a mode note.
fn is_context_line(l: &str) -> bool {
    l.is_empty()
        || l.starts_with("Note:")
        || l.starts_with('<')
        || (l.starts_with('[') && l.ends_with(']'))
}

fn first_line(text: &str, limit: usize) -> String {
    let after_blocks = text
        .rsplit_once("--- end of output ---")
        .map_or(text, |(_, rest)| rest);
    let line = after_blocks
        .lines()
        .map(str::trim)
        .find(|l| !is_context_line(l))
        .unwrap_or_else(|| after_blocks.trim());
    if line.chars().count() <= limit {
        line.to_string()
    } else {
        let cut: String = line.chars().take(limit - 1).collect();
        format!("{}…", cut.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(p: &Path) -> Option<String> {
        fs::read_to_string(p).ok()
    }

    #[test]
    fn turns_capture_pre_images_once_and_forget_untouched_files() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        fs::write(cwd.join("a.txt"), "one\n").unwrap();
        fs::write(cwd.join("same.txt"), "same\n").unwrap();
        let mut r = Rewind::new(cwd.to_str().unwrap());
        assert!(r.is_empty());
        r.capture("a.txt");
        assert!(r.turns().is_empty(), "nothing is recorded outside a turn");

        r.begin_turn("change a and make b", 1);
        r.capture("a.txt");
        fs::write(cwd.join("a.txt"), "two\n").unwrap();
        r.capture("a.txt");
        fs::write(cwd.join("a.txt"), "three\n").unwrap();
        r.capture("sub/b.txt");
        fs::create_dir_all(cwd.join("sub")).unwrap();
        fs::write(cwd.join("sub/b.txt"), "b\n").unwrap();
        r.capture("same.txt");
        fs::write(cwd.join("same.txt"), "same\n").unwrap();
        r.capture("adir");
        r.end_turn().unwrap();
        let t = &r.turns()[0];
        assert_eq!(t.text, "change a and make b");
        assert_eq!(t.user_index, Some(1));
        assert_eq!(
            t.files,
            BTreeMap::from([
                ("a.txt".to_string(), Some(b"one\n".to_vec())),
                ("sub/b.txt".to_string(), None),
            ]),
            "the first capture of a path is the one kept; an unchanged file is dropped"
        );
        assert_eq!(
            r.describe(),
            ["1. change a and make b - 2 files: a.txt, sub/b.txt"]
        );

        r.begin_turn("change a again", 7);
        r.capture("a.txt");
        fs::write(cwd.join("a.txt"), "four\n").unwrap();
        r.end_turn().unwrap();
        assert_eq!(r.turns()[1].files["a.txt"], Some(b"three\n".to_vec()));

        // Rewinding to the second turn puts a back to "three"; to the first,
        // to "one" and removes b with its directory.
        let report = r.rewind(1, What::Both, 12).unwrap();
        assert_eq!(
            report,
            Report {
                restored: vec!["a.txt".to_string()],
                dropped_messages: 5,
                turns: 1,
                truncate_to: Some(7),
                ..Report::default()
            }
        );
        assert_eq!(
            report.describe(2, What::Both),
            "rewound to before turn 2: put 1 file back (a.txt); dropped 5 messages of 1 turn from the conversation"
        );
        assert_eq!(read(&cwd.join("a.txt")).unwrap(), "three\n");
        assert_eq!(r.turns().len(), 1);

        let report = r.rewind(0, What::Both, 7).unwrap();
        assert_eq!(report.restored, ["a.txt"]);
        assert_eq!(report.removed, ["sub/b.txt"]);
        assert_eq!(report.truncate_to, Some(1));
        assert_eq!(report.dropped_messages, 6);
        assert_eq!(read(&cwd.join("a.txt")).unwrap(), "one\n");
        assert!(!cwd.join("sub").exists(), "the empty directory goes too");
        assert!(cwd.join("same.txt").exists());
        assert!(r.is_empty());
        assert_eq!(
            r.rewind(0, What::Both, 1).unwrap_err(),
            "there are 0 turns to rewind; /rewind lists them"
        );
    }

    #[test]
    fn files_only_keeps_the_turns_and_conversation_only_keeps_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        fs::write(cwd.join("a.txt"), "one\n").unwrap();
        let mut r = Rewind::new(cwd.to_str().unwrap());
        r.begin_turn("edit a", 1);
        r.capture("a.txt");
        fs::write(cwd.join("a.txt"), "two\n").unwrap();
        r.end_turn().unwrap();

        let report = r.rewind(0, What::Files, 5).unwrap();
        assert_eq!(report.restored, ["a.txt"]);
        assert_eq!(report.truncate_to, None);
        assert_eq!(report.dropped_messages, 0);
        assert_eq!(
            report.describe(1, What::Files),
            "rewound to before turn 1: put 1 file back (a.txt)"
        );
        assert_eq!(read(&cwd.join("a.txt")).unwrap(), "one\n");
        assert_eq!(r.turns().len(), 1, "the turn stays in the list");
        assert!(
            r.turns()[0].files.is_empty(),
            "with nothing left to put back"
        );
        assert_eq!(r.describe(), ["1. edit a - no file changes"]);

        fs::write(cwd.join("a.txt"), "three\n").unwrap();
        let report = r.rewind(0, What::Conversation, 5).unwrap();
        assert_eq!(report.truncate_to, Some(1));
        assert_eq!(report.dropped_messages, 4);
        assert_eq!(
            report.describe(1, What::Conversation),
            "rewound to before turn 1: dropped 4 messages of 1 turn from the conversation"
        );
        assert_eq!(
            read(&cwd.join("a.txt")).unwrap(),
            "three\n",
            "files untouched"
        );
        assert!(r.is_empty());
    }

    #[test]
    fn compaction_moves_indices_and_folded_turns_rewind_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        let mut r = Rewind::new(cwd.to_str().unwrap());
        r.begin_turn("first", 1);
        r.end_turn().unwrap();
        r.begin_turn("second", 6);
        r.capture("n.txt");
        fs::write(cwd.join("n.txt"), "new\n").unwrap();
        r.end_turn().unwrap();
        r.begin_turn("third", 10);
        r.end_turn().unwrap();
        // msgs[2..8] became one summary at index 2: 8 -> 3, 10 -> 5.
        r.on_compaction(8);
        assert_eq!(
            r.turns().iter().map(|t| t.user_index).collect::<Vec<_>>(),
            [Some(1), None, Some(5)]
        );
        assert_eq!(
            r.describe()[1],
            "2. second - 1 file: n.txt (conversation folded by compaction; files only)"
        );
        let err = r.rewind(1, What::Both, 9).unwrap_err();
        assert_eq!(
            err,
            "turn 2 was folded into a compaction summary, so the conversation cannot go back to it; `/rewind 2 files` puts the files back, or rewind to turn 3 or later"
        );
        let report = r.rewind(1, What::Files, 9).unwrap();
        assert_eq!(report.removed, ["n.txt"]);
        assert!(!cwd.join("n.txt").exists());
        let report = r.rewind(2, What::Both, 9).unwrap();
        assert_eq!(report.truncate_to, Some(5));
        assert_eq!(report.dropped_messages, 4);
        let report = r.rewind(0, What::Both, 5).unwrap();
        assert_eq!(report.truncate_to, Some(1));
        assert_eq!(report.turns, 2);
    }

    #[test]
    fn records_save_to_the_directory_and_load_into_a_later_session() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("repo");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("a.txt"), "one\n").unwrap();
        let store = dir.path().join("home/harness/hs-1.rewind");
        let mut r = Rewind::new(cwd.to_str().unwrap());
        r.set_dir(store.clone());
        assert!(!store.exists(), "nothing is written before a turn");
        r.begin_turn("edit a and add b", 1);
        r.capture("a.txt");
        fs::write(cwd.join("a.txt"), "two\n").unwrap();
        r.capture("b.bin");
        fs::write(cwd.join("b.bin"), [0u8, 159, 146, 150]).unwrap();
        r.end_turn().unwrap();
        r.begin_turn("nothing", 5);
        r.end_turn().unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(store.join("index.json")).unwrap()).unwrap();
        assert_eq!(index["turns"].as_array().unwrap().len(), 2);
        assert_eq!(index["turns"][0]["files"][0]["path"], "a.txt");
        assert_eq!(index["turns"][0]["files"][0]["blob"], "1/0");
        assert_eq!(
            index["turns"][0]["files"][1]["blob"],
            serde_json::Value::Null
        );
        assert_eq!(fs::read_to_string(store.join("1/0")).unwrap(), "one\n");
        assert!(!dir.path().join("home/harness/hs-1.rewind.tmp").exists());

        // A later session loads them before its own turns and finds their
        // messages in the conversation it carried over.
        let mut later = Rewind::new(cwd.to_str().unwrap());
        later.set_dir(dir.path().join("home/harness/hs-2.rewind"));
        assert_eq!(later.load(&store).unwrap(), 2);
        assert_eq!(
            later
                .load(&dir.path().join("home/harness/none.rewind"))
                .unwrap(),
            0
        );
        assert_eq!(later.turns().len(), 2);
        assert!(later.turns().iter().all(|t| t.user_index.is_none()));
        later.match_messages(&[
            (1, "edit a and add b".to_string()),
            (5, "nothing".to_string()),
            (9, "Note: resumed".to_string()),
        ]);
        assert_eq!(
            later
                .turns()
                .iter()
                .map(|t| t.user_index)
                .collect::<Vec<_>>(),
            [Some(1), Some(5)]
        );
        later.begin_turn("more", 10);
        later.end_turn().unwrap();
        let report = later.rewind(0, What::Both, 14).unwrap();
        assert_eq!(report.restored, ["a.txt"]);
        assert_eq!(report.removed, ["b.bin"]);
        assert_eq!(report.turns, 3);
        assert_eq!(fs::read_to_string(cwd.join("a.txt")).unwrap(), "one\n");
        assert!(!cwd.join("b.bin").exists());
        assert!(later.is_empty());
        let index: serde_json::Value = serde_json::from_slice(
            &fs::read(dir.path().join("home/harness/hs-2.rewind/index.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(index["turns"].as_array().unwrap().len(), 0);

        // Loaded records whose message is gone stay files-only.
        let mut other = Rewind::new(cwd.to_str().unwrap());
        other.load(&store).unwrap();
        other.match_messages(&[(3, "nothing".to_string())]);
        assert_eq!(
            other
                .turns()
                .iter()
                .map(|t| t.user_index)
                .collect::<Vec<_>>(),
            [None, Some(3)]
        );
    }

    #[test]
    fn what_parses_and_first_lines_skip_context_blocks() {
        assert_eq!(What::parse(""), Some(What::Both));
        assert_eq!(What::parse("files"), Some(What::Files));
        assert_eq!(What::parse(" conversation "), Some(What::Conversation));
        assert_eq!(What::parse("chat"), Some(What::Conversation));
        assert_eq!(What::parse("dance"), None);
        assert_eq!(
            first_line("<returncode>0</returncode>\nhello there", 60),
            "hello there"
        );
        assert_eq!(
            first_line("Note: plan mode\n\nfix the test\nplease", 60),
            "fix the test"
        );
        assert_eq!(
            first_line(
                "[Plan mode is off: carry on.]\n\n--- the user ran `ls` in the shell; its output ---\na.txt\nb.txt\n--- end of output ---\n\nnow fix b",
                60
            ),
            "now fix b"
        );
        assert_eq!(first_line("a".repeat(70).as_str(), 10), "aaaaaaaaa…");
        assert_eq!(first_line("   ", 10), "");
    }
}
