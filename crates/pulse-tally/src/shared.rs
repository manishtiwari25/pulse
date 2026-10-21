//! The shared board (the commons) as every other project's digest sees it:
//! the pitfalls, patterns and wishes about PULSE that workers on any project
//! left for the rest of the machine, with the project each came from.

use anyhow::Result;
use chrono::DateTime;

use crate::tally::compute;
use crate::text::{plural, trunc};
use crate::thresholds::Thresholds;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedThread {
    pub key: String,
    /// The project the thread's first post came from (`origin_project`);
    /// empty when the poster did not say.
    pub origin: String,
    /// Who opened the thread.
    pub agent: String,
    pub kind: String,
    pub posts: i32,
    pub last: String,
    pub last_summary: String,
    pub first_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedBoard {
    /// Threads nobody closed with a result, newest activity first.
    pub unresolved: Vec<SharedThread>,
    pub open_questions: usize,
    pub posts: usize,
    /// RFC 3339 date of the newest post, empty when the board has none.
    pub last_post: String,
}

impl SharedBoard {
    /// Whole days since the newest post, `None` when the board is empty.
    pub fn quiet_days(&self, now: &str) -> Option<i64> {
        let a = DateTime::parse_from_rfc3339(&self.last_post).ok()?;
        let b = DateTime::parse_from_rfc3339(now).ok()?;
        Some(((b - a).num_hours() / 24).max(0))
    }

    /// Wishes and lessons - every unresolved thread that is not a question.
    pub fn lessons(&self) -> usize {
        self.unresolved
            .iter()
            .filter(|t| !t.key.starts_with("questions/"))
            .count()
    }

    /// The one-line NOTICES entry: how much waits there and how long it has
    /// been quiet.
    pub fn notice(&self, now: &str) -> String {
        let quiet = match self.quiet_days(now) {
            None => "no post yet".to_string(),
            Some(0) => "last post today".to_string(),
            Some(d) => format!("last post {d}d ago"),
        };
        format!(
            "shared board: {} open {}, {} unresolved pitfall/pattern/wish {}, {} - pulse board --project {}",
            self.open_questions,
            plural(self.open_questions, "question"),
            self.lessons(),
            plural(self.lessons(), "thread"),
            quiet,
            pulse_store::COMMONS_SLUG
        )
    }
}

/// Read the commons once and fold it into what a digest needs.
pub fn shared_board(s: &pulse_store::Store, th: Thresholds) -> Result<SharedBoard> {
    let cres = compute(s, pulse_store::COMMONS_SLUG, "", th)?;
    let mut out = SharedBoard {
        posts: cres.posts.len(),
        ..SharedBoard::default()
    };
    for p in &cres.posts {
        if p.date > out.last_post {
            out.last_post = p.date.clone();
        }
    }
    for t in &cres.threads {
        if t.status == "closed" {
            continue;
        }
        if t.key.starts_with("questions/") {
            out.open_questions += 1;
        }
        let first = cres
            .posts
            .iter()
            .filter(|p| format!("{}/{}", p.area, p.thread) == t.key)
            .min_by(|a, b| a.id.cmp(&b.id));
        let (origin, agent, kind, first_id) = first
            .map(|p| {
                (
                    p.origin_project.clone(),
                    p.agent.clone(),
                    p.kind.clone(),
                    p.id.clone(),
                )
            })
            .unwrap_or_default();
        out.unresolved.push(SharedThread {
            key: t.key.clone(),
            origin,
            agent,
            kind,
            posts: t.posts,
            last: t.last.clone(),
            last_summary: t.last_summary.clone(),
            first_id,
        });
    }
    out.unresolved
        .sort_by(|a, b| b.last.cmp(&a.last).then_with(|| a.key.cmp(&b.key)));
    Ok(out)
}

/// How many threads the digest's SHARED BOARD section lists.
pub const SHARED_BOARD_ROWS: usize = 8;

/// The SHARED BOARD section of a project's digest: the newest unresolved
/// threads with the project each came from, and what to do with them -
/// the brain promotes or declines wishes; any other project reads the
/// pitfalls before it hits them.
pub fn shared_board_text(sb: &SharedBoard, project: &str, brain: &str) -> String {
    let mut b = String::new();
    b.push_str(
        "SHARED BOARD (commons - pitfalls, patterns and wishes about PULSE from every project on this machine; pulse show ID)\n",
    );
    if sb.unresolved.is_empty() {
        if sb.posts == 0 {
            b.push_str("  empty - ");
        } else {
            b.push_str("  nothing unresolved - ");
        }
        b.push_str(
            "a lesson every project should know is a `FINDING: shared: AREA/THREAD - ...` line in a worker's report (or pulse post finding --project commons --origin-project ",
        );
        b.push_str(project);
        b.push_str(" ...); a wish about PULSE itself is a card with `Class: pulse`\n");
    }
    for t in sb.unresolved.iter().take(SHARED_BOARD_ROWS) {
        let origin = if !t.origin.is_empty() {
            format!("from {}", t.origin)
        } else if !t.agent.is_empty() {
            format!("by {}", t.agent)
        } else {
            String::new()
        };
        let _ = std::fmt::Write::write_fmt(
            &mut b,
            format_args!(
                "  {:<38} {:>2} {:<6} {:<22} last {:<20} {}\n",
                t.key,
                t.posts,
                if t.posts == 1 { "post" } else { "posts" },
                trunc(&origin, 22),
                t.last,
                trunc(&t.last_summary, 56)
            ),
        );
    }
    if sb.unresolved.len() > SHARED_BOARD_ROWS {
        let _ = std::fmt::Write::write_fmt(
            &mut b,
            format_args!(
                "  ... and {} more - pulse board --project {}\n",
                sb.unresolved.len() - SHARED_BOARD_ROWS,
                pulse_store::COMMONS_SLUG
            ),
        );
    }
    if !sb.unresolved.is_empty() {
        if !brain.is_empty() && project == brain {
            b.push_str(
                "  this project is the brain: a wish worth it becomes a card here (Signal: the commons post id), then close it - pulse post result --project commons --on ID --resolution promoted|declined\n",
            );
        } else {
            b.push_str(
                "  read the pitfalls before you hit them; one this project hit is a FINDING here, one it could enforce is a card\n",
            );
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(key: &str, origin: &str, last: &str) -> SharedThread {
        SharedThread {
            key: key.into(),
            origin: origin.into(),
            agent: "alice".into(),
            kind: "finding".into(),
            posts: 1,
            last: last.into(),
            last_summary: format!("summary of {key}"),
            first_id: format!("{last}-x"),
        }
    }

    #[test]
    fn notice_counts_questions_and_lessons_and_names_the_quiet_time() {
        let empty = SharedBoard::default();
        assert_eq!(
            empty.notice("2026-10-14T12:00:00Z"),
            "shared board: 0 open questions, 0 unresolved pitfall/pattern/wish threads, no post yet - pulse board --project commons"
        );
        let sb = SharedBoard {
            unresolved: vec![
                thread("questions/what-next", "a", "2026-10-03T10:00:00Z"),
                thread("pitfalls/gh", "b", "2026-10-01T10:00:00Z"),
                thread("patterns/disk", "c", "2026-09-01T10:00:00Z"),
            ],
            open_questions: 1,
            posts: 5,
            last_post: "2026-10-03T10:00:00Z".into(),
        };
        assert_eq!(sb.quiet_days("2026-10-14T12:00:00Z"), Some(11));
        assert_eq!(sb.lessons(), 2);
        assert_eq!(
            sb.notice("2026-10-14T12:00:00Z"),
            "shared board: 1 open question, 2 unresolved pitfall/pattern/wish threads, last post 11d ago - pulse board --project commons"
        );
        assert!(
            sb.notice("2026-10-03T18:00:00Z")
                .contains("last post today")
        );
    }

    #[test]
    fn text_lists_newest_first_with_origin_and_tells_the_brain_what_to_do() {
        let sb = SharedBoard {
            unresolved: (0..10)
                .rev()
                .map(|i| {
                    thread(
                        &format!("patterns/p{i}"),
                        "project-light",
                        &format!("2026-10-{:02}T10:00:00Z", i + 1),
                    )
                })
                .collect(),
            open_questions: 0,
            posts: 10,
            last_post: "2026-10-10T10:00:00Z".into(),
        };
        let text = shared_board_text(&sb, "proj", "brain");
        assert!(text.starts_with("SHARED BOARD (commons"), "{text}");
        assert!(
            text.contains("patterns/p9") && text.contains("from project-light"),
            "{text}"
        );
        assert_eq!(text.matches("\n  patterns/").count(), SHARED_BOARD_ROWS);
        assert!(
            text.contains("... and 2 more - pulse board --project commons"),
            "{text}"
        );
        assert!(
            text.contains("read the pitfalls before you hit them"),
            "{text}"
        );
        let brain = shared_board_text(&sb, "brain", "brain");
        assert!(
            brain.contains("this project is the brain")
                && brain.contains("--resolution promoted|declined"),
            "{brain}"
        );
        let empty = shared_board_text(&SharedBoard::default(), "proj", "brain");
        assert!(
            empty.contains("empty - a lesson every project should know is a `FINDING: shared: AREA/THREAD - ...` line")
                && empty.contains("--origin-project proj"),
            "{empty}"
        );
        assert!(!empty.contains("this project is the brain"));
    }
}
