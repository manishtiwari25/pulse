//! Conversations: the agent behind the hub's chat window.
//!
//! A conversation is `pulse harness chat` without a terminal. The owner
//! types in the hub, the message goes to an [`Agent`] that runs on a worker
//! thread inside the hub process, the steps it takes land in the store as
//! the harness session's steps (the Runs page shows them too), the reply
//! streams back, and when the agent needs the owner - a question, a call
//! that changes things while approvals are on - the turn parks until the
//! answer arrives over HTTP. A conversation outlives its harness sessions:
//! a hub restart, a move into a project's worktree, or a session the
//! provider ended all start a new session with the messages carried over.
//!
//! Where it works: a conversation that names a project gets its own git
//! worktree of that project under `PULSE_HOME/work/chat/<id>/<slug>` on
//! branch `pulse/chat-<id>`, so two conversations never collide and the
//! owner's checkout stays theirs; `pulse land --branch` merges it through
//! the project's checks. A conversation without a project starts in a
//! scratch folder with a brief that lists the projects and tells the agent
//! how to pick one (`pulse chat attach SLUG`), queue tasks for the fleet, or
//! answer outright. Files for the owner's eyes go in `$PULSE_ARTIFACTS`,
//! which the hub serves beside the conversation.
use crate::commands::{PrepareOpts, Prepared, add_agent_flags, prepare_agent_with};
use crate::harness::{
    Agent, Approval, ApprovalRequest, ChatReply, ChatRequest, EXIT_ENDED, Sink, Step, StreamSink,
    Trajectory, Turn, Ui,
};
use anyhow::{Context as _, Result, anyhow, bail};
use pulse_cli::{Context, FlagSet};
use pulse_provider::{CancelToken, ImageContent};
use pulse_store::{Conversation as Row, Project, Store};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// Branches conversations commit on; `pulse land` accepts the `pulse/`
/// prefix.
pub const BRANCH_PREFIX: &str = "pulse/chat-";
/// The environment variable that names the conversation inside its agent's
/// commands (`pulse chat attach` reads it).
pub const CONVERSATION_ENV: &str = "PULSE_CONVERSATION";
/// The environment variable that names the artifacts folder.
pub const ARTIFACTS_ENV: &str = "PULSE_ARTIFACTS";
/// The memory key conversations without a project share
/// (`~/.pulse/memory/conversations.md`): one of them learning a thing is
/// enough for the next. A conversation attached to a project reads and
/// extends that project's memory instead, the fleet's file for it.
pub const SHARED_MEMORY_KEY: &str = "conversations";
/// How long a parked question or approval waits between checks of the
/// cancel flag.
const PARK_TICK: Duration = Duration::from_millis(250);
/// The row note a turn leaves when PULSE itself stopped it (a hub restart,
/// a shutdown, a crash) rather than the owner or the agent: the next turn
/// tells the agent so it checks the workspace before redoing work.
pub const RESTART_NOTE: &str = "PULSE restarted during the last turn";
/// What the transcript says where a restart cut a working turn.
const CUT_RUNNING_LINE: &str = "PULSE restarted while the agent was working on this, so the turn stopped here. What it did until then is above and the workspace keeps those changes; send a message to carry on - \"continue\" is enough.";
/// What the transcript says where a restart cut a turn parked on the owner.
const CUT_WAITING_LINE: &str = "PULSE restarted while the agent waited for you, so the turn stopped here. Send your answer as a message and it carries on.";
/// What the transcript says where a shutdown stopped the turn itself.
const STOPPED_TO_RESTART_LINE: &str = "PULSE stopped this turn to restart. What it did until then is above and the workspace keeps those changes; send a message to carry on - \"continue\" is enough.";
/// The note the hub leaves when it carries a turn it stopped to restart
/// on by itself ([`Manager::carry_on_restarted`]).
pub const CARRIED_ON_LINE: &str =
    "PULSE is back after the update and carries this turn on by itself.";
/// What the agent is told to pick the stopped turn back up; the restart
/// hint ([`restart_hint`]) with the steps it had taken comes first.
const CARRY_ON_PROMPT: &str = "PULSE is back after its update. Carry on with the turn it stopped: finish what the owner last asked for, picking up from the steps listed above without redoing what is already there. If all of it was already done, say so in a line.";
/// What the transcript says under a message the agent never saw because
/// PULSE went down first: the agent's memory does not hold it, so
/// "continue" would not do.
const UNSENT_LINE: &str =
    "PULSE stopped to restart before this message reached the agent. Send it again.";
/// The same where the owner stopped the turn the message waited behind.
const UNSENT_STOPPED_LINE: &str = "You stopped the turn before this message reached the agent. Send it again if you still want it.";
/// The same where the conversation was finished or archived first.
const UNSENT_CLOSED_LINE: &str =
    "The conversation was closed before this message reached the agent. Send it again to carry on.";
/// The same where the agent could not start.
const UNSENT_NO_AGENT_LINE: &str = "The agent could not start, so this message never reached it. Send it again once the cause above is fixed.";
/// The row note of a conversation the owner finished.
const CLOSED_BY_OWNER_NOTE: &str = "closed by you";
/// What the transcript says where the owner finished the conversation.
const CLOSED_BY_OWNER_LINE: &str = "You finished this conversation. Write again to reopen it.";
/// What the transcript says where the agent finished the conversation;
/// its summary follows.
const FINISHED_BY_AGENT_LINE: &str = "The agent finished this conversation:";
/// The message the conversation sends itself once when a transport
/// provider_error cut the last turn.
const RESUME_AFTER_PROVIDER_ERROR: &str =
    "Continue from where the turn stopped; check the worktree first.";

/// Whether a turn that stopped on `provider_error` resumes itself once:
/// the failure is a transport one (a failed send or a broken reply body,
/// the classes pulse-provider's own retries cannot outlast) and
/// chat.resume_after_provider_error_secs (default 90, 0 = off) gives the
/// pause before the retry. A second failure in a row goes idle as before.
fn resume_after_provider_error(store: &Store, why: &str, error: &str) -> bool {
    why == "provider_error"
        && (error.contains("error sending request")
            || error.contains("error decoding response body"))
        && store
            .setting("chat.resume_after_provider_error_secs", "90")
            .trim()
            .parse::<u64>()
            .unwrap_or(0)
            > 0
}
/// How many of the cut turn's steps the next turn's hint lists for the agent.
const HINT_STEPS: usize = 12;
/// The most text a diff endpoint returns.
const DIFF_LIMIT: usize = 400 * 1024;

pub fn new_conversation_id() -> String {
    let b: [u8; 3] = rand::random();
    format!(
        "chat-{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S"),
        hex::encode(b)
    )
}

fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// What the agent waits for from the owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Pending {
    Question {
        id: u64,
        question: String,
        choices: Vec<String>,
    },
    Approval {
        id: u64,
        tool: String,
        summary: String,
        detail: String,
        reason: String,
        always: String,
    },
}

impl Pending {
    fn id(&self) -> u64 {
        match self {
            Pending::Question { id, .. } | Pending::Approval { id, .. } => *id,
        }
    }
}

/// The conversation as the hub shows it between polls.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Live {
    /// `idle`, `running`, `waiting`, `ended`, `closed`.
    pub status: String,
    pub note: String,
    /// The reply streamed so far in the running turn.
    pub stream: String,
    pub pending: Option<Pending>,
    /// Messages typed while a turn ran, in order.
    pub queued: Vec<String>,
    /// The harness step number the running turn started after (-1 when the
    /// session has no step yet); the turn's steps are `turn_from < n`.
    pub turn_from: i64,
    pub turn_started_at: String,
    pub auto_approve: bool,
    /// Moves on every change; the hub skips a re-render when it stands.
    pub version: u64,
    /// The harness session the live steps belong to.
    pub session_id: String,
}

/// One file in the artifacts folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Artifact {
    pub name: String,
    pub size: u64,
    pub modified: String,
}

/// A file the owner handed the agent with a message. It lives in the
/// conversation's attachments folder, never in the branch; the agent gets
/// its absolute path with the message.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Attachment {
    pub name: String,
    pub size: u64,
    pub path: String,
}

/// The most bytes one attachment may hold.
pub const ATTACHMENT_LIMIT: usize = 8 * 1024 * 1024;
/// How many files one message may carry.
pub const ATTACHMENTS_PER_MESSAGE: usize = 12;
/// The most decoded bytes one inline image may hold.
pub const IMAGE_LIMIT: usize = 5 * 1024 * 1024;

/// An image the owner pasted into a message, base64-decoded and saved into
/// the conversation's attachments folder. The bytes are not handed to the
/// model yet (that is a later job); the hub serves them back over
/// `/api/v1/chat/{id}/image/...` so the thread can render them.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct ImageAttachment {
    pub mime: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub data: String,
}

impl ImageAttachment {
    /// The decoded bytes, or why this image is refused: a mime that is not
    /// png, jpeg or webp, data over [`IMAGE_LIMIT`], or bad base64.
    pub fn decode(&self) -> Result<Vec<u8>> {
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD;
        let mime = self.mime.trim().to_ascii_lowercase();
        if !matches!(mime.as_str(), "image/png" | "image/jpeg" | "image/webp") {
            bail!(
                "Invalid images: mime must be image/png, image/jpeg or image/webp, got {:?}",
                self.mime
            );
        }
        let bytes = STANDARD
            .decode(self.data.trim())
            .map_err(|e| anyhow!("Invalid images: {e}"))?;
        if bytes.len() > IMAGE_LIMIT {
            bail!(
                "Invalid images: one image holds up to {} bytes, this one holds {}",
                IMAGE_LIMIT,
                bytes.len()
            );
        }
        Ok(bytes)
    }

    fn ext(&self) -> &'static str {
        match self.mime.trim().to_ascii_lowercase().as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpeg",
            "image/webp" => "webp",
            _ => "bin",
        }
    }
}

/// What the conversation's branch holds against the project's default
/// branch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Changes {
    pub project: String,
    pub branch: String,
    pub base: String,
    pub cwd: String,
    /// Commits on the branch the base does not have.
    pub commits: i64,
    /// `git diff --stat base...HEAD`, last line first.
    pub stat: String,
    /// Files changed but not committed (`git status --porcelain` lines).
    pub dirty: Vec<String>,
}

/// What `attach` did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Attached {
    pub id: String,
    pub project: String,
    pub cwd: String,
    pub branch: String,
    pub base: String,
    /// Whether the worktree was made now (false: it already existed).
    pub created: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Idle,
    Running,
    Waiting,
    Ended,
    /// Finished: the agent said the request is done, or the owner closed
    /// it. A new message reopens it.
    Closed,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::Running => "running",
            Status::Waiting => "waiting",
            Status::Ended => "ended",
            Status::Closed => "closed",
        }
    }
}

/// Who finished the conversation, and their words for it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Finished {
    /// The agent's `finish` tool, with its summary.
    Agent(String),
    /// The owner, from the hub or `pulse chat close`.
    Owner,
}

impl Finished {
    /// The row note while the conversation stands finished.
    fn note(&self) -> String {
        match self {
            Finished::Agent(summary) => one_line(summary),
            Finished::Owner => CLOSED_BY_OWNER_NOTE.to_string(),
        }
    }

    /// The transcript line that marks the end.
    fn line(&self) -> String {
        match self {
            Finished::Agent(summary) => format!("{FINISHED_BY_AGENT_LINE} {summary}"),
            Finished::Owner => CLOSED_BY_OWNER_LINE.to_string(),
        }
    }

    fn by(&self) -> &'static str {
        match self {
            Finished::Agent(_) => "agent",
            Finished::Owner => "owner",
        }
    }
}

enum Answer {
    Text(String),
    Approval(Approval),
}

struct LiveState {
    status: Status,
    note: String,
    stream: String,
    pending: Option<Pending>,
    answer: Option<(u64, Answer)>,
    queued: VecDeque<Queued>,
    /// Messages dropped from the queue (a stop, a close, a shutdown) that
    /// the worker still has to write to the transcript, after the note of
    /// the turn they waited behind.
    unsent: Vec<(Queued, Unsent)>,
    /// A finish asked for during the turn (the agent's tool, the owner's
    /// close): the turn ends closed.
    finish: Option<Finished>,
    cancel: Option<CancelToken>,
    auto: bool,
    version: u64,
    worker: bool,
    last_step: i64,
    turn_from: i64,
    turn_started_at: String,
    next_id: u64,
    closed: bool,
    /// Closed by the hub going down, not by the owner: the stopped turn's
    /// note says so.
    shutdown: bool,
    session_id: String,
}

struct Shared {
    state: Mutex<LiveState>,
    cv: Condvar,
}

impl Shared {
    fn new(auto: bool) -> Self {
        Self {
            state: Mutex::new(LiveState {
                status: Status::Idle,
                note: String::new(),
                stream: String::new(),
                pending: None,
                answer: None,
                queued: VecDeque::new(),
                unsent: Vec::new(),
                finish: None,
                cancel: None,
                auto,
                version: 1,
                worker: false,
                // Steps count from 0, so "none yet" is -1 and a turn's steps
                // are `turn_from < n <= last_step`.
                last_step: -1,
                turn_from: -1,
                turn_started_at: String::new(),
                next_id: 1,
                closed: false,
                shutdown: false,
                session_id: String::new(),
            }),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, LiveState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn bump(&self, st: &mut LiveState) {
        st.version += 1;
        self.cv.notify_all();
    }

    fn snapshot(&self) -> Live {
        let st = self.lock();
        // A worker holding a message that has not reached the agent yet
        // counts as running: the owner just sent it.
        let status = if st.worker && matches!(st.status, Status::Idle | Status::Closed) {
            Status::Running
        } else {
            st.status
        };
        Live {
            status: status.word().to_string(),
            note: st.note.clone(),
            stream: st.stream.clone(),
            pending: st.pending.clone(),
            queued: st.queued.iter().map(Queued::shown).collect(),
            turn_from: st.turn_from,
            turn_started_at: st.turn_started_at.clone(),
            auto_approve: st.auto,
            version: st.version,
            session_id: st.session_id.clone(),
        }
    }

    /// Parks the agent's thread until the owner answers pending item `id`,
    /// the turn is cancelled, or the runtime closes.
    fn wait_answer(&self, id: u64) -> Option<Answer> {
        let mut st = self.lock();
        loop {
            if let Some((for_id, _)) = &st.answer
                && *for_id == id
            {
                let (_, answer) = st.answer.take().expect("answer present");
                return Some(answer);
            }
            if st.closed || st.cancel.as_ref().is_none_or(CancelToken::is_cancelled) {
                return None;
            }
            st = self
                .cv
                .wait_timeout(st, PARK_TICK)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// The reply's text as it streams in, for the page to show; a restart
/// (the stream broke and the model is asked again) clears it.
struct LiveStream {
    shared: Arc<Shared>,
}

impl StreamSink for LiveStream {
    fn text(&mut self, text: &str) {
        let mut st = self.shared.lock();
        st.stream.push_str(text);
        self.shared.bump(&mut st);
    }

    fn restart(&mut self) {
        let mut st = self.shared.lock();
        st.stream.clear();
        self.shared.bump(&mut st);
    }
}

/// The owner at the other end of HTTP.
struct WebUi {
    shared: Arc<Shared>,
    store: Store,
    id: String,
    /// The manager, for the sub-conversations this agent starts and reads.
    manager: Weak<ManagerInner>,
}

impl WebUi {
    fn manager(&self) -> Option<Manager> {
        self.manager.upgrade().map(Manager::from_inner)
    }

    /// Tells the conversation that started this one (if any) that its
    /// agent asks a question, so it can answer or leave it to the owner.
    fn report_question(&self, question: &str, choices: &[String]) {
        let Some(m) = self.manager() else { return };
        let Ok(Some(row)) = self.store.chat_get(&self.id) else {
            return;
        };
        if row.parent.is_empty() {
            return;
        }
        let mut text = question.to_string();
        if !choices.is_empty() {
            text.push_str("\nChoices: ");
            text.push_str(&choices.join(" | "));
        }
        m.deliver_report(&row, REPORT_ASKS, &text);
    }
    fn park(&self, pending: Pending, note: String) -> Option<Answer> {
        let id = pending.id();
        {
            let mut st = self.shared.lock();
            st.pending = Some(pending);
            st.status = Status::Waiting;
            st.note = note.clone();
            self.shared.bump(&mut st);
        }
        let _ = self.store.chat_set_status(&self.id, "waiting", &note);
        let answer = self.shared.wait_answer(id);
        {
            let mut st = self.shared.lock();
            if st.pending.as_ref().is_some_and(|p| p.id() == id) {
                st.pending = None;
            }
            if !st.closed {
                st.status = Status::Running;
                st.note.clear();
            }
            self.shared.bump(&mut st);
        }
        let _ = self.store.chat_set_status(&self.id, "running", "");
        answer
    }

    fn next_id(&self) -> u64 {
        let mut st = self.shared.lock();
        let id = st.next_id;
        st.next_id += 1;
        id
    }
}

impl Ui for WebUi {
    fn finish(&mut self, summary: &str) -> bool {
        let mut st = self.shared.lock();
        if st.finish.is_none() {
            st.finish = Some(Finished::Agent(summary.to_string()));
        }
        self.shared.bump(&mut st);
        true
    }

    fn approve(&mut self, req: &ApprovalRequest) -> Approval {
        if self.shared.lock().auto {
            return Approval::Allow;
        }
        let id = self.next_id();
        let pending = Pending::Approval {
            id,
            tool: req.tool.clone(),
            summary: req.summary.clone(),
            detail: req.detail.clone(),
            reason: req.reason.clone(),
            always: req.always.clone(),
        };
        let note = format!(
            "waiting for your approval: {} {}",
            req.tool,
            one_line(&req.summary)
        );
        let decision = match self.park(pending, note) {
            Some(Answer::Approval(a)) => a,
            Some(Answer::Text(_)) | None => {
                Approval::Deny("the owner stopped this turn before deciding".to_string())
            }
        };
        let (word, reason) = match &decision {
            Approval::Allow => ("allow", String::new()),
            Approval::Always => ("always", req.always.clone()),
            Approval::Deny(why) => ("deny", why.clone()),
        };
        let _ = self.store.chat_add_message(
            &self.id,
            "approval",
            &format!("{}: {}", req.tool, one_line(&req.summary)),
            &json!({"tool": req.tool, "decision": word, "reason": reason, "always": req.always}),
        );
        decision
    }

    fn ask(&mut self, question: &str, choices: &[String]) -> Option<String> {
        let id = self.next_id();
        let _ = self.store.chat_add_message(
            &self.id,
            "question",
            question,
            &json!({"choices": choices, "pending": id}),
        );
        let pending = Pending::Question {
            id,
            question: question.to_string(),
            choices: choices.to_vec(),
        };
        self.report_question(question, choices);
        match self.park(
            pending,
            format!("waiting for your answer: {}", one_line(question)),
        ) {
            Some(Answer::Text(t)) => Some(t),
            Some(Answer::Approval(_)) | None => None,
        }
    }

    fn chat(&mut self, req: &ChatRequest) -> Option<ChatReply> {
        let m = self.manager()?;
        Some(match req {
            ChatRequest::Start { title, text } => m
                .create_child(&self.id, title, text)
                .map(|row| {
                    format!(
                        "Started sub-conversation {} ({:?}). It works in your workspace and every reply it makes reaches you as a message from it; the owner sees it on its own page too. Carry on with your own work meanwhile, or end your turn and wait for it.",
                        row.id, row.title
                    )
                })
                .map_err(|e| e.to_string()),
            ChatRequest::Send { id, text } => m
                .send_from_parent(id, &self.id, text)
                .map(|()| format!("Sent to {id}; its reply reaches you as a message from it."))
                .map_err(|e| e.to_string()),
            ChatRequest::Read { id } => m
                .read_children(&self.id, id)
                .map_err(|e| e.to_string()),
        })
    }
}

/// Keeps the live step counter current so the hub knows which steps belong
/// to the running turn.
struct LiveSink(Arc<Shared>);

impl Sink for LiveSink {
    fn step(&mut self, step: Step) {
        let mut st = self.0.lock();
        st.last_step = st.last_step.max(i64::from(step.n));
        self.0.bump(&mut st);
    }
}

fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 160 {
        let cut: String = line.chars().take(157).collect();
        format!("{cut}...")
    } else {
        line.to_string()
    }
}

/// The agent of one conversation while the hub holds it.
struct Started {
    agent: Arc<Mutex<Agent>>,
    prepared: Prepared,
    ctx: Context,
    session_id: String,
    cwd: String,
    /// The model the session resolved to; a row that names another one
    /// restarts the session on the next turn.
    model: String,
    /// The reasoning effort the session runs on (empty: the provider's
    /// default), compared like the model.
    reasoning: String,
    last_cost: f64,
    ended: bool,
}

/// One message waiting for the agent: its text as the model sees it, the
/// images the owner attached inline (`Message.images`) and, for a message
/// the owner typed, its transcript row. The row is written when the agent
/// takes the message up, not when it was typed: a message sent during a
/// turn then lands after that turn's reply, the order the agent read them
/// in, instead of above a reply it never saw. Until then the hub shows it
/// as queued.
#[derive(Clone, Debug, Default)]
struct Queued {
    text: String,
    images: Vec<ImageContent>,
    /// The owner's words and the row's extra (attachments, images); None
    /// for the runtime's own prompts, which leave no row.
    row: Option<(String, Value)>,
    /// A report from a sub-conversation rather than the owner's words: the
    /// row is a note (the page shows who it is from), not a message of
    /// the owner's.
    report: bool,
}

impl From<String> for Queued {
    fn from(text: String) -> Self {
        Queued {
            text,
            images: Vec::new(),
            row: None,
            report: false,
        }
    }
}

impl From<&str> for Queued {
    fn from(text: &str) -> Self {
        text.to_string().into()
    }
}

impl Queued {
    /// The words the hub lists while the message waits: the owner's own,
    /// or the prompt itself for the runtime's.
    fn shown(&self) -> String {
        match &self.row {
            Some((shown, _)) => shown.clone(),
            None => self.text.clone(),
        }
    }

    /// The transcript role of the message's row.
    fn role(&self) -> &'static str {
        if self.report { "note" } else { "user" }
    }
}

/// Why a queued message never reached the agent; the transcript says so
/// under it, since "continue" would not bring it back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unsent {
    /// The hub went down (restart, shutdown).
    Shutdown,
    /// The owner stopped the turn it waited behind.
    Stopped,
    /// The conversation was finished or put away first.
    Closed,
    /// The agent could not start.
    NoAgent,
}

impl Unsent {
    fn line(self) -> &'static str {
        match self {
            Unsent::Shutdown => UNSENT_LINE,
            Unsent::Stopped => UNSENT_STOPPED_LINE,
            Unsent::Closed => UNSENT_CLOSED_LINE,
            Unsent::NoAgent => UNSENT_NO_AGENT_LINE,
        }
    }
}

/// The conversation's runtime: its queue, its live state and, once the
/// first message arrives, its agent.
pub struct Runtime {
    pub id: String,
    home: String,
    store: Store,
    provider: String,
    model: String,
    shared: Arc<Shared>,
    started: Mutex<Option<Started>>,
    /// The manager this runtime belongs to, for the sub-conversations its
    /// agent starts and the reports it sends up to its parent.
    manager: Weak<ManagerInner>,
}

impl Runtime {
    fn new(
        home: &str,
        store: Store,
        provider: &str,
        model: &str,
        row: &Row,
        manager: Weak<ManagerInner>,
    ) -> Arc<Self> {
        let shared = Arc::new(Shared::new(row.auto_approve));
        shared.lock().session_id = row.session_id.clone();
        Arc::new(Self {
            id: row.id.clone(),
            home: home.to_string(),
            store,
            provider: provider.to_string(),
            model: model.to_string(),
            shared,
            started: Mutex::new(None),
            manager,
        })
    }

    /// The manager, while the hub holds it.
    fn manager(&self) -> Option<Manager> {
        self.manager.upgrade().map(Manager::from_inner)
    }

    pub fn live(&self) -> Live {
        self.shared.snapshot()
    }

    /// Queues a message and makes sure a worker is running it. The owner's
    /// row goes to the transcript now when nothing runs ahead of it (the
    /// turn starts on it at once); behind a running turn it waits with the
    /// message ([`Queued::row`]).
    fn enqueue(self: &Arc<Self>, message: impl Into<Queued>) {
        let mut message = message.into();
        let role = message.role();
        let (spawn, row) = {
            let mut st = self.shared.lock();
            let spawn = !st.worker;
            let row = if spawn { message.row.take() } else { None };
            st.queued.push_back(message);
            st.worker = true;
            self.shared.bump(&mut st);
            (spawn, row)
        };
        if let Some((shown, extra)) = row {
            let _ = self.store.chat_add_message(&self.id, role, &shown, &extra);
        }
        if spawn {
            let me = self.clone();
            thread::Builder::new()
                .name(format!("chat-{}", self.id))
                .spawn(move || me.work())
                .expect("spawn conversation worker");
        }
    }

    fn work(self: Arc<Self>) {
        loop {
            // Taking the next message and standing down are one step under
            // the lock: a message that arrives between them would find a
            // worker that is no longer looking and wait for the next one.
            let next = {
                let mut st = self.shared.lock();
                if st.closed {
                    let why = if st.shutdown {
                        Unsent::Shutdown
                    } else {
                        Unsent::Closed
                    };
                    let dropped: Vec<Queued> = st.queued.drain(..).collect();
                    st.unsent.extend(dropped.into_iter().map(|q| (q, why)));
                }
                let next = st.queued.pop_front();
                let mut finished = None;
                if next.is_none() && !st.closed {
                    // A finish that found no turn to end (the owner closed
                    // between two) lands here.
                    finished = st.finish.take();
                    if let Some(f) = &finished {
                        st.status = Status::Closed;
                        st.note = f.note();
                    }
                    st.worker = false;
                }
                self.shared.bump(&mut st);
                drop(st);
                if let Some(f) = finished {
                    self.record_finished(&f);
                }
                next
            };
            match next {
                Some(message) => self.turn(message),
                None => break,
            }
        }
        self.write_unsent();
        // A close that came while this worker was active left ending the
        // session to it, so the session ends after the last turn, never
        // under one.
        let close_here = self.shared.lock().closed;
        if close_here {
            self.end_session();
            let mut st = self.shared.lock();
            st.worker = false;
            self.shared.bump(&mut st);
        }
    }

    /// Runs one turn: starts or restarts the agent when the workspace
    /// changed, sends the message, records what came back.
    fn turn(self: &Arc<Self>, message: Queued) {
        let row = match self.store.chat_get(&self.id) {
            Ok(Some(r)) if r.status != "archived" => r,
            _ => {
                self.shared.lock().closed = true;
                return;
            }
        };
        {
            let mut st = self.shared.lock();
            if st.closed {
                // Put away or going down before the turn began: no session
                // starts for a message that cannot run; the transcript says
                // the message never arrived.
                let why = if st.shutdown {
                    Unsent::Shutdown
                } else {
                    Unsent::Closed
                };
                st.unsent.push((message, why));
                drop(st);
                self.write_unsent();
                return;
            }
        }
        let row_role = message.role();
        let Queued {
            text,
            images,
            row: owner_row,
            ..
        } = message;
        // The owner's message enters the transcript as the agent takes it
        // up: after the reply of the turn it waited behind.
        if let Some((shown, extra)) = &owner_row {
            let _ = self
                .store
                .chat_add_message(&self.id, row_role, shown, extra);
        }
        let row_note = row.note.clone();
        let (agent, session_id, ran_in, first) = match self.ensure_started(&row) {
            Ok(v) => v,
            Err(e) => {
                let why = format!("could not start the agent: {e:#}");
                let _ = self
                    .store
                    .chat_add_message(&self.id, "note", &why, &Value::Null);
                let _ = self
                    .store
                    .chat_set_status(&self.id, "ended", &one_line(&why));
                {
                    let mut st = self.shared.lock();
                    st.status = Status::Ended;
                    st.note = one_line(&why);
                    let dropped: Vec<Queued> = st.queued.drain(..).collect();
                    st.unsent
                        .extend(dropped.into_iter().map(|q| (q, Unsent::NoAgent)));
                    self.shared.bump(&mut st);
                }
                self.write_unsent();
                return;
            }
        };
        let message = if first {
            let row = self.store.chat_get(&self.id).ok().flatten().unwrap_or(row);
            let brief = self.brief(&row);
            format!("{brief}\n\n{text}")
        } else {
            text.clone()
        };
        let message = match restart_hint(&self.store, &self.id, &row_note) {
            Some(hint) => format!("{hint}\n\n{message}"),
            None => message,
        };
        let cancel = CancelToken::new();
        {
            let mut st = self.shared.lock();
            st.status = Status::Running;
            st.note.clear();
            st.stream.clear();
            st.cancel = Some(cancel.clone());
            st.turn_from = st.last_step;
            st.turn_started_at = now();
            st.session_id = session_id.clone();
            if st.closed || st.finish.is_some() {
                // Closed or finished while the message was on its way to
                // the agent: the turn stops at its first step instead of
                // running on.
                cancel.cancel();
            }
            self.shared.bump(&mut st);
        }
        let _ = self.store.chat_set_status(&self.id, "running", "");
        let stop_watch = self
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|s| s.prepared.watch_cancel(&cancel, None));
        let outcome = agent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .turn_with_images(&cancel, &message, images);
        if let Some(stop) = stop_watch {
            stop();
        }
        let turn_from = self.shared.lock().turn_from;
        let steps_to = self.shared.lock().last_step;
        let extra = json!({"session": session_id, "steps": [turn_from, steps_to]});
        let mut ended = false;
        let mut status = Status::Idle;
        let mut note = String::new();
        // What the conversation that started this one hears of the turn.
        let mut report: Option<(&str, String)> = None;
        match outcome {
            Ok(Turn::Reply(reply)) => {
                let _ = self
                    .store
                    .chat_add_message(&self.id, "assistant", &reply, &extra);
                report = Some((REPORT_REPLIED, reply));
            }
            Ok(Turn::Blocked(reason)) => {
                let _ = self
                    .store
                    .chat_add_message(&self.id, "note", &reason, &extra);
                report = Some((REPORT_BLOCKED, reason));
            }
            Ok(Turn::Stopped { status: why, error }) => {
                let resume = !cancel.is_cancelled()
                    && resume_after_provider_error(&self.store, &why, &error);
                let line = if cancel.is_cancelled() {
                    let st = self.shared.lock();
                    if st.shutdown {
                        drop(st);
                        note = RESTART_NOTE.to_string();
                        STOPPED_TO_RESTART_LINE.to_string()
                    } else if matches!(st.finish, Some(Finished::Owner)) {
                        "The turn stopped here: you finished the conversation.".to_string()
                    } else {
                        drop(st);
                        report = Some((REPORT_STOPPED, "The owner stopped its turn.".to_string()));
                        "You stopped this turn.".to_string()
                    }
                } else if error.is_empty() {
                    let line = format!("The turn stopped: {why}.");
                    report = Some((REPORT_STOPPED, line.clone()));
                    line
                } else if resume {
                    format!(
                        "The turn stopped: {why}: {error} (resuming once; if it fails again the conversation waits for your message)"
                    )
                } else {
                    let line = format!("The turn stopped: {why}: {error}");
                    report = Some((REPORT_STOPPED, line.clone()));
                    line
                };
                if resume {
                    // A transport provider_error (a failed send or a broken reply body,
                    // after pulse-provider already spent its own retries) cut the turn
                    // while the owner may be away from the console: leave the note, then
                    // resume the turn once after the configured pause. A second failure
                    // in a row goes idle as before.
                    self.shared
                        .lock()
                        .queued
                        .push_front(RESUME_AFTER_PROVIDER_ERROR.to_string().into());
                }
                let _ = self.store.chat_add_message(&self.id, "note", &line, &extra);
            }
            Ok(Turn::Ended(result)) => {
                ended = true;
                status = Status::Ended;
                note = format!("session ended: {}", result.exit_status);
                let line = if result.error.is_empty() {
                    format!(
                        "The session ended ({}). Your next message starts a new one with this conversation carried over.",
                        result.exit_status
                    )
                } else {
                    format!(
                        "The session ended ({}): {}. Your next message starts a new one with this conversation carried over.",
                        result.exit_status, result.error
                    )
                };
                let _ = self.store.chat_add_message(&self.id, "note", &line, &extra);
                report = Some((REPORT_ENDED, line));
            }
            Err(e) => {
                let line = format!("The turn failed: {e:#}");
                let _ = self.store.chat_add_message(&self.id, "note", &line, &extra);
                if agent.lock().unwrap_or_else(|e| e.into_inner()).is_ended() {
                    ended = true;
                    status = Status::Ended;
                    note = "session ended".to_string();
                }
                report = Some((REPORT_STOPPED, line));
            }
        }
        // Costs, the trajectory on disk, and the session's end when it came.
        let cost = agent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .result()
            .usage
            .cost;
        {
            let mut guard = self.started.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = guard.as_mut() {
                let delta = (cost - s.last_cost).max(0.0);
                s.last_cost = cost;
                let _ = self.store.chat_add_turn(&self.id, delta);
                let _ = s.prepared.write_trajectory(&mut s.ctx);
                if ended {
                    s.ended = true;
                    let result = s.agent.lock().unwrap_or_else(|e| e.into_inner()).result();
                    let _ = s.prepared.finish(&mut s.ctx, &result);
                }
            }
        }
        // A move into a project during the turn (`pulse chat attach`): the
        // next turn runs there, and starts by itself so the owner need not
        // ask twice. A conversation put away during the turn stays put
        // away.
        let row_now = self.store.chat_get(&self.id).ok().flatten();
        let archived = row_now.as_ref().is_some_and(|r| r.status == "archived");
        let finishing = !ended && self.shared.lock().finish.is_some();
        let moved = row_now
            .as_ref()
            .filter(|r| !archived && !finishing && !r.cwd.is_empty() && r.cwd != ran_in);
        let finished = {
            let mut st = self.shared.lock();
            st.cancel = None;
            st.stream.clear();
            // A finish during the turn (the agent's tool, the owner's
            // close) ends it closed, unless the session itself ended.
            let finished = st.finish.take().filter(|_| !ended);
            if let Some(f) = &finished {
                status = Status::Closed;
                note = f.note();
                // The conversation's own prompts (a resume) stop here; a
                // message the owner sent meanwhile runs next and reopens it.
                st.queued.retain(|q| q.row.is_some());
            }
            st.status = status;
            st.note = note.clone();
            if let Some(r) = &moved {
                st.queued.push_front(
                    format!(
                        "You now work in {} (project {}, branch {}). Carry on with the request above from here.",
                        r.cwd, r.project, r.branch
                    )
                    .into(),
                );
            }
            self.shared.bump(&mut st);
            finished
        };
        if let Some(f) = &finished {
            let _ = self.store.chat_add_message(
                &self.id,
                "note",
                &f.line(),
                &json!({"closed": f.by(), "session": session_id}),
            );
            // The finish is the report: the summary, with the closing
            // reply when the agent made one.
            let closing = report
                .take()
                .filter(|(kind, _)| *kind == REPORT_REPLIED)
                .map(|(_, text)| text)
                .unwrap_or_default();
            let text = match f {
                Finished::Agent(summary) if closing.trim().is_empty() => summary.clone(),
                Finished::Agent(summary) => format!("{summary}\n\n{closing}"),
                Finished::Owner => "The owner finished it.".to_string(),
            };
            report = Some((REPORT_FINISHED, text));
        }
        if let Some(r) = &moved {
            let _ = self.store.chat_add_message(
                &self.id,
                "note",
                &format!("Moved into project {} ({}).", r.project, r.cwd),
                &json!({"project": r.project, "cwd": r.cwd, "branch": r.branch}),
            );
        }
        if !archived {
            let _ = self.store.chat_set_status(&self.id, status.word(), &note);
        }
        // What was dropped behind this turn (a stop, a close) follows its
        // note, so the transcript reads in the order things happened.
        self.write_unsent();
        // A sub-conversation's turn ends with its report to the
        // conversation that started it.
        if let Some((kind, text)) = report
            && !archived
            && let Some(row) = row_now.as_ref().filter(|r| !r.parent.is_empty())
            && let Some(m) = self.manager()
        {
            m.deliver_report(row, kind, &text);
        }
    }

    /// The agent for the row's current workspace: the one running, or a
    /// new session with the messages so far carried over.
    /// Returns the agent, its harness session, the folder it works in and
    /// whether this is the session's first turn (the brief goes with it).
    fn ensure_started(
        self: &Arc<Self>,
        row: &Row,
    ) -> Result<(Arc<Mutex<Agent>>, String, String, bool)> {
        let mut guard = self.started.lock().unwrap_or_else(|e| e.into_inner());
        let row = self.ensure_workspace(row)?;
        // A pinned model (tests) wins over the row's; otherwise a row that
        // names another model than the session runs on ends that session.
        let same_model = |s: &Started| !self.model.is_empty() || s.model == row.model;
        if let Some(s) = guard.as_ref()
            && !s.ended
            && s.cwd == row.cwd
            && same_model(s)
            && s.reasoning == row.reasoning
        {
            return Ok((s.agent.clone(), s.session_id.clone(), s.cwd.clone(), false));
        }
        let resume: Option<Trajectory> = match guard.take() {
            Some(mut old) => {
                let traj = old
                    .agent
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .trajectory();
                if !old.ended {
                    let result = old
                        .agent
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .end(EXIT_ENDED);
                    let _ = old.prepared.write_trajectory(&mut old.ctx);
                    let _ = old.prepared.finish(&mut old.ctx, &result);
                }
                Some(traj)
            }
            None if !row.session_id.is_empty() => read_trajectory(&self.home, &row.session_id),
            None => None,
        };
        let mut ctx = Context {
            store: Some(self.store.clone()),
            home: self.home.clone(),
            out: Box::new(io::sink()),
            err: Box::new(io::sink()),
            json: false,
            quiet: true,
            color: false,
            agent: String::new(),
            project: row.project.clone(),
            session: String::new(),
            now: String::new(),
            args: Vec::new(),
        };
        let mut fs = FlagSet::new("chat");
        fs.set_output(pulse_cli::flags::Output::Discard);
        add_agent_flags(&mut fs);
        fs.parse(&[])?;
        fs.set("C", &row.cwd)?;
        if !self.provider.is_empty() {
            fs.set("provider", &self.provider)?;
        }
        if !self.model.is_empty() {
            fs.set("model", &self.model)?;
        } else if !row.model.is_empty() {
            fs.set("model", &row.model)?;
        }
        if !row.reasoning.is_empty() {
            fs.set("reasoning", &row.reasoning)?;
        }
        let mut env = vec![
            format!("{CONVERSATION_ENV}={}", row.id),
            format!(
                "{ARTIFACTS_ENV}={}",
                artifacts_dir(&self.home, &row.id).display()
            ),
        ];
        if !row.project.is_empty() {
            env.push(format!("PULSE_PROJECT={}", row.project));
        }
        if let Some(path) = path_with_pulse() {
            env.push(format!("PATH={path}"));
        }
        env.extend(self.build_env(&row));
        let prepared = prepare_agent_with(
            &mut ctx,
            &fs,
            "chat",
            String::new(),
            true,
            vec![Box::new(LiveSink(self.shared.clone()))],
            false,
            PrepareOpts {
                project: row.project.clone(),
                memory: if row.project.is_empty() {
                    SHARED_MEMORY_KEY.to_string()
                } else {
                    String::new()
                },
                env,
            },
        )?;
        let agent = prepared.shared.clone();
        {
            let mut a = agent.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(traj) = resume
                && !traj.messages.is_empty()
            {
                a.resume_messages(traj.messages)?;
            }
            a.set_ui(Arc::new(Mutex::new(WebUi {
                shared: self.shared.clone(),
                store: self.store.clone(),
                id: row.id.clone(),
                manager: self.manager.clone(),
            })));
            a.offer_finish(true);
            a.offer_chat(true);
            let shared = self.shared.clone();
            a.set_stream(Box::new(LiveStream { shared }));
        }
        let session_id = prepared.session_id.clone();
        self.store.chat_set_session(
            &row.id,
            &session_id,
            &prepared.provider_name,
            &prepared.model_name,
        )?;
        {
            let mut st = self.shared.lock();
            st.last_step = -1;
            st.turn_from = -1;
            st.session_id = session_id.clone();
            self.shared.bump(&mut st);
        }
        let model = prepared.model_name.clone();
        *guard = Some(Started {
            agent: agent.clone(),
            prepared,
            ctx,
            session_id: session_id.clone(),
            cwd: row.cwd.clone(),
            model,
            reasoning: row.reasoning.clone(),
            last_cost: 0.0,
            ended: false,
        });
        Ok((agent, session_id, row.cwd, true))
    }

    /// The build hygiene the conversation's commands run with
    /// ([`crate::buildenv`]): a shared Cargo target per project for a
    /// worktree PULSE made, bounded compile jobs, no debug info or
    /// incremental caches - the daemon's rules for its own children.
    fn build_env(&self, row: &Row) -> Vec<String> {
        let cwd = Path::new(&row.cwd);
        let owned = chat_dir(&self.home, &row.id);
        let target = crate::buildenv::shared_target(&self.home, &row.project, cwd, &owned);
        if let Some(dir) = &target {
            crate::buildenv::link_target(cwd, dir);
        }
        let jobs = crate::buildenv::jobs(self.store.setting_int("daemon.build_jobs"));
        crate::buildenv::lines(
            |name| std::env::var_os(name).is_some(),
            target.as_deref(),
            jobs,
        )
    }

    /// Gives the conversation a place to work: the project's worktree when
    /// it names one, a scratch folder otherwise. Returns the row as it
    /// stands afterwards.
    fn ensure_workspace(&self, row: &Row) -> Result<Row> {
        let dir = artifacts_dir(&self.home, &row.id);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        if !row.project.is_empty() && row.branch.is_empty() {
            let attached = attach_workspace(&self.store, &self.home, &row.id, &row.project)?;
            let mut r = row.clone();
            r.cwd = attached.cwd;
            r.branch = attached.branch;
            return Ok(r);
        }
        if row.cwd.is_empty() || !Path::new(&row.cwd).is_dir() {
            let scratch = scratch_dir(&self.home, &row.id);
            std::fs::create_dir_all(&scratch)
                .with_context(|| format!("create {}", scratch.display()))?;
            let cwd = scratch.display().to_string();
            self.store
                .chat_set_workspace(&row.id, &row.project, &cwd, &row.branch)?;
            let mut r = row.clone();
            r.cwd = cwd;
            return Ok(r);
        }
        Ok(row.clone())
    }

    /// What the agent reads before the owner's first message of a session.
    fn brief(&self, row: &Row) -> String {
        let projects = self.store.projects(true).unwrap_or_default();
        brief_text(row, &projects, &artifacts_dir(&self.home, &row.id))
    }

    fn answer(&self, text: String) -> Result<()> {
        let mut st = self.shared.lock();
        let Some(Pending::Question { id, .. }) = &st.pending else {
            bail!("the agent is not waiting for an answer");
        };
        let id = *id;
        st.answer = Some((id, Answer::Text(text)));
        st.pending = None;
        self.shared.bump(&mut st);
        Ok(())
    }

    fn approve(&self, decision: Approval) -> Result<()> {
        let mut st = self.shared.lock();
        let Some(Pending::Approval { id, .. }) = &st.pending else {
            bail!("the agent is not waiting for an approval");
        };
        let id = *id;
        st.answer = Some((id, Answer::Approval(decision)));
        st.pending = None;
        self.shared.bump(&mut st);
        Ok(())
    }

    fn set_auto(&self, on: bool) {
        let mut st = self.shared.lock();
        st.auto = on;
        if on && let Some(Pending::Approval { id, .. }) = &st.pending {
            let id = *id;
            st.answer = Some((id, Answer::Approval(Approval::Allow)));
            st.pending = None;
        }
        self.shared.bump(&mut st);
    }

    /// Stops the running turn and drops what was queued behind it; the
    /// dropped messages reach the transcript after the stopped turn's note.
    fn cancel(&self) {
        let mut st = self.shared.lock();
        Self::drop_queue(&mut st, Unsent::Stopped);
        if let Some(c) = &st.cancel {
            c.cancel();
        }
        self.shared.bump(&mut st);
    }

    /// Moves what waits in the queue to the unsent list, for the worker to
    /// write out ([`Runtime::write_unsent`]).
    fn drop_queue(st: &mut LiveState, why: Unsent) {
        let dropped: Vec<Queued> = st.queued.drain(..).collect();
        st.unsent.extend(dropped.into_iter().map(|q| (q, why)));
    }

    /// Writes the messages that never reached the agent: the owner's words
    /// as their own row, then the note that says why and what to do.
    fn write_unsent(&self) {
        let unsent = std::mem::take(&mut self.shared.lock().unsent);
        for (message, why) in unsent {
            if let Some((shown, extra)) = &message.row {
                let _ = self
                    .store
                    .chat_add_message(&self.id, message.role(), shown, extra);
            }
            let extra = if why == Unsent::Shutdown {
                json!({"restart": true, "unsent": true})
            } else {
                json!({"unsent": true})
            };
            let _ = self
                .store
                .chat_add_message(&self.id, "note", why.line(), &extra);
        }
    }

    /// The owner finishes the conversation. With a worker active the turn
    /// in flight stops and ends closed (recording it); otherwise the
    /// runtime stands closed at once and the caller records it. The session
    /// stays open either way: a new message reopens the conversation where
    /// it was.
    fn finish_by_owner(&self) -> bool {
        let mut st = self.shared.lock();
        Self::drop_queue(&mut st, Unsent::Closed);
        if st.worker {
            if st.finish.is_none() {
                st.finish = Some(Finished::Owner);
            }
            if let Some(c) = &st.cancel {
                c.cancel();
            }
            self.shared.bump(&mut st);
            false
        } else {
            st.status = Status::Closed;
            st.note = Finished::Owner.note();
            self.shared.bump(&mut st);
            true
        }
    }

    /// Writes a finish to the store: its transcript line, then the row.
    fn record_finished(&self, f: &Finished) {
        let _ =
            self.store
                .chat_add_message(&self.id, "note", &f.line(), &json!({"closed": f.by()}));
        let _ = self.store.chat_set_status(&self.id, "closed", &f.note());
        self.write_unsent();
        let text = match f {
            Finished::Agent(summary) => summary.clone(),
            Finished::Owner => "The owner finished it.".to_string(),
        };
        self.report_up(REPORT_FINISHED, &text);
    }

    /// Hands a report to the conversation that started this one, when
    /// there is one and the hub still holds the manager.
    fn report_up(&self, kind: &str, text: &str) {
        let Ok(Some(row)) = self.store.chat_get(&self.id) else {
            return;
        };
        if row.parent.is_empty() || row.status == "archived" {
            return;
        }
        if let Some(m) = self.manager() {
            m.deliver_report(&row, kind, text);
        }
    }

    /// Ends the agent's session; the worker, if any, winds down after the
    /// cancelled turn returns. The handle is the thread ending the session.
    fn close(self: &Arc<Self>) -> thread::JoinHandle<()> {
        self.stop(false)
    }

    /// [`Runtime::close`] for a hub going down: the stopped turn's note
    /// says PULSE stopped it, not the owner.
    fn stop(self: &Arc<Self>, shutdown: bool) -> thread::JoinHandle<()> {
        let worker = {
            let mut st = self.shared.lock();
            st.closed = true;
            st.shutdown |= shutdown;
            let why = if st.shutdown {
                Unsent::Shutdown
            } else {
                Unsent::Closed
            };
            Self::drop_queue(&mut st, why);
            if let Some(c) = &st.cancel {
                c.cancel();
            }
            self.shared.bump(&mut st);
            st.worker
        };
        let me = self.clone();
        thread::Builder::new()
            .name(format!("chat-close-{}", self.id))
            .spawn(move || {
                // An active worker ends the session itself once its turn
                // has returned ([`Runtime::work`]); ending it from here
                // would cut the turn off before its note is written.
                if !worker {
                    me.end_session();
                }
            })
            .expect("spawn conversation close")
    }

    /// Ends the agent's session when one is open: the trajectory goes to
    /// disk and the harness session finishes as ended.
    fn end_session(&self) {
        let mut guard = self.started.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut s) = guard.take()
            && !s.ended
        {
            let result = s
                .agent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .end(EXIT_ENDED);
            let _ = s.prepared.write_trajectory(&mut s.ctx);
            let _ = s.prepared.finish(&mut s.ctx, &result);
        }
    }

    /// True once no worker thread holds a turn (or a queued message) and,
    /// after a close, the session has ended.
    fn settled(&self) -> bool {
        !self.shared.lock().worker
    }
}

/// The hub's handle on every conversation: rows in the store, runtimes in
/// memory while the hub runs. A thin handle on [`ManagerInner`], which the
/// runtimes hold weakly so a conversation's agent can start and steer its
/// sub-conversations through the same manager.
pub struct Manager {
    inner: Arc<ManagerInner>,
}

pub struct ManagerInner {
    home: String,
    store: Store,
    provider: String,
    model: String,
    runtimes: Mutex<BTreeMap<String, Arc<Runtime>>>,
}

impl std::ops::Deref for Manager {
    type Target = ManagerInner;

    fn deref(&self) -> &ManagerInner {
        &self.inner
    }
}

/// How many levels of sub-conversations a conversation may stand under
/// and still start one: a top-level conversation's children may have
/// children of their own, and those may not.
pub const CHILD_DEPTH_LIMIT: usize = 2;
/// How many sub-conversations one conversation may have open (not
/// archived) at a time.
pub const CHILDREN_LIMIT: usize = 8;
/// How many of a sub-conversation's last messages its parent's agent reads
/// at a time, and how much of each.
const READ_MESSAGES: usize = 6;
const READ_MESSAGE_CHARS: usize = 2_000;

/// The kinds of report a sub-conversation sends its parent.
pub const REPORT_FINISHED: &str = "finished";
pub const REPORT_REPLIED: &str = "replied";
pub const REPORT_ASKS: &str = "asks";
pub const REPORT_BLOCKED: &str = "blocked";
pub const REPORT_STOPPED: &str = "stopped";
pub const REPORT_ENDED: &str = "ended";

/// The words the parent's agent reads for a report's kind.
fn report_phrase(kind: &str) -> &'static str {
    match kind {
        REPORT_FINISHED => "it finished and closed",
        REPORT_REPLIED => "it replied and waits for its next message",
        REPORT_ASKS => {
            "it asks a question and waits for the answer (the owner can answer it on its page; you can with the chat tool, action send)"
        }
        REPORT_BLOCKED => "it is blocked",
        REPORT_STOPPED => "its turn stopped",
        REPORT_ENDED => "its agent ended the turn without a reply",
        _ => "it reports",
    }
}

impl Manager {
    pub fn new(home: &str, store: Store) -> Self {
        Self {
            inner: Arc::new(ManagerInner {
                home: home.to_string(),
                store,
                provider: String::new(),
                model: String::new(),
                runtimes: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    fn from_inner(inner: Arc<ManagerInner>) -> Self {
        Self { inner }
    }

    /// Pins the provider and model every conversation uses (tests). Call
    /// it before the first conversation runs.
    pub fn with_provider(mut self, provider: &str, model: &str) -> Self {
        let inner = Arc::get_mut(&mut self.inner)
            .expect("with_provider is called before any conversation runs");
        inner.provider = provider.to_string();
        inner.model = model.to_string();
        self
    }

    pub fn home(&self) -> &str {
        &self.home
    }

    fn runtime(&self, row: &Row) -> Arc<Runtime> {
        let mut map = self.runtimes.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(row.id.clone())
            .or_insert_with(|| {
                Runtime::new(
                    &self.home,
                    self.store.clone(),
                    &self.provider,
                    &self.model,
                    row,
                    Arc::downgrade(&self.inner),
                )
            })
            .clone()
    }

    fn running(&self, id: &str) -> Option<Arc<Runtime>> {
        self.runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    fn row(&self, id: &str) -> Result<Row> {
        self.store
            .chat_get(id)?
            .ok_or_else(|| anyhow!("no such conversation: {id}"))
    }

    /// Starts a conversation and sends its first message.
    pub fn create(
        &self,
        title: &str,
        project: &str,
        auto_approve: bool,
        text: &str,
        by: &str,
    ) -> Result<Row> {
        self.create_with(title, project, "", "", auto_approve, text, by)
    }

    /// [`Manager::create`] on a chosen model and reasoning effort (empty:
    /// the provider's defaults).
    #[allow(clippy::too_many_arguments)]
    pub fn create_with(
        &self,
        title: &str,
        project: &str,
        model: &str,
        reasoning: &str,
        auto_approve: bool,
        text: &str,
        by: &str,
    ) -> Result<Row> {
        let text = text.trim();
        if text.is_empty() {
            bail!("the first message is empty");
        }
        let model = model_id(model)?;
        let reasoning = reasoning_effort(reasoning)?;
        if !project.is_empty() {
            let p = self
                .store
                .project(project)?
                .ok_or_else(|| anyhow!("no such project: {project}"))?;
            if p.status != "active" {
                bail!("project {project} is {}", p.status);
            }
        }
        let title = if title.trim().is_empty() {
            title_from(text)
        } else {
            title.trim().to_string()
        };
        let row = self.store.chat_create(&Row {
            id: new_conversation_id(),
            title,
            project: project.to_string(),
            model,
            reasoning,
            status: "idle".into(),
            auto_approve,
            created_by: by.to_string(),
            ..Row::default()
        })?;
        self.send(&row.id, text)?;
        self.row(&row.id)
    }

    /// Starts a sub-conversation of `parent`: its agent's brief is `text`,
    /// and it works in the parent's workspace on the parent's model,
    /// effort and approval setting, reporting each reply back to the
    /// parent ([`Manager::deliver_report`]). Refused past
    /// [`CHILD_DEPTH_LIMIT`] levels or [`CHILDREN_LIMIT`] open children,
    /// and for an archived parent.
    pub fn create_child(&self, parent: &str, title: &str, text: &str) -> Result<Row> {
        let text = text.trim();
        if text.is_empty() {
            bail!("the sub-conversation's brief is empty");
        }
        let p = self.row(parent)?;
        if p.status == "archived" {
            bail!("conversation {parent} is archived");
        }
        let depth = self.store.chat_depth(parent)?;
        if depth >= CHILD_DEPTH_LIMIT {
            bail!(
                "this conversation already stands {depth} levels down; sub-conversations go {CHILD_DEPTH_LIMIT} levels deep at most - do this part here, or with the subagent tool"
            );
        }
        let open = self.store.chat_children(parent, false)?.len();
        if open >= CHILDREN_LIMIT {
            bail!(
                "this conversation already has {open} sub-conversations open; finish or archive one before starting another"
            );
        }
        let title = if title.trim().is_empty() {
            title_from(text)
        } else {
            title.trim().to_string()
        };
        let row = self.store.chat_create(&Row {
            id: new_conversation_id(),
            title,
            project: p.project.clone(),
            cwd: p.cwd.clone(),
            branch: p.branch.clone(),
            model: p.model.clone(),
            reasoning: p.reasoning.clone(),
            status: "idle".into(),
            auto_approve: p.auto_approve,
            created_by: p.id.clone(),
            parent: p.id.clone(),
            ..Row::default()
        })?;
        let _ = self.store.chat_add_message(
            parent,
            "note",
            &format!("Started sub-conversation: {}", row.title),
            &json!({"child": row.id, "title": row.title}),
        );
        self.send(&row.id, text)?;
        self.row(&row.id)
    }

    /// A message from the conversation that started `id` (the parent's
    /// agent steering it): recorded as a note from the parent, and the
    /// agent reads it marked as the parent's. A question the
    /// sub-conversation waits on takes the text as its answer.
    pub fn send_from_parent(&self, id: &str, parent: &str, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            bail!("the message is empty");
        }
        let row = self.row(id)?;
        if row.parent != parent {
            bail!("conversation {id} is not a sub-conversation of this one");
        }
        if row.status == "archived" {
            bail!("sub-conversation {id} is archived");
        }
        if let Some(rt) = self.running(id)
            && matches!(rt.live().pending, Some(Pending::Question { .. }))
        {
            return self.answer(id, text);
        }
        let rt = self.runtime(&row);
        rt.enqueue(Queued {
            text: format!("Message from the conversation that started you ({parent}):\n{text}"),
            images: Vec::new(),
            row: Some((text.to_string(), json!({"from": parent}))),
            report: true,
        });
        Ok(())
    }

    /// Hands a sub-conversation's report to its parent: a note in the
    /// parent's transcript that says who it is from, and a turn there on
    /// it, the way the owner's message would run (after the turn in
    /// flight, reopening a finished parent). Nothing reaches an archived
    /// parent.
    pub fn deliver_report(&self, child: &Row, kind: &str, text: &str) {
        if child.parent.is_empty() {
            return;
        }
        let Ok(Some(parent)) = self.store.chat_get(&child.parent) else {
            return;
        };
        if parent.status == "archived" {
            return;
        }
        let text = text.trim();
        let prompt = format!(
            "Report from sub-conversation {:?} ({}) - {}:\n{}\n\n(Steer it with the chat tool: action send, id {}; read it: action read, id {}. It works in your workspace.)",
            child.title,
            child.id,
            report_phrase(kind),
            text,
            child.id,
            child.id
        );
        self.runtime(&parent).enqueue(Queued {
            text: prompt,
            images: Vec::new(),
            row: Some((
                text.to_string(),
                json!({"report": {"child": child.id, "title": child.title, "kind": kind}}),
            )),
            report: true,
        });
    }

    /// What a conversation's agent reads of its sub-conversations: all of
    /// them in a line each (`id` empty), or one with its state and its
    /// last replies.
    pub fn read_children(&self, parent: &str, id: &str) -> Result<String> {
        if id.is_empty() {
            let children = self.store.chat_children(parent, false)?;
            if children.is_empty() {
                return Ok("No sub-conversations yet.".to_string());
            }
            let lines: Vec<String> = children
                .iter()
                .map(|c| {
                    let live = self.live(c);
                    format!(
                        "{}  {:<8} {}{}",
                        c.id,
                        live.status,
                        c.title,
                        if live.note.is_empty() {
                            String::new()
                        } else {
                            format!(" - {}", live.note)
                        }
                    )
                })
                .collect();
            return Ok(lines.join("\n"));
        }
        let row = self.row(id)?;
        if row.parent != parent {
            bail!("conversation {id} is not a sub-conversation of this one");
        }
        let live = self.live(&row);
        let mut out = format!(
            "{}  {}\nstatus {}{}  turns {}\n",
            row.id,
            row.title,
            live.status,
            if live.note.is_empty() {
                String::new()
            } else {
                format!(" ({})", live.note)
            },
            row.turns
        );
        let messages = self.store.chat_messages(id, 0)?;
        let shown: Vec<&pulse_store::ConversationMessage> =
            messages.iter().filter(|m| m.role != "approval").collect();
        let skipped = shown.len().saturating_sub(READ_MESSAGES);
        if skipped > 0 {
            out.push_str(&format!("({skipped} earlier messages not shown)\n"));
        }
        for m in &shown[skipped..] {
            let who = match m.role.as_str() {
                "user" if m.n == 1 => "brief",
                "user" => "owner",
                "assistant" => "agent",
                "note" if m.extra.get("from").is_some() => "you",
                other => other,
            };
            let content: String = m.content.chars().take(READ_MESSAGE_CHARS).collect();
            let cut = if content.len() < m.content.len() {
                "..."
            } else {
                ""
            };
            out.push_str(&format!("\n[{who}] {}{cut}\n", content.trim_end()));
        }
        Ok(out.trim_end().to_string())
    }

    /// Records the owner's message and runs a turn on it (after the one
    /// running, when there is one).
    pub fn send(&self, id: &str, text: &str) -> Result<()> {
        self.send_with(id, text, &[])
    }

    /// [`Manager::send`] with files the owner uploaded first
    /// ([`Manager::save_attachment`]), named by their saved names. The
    /// message keeps the owner's words; the agent gets the files' paths
    /// with them.
    pub fn send_with(&self, id: &str, text: &str, attachments: &[String]) -> Result<()> {
        let text = text.trim();
        if text.is_empty() && attachments.is_empty() {
            bail!("the message is empty");
        }
        if attachments.len() > ATTACHMENTS_PER_MESSAGE {
            bail!("Invalid attachments: one message carries up to {ATTACHMENTS_PER_MESSAGE} files");
        }
        let row = self.row(id)?;
        if row.status == "archived" {
            bail!("conversation {id} is archived");
        }
        let mut files: Vec<Attachment> = Vec::new();
        for name in attachments {
            let path = self.attachment_path(id, name).ok_or_else(|| {
                anyhow!(
                    "Invalid attachments: no file named {name:?} in this conversation; upload it first"
                )
            })?;
            if files.iter().any(|f| f.name == *name) {
                continue;
            }
            files.push(Attachment {
                name: name.clone(),
                size: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                path: path.display().to_string(),
            });
        }
        let text = if text.is_empty() {
            if files.len() == 1 {
                "Look at the attached file.".to_string()
            } else {
                "Look at the attached files.".to_string()
            }
        } else {
            text.to_string()
        };
        let extra = if files.is_empty() {
            Value::Null
        } else {
            json!({"attachments": files})
        };
        let rt = self.runtime(&row);
        rt.enqueue(Queued {
            text: message_with_attachments(&text, &files),
            images: Vec::new(),
            row: Some((text, extra)),
            report: false,
        });
        Ok(())
    }

    /// Owner posts a message with inline images: each image is validated,
    /// its bytes saved into the attachments folder, and the stored message
    /// carries an `images` array so the hub can serve the bytes back at
    /// `GET /api/v1/chat/{id}/image/{message}/{n}`. The images ride the
    /// message to the model too (`Message.images`): a provider that takes
    /// images shows them to the model, one that does not ignores them.
    pub fn send_with_images(
        &self,
        id: &str,
        text: &str,
        attachments: &[String],
        images: &[ImageAttachment],
    ) -> Result<()> {
        if attachments.len() + images.len() > ATTACHMENTS_PER_MESSAGE {
            bail!("Invalid attachments: one message carries up to {ATTACHMENTS_PER_MESSAGE} files");
        }
        let mut stored: Vec<Value> = Vec::with_capacity(images.len());
        let mut shown: Vec<ImageContent> = Vec::with_capacity(images.len());
        for (n, image) in images.iter().enumerate() {
            let data = image.decode()?;
            shown.push(ImageContent {
                media_type: image.mime.trim().to_ascii_lowercase(),
                data: image.data.trim().to_string(),
            });
            let name = if image.name.trim().is_empty() {
                format!("image-{}.{}", n + 1, image.ext())
            } else {
                image.name.trim().to_string()
            };
            let saved = self.save_attachment(id, &name, &data)?;
            stored.push(json!({
                "name": saved.name,
                "mime": image.mime.trim().to_ascii_lowercase(),
                "size": saved.size,
                "path": format!("image/{id}/{n}"),
            }));
        }
        let text = text.trim();
        if text.is_empty() && attachments.is_empty() && stored.is_empty() {
            bail!("the message is empty");
        }
        let row = self.row(id)?;
        if row.status == "archived" {
            bail!("conversation {id} is archived");
        }
        let mut files: Vec<Attachment> = Vec::new();
        for name in attachments {
            let path = self.attachment_path(id, name).ok_or_else(|| {
                anyhow!(
                    "Invalid attachments: no file named {name:?} in this conversation; upload it first"
                )
            })?;
            if files.iter().any(|f| f.name == *name) {
                continue;
            }
            files.push(Attachment {
                name: name.clone(),
                size: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                path: path.display().to_string(),
            });
        }
        let text = if text.is_empty() {
            if files.len() + stored.len() == 1 {
                "Look at the attached file.".to_string()
            } else {
                "Look at the attached files.".to_string()
            }
        } else {
            text.to_string()
        };
        let extra = match (files.is_empty(), stored.is_empty()) {
            (true, true) => Value::Null,
            (false, true) => json!({"attachments": files}),
            (true, false) => json!({"images": stored}),
            (false, false) => json!({"attachments": files, "images": stored}),
        };
        let rt = self.runtime(&row);
        rt.enqueue(Queued {
            text: message_with_attachments(&text, &files),
            images: shown,
            row: Some((text, extra)),
            report: false,
        });
        Ok(())
    }

    /// Reads one image back from a message `send_with_images` stored: the
    /// `n`-th entry in its `images` array, bytes plus the mime it arrived
    /// with. `None` when the entry or its file is gone.
    pub fn image(&self, id: &str, message_n: i64, n: usize) -> Result<Option<(String, Vec<u8>)>> {
        let _ = self.row(id)?;
        let Some(m) = self.store.chat_message(id, message_n)? else {
            return Ok(None);
        };
        let meta = m.extra.get("images").and_then(|v| v.as_array()).cloned();
        let Some(entry) = meta.and_then(|a| a.into_iter().nth(n)) else {
            return Ok(None);
        };
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("image entry has no name"))?;
        let Some(path) = self.attachment_path(id, name) else {
            return Ok(None);
        };
        let data = std::fs::read(&path)?;
        if data.len() > ATTACHMENT_LIMIT {
            bail!("one attachment holds up to {ATTACHMENT_LIMIT} bytes");
        }
        let mime = entry
            .get("mime")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream")
            .to_string();
        Ok(Some((mime, data)))
    }

    /// Keeps a file the owner is attaching to a message, under its own
    /// name when that is free, a numbered one otherwise. The bytes are
    /// already in memory: the hub capped them at [`ATTACHMENT_LIMIT`].
    pub fn save_attachment(&self, id: &str, name: &str, data: &[u8]) -> Result<Attachment> {
        let row = self.row(id)?;
        if row.status == "archived" {
            bail!("conversation {id} is archived");
        }
        if data.is_empty() {
            bail!("Invalid file: the file is empty");
        }
        if data.len() > ATTACHMENT_LIMIT {
            bail!(
                "Invalid file: the file is {} bytes; attachments go up to {ATTACHMENT_LIMIT}",
                data.len()
            );
        }
        let name = attachment_name(name)?;
        let dir = attachments_dir(&self.home, id);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let unique = unique_name(&dir, &name);
        let path = dir.join(&unique);
        std::fs::write(&path, data).with_context(|| format!("write {}", path.display()))?;
        Ok(Attachment {
            name: unique,
            size: data.len() as u64,
            path: path.display().to_string(),
        })
    }

    /// The path of one attachment, or None when the name is not a plain
    /// file name there.
    pub fn attachment_path(&self, id: &str, name: &str) -> Option<PathBuf> {
        if !plain_file_name(name) {
            return None;
        }
        let path = attachments_dir(&self.home, id).join(name);
        path.is_file().then_some(path)
    }

    /// The model the conversation's next turn runs on; empty means the
    /// provider's default. A turn in flight finishes on the model it
    /// started with; the session then restarts onto the new one with the
    /// conversation carried over.
    pub fn set_model(&self, id: &str, model: &str) -> Result<()> {
        let model = model_id(model)?;
        let row = self.row(id)?;
        if row.status == "archived" {
            bail!("conversation {id} is archived");
        }
        self.store.chat_set_model(id, &model)
    }

    /// The reasoning effort the conversation's next turn runs with
    /// (`minimal` to `max`; empty or `default` for the provider's own).
    /// Like [`Manager::set_model`], a turn in flight finishes as it
    /// started; the session then restarts with the conversation carried
    /// over.
    pub fn set_reasoning(&self, id: &str, reasoning: &str) -> Result<()> {
        let reasoning = reasoning_effort(reasoning)?;
        let row = self.row(id)?;
        if row.status == "archived" {
            bail!("conversation {id} is archived");
        }
        self.store.chat_set_reasoning(id, &reasoning)
    }

    /// The owner's answer to the question the agent asked.
    pub fn answer(&self, id: &str, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            bail!("the answer is empty");
        }
        let rt = self
            .running(id)
            .ok_or_else(|| anyhow!("the agent is not waiting for an answer"))?;
        rt.answer(text.to_string())?;
        self.store
            .chat_add_message(id, "answer", text, &Value::Null)?;
        Ok(())
    }

    /// The owner's decision on the call the agent wants to make:
    /// `allow`, `always` or `deny` (with a reason to pass on).
    pub fn approve(&self, id: &str, decision: &str, reason: &str) -> Result<()> {
        let decision = match decision {
            "allow" | "yes" => Approval::Allow,
            "always" | "all" => Approval::Always,
            "deny" | "no" => Approval::Deny(reason.trim().to_string()),
            other => bail!("decision {other:?} is not allow, always or deny"),
        };
        let rt = self
            .running(id)
            .ok_or_else(|| anyhow!("the agent is not waiting for an approval"))?;
        rt.approve(decision)
    }

    pub fn cancel(&self, id: &str) -> Result<()> {
        self.row(id)?;
        if let Some(rt) = self.running(id) {
            rt.cancel();
        }
        Ok(())
    }

    pub fn set_auto_approve(&self, id: &str, on: bool) -> Result<()> {
        self.row(id)?;
        self.store.chat_set_auto_approve(id, on)?;
        if let Some(rt) = self.running(id) {
            rt.set_auto(on);
        }
        Ok(())
    }

    pub fn set_title(&self, id: &str, title: &str) -> Result<()> {
        if title.trim().is_empty() {
            bail!("the title is empty");
        }
        self.store.chat_set_title(id, title)
    }

    /// Puts the conversation away: the running turn stops, the session
    /// ends, the row stays (and its branch, for `pulse land`).
    pub fn archive(&self, id: &str) -> Result<()> {
        self.row(id)?;
        let rt = self
            .runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        if let Some(rt) = rt {
            let _ = rt.close();
        }
        self.store.chat_set_status(id, "archived", "")
    }

    /// Finishes the conversation from the owner's side: a running turn
    /// stops, what waited behind it is noted as unsent, and the row stands
    /// closed until a new message reopens it. The session and the branch
    /// stay as they are.
    pub fn close(&self, id: &str) -> Result<()> {
        let row = self.row(id)?;
        if row.status == "archived" {
            bail!("conversation {id} is archived");
        }
        if row.status == "closed" {
            return Ok(());
        }
        let record_now = match self.running(id) {
            Some(rt) => rt.finish_by_owner(),
            None => true,
        };
        if record_now {
            self.store.chat_add_message(
                id,
                "note",
                &Finished::Owner.line(),
                &json!({"closed": "owner"}),
            )?;
            self.store
                .chat_set_status(id, "closed", &Finished::Owner.note())?;
            self.deliver_report(&row, REPORT_FINISHED, "The owner finished it.");
        }
        Ok(())
    }

    /// Moves the conversation into a project's worktree. A running turn
    /// finishes where it is; the next one runs there.
    pub fn attach(&self, id: &str, project: &str) -> Result<Attached> {
        attach_workspace(&self.store, &self.home, id, project)
    }

    /// The live state: from the runtime while the hub holds one, from the
    /// row otherwise.
    pub fn live(&self, row: &Row) -> Live {
        match self.running(&row.id) {
            Some(rt) => rt.live(),
            None => Live {
                status: if row.status == "running" || row.status == "waiting" {
                    // The hub restarted under it: nothing runs now.
                    "idle".to_string()
                } else {
                    row.status.clone()
                },
                note: row.note.clone(),
                auto_approve: row.auto_approve,
                session_id: row.session_id.clone(),
                version: 1,
                ..Live::default()
            },
        }
    }

    pub fn artifacts_dir(&self, id: &str) -> PathBuf {
        artifacts_dir(&self.home, id)
    }

    /// The files in the artifacts folder, newest first.
    pub fn artifacts(&self, id: &str) -> Vec<Artifact> {
        list_artifacts(&artifacts_dir(&self.home, id))
    }

    /// The path of one artifact, or None when the name is not a plain file
    /// name there.
    pub fn artifact_path(&self, id: &str, name: &str) -> Option<PathBuf> {
        if !plain_file_name(name) {
            return None;
        }
        let path = artifacts_dir(&self.home, id).join(name);
        path.is_file().then_some(path)
    }

    /// What the branch holds, when the conversation has one.
    pub fn changes(&self, row: &Row) -> Option<Changes> {
        if row.branch.is_empty() || row.cwd.is_empty() || !Path::new(&row.cwd).is_dir() {
            return None;
        }
        let base = self
            .store
            .project(&row.project)
            .ok()
            .flatten()
            .map(|p| default_branch(&p))
            .unwrap_or_else(|| "main".to_string());
        Some(changes_in(&row.cwd, &row.project, &row.branch, &base))
    }

    /// The branch's diff against its base, committed and not, capped.
    pub fn diff(&self, row: &Row) -> Option<String> {
        let changes = self.changes(row)?;
        let mut out = String::new();
        let committed = git(&row.cwd, &["diff", &format!("{}...HEAD", changes.base)]);
        if !committed.trim().is_empty() {
            out.push_str(&committed);
        }
        let working = git(&row.cwd, &["diff", "HEAD"]);
        if !working.trim().is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str("# not committed yet\n");
            out.push_str(&working);
        }
        if out.len() > DIFF_LIMIT {
            let mut cut = DIFF_LIMIT;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            out.push_str("\n... (the diff goes on; open the worktree to see it all)\n");
        }
        Some(out)
    }

    /// Ends every session the hub holds (shutdown).
    pub fn close_all(&self) {
        let all: Vec<Arc<Runtime>> = self
            .runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for rt in all {
            let _ = rt.close();
        }
    }

    /// How many conversations have a turn in flight right now (a parked
    /// one waits on the owner and does not count).
    pub fn working(&self) -> usize {
        let all: Vec<Arc<Runtime>> = self
            .runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        all.iter()
            .filter(|rt| rt.live().status == "running")
            .count()
    }

    /// The hub is going down: stops every turn with a note that says so,
    /// ends every session (trajectory written, harness session finished)
    /// and waits up to `grace` for that to land. Returns how many
    /// conversations were still working, and whether everything settled.
    pub fn shutdown(&self, grace: Duration) -> (usize, bool) {
        let all: Vec<Arc<Runtime>> =
            std::mem::take(&mut *self.runtimes.lock().unwrap_or_else(|e| e.into_inner()))
                .into_values()
                .collect();
        if all.is_empty() {
            return (0, true);
        }
        let working = all
            .iter()
            .filter(|rt| rt.live().status == "running")
            .count();
        let deadline = Instant::now() + grace;
        let (tx, rx) = mpsc::channel();
        let handles: Vec<thread::JoinHandle<()>> = all.iter().map(|rt| rt.stop(true)).collect();
        thread::Builder::new()
            .name("chat-shutdown".into())
            .spawn(move || {
                for h in handles {
                    let _ = h.join();
                    let _ = tx.send(());
                }
            })
            .expect("spawn conversation shutdown");
        let mut ended = 0;
        while ended < all.len() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return (working, false);
            }
            match rx.recv_timeout(left) {
                Ok(()) => ended += 1,
                Err(_) => return (working, false),
            }
        }
        // The stopped turns write their notes after the session ends; give
        // them the rest of the grace.
        while all.iter().any(|rt| !rt.settled()) {
            if Instant::now() >= deadline {
                return (working, false);
            }
            thread::sleep(Duration::from_millis(20));
        }
        (working, true)
    }

    /// Conversations whose turn the last hub stopped to restart (the
    /// transcript ends on [`STOPPED_TO_RESTART_LINE`]) carry on by
    /// themselves: a note says the hub is back and the agent is told to
    /// pick the turn back up, with the restart hint naming the steps it
    /// had taken. A turn cut without a clean stop (a crash, a kill:
    /// [`CUT_RUNNING_LINE`]) or one that waited for the owner is left for
    /// the owner, so a turn that brings the hub down cannot loop. Returns
    /// the ids carried on.
    pub fn carry_on_restarted(&self) -> Vec<String> {
        let Ok(rows) = self.store.chat_list(false, i64::MAX) else {
            return Vec::new();
        };
        let mut carried = Vec::new();
        for row in rows {
            if row.status != "idle" || row.note != RESTART_NOTE || self.running(&row.id).is_some() {
                continue;
            }
            let messages = self.store.chat_messages(&row.id, 0).unwrap_or_default();
            let stopped = messages
                .last()
                .is_some_and(|m| m.role == "note" && m.content == STOPPED_TO_RESTART_LINE);
            if !stopped {
                continue;
            }
            if self
                .store
                .chat_add_message(&row.id, "note", CARRIED_ON_LINE, &Value::Null)
                .is_err()
            {
                continue;
            }
            self.runtime(&row).enqueue(CARRY_ON_PROMPT.to_string());
            carried.push(row.id);
        }
        carried
    }

    /// Rows that say a turn is running or parked while no runtime holds
    /// one are from a hub that is gone (a crash, a restart that could not
    /// wait): each goes idle with a note in the transcript that carries the
    /// cut turn's steps, and its harness session closes as `lost`. Sessions
    /// idle conversations kept open close as `ended`. Returns the ids whose
    /// turn was cut.
    pub fn recover(&self) -> Vec<String> {
        let Ok(rows) = self.store.chat_list(false, i64::MAX) else {
            return Vec::new();
        };
        let mut cut = Vec::new();
        for row in rows {
            if self.running(&row.id).is_some() {
                continue;
            }
            let open = row.status == "running" || row.status == "waiting";
            let sid = row.session_id.clone();
            let (session, last) = if sid.is_empty() {
                (None, None)
            } else {
                (
                    self.store.harness_session_head(&sid).ok().flatten(),
                    self.store.last_harness_step(&sid).ok().flatten(),
                )
            };
            if open {
                let to = last.as_ref().map_or(-1, |s| s.n);
                let from = turn_from(&self.store, &row.id, &sid);
                let extra = if sid.is_empty() {
                    json!({"restart": true})
                } else {
                    json!({"session": sid, "steps": [from, to], "restart": true})
                };
                let line = if row.status == "waiting" {
                    CUT_WAITING_LINE
                } else {
                    CUT_RUNNING_LINE
                };
                let _ = self.store.chat_add_message(&row.id, "note", line, &extra);
                let _ = self.store.chat_set_status(&row.id, "idle", RESTART_NOTE);
                cut.push(row.id.clone());
            }
            if let Some(s) = session
                && s.status == "running"
            {
                let (status, error) = if open {
                    ("lost", cut_error(&self.store, last.as_ref()))
                } else {
                    (EXIT_ENDED, String::new())
                };
                let path = Path::new(&self.home)
                    .join("harness")
                    .join(format!("{sid}.json"));
                let trajectory = if path.is_file() {
                    path.display().to_string()
                } else {
                    String::new()
                };
                let _ = self.store.finish_harness_session(
                    &sid,
                    status,
                    status,
                    "",
                    &error,
                    0.0,
                    &trajectory,
                );
            }
        }
        cut
    }
}

/// Where the cut turn's steps start: after the last turn the transcript
/// records for the session (`-1`: it was the session's first).
fn turn_from(store: &Store, id: &str, session_id: &str) -> i64 {
    if session_id.is_empty() {
        return -1;
    }
    store
        .chat_messages(id, 0)
        .unwrap_or_default()
        .iter()
        .filter(|m| m.extra["session"] == session_id)
        .filter_map(|m| m.extra["steps"][1].as_i64())
        .max()
        .unwrap_or(-1)
}

/// The error a cut turn's session closes with: what happened and the last
/// step, the way the runner closes a worker's sessions.
fn cut_error(store: &Store, last: Option<&pulse_store::HarnessStep>) -> String {
    let mut error = "the hub stopped while the agent was working".to_string();
    if let Some(step) = last {
        error.push_str(&format!("; last step {} {}", step.n, step.kind));
        if !step.command.trim().is_empty() {
            let cmd: String = step.command.trim().chars().take(120).collect();
            error.push_str(&format!(" `{}`", cmd.replace('\n', " ")));
        }
        if let (Ok(a), Ok(b)) = (
            chrono::DateTime::parse_from_rfc3339(&step.at),
            chrono::DateTime::parse_from_rfc3339(&store.now()),
        ) {
            error.push_str(&format!(" {} min ago", (b - a).num_minutes().max(0)));
        }
    }
    error
}

/// What the agent reads first when its previous turn here was cut: the
/// steps it took then may be gone from its context while their effects
/// stay in the workspace. `None` unless the row's note says a restart cut
/// the turn ([`RESTART_NOTE`], cleared as the turn starts).
fn restart_hint(store: &Store, id: &str, row_note: &str) -> Option<String> {
    if row_note != RESTART_NOTE {
        return None;
    }
    let mut hint = String::from(
        "PULSE restarted while you were working on your previous turn in this conversation, so that turn may be missing from your context; the workspace keeps whatever it changed.",
    );
    let messages = store.chat_messages(id, 0).unwrap_or_default();
    let cut = messages.iter().rev().find(|m| m.extra["restart"] == true);
    if let Some(m) = cut
        && let Some(sid) = m.extra["session"].as_str()
        && let (Some(from), Some(to)) = (m.extra["steps"][0].as_i64(), m.extra["steps"][1].as_i64())
        && let Ok(steps) = store.harness_steps(sid, from)
    {
        let lines: Vec<String> = steps
            .iter()
            .filter(|s| s.n <= to && s.depth == 0 && s.kind != "exit")
            .map(|s| {
                let what = if !s.command.trim().is_empty() {
                    s.command.trim().to_string()
                } else if !s.title.trim().is_empty() {
                    s.title.trim().to_string()
                } else {
                    s.content.trim().to_string()
                };
                format!("- {}: {}", s.kind, one_line(&what))
            })
            .collect();
        if !lines.is_empty() {
            let skipped = lines.len().saturating_sub(HINT_STEPS);
            hint.push_str(&format!(
                "\nThe steps you had taken{}:\n{}",
                if skipped > 0 {
                    format!(" (the last {HINT_STEPS} of {})", lines.len())
                } else {
                    String::new()
                },
                lines[skipped..].join("\n")
            ));
        }
    }
    hint.push_str(
        "\nCheck what is already there (git status, the files, the artifacts folder) before redoing any of it.",
    );
    Some(hint)
}

fn title_from(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut title = String::new();
    for w in words {
        if title.chars().count() + w.chars().count() + 1 > 60 {
            title.push_str("...");
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(w);
    }
    if title.is_empty() {
        "New conversation".to_string()
    } else {
        title
    }
}

/// `PULSE_HOME/work/chat/<id>`: the conversation's folder (on the bulk
/// disk when one is attached, like the job clones).
pub fn chat_dir(home: &str, id: &str) -> PathBuf {
    Path::new(home).join("work").join("chat").join(id)
}

pub fn artifacts_dir(home: &str, id: &str) -> PathBuf {
    chat_dir(home, id).join("artifacts")
}

/// Where the files the owner attached to messages live.
pub fn attachments_dir(home: &str, id: &str) -> PathBuf {
    chat_dir(home, id).join("attachments")
}

/// Whether `name` names a file in a folder and nothing more: no path
/// separators, no `.`/`..`, no control characters.
fn plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.chars().any(|c| c.is_control())
}

/// The file name an upload is kept under: the browser's name trimmed to a
/// plain file name of at most 120 characters, spaces kept, path parts and
/// control characters refused.
fn attachment_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.chars().count() > 120 {
        bail!("Invalid file name: longer than 120 characters");
    }
    if !plain_file_name(name) || name.starts_with('.') {
        bail!("Invalid file name: {name:?} is not a plain file name");
    }
    Ok(name.to_string())
}

/// `name` when nothing in `dir` has it, else `stem-2.ext`, `stem-3.ext`...
fn unique_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem}-{n}{ext}"))
        .find(|candidate| !dir.join(candidate).exists())
        .unwrap_or_else(|| name.to_string())
}

/// A model id as the provider names them: letters, digits and `. _ - : /`,
/// up to 80 characters; empty for the default.
fn model_id(model: &str) -> Result<String> {
    let model = model.trim();
    if model.chars().count() > 80
        || !model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'))
    {
        bail!(
            "Invalid model: {model:?} is not a model id (letters, digits and . _ - : /, up to 80 characters)"
        );
    }
    Ok(model.to_string())
}

/// A reasoning effort the owner may pick for a conversation: one of
/// [`pulse_provider::REASONING_EFFORTS`], or empty (`default` reads as
/// empty) for the provider's own default.
fn reasoning_effort(effort: &str) -> Result<String> {
    let effort = effort.trim();
    if effort.is_empty() || effort == "default" {
        return Ok(String::new());
    }
    if !pulse_provider::REASONING_EFFORTS.contains(&effort) {
        bail!(
            "Invalid reasoning: {effort:?} is not an effort (one of {}, or default)",
            pulse_provider::REASONING_EFFORTS.join(", ")
        );
    }
    Ok(effort.to_string())
}

/// The message the agent gets: the owner's words, then where the files
/// they attached are.
fn message_with_attachments(text: &str, files: &[Attachment]) -> String {
    if files.is_empty() {
        return text.to_string();
    }
    let mut out = String::from(text);
    out.push_str(
        "\n\nAttached by the owner - files on this machine, outside the branch; read them with your tools (an image is a file to inspect with a tool, not something you see):\n",
    );
    for f in files {
        out.push_str(&format!("- {} ({})\n", f.path, size_words(f.size)));
    }
    out
}

fn size_words(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn scratch_dir(home: &str, id: &str) -> PathBuf {
    chat_dir(home, id).join("scratch")
}

fn default_branch(p: &Project) -> String {
    if p.default_branch.is_empty() {
        "main".to_string()
    } else {
        p.default_branch.clone()
    }
}

fn read_trajectory(home: &str, session_id: &str) -> Option<Trajectory> {
    let path = Path::new(home)
        .join("harness")
        .join(format!("{session_id}.json"));
    let data = std::fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

/// PATH with the running binary's folder first, so the agent's `pulse` is
/// this one.
fn path_with_pulse() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.display().to_string();
    let path = std::env::var("PATH").unwrap_or_default();
    if path.split(':').any(|p| p == dir) {
        return None;
    }
    Some(if path.is_empty() {
        dir
    } else {
        format!("{dir}:{path}")
    })
}

fn git(cwd: &str, args: &[&str]) -> String {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

fn git_ok(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Makes (or finds) the conversation's worktree of `project` and records
/// it on the row. Used by the manager and by `pulse chat attach` from
/// inside the agent's own commands.
pub fn attach_workspace(store: &Store, home: &str, id: &str, project: &str) -> Result<Attached> {
    let row = store
        .chat_get(id)?
        .ok_or_else(|| anyhow!("no such conversation: {id}"))?;
    if row.status == "archived" {
        bail!("conversation {id} is archived");
    }
    let p = store
        .project(project)?
        .ok_or_else(|| anyhow!("no such project: {project}"))?;
    if p.status != "active" {
        bail!("project {project} is {}", p.status);
    }
    let root = Path::new(&p.path);
    if !root.join(".git").exists() {
        bail!("project {project} at {} is not a git checkout", p.path);
    }
    let base = default_branch(&p);
    let dir = chat_dir(home, id).join(&p.slug);
    let branch = format!("{BRANCH_PREFIX}{id}");
    let created = if dir.join(".git").exists() {
        false
    } else {
        std::fs::create_dir_all(chat_dir(home, id))
            .with_context(|| format!("create {}", chat_dir(home, id).display()))?;
        if dir.exists() {
            // A folder left behind without a worktree: git refuses to add
            // over it unless it is empty.
            let _ = std::fs::remove_dir(&dir);
        }
        let dir_s = dir.display().to_string();
        let have_branch = git_ok(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok();
        if have_branch {
            git_ok(root, &["worktree", "add", &dir_s, &branch])?;
        } else {
            git_ok(root, &["worktree", "add", "-b", &branch, &dir_s, &base])?;
        }
        true
    };
    let cwd = dir.display().to_string();
    std::fs::create_dir_all(artifacts_dir(home, id)).ok();
    store.chat_set_workspace(id, &p.slug, &cwd, &branch)?;
    Ok(Attached {
        id: id.to_string(),
        project: p.slug,
        cwd,
        branch,
        base,
        created,
    })
}

fn changes_in(cwd: &str, project: &str, branch: &str, base: &str) -> Changes {
    let commits = git(cwd, &["rev-list", "--count", &format!("{base}..HEAD")])
        .trim()
        .parse::<i64>()
        .unwrap_or(0);
    let stat = git(cwd, &["diff", "--stat", &format!("{base}...HEAD")])
        .trim_end()
        .to_string();
    let dirty: Vec<String> = git(cwd, &["status", "--porcelain"])
        .lines()
        .map(str::to_string)
        .filter(|l| !l.is_empty())
        .collect();
    Changes {
        project: project.to_string(),
        branch: branch.to_string(),
        base: base.to_string(),
        cwd: cwd.to_string(),
        commits,
        stat,
        dirty,
    }
}

fn list_artifacts(dir: &Path) -> Vec<Artifact> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(std::time::SystemTime, Artifact)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let modified = meta.modified().ok()?;
            let when: chrono::DateTime<chrono::Utc> = modified.into();
            Some((
                modified,
                Artifact {
                    name,
                    size: meta.len(),
                    modified: when.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                },
            ))
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    out.into_iter().map(|(_, a)| a).collect()
}

/// The brief the agent reads with the first message of each session.
pub fn brief_text(row: &Row, projects: &[Project], artifacts: &Path) -> String {
    let mut b = String::new();
    if row.parent.is_empty() {
        b.push_str(&format!(
            "You are working with the owner of PULSE in a conversation from the hub (conversation {}). \
             Keep going until the request is done; use the question tool only for a decision that is the owner's. \
             Report progress in short plain words, and say what you did when you finish. \
             When the request is complete (or the owner says it is), call the finish tool with a one-line summary, then give your closing reply: the conversation is marked finished, and the owner's next message reopens it.\n",
            row.id
        ));
    } else {
        b.push_str(&format!(
            "You are working in sub-conversation {} of conversation {}: the agent of that conversation started you for one part of its work, and the owner of PULSE can read and write here too. \
             Your brief follows below. Every reply you make is delivered to the conversation that started you, so reply when there is something to report and keep replies to the point; a message marked as from that conversation is its agent steering you. \
             Use the question tool only for a decision that is the owner's. \
             When the part is done, call the finish tool with a one-line summary, then give your closing reply: that summary and reply are your report.\n",
            row.id, row.parent
        ));
    }
    b.push_str(&format!(
        "Files meant for the owner to look at (screenshots, reports, HTML pages, images, CSVs) go in ${ARTIFACTS_ENV} ({}); the hub shows that folder beside this conversation.\n",
        artifacts.display()
    ));
    b.push_str("The `pulse` command is on PATH.\n");
    b.push_str(
        "The owner's words are PULSE's own - RSI, lead, board, ballot, evolve, experiment, land, brain, goal - and the fleet has already written about them. \
         Before asking what a word or a thing means, look it up: `pulse ask --all \"...\"` answers from the shared board of every project (add --no-model for the matching posts alone), \
         `pulse guide` explains how PULSE works, `pulse lead list` names the standing leads and their roles, `pulse status` shows the fleet, `pulse board --project SLUG` reads one project's board. \
         Ask the owner only for what none of these can tell.\n",
    );
    b.push_str(
        "A part of the work that can run on its own for a while (a review, a check, a second thing to build) can go to a sub-conversation: the chat tool with action start, a title and a full brief, starts one in your workspace, with its own page in the hub. \
         Its replies reach you as messages from it between your turns; action read shows its state and last replies, action send steers it or answers its question. \
         Reply or end your turn while it works - you are not blocked on it. A sub-conversation is for work the owner may want to watch or steer; a quick helper is the subagent tool.\n",
    );
    if !row.parent.is_empty() {
        b.push_str(&format!(
            "Workspace: you work in {}, the same folder{} as the conversation that started you, on its branch{}: commit on that branch and leave landing to it. \
             Do not run `pulse chat attach`; this sub-conversation stays where its parent is.\n",
            row.cwd,
            if row.project.is_empty() {
                String::new()
            } else {
                format!(" (project {})", row.project)
            },
            if row.branch.is_empty() {
                String::new()
            } else {
                format!(" {}", row.branch)
            }
        ));
        return b.trim_end().to_string();
    }
    if !row.project.is_empty() {
        let p = projects.iter().find(|p| p.slug == row.project);
        let (name, base, path) = p
            .map(|p| (p.name.clone(), default_branch(p), p.path.clone()))
            .unwrap_or_else(|| (row.project.clone(), "main".to_string(), String::new()));
        b.push_str(&format!(
            "Workspace: {} is your own git worktree of project {} ({}) on branch {}, started from {}. \
             Commit your work on this branch. The owner's checkout{} is not yours to edit.\n",
            row.cwd,
            row.project,
            name,
            row.branch,
            base,
            if path.is_empty() {
                String::new()
            } else {
                format!(" at {path}")
            }
        ));
        b.push_str(&format!(
            "When the work is complete and verified, or when the owner asks, land it with `pulse land --project {} --branch {}`: the project's checks run first, then it merges into {}. \
             Other projects: `pulse chat attach SLUG` moves this conversation into that project's worktree after your reply.\n",
            row.project, row.branch, base
        ));
    } else {
        b.push_str(
            "This conversation is not attached to a project yet. Decide what the request is about and act:\n",
        );
        if projects.is_empty() {
            b.push_str("- No projects are registered; help from here, or tell the owner to run `pulse add project`.\n");
        } else {
            b.push_str("- The projects PULSE manages (slug: name, checkout):\n");
            for p in projects {
                b.push_str(&format!("  - {}: {}, {}\n", p.slug, p.name, p.path));
            }
        }
        b.push_str(
            "- Work inside one project's code: run `pulse chat attach SLUG`, then end your reply; the conversation moves into your own worktree of that project (its own branch) and carries on there in the next turn by itself.\n\
             - Work for the fleet rather than for you now, or work across several projects: queue it with `pulse task new --project SLUG --title \"...\" --brief \"what and why\"`, one task per project, and say which tasks you made.\n\
             - A question about a project or about PULSE: answer it from the shared board and the store - `pulse ask --all \"...\"`, `pulse status`, `pulse task list --project SLUG`, `pulse goal show --project SLUG`, `pulse ask --project SLUG \"...\"` - and the checkouts (read them where they are; do not edit them).\n\
             - Ask with the question tool only when the project really cannot be told from the request and the board does not say.\n",
        );
        b.push_str(&format!("Scratch space: {}.\n", row.cwd));
    }
    b.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_store::Options;
    use std::io::Write;

    fn store(home: &Path) -> Store {
        pulse_testutil::neutralize_runner_environment();
        Store::open(Options {
            home: home.display().to_string(),
            create: true,
            now: "2026-10-19T10:00:00Z".into(),
            ..Options::default()
        })
        .expect("store")
    }

    fn script(home: &Path, replies: serde_json::Value) -> PathBuf {
        let path = home.join("script.json");
        std::fs::write(&path, json!({"replies": replies}).to_string()).expect("script");
        path
    }

    fn wait_idle(m: &Manager, id: &str) -> Row {
        for _ in 0..400 {
            let row = m.row(id).expect("row");
            let live = m.live(&row);
            if (live.status == "idle" || live.status == "ended") && live.queued.is_empty() {
                return row;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("the conversation never went idle");
    }

    #[test]
    fn resume_after_provider_error_gates() {
        let home = tempfile::tempdir().expect("tempdir");
        let s = store(home.path());
        // A transport provider_error (a failed send or a broken reply body) resumes.
        assert!(resume_after_provider_error(
            &s,
            "provider_error",
            "copilot: error sending request for url (https://api.githubcopilot.test/chat)"
        ));
        assert!(resume_after_provider_error(
            &s,
            "provider_error",
            "copilot: error decoding response body"
        ));
        // Budget stops, questions and non-transport errors wait for a human as before.
        assert!(!resume_after_provider_error(&s, "budget", "out of credits"));
        assert!(!resume_after_provider_error(
            &s,
            "provider_error",
            "copilot: HTTP 429: rate limited"
        ));
        // chat.resume_after_provider_error_secs = 0 turns the resume off.
        s.set_setting("test", "chat.resume_after_provider_error_secs", "0")
            .expect("set");
        assert!(!resume_after_provider_error(
            &s,
            "provider_error",
            "copilot: error sending request for url (https://api.githubcopilot.test/chat)"
        ));
    }

    fn wait_pending(m: &Manager, id: &str) -> Pending {
        for _ in 0..400 {
            let row = m.row(id).expect("row");
            if let Some(p) = m.live(&row).pending {
                return p;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("the agent never asked");
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Keeps the developer's global git config (signing, hooks) out of
    /// the test repositories and out of the agent's commands.
    struct PlainGit;

    impl PlainGit {
        fn set() -> Self {
            unsafe {
                std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
                std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
            }
            PlainGit
        }
    }

    impl Drop for PlainGit {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("GIT_CONFIG_NOSYSTEM");
                std::env::remove_var("GIT_CONFIG_GLOBAL");
            }
        }
    }

    fn project_repo(home: &Path, store: &Store, slug: &str) -> PathBuf {
        let dir = home.join(format!("{slug}-checkout"));
        std::fs::create_dir_all(&dir).unwrap();
        git_in(&dir, &["init", "-q", "-b", "main"]);
        git_in(&dir, &["config", "user.email", "t@example.com"]);
        git_in(&dir, &["config", "user.name", "t"]);
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        git_in(&dir, &["add", "."]);
        git_in(&dir, &["commit", "-q", "-m", "first"]);
        store
            .conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, default_branch, status, added_at) VALUES (?, ?, ?, 'main', 'active', ?)",
                rusqlite::params![slug, slug, dir.display().to_string(), store.now()],
            )
            .unwrap();
        dir
    }

    #[test]
    fn a_conversation_runs_a_turn_and_keeps_its_transcript() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(
            home.path(),
            json!([{"content": "Hello from the agent."}, {"content": "Second reply."}]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let row = m
            .create("", "", true, "say hello please", "owner")
            .expect("create");
        assert_eq!(row.title, "say hello please");
        assert!(row.project.is_empty());
        let row = wait_idle(&m, &row.id);
        assert!(!row.session_id.is_empty(), "a harness session was opened");
        assert!(
            row.cwd.ends_with("scratch"),
            "a scratch folder was made: {}",
            row.cwd
        );
        let msgs = s.chat_messages(&row.id, 0).unwrap();
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant"], "{msgs:?}");
        assert_eq!(msgs[1].content, "Hello from the agent.");
        assert_eq!(msgs[1].extra["session"], row.session_id);
        // The harness session carries the steps; the brief went with the
        // first message only.
        let (session, steps) = s.harness_session(&row.session_id).unwrap();
        assert!(session.is_some());
        // The reply names its step range: every step of the session so far.
        assert_eq!(msgs[1].extra["steps"][0], -1, "{:?}", msgs[1].extra);
        assert_eq!(
            msgs[1].extra["steps"][1].as_i64().unwrap(),
            steps.last().map(|st| st.n).unwrap_or(-1),
            "{:?}",
            msgs[1].extra
        );
        assert!(steps.iter().any(|st| st.kind == "assistant"));
        m.send(&row.id, "and again").expect("send");
        let row = wait_idle(&m, &row.id);
        let msgs = s.chat_messages(&row.id, 0).unwrap();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[3].content, "Second reply.");
        assert_eq!(row.turns, 2);
        m.archive(&row.id).expect("archive");
        assert_eq!(m.row(&row.id).unwrap().status, "archived");
        assert!(m.send(&row.id, "more").is_err());
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    #[test]
    fn a_model_switch_restarts_the_session_and_attachments_ride_with_the_message() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(
            home.path(),
            json!([{"content": "First."}, {"content": "Second."}, {"content": "Third."}]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        // The model is not pinned: the row's model is the one the session runs on.
        let m =
            Manager::new(&home.path().display().to_string(), s.clone()).with_provider("fake", "");
        assert!(
            m.create_with("", "", "bad model!", "", true, "hi", "owner")
                .is_err(),
            "a model id has no spaces or punctuation beyond . _ - : /"
        );
        let row = m
            .create_with("", "", "fake-1", "", true, "say hello please", "owner")
            .expect("create");
        assert_eq!(row.model, "fake-1");
        let row = wait_idle(&m, &row.id);
        let first_session = row.session_id.clone();
        assert_eq!(
            row.model, "fake-1",
            "the session resolved to the chosen model"
        );

        // The same model: the session stays.
        m.send(&row.id, "again").unwrap();
        let row = wait_idle(&m, &row.id);
        assert_eq!(row.session_id, first_session);

        // Another model: the next turn runs in a new session, the
        // conversation carried over.
        assert!(m.set_model(&row.id, "no spaces here").is_err());
        m.set_model(&row.id, "fake-2").expect("set model");
        assert_eq!(m.row(&row.id).unwrap().model, "fake-2");

        // An attachment: saved under its name, a second one numbered, the
        // message carries the paths to the agent and the names in its record.
        let a = m
            .save_attachment(&row.id, "notes.txt", b"remember the milk")
            .expect("save");
        assert_eq!(a.name, "notes.txt");
        assert_eq!(a.size, 17);
        assert!(Path::new(&a.path).is_file());
        let b = m
            .save_attachment(&row.id, "notes.txt", b"second")
            .expect("save");
        assert_eq!(b.name, "notes-2.txt");
        assert!(m.save_attachment(&row.id, "../evil.txt", b"x").is_err());
        assert!(m.save_attachment(&row.id, ".hidden", b"x").is_err());
        assert!(m.save_attachment(&row.id, "empty.txt", b"").is_err());
        assert!(m.attachment_path(&row.id, "notes.txt").is_some());
        assert!(m.attachment_path(&row.id, "missing.txt").is_none());
        assert!(
            m.send_with(&row.id, "read this", &["missing.txt".to_string()])
                .is_err(),
            "a name that was not uploaded is refused"
        );
        m.send_with(&row.id, "", std::slice::from_ref(&a.name))
            .expect("send");
        let row = wait_idle(&m, &row.id);
        assert_ne!(
            row.session_id, first_session,
            "the model switch restarted the session"
        );
        assert_eq!(row.model, "fake-2");
        let msgs = s.chat_messages(&row.id, 0).unwrap();
        let last_user = msgs.iter().rev().find(|m| m.role == "user").unwrap();
        assert_eq!(last_user.content, "Look at the attached file.");
        assert_eq!(last_user.extra["attachments"][0]["name"], "notes.txt");
        assert_eq!(last_user.extra["attachments"][0]["size"], 17);
        let traj = read_trajectory(&home.path().display().to_string(), &row.session_id)
            .expect("trajectory");
        let sent = traj
            .messages
            .iter()
            .filter(|m| m.content.contains("Look at the attached file."))
            .map(|m| m.content.clone())
            .next_back()
            .expect("the agent got the message");
        assert!(sent.contains(&a.path), "{sent}");
        assert!(sent.contains("Attached by the owner"), "{sent}");
        // The fake's script restarted with the new provider instance.
        assert_eq!(msgs.last().unwrap().content, "First.");
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    #[test]
    fn inline_images_validate_save_and_read_back() {
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD;
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(home.path(), json!([{"content": "Saw it."}]));
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m =
            Manager::new(&home.path().display().to_string(), s.clone()).with_provider("fake", "");
        let row = m
            .create_with("", "", "fake", "", true, "watch out", "owner")
            .expect("create");
        let row = wait_idle(&m, &row.id);
        let png = b"\x89PNG fake bytes".to_vec();
        let image = ImageAttachment {
            mime: "image/png".into(),
            name: "".into(),
            data: STANDARD.encode(&png),
        };
        // Bad shapes are refused before anything is stored.
        for bad in [
            ImageAttachment {
                mime: "image/gif".into(),
                ..image.clone()
            },
            ImageAttachment {
                data: "not base64!!!".into(),
                ..image.clone()
            },
            ImageAttachment {
                data: STANDARD.encode(vec![0u8; IMAGE_LIMIT + 1]),
                ..image.clone()
            },
        ] {
            assert!(
                m.send_with_images(&row.id, "see", &[], std::slice::from_ref(&bad))
                    .is_err()
            );
        }
        // A good image rides the message: saved under a numbered name and
        // read back with its mime.
        m.send_with_images(&row.id, "look", &[], std::slice::from_ref(&image))
            .expect("send image");
        let row = wait_idle(&m, &row.id);
        let msgs = s.chat_messages(&row.id, 0).unwrap();
        let user = msgs.iter().rev().find(|m| m.role == "user").unwrap();
        assert_eq!(user.content, "look");
        assert_eq!(user.extra["images"][0]["name"], "image-1.png");
        assert_eq!(user.extra["images"][0]["mime"], "image/png");
        assert_eq!(
            user.extra["images"][0]["path"],
            format!("image/{}/0", row.id)
        );
        let (mime, data) = m
            .image(&row.id, user.n, 0)
            .expect("image")
            .expect("present");
        assert_eq!(mime, "image/png");
        assert_eq!(data, png);
        assert!(m.image(&row.id, user.n, 1).expect("none").is_none());
        assert!(m.image(&row.id, 9999, 0).expect("none").is_none());
        // The model saw the image: it rides the user message in the
        // trajectory as `images`, base64 as it arrived.
        let traj = read_trajectory(&home.path().display().to_string(), &row.session_id)
            .expect("trajectory");
        let sent = traj
            .messages
            .iter()
            .rfind(|m| m.content == "look")
            .expect("the agent got the message");
        assert_eq!(sent.images.len(), 1, "{sent:?}");
        assert_eq!(sent.images[0].media_type, "image/png");
        assert_eq!(sent.images[0].data, STANDARD.encode(&png));
        // Text-only messages carry none.
        assert!(
            traj.messages
                .iter()
                .filter(|m| m.content != "look")
                .all(|m| m.images.is_empty())
        );
        // A renamed image keeps its own name.
        let named = ImageAttachment {
            name: "shot.webp".into(),
            mime: "image/webp".into(),
            data: STANDARD.encode(b"RIFF webp"),
        };
        m.send_with_images(&row.id, "", &[], std::slice::from_ref(&named))
            .expect("send named");
        let row = wait_idle(&m, &row.id);
        let msgs = s.chat_messages(&row.id, 0).unwrap();
        let user = msgs.iter().rev().find(|m| m.role == "user").unwrap();
        assert_eq!(user.content, "Look at the attached file.");
        assert_eq!(user.extra["images"][0]["name"], "shot.webp");
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    #[test]
    fn a_question_parks_the_turn_until_the_owner_answers() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(
            home.path(),
            json!([
                {"tool": "question", "args": {"question": "Red or blue?", "choices": ["red", "blue"]}},
                {"content": "Blue it is."}
            ]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let row = m
            .create("colours", "", true, "pick a colour", "owner")
            .unwrap();
        let pending = wait_pending(&m, &row.id);
        match &pending {
            Pending::Question {
                question, choices, ..
            } => {
                assert_eq!(question, "Red or blue?");
                assert_eq!(choices, &["red", "blue"]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(m.live(&m.row(&row.id).unwrap()).status, "waiting");
        assert_eq!(m.row(&row.id).unwrap().status, "waiting");
        assert!(
            m.approve(&row.id, "allow", "").is_err(),
            "no approval waits"
        );
        m.answer(&row.id, "blue").expect("answer");
        let row = wait_idle(&m, &row.id);
        let msgs = s.chat_messages(&row.id, 0).unwrap();
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(
            roles,
            ["user", "question", "answer", "assistant"],
            "{msgs:?}"
        );
        assert_eq!(msgs[1].extra["choices"][1], "blue");
        assert_eq!(msgs[3].content, "Blue it is.");
        m.archive(&row.id).unwrap();
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    #[test]
    fn approvals_park_when_auto_is_off_and_pass_when_on() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(
            home.path(),
            json!([
                {"tool": "write", "args": {"path": "note.txt", "content": "hi\n"}},
                {"content": "Written."},
                {"tool": "write", "args": {"path": "two.txt", "content": "hi\n"}},
                {"content": "Written again."}
            ]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let row = m.create("", "", false, "write a note", "owner").unwrap();
        let pending = wait_pending(&m, &row.id);
        match &pending {
            Pending::Approval { tool, .. } => assert_eq!(tool, "write"),
            other => panic!("{other:?}"),
        }
        assert!(m.answer(&row.id, "x").is_err(), "no question waits");
        m.approve(&row.id, "deny", "not that file").expect("deny");
        let row = wait_idle(&m, &row.id);
        assert!(!Path::new(&row.cwd).join("note.txt").exists());
        let msgs = s.chat_messages(&row.id, 0).unwrap();
        let approval = msgs
            .iter()
            .find(|m| m.role == "approval")
            .expect("approval row");
        assert_eq!(approval.extra["decision"], "deny");
        assert_eq!(approval.extra["reason"], "not that file");
        // Auto on: the next write runs without a question.
        m.set_auto_approve(&row.id, true).unwrap();
        m.send(&row.id, "write another").unwrap();
        let row = wait_idle(&m, &row.id);
        assert!(
            Path::new(&row.cwd).join("two.txt").exists(),
            "{:?}",
            row.cwd
        );
        assert!(m.live(&row).auto_approve);
        m.archive(&row.id).unwrap();
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    #[test]
    fn a_project_conversation_works_in_its_own_worktree_and_reports_changes() {
        let _env = pulse_testutil::env_lock();
        let _git = PlainGit::set();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let checkout = project_repo(home.path(), &s, "demo");
        let script = script(
            home.path(),
            json!([
                {"command": "printf 'more\\n' >> README.md && git add README.md && git -c user.email=a@b.c -c user.name=a commit -q -m 'chat change' && echo done"},
                {"content": "Committed on my branch."}
            ]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let row = m
            .create("readme", "demo", true, "add a line to the readme", "owner")
            .unwrap();
        let row = wait_idle(&m, &row.id);
        assert_eq!(row.project, "demo");
        assert_eq!(row.branch, format!("pulse/chat-{}", row.id));
        assert!(row.cwd.ends_with("/demo"), "{}", row.cwd);
        assert!(Path::new(&row.cwd).join(".git").exists(), "a worktree");
        let changes = m.changes(&row).expect("changes");
        assert_eq!(changes.commits, 1, "{changes:?}");
        assert!(changes.stat.contains("README.md"), "{changes:?}");
        assert!(changes.dirty.is_empty());
        let diff = m.diff(&row).expect("diff");
        assert!(diff.contains("+more"), "{diff}");
        // The owner's checkout did not move.
        assert_eq!(
            std::fs::read_to_string(checkout.join("README.md")).unwrap(),
            "hello\n"
        );
        // Artifacts: a file in the folder shows up; names with slashes do not resolve.
        let art = m.artifacts_dir(&row.id);
        std::fs::File::create(art.join("report.md"))
            .unwrap()
            .write_all(b"# r\n")
            .unwrap();
        let list = m.artifacts(&row.id);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "report.md");
        assert!(m.artifact_path(&row.id, "report.md").is_some());
        assert!(m.artifact_path(&row.id, "../report.md").is_none());
        m.archive(&row.id).unwrap();
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    #[test]
    fn a_cargo_project_conversation_builds_into_the_shared_target() {
        let _env = pulse_testutil::env_lock();
        let _git = PlainGit::set();
        // The test process may run under an owner's own exports; the rule
        // under test is what a conversation gets when it has none.
        let _clear: Vec<_> = [
            "CARGO_TARGET_DIR",
            "CARGO_BUILD_JOBS",
            "CARGO_INCREMENTAL",
            "CARGO_PROFILE_DEV_DEBUG",
        ]
        .iter()
        .map(|k| pulse_testutil::unsetenv(k))
        .collect();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let checkout = project_repo(home.path(), &s, "demo");
        std::fs::write(
            checkout.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        git_in(&checkout, &["add", "Cargo.toml"]);
        git_in(&checkout, &["commit", "-q", "-m", "cargo"]);
        s.set_setting("test", "daemon.build_jobs", "3").unwrap();
        let script = script(
            home.path(),
            json!([
                {"command": "env | grep '^CARGO_' | sort > env.txt; readlink target > link.txt; echo done"},
                {"content": "Printed."}
            ]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let row = m
            .create("env", "demo", true, "show the build env", "owner")
            .unwrap();
        let row = wait_idle(&m, &row.id);
        let cwd = Path::new(&row.cwd);
        let want = home.path().join("cache/cargo-target/demo/chat");
        let env = std::fs::read_to_string(cwd.join("env.txt")).expect("env.txt");
        for line in [
            format!("CARGO_TARGET_DIR={}", want.display()),
            "CARGO_BUILD_JOBS=3".to_string(),
            "CARGO_INCREMENTAL=0".to_string(),
            "CARGO_PROFILE_DEV_DEBUG=0".to_string(),
        ] {
            assert!(
                env.lines().any(|l| l == line),
                "{line} missing from:\n{env}"
            );
        }
        let link = std::fs::read_to_string(cwd.join("link.txt")).expect("link.txt");
        assert_eq!(link.trim(), want.display().to_string());
        assert!(want.is_dir(), "the shared target exists");
        // The link stays out of the Changes tab: a worktree's exclude file
        // lives under the owner's .git, not its own.
        let status = git(&row.cwd, &["status", "--porcelain"]);
        assert!(
            !status.lines().any(|l| l.ends_with("target")),
            "target listed by git status:\n{status}"
        );
        assert!(status.contains("env.txt"), "{status}");
        m.archive(&row.id).unwrap();
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    #[test]
    fn attach_moves_the_conversation_after_the_turn_and_carries_on() {
        let _env = pulse_testutil::env_lock();
        let _git = PlainGit::set();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        project_repo(home.path(), &s, "demo");
        let script = script(
            home.path(),
            json!([
                {"content": "I will work in demo."},
                {"content": "Now in the worktree."}
            ]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let row = m
            .create("", "", true, "fix the demo readme", "owner")
            .unwrap();
        let before = wait_idle(&m, &row.id);
        assert!(before.project.is_empty());
        let first_session = before.session_id.clone();
        // What `pulse chat attach demo` does between turns.
        let attached = m.attach(&row.id, "demo").expect("attach");
        assert!(attached.created);
        assert_eq!(attached.branch, format!("pulse/chat-{}", row.id));
        let again = m.attach(&row.id, "demo").expect("attach again");
        assert!(!again.created);
        // The next message runs in the worktree, in a new session that
        // carries the earlier messages (the fake's script restarts with
        // the new provider instance, so the reply is the first one again).
        m.send(&row.id, "go on").unwrap();
        let row = wait_idle(&m, &row.id);
        assert_eq!(row.project, "demo");
        assert_ne!(row.session_id, first_session);
        let msgs = s.chat_messages(&row.id, 0).unwrap();
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(
            roles,
            ["user", "assistant", "user", "assistant"],
            "{msgs:?}"
        );
        assert_eq!(msgs[3].extra["session"], row.session_id);
        let (session, steps) = s.harness_session(&row.session_id).unwrap();
        assert_eq!(session.expect("session row").project, "demo");
        assert!(!steps.is_empty());
        let traj = read_trajectory(&home.path().display().to_string(), &row.session_id)
            .expect("trajectory on disk");
        let carried = traj.messages.iter().any(|m| {
            serde_json::to_string(m)
                .unwrap()
                .contains("fix the demo readme")
        });
        assert!(carried, "the first message came along into the new session");
        m.archive(&row.id).unwrap();
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    #[test]
    fn attach_during_a_turn_moves_after_it_and_carries_on_by_itself() {
        let _env = pulse_testutil::env_lock();
        let _git = PlainGit::set();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        project_repo(home.path(), &s, "demo");
        // Each session replays the script: a question, then a reply.
        let script = script(
            home.path(),
            json!([
                {"tool": "question", "args": {"question": "Which project?", "choices": ["demo"]}},
                {"content": "Carrying on."}
            ]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let row = m.create("", "", true, "fix the readme", "owner").unwrap();
        wait_pending(&m, &row.id);
        let first_session = m.row(&row.id).unwrap().session_id;
        // The agent ran `pulse chat attach demo` mid-turn, then answered.
        m.attach(&row.id, "demo").expect("attach");
        m.answer(&row.id, "demo").unwrap();
        // The turn ends in the scratch folder; the runtime notes the move
        // and starts the next turn in the worktree by itself, where the
        // replayed script asks again.
        let pending = wait_pending(&m, &row.id);
        assert!(matches!(pending, Pending::Question { .. }));
        let row_now = m.row(&row.id).unwrap();
        assert_ne!(
            row_now.session_id, first_session,
            "a new session in the worktree"
        );
        m.answer(&row.id, "demo").unwrap();
        let row = wait_idle(&m, &row.id);
        let msgs = s.chat_messages(&row.id, 0).unwrap();
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(
            roles,
            [
                "user",
                "question",
                "answer",
                "assistant",
                "note",
                "question",
                "answer",
                "assistant"
            ],
            "{msgs:?}"
        );
        assert!(
            msgs[4].content.starts_with("Moved into project demo"),
            "{}",
            msgs[4].content
        );
        assert_eq!(msgs[4].extra["branch"], format!("pulse/chat-{}", row.id));
        assert_eq!(msgs[7].extra["session"], row.session_id);
        assert_eq!(row.turns, 2);
        m.archive(&row.id).unwrap();
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    #[test]
    fn attachment_names_and_model_ids_are_checked() {
        assert_eq!(attachment_name(" notes.txt ").unwrap(), "notes.txt");
        assert_eq!(attachment_name("my file.PNG").unwrap(), "my file.PNG");
        assert!(attachment_name("../evil").is_err());
        assert!(attachment_name("a/b.txt").is_err());
        assert!(attachment_name("a\\b.txt").is_err());
        assert!(attachment_name(".env").is_err());
        assert!(attachment_name("").is_err());
        assert!(attachment_name(&"x".repeat(121)).is_err());
        assert!(attachment_name("tab\there").is_err());

        let dir = tempfile::tempdir().unwrap();
        assert_eq!(unique_name(dir.path(), "a.txt"), "a.txt");
        std::fs::write(dir.path().join("a.txt"), "1").unwrap();
        assert_eq!(unique_name(dir.path(), "a.txt"), "a-2.txt");
        std::fs::write(dir.path().join("a-2.txt"), "2").unwrap();
        assert_eq!(unique_name(dir.path(), "a.txt"), "a-3.txt");
        std::fs::write(dir.path().join("Makefile"), "").unwrap();
        assert_eq!(unique_name(dir.path(), "Makefile"), "Makefile-2");
        std::fs::write(dir.path().join("archive.tar.gz"), "").unwrap();
        assert_eq!(
            unique_name(dir.path(), "archive.tar.gz"),
            "archive.tar-2.gz"
        );

        assert_eq!(model_id("").unwrap(), "");
        assert_eq!(model_id(" gpt-5.4 ").unwrap(), "gpt-5.4");
        assert_eq!(
            model_id("anthropic/claude-3:latest").unwrap(),
            "anthropic/claude-3:latest"
        );
        assert!(model_id("bad model").is_err());
        assert!(model_id("x;rm").is_err());
        assert!(model_id(&"m".repeat(81)).is_err());

        let files = vec![Attachment {
            name: "notes.txt".into(),
            size: 2048,
            path: "/tmp/x/notes.txt".into(),
        }];
        let msg = message_with_attachments("read this", &files);
        assert!(msg.starts_with("read this\n\nAttached by the owner"));
        assert!(msg.contains("- /tmp/x/notes.txt (2.0 KB)"));
        assert_eq!(message_with_attachments("plain", &[]), "plain");
        assert_eq!(size_words(512), "512 B");
        assert_eq!(size_words(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn briefs_name_the_workspace_or_the_projects() {
        let projects = vec![Project {
            slug: "demo".into(),
            name: "Demo".into(),
            path: "/srv/demo".into(),
            default_branch: "main".into(),
            status: "active".into(),
            ..Project::default()
        }];
        let art = Path::new("/tmp/art");
        let loose = Row {
            id: "chat-1".into(),
            cwd: "/tmp/scratch".into(),
            ..Row::default()
        };
        let text = brief_text(&loose, &projects, art);
        assert!(text.contains("not attached to a project yet"));
        assert!(text.contains("- demo: Demo, /srv/demo"));
        assert!(text.contains("pulse chat attach SLUG"));
        assert!(text.contains("pulse task new --project SLUG"));
        assert!(text.contains("$PULSE_ARTIFACTS (/tmp/art)"));
        // The owner's words are looked up on the shared board before the
        // agent asks what they mean.
        assert!(text.contains("The owner's words are PULSE's own - RSI, lead, board"));
        assert!(
            text.contains(
                "`pulse ask --all \"...\"` answers from the shared board of every project"
            )
        );
        assert!(text.contains("`pulse guide` explains how PULSE works"));
        assert!(text.contains("answer it from the shared board and the store - `pulse ask --all"));
        let attached = Row {
            id: "chat-1".into(),
            project: "demo".into(),
            cwd: "/w/demo".into(),
            branch: "pulse/chat-chat-1".into(),
            ..Row::default()
        };
        let text = brief_text(&attached, &projects, art);
        assert!(text.contains("Workspace: /w/demo is your own git worktree of project demo (Demo) on branch pulse/chat-chat-1, started from main"));
        assert!(text.contains("pulse land --project demo --branch pulse/chat-chat-1"));
        assert!(text.contains("The owner's checkout at /srv/demo is not yours to edit"));
        assert!(text.contains("`pulse ask --all \"...\"` answers from the shared board"));
        assert_eq!(
            title_from("  Fix the login page\nmore"),
            "Fix the login page"
        );
        assert_eq!(title_from(""), "New conversation");
        let long = "word ".repeat(30);
        assert!(title_from(&long).ends_with("..."));
        assert!(title_from(&long).chars().count() <= 64);
    }

    /// A conversation as a hub that died mid-turn leaves it: the row says
    /// running, its session is still open with steps past the last reply.
    fn cut_conversation(s: &Store, id: &str, status: &str) -> String {
        let sid = format!("hs-{id}");
        s.chat_create(&Row {
            id: id.to_string(),
            title: "cut".into(),
            status: "idle".into(),
            ..Row::default()
        })
        .unwrap();
        s.chat_set_session(id, &sid, "fake", "fake-1").unwrap();
        s.begin_harness_session(pulse_store::HarnessSession {
            id: sid.clone(),
            cwd: "/tmp".into(),
            ..Default::default()
        })
        .unwrap();
        let step = |n: i64, kind: &str, content: &str, command: &str| pulse_store::HarnessStep {
            session_id: sid.clone(),
            n,
            at: "2026-10-19T09:58:00Z".into(),
            kind: kind.into(),
            content: content.into(),
            command: command.into(),
            ..Default::default()
        };
        s.append_harness_step(step(0, "assistant", "On it.", ""))
            .unwrap();
        s.append_harness_step(step(1, "command", "", "ls")).unwrap();
        s.chat_add_message(id, "user", "do the thing", &Value::Null)
            .unwrap();
        s.chat_add_message(
            id,
            "assistant",
            "Done with part one.",
            &json!({"session": sid, "steps": [-1, 1]}),
        )
        .unwrap();
        s.chat_add_message(id, "user", "now part two", &Value::Null)
            .unwrap();
        s.append_harness_step(step(2, "assistant", "Part two.", ""))
            .unwrap();
        s.append_harness_step(step(3, "command", "", "cargo test -p demo"))
            .unwrap();
        s.append_harness_step(step(4, "edit", "src/lib.rs", ""))
            .unwrap();
        s.chat_set_status(id, status, "").unwrap();
        sid
    }

    #[test]
    fn recover_notes_the_cut_turn_and_closes_the_sessions_the_last_hub_left() {
        let _env = pulse_testutil::env_lock();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let cut = cut_conversation(&s, "chat-cut", "running");
        let parked = cut_conversation(&s, "chat-parked", "waiting");
        // An idle conversation whose session the last hub kept open.
        let idle = cut_conversation(&s, "chat-idle", "idle");
        let m = Manager::new(&home.path().display().to_string(), s.clone());
        let mut ids = m.recover();
        ids.sort();
        assert_eq!(ids, ["chat-cut", "chat-parked"]);

        let row = s.chat_get("chat-cut").unwrap().unwrap();
        assert_eq!(row.status, "idle");
        assert_eq!(row.note, RESTART_NOTE);
        assert_eq!(m.live(&row).status, "idle");
        let msgs = s.chat_messages("chat-cut", 0).unwrap();
        let note = msgs.last().unwrap();
        assert_eq!(note.role, "note");
        assert_eq!(note.content, CUT_RUNNING_LINE);
        assert_eq!(note.extra["restart"], true);
        assert_eq!(note.extra["session"], cut);
        // The steps after the last reply up to the last one recorded.
        assert_eq!(note.extra["steps"], json!([1, 4]));
        let session = s.harness_session_head(&cut).unwrap().unwrap();
        assert_eq!(session.status, "lost");
        assert_eq!(session.exit_status, "lost");
        assert_eq!(
            session.error,
            "the hub stopped while the agent was working; last step 4 edit 2 min ago"
        );
        assert!(!session.ended_at.is_empty());

        let msgs = s.chat_messages("chat-parked", 0).unwrap();
        assert_eq!(msgs.last().unwrap().content, CUT_WAITING_LINE);
        assert_eq!(s.chat_get("chat-parked").unwrap().unwrap().status, "idle");
        assert_eq!(
            s.harness_session_head(&parked).unwrap().unwrap().status,
            "lost"
        );

        // The idle one keeps its transcript; only its session closes.
        let row = s.chat_get("chat-idle").unwrap().unwrap();
        assert_eq!(row.status, "idle");
        assert_eq!(row.note, "");
        assert_eq!(s.chat_messages("chat-idle", 0).unwrap().len(), 3);
        let session = s.harness_session_head(&idle).unwrap().unwrap();
        assert_eq!(session.status, EXIT_ENDED);
        assert_eq!(session.error, "");

        // Running it again finds nothing to do.
        assert!(m.recover().is_empty());
        assert_eq!(s.chat_messages("chat-cut", 0).unwrap().len(), 4);
    }

    #[test]
    fn the_turn_after_a_cut_tells_the_agent_what_it_had_done() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(home.path(), json!([{"content": "Picking it up."}]));
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        cut_conversation(&s, "chat-cut", "running");
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        assert_eq!(m.recover(), ["chat-cut"]);
        m.send("chat-cut", "continue").expect("send");
        let row = wait_idle(&m, "chat-cut");
        assert_eq!(row.note, "", "the hint is given once");
        assert_ne!(row.session_id, "hs-chat-cut", "a new session started");
        let traj = read_trajectory(&home.path().display().to_string(), &row.session_id)
            .expect("trajectory");
        let sent = traj
            .messages
            .iter()
            .filter(|m| m.content.ends_with("continue"))
            .map(|m| m.content.clone())
            .next_back()
            .expect("the agent got the message");
        assert!(
            sent.contains("PULSE restarted while you were working on your previous turn"),
            "{sent}"
        );
        assert!(sent.contains("The steps you had taken:\n- assistant: Part two.\n- command: cargo test -p demo\n- edit: src/lib.rs"), "{sent}");
        assert!(sent.contains("Check what is already there"), "{sent}");
        // The hint is not repeated on the turn after.
        m.send("chat-cut", "and more").expect("send");
        let row = wait_idle(&m, "chat-cut");
        let traj = read_trajectory(&home.path().display().to_string(), &row.session_id)
            .expect("trajectory");
        let again = traj
            .messages
            .iter()
            .find(|m| m.content.ends_with("and more"))
            .expect("the agent got the second message");
        assert!(
            !again.content.contains("PULSE restarted"),
            "{}",
            again.content
        );
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    #[test]
    fn shutdown_stops_a_working_turn_with_a_note_and_ends_the_sessions() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(
            home.path(),
            json!([{"content": "First."}, {"command": "sleep 5"}, {"content": "Never reached."}]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        // Every session plays the script from its start: both answer
        // "First." and go idle; the second message to one of them sleeps.
        let working = m
            .create("", "", true, "take your time", "owner")
            .expect("create");
        let idle = m
            .create("", "", true, "quick one", "owner")
            .expect("create");
        let _ = wait_idle(&m, &working.id);
        let _ = wait_idle(&m, &idle.id);
        m.send(&working.id, "go on").expect("send");
        // The queued message counts as working at once; the stop should
        // land on the running command, so wait until the turn reached the
        // agent (the row says running) and its sleep began.
        for _ in 0..400 {
            if s.chat_get(&working.id).unwrap().unwrap().status == "running" {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(s.chat_get(&working.id).unwrap().unwrap().status, "running");
        thread::sleep(Duration::from_millis(300));
        assert_eq!(m.working(), 1, "the sleeping turn counts as working");
        let started = Instant::now();
        let (stopped, settled) = m.shutdown(Duration::from_secs(10));
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "the shutdown did not wait the sleep out"
        );
        assert_eq!(stopped, 1);
        assert!(settled, "every session ended within the grace");
        assert_eq!(m.working(), 0);
        let row = s.chat_get(&working.id).unwrap().unwrap();
        let msgs = s.chat_messages(&working.id, 0).unwrap();
        assert_eq!(row.status, "idle", "{msgs:#?}");
        assert_eq!(
            row.note,
            RESTART_NOTE,
            "{row:#?}\n{msgs:#?}\n{:#?}",
            s.harness_sessions(pulse_store::HarnessSessionFilter {
                limit: 100,
                ..Default::default()
            })
            .unwrap()
        );
        let note = msgs.last().unwrap();
        assert_eq!(note.role, "note");
        assert_eq!(note.content, STOPPED_TO_RESTART_LINE);
        assert_eq!(note.extra["session"], row.session_id);
        for id in [&working.id, &idle.id] {
            let sid = s.chat_get(id).unwrap().unwrap().session_id;
            let session = s.harness_session_head(&sid).unwrap().unwrap();
            assert_eq!(session.status, EXIT_ENDED, "{id}: {session:?}");
        }
        // A message still on its way when the hub went down never reached
        // the agent: the transcript says so, since "continue" would not do.
        let late = Runtime::new(
            &home.path().display().to_string(),
            s.clone(),
            "fake",
            "fake-1",
            &row,
            Weak::new(),
        );
        {
            let mut st = late.shared.lock();
            st.closed = true;
            st.shutdown = true;
        }
        late.turn("one more thing".into());
        let last = s.chat_messages(&working.id, 0).unwrap().pop().unwrap();
        assert_eq!(last.role, "note");
        assert_eq!(last.content, UNSENT_LINE);
        assert_eq!(last.extra["unsent"], true);
        assert!(late.started.lock().unwrap().is_none(), "no session started");
        // Nothing is left for the next hub to recover.
        let next = Manager::new(&home.path().display().to_string(), s.clone());
        assert!(next.recover().is_empty());
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    #[test]
    fn the_next_hub_carries_a_turn_stopped_to_restart_on_by_itself() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(
            home.path(),
            json!([{"content": "First."}, {"command": "sleep 5"}, {"content": "Never reached."}]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let stopped = m
            .create("", "", true, "take your time", "owner")
            .expect("create");
        let crashed = m
            .create("", "", true, "quick one", "owner")
            .expect("create");
        let _ = wait_idle(&m, &stopped.id);
        let _ = wait_idle(&m, &crashed.id);
        m.send(&stopped.id, "go on").expect("send");
        for _ in 0..400 {
            if s.chat_get(&stopped.id).unwrap().unwrap().status == "running" {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        thread::sleep(Duration::from_millis(300));
        assert_eq!(m.shutdown(Duration::from_secs(10)), (1, true));
        let before = s.chat_messages(&stopped.id, 0).unwrap().len();
        // The other conversation's hub went down mid-turn without stopping
        // it: the next hub cuts it with a note and leaves it to the owner.
        s.chat_set_status(&crashed.id, "running", "").unwrap();

        let next = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        assert_eq!(next.recover(), [crashed.id.as_str()]);
        assert_eq!(next.carry_on_restarted(), [stopped.id.as_str()]);
        assert!(
            next.carry_on_restarted().is_empty(),
            "a turn is carried on once"
        );
        let row = wait_idle(&next, &stopped.id);
        assert_eq!(row.status, "idle");
        assert_ne!(
            row.note, RESTART_NOTE,
            "the carried-on turn cleared the note"
        );
        let msgs = s.chat_messages(&stopped.id, 0).unwrap();
        assert_eq!(msgs.len(), before + 2, "{msgs:#?}");
        assert_eq!(msgs[before - 1].content, STOPPED_TO_RESTART_LINE);
        assert_eq!(msgs[before].role, "note");
        assert_eq!(msgs[before].content, CARRIED_ON_LINE);
        assert_eq!(msgs[before + 1].role, "assistant", "{msgs:#?}");
        assert_eq!(msgs[before + 1].content, "First.");
        // The agent read the restart hint and the carry-on prompt, not an
        // owner message: the transcript has no new user line.
        assert!(
            !msgs[before..].iter().any(|m| m.role == "user"),
            "{msgs:#?}"
        );
        let other = s.chat_get(&crashed.id).unwrap().unwrap();
        assert_eq!(other.status, "idle");
        assert_eq!(other.note, RESTART_NOTE);
        let last = s.chat_messages(&crashed.id, 0).unwrap().pop().unwrap();
        assert_eq!(last.content, CUT_RUNNING_LINE);
        assert!(next.running(&crashed.id).is_none(), "left to the owner");
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    fn wait_status(m: &Manager, id: &str, want: &[&str]) -> Row {
        for _ in 0..400 {
            let row = m.row(id).expect("row");
            let live = m.live(&row);
            if want.contains(&live.status.as_str())
                && (live.status != "idle" || live.queued.is_empty())
            {
                return row;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("the conversation never reached {want:?}");
    }

    fn transcript(s: &Store, id: &str) -> Vec<(String, String)> {
        s.chat_messages(id, 0)
            .unwrap()
            .into_iter()
            .map(|m| (m.role, m.content))
            .collect()
    }

    /// A message typed while a turn runs waits behind it, shown as queued,
    /// and lands in the transcript after that turn's reply - never between
    /// the owner's earlier message and the answer to it.
    #[test]
    fn a_message_sent_during_a_turn_lands_after_that_turns_reply() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(
            home.path(),
            json!([
                {"content": "First."},
                {"command": "sleep 1"},
                {"content": "Second."},
                {"content": "Third."}
            ]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let row = m.create("", "", true, "one", "owner").expect("create");
        let id = row.id.clone();
        wait_idle(&m, &id);
        m.send(&id, "two").expect("send");
        wait_status(&m, &id, &["running"]);
        thread::sleep(Duration::from_millis(200));
        m.send(&id, "three").expect("send");
        let live = m.live(&m.row(&id).unwrap());
        assert_eq!(live.queued, vec!["three".to_string()]);
        assert_eq!(
            transcript(&s, &id),
            vec![
                ("user".to_string(), "one".to_string()),
                ("assistant".to_string(), "First.".to_string()),
                ("user".to_string(), "two".to_string()),
            ],
            "the queued message is not in the transcript yet"
        );
        wait_idle(&m, &id);
        assert_eq!(
            transcript(&s, &id),
            vec![
                ("user".to_string(), "one".to_string()),
                ("assistant".to_string(), "First.".to_string()),
                ("user".to_string(), "two".to_string()),
                ("assistant".to_string(), "Second.".to_string()),
                ("user".to_string(), "three".to_string()),
                ("assistant".to_string(), "Third.".to_string()),
            ]
        );
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    /// Stop drops what waited behind the turn: the owner's words stay in
    /// the transcript after the stop note, with a note that they never
    /// reached the agent.
    #[test]
    fn stop_notes_the_messages_that_waited_behind_the_turn() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(
            home.path(),
            json!([{"content": "First."}, {"command": "sleep 5"}, {"content": "Never reached."}]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let row = m.create("", "", true, "one", "owner").expect("create");
        let id = row.id.clone();
        wait_idle(&m, &id);
        m.send(&id, "two").expect("send");
        wait_status(&m, &id, &["running"]);
        thread::sleep(Duration::from_millis(300));
        m.send(&id, "three").expect("send");
        m.cancel(&id).expect("cancel");
        let row = wait_idle(&m, &id);
        assert_eq!(row.status, "idle");
        let msgs = s.chat_messages(&id, 0).unwrap();
        let tail: Vec<(&str, &str)> = msgs[3..]
            .iter()
            .map(|m| (m.role.as_str(), m.content.as_str()))
            .collect();
        assert_eq!(
            tail,
            vec![
                ("note", "You stopped this turn."),
                ("user", "three"),
                ("note", UNSENT_STOPPED_LINE),
            ],
            "{msgs:#?}"
        );
        assert_eq!(msgs.last().unwrap().extra["unsent"], true);
        assert!(m.live(&row).queued.is_empty());
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    /// The agent's finish tool closes the conversation after its closing
    /// reply; the owner's next message reopens it.
    #[test]
    fn the_finish_tool_closes_the_conversation_and_a_message_reopens_it() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(
            home.path(),
            json!([
                {"tool": "finish", "args": {"summary": "The login works again."}},
                {"content": "All done."},
                {"content": "Reopened."}
            ]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let row = m
            .create("", "", true, "fix the login", "owner")
            .expect("create");
        let id = row.id.clone();
        let row = wait_status(&m, &id, &["closed"]);
        assert_eq!(row.status, "closed");
        assert_eq!(row.note, "The login works again.");
        let msgs = s.chat_messages(&id, 0).unwrap();
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "note"], "{msgs:#?}");
        assert_eq!(msgs[1].content, "All done.");
        assert_eq!(
            msgs[2].content,
            format!("{FINISHED_BY_AGENT_LINE} The login works again.")
        );
        assert_eq!(msgs[2].extra["closed"], "agent");
        let steps = s.harness_steps(&row.session_id, -1).unwrap();
        assert!(
            steps
                .iter()
                .any(|st| st.content == "finish: The login works again."),
            "{steps:#?}"
        );
        // Closed is not archived: a message runs and reopens it.
        m.send(&id, "one more").expect("send");
        let row = wait_idle(&m, &id);
        assert_eq!(row.status, "idle");
        assert_eq!(row.note, "");
        let last = s.chat_messages(&id, 0).unwrap().pop().unwrap();
        assert_eq!(last.content, "Reopened.");
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    /// Waits until `id`'s transcript has a note whose extra matches `want`.
    fn wait_note(
        s: &Store,
        id: &str,
        want: impl Fn(&Value) -> bool,
    ) -> pulse_store::ConversationMessage {
        wait_note_after(s, id, 0, want)
    }

    /// The same for a note after message `after`.
    fn wait_note_after(
        s: &Store,
        id: &str,
        after: i64,
        want: impl Fn(&Value) -> bool,
    ) -> pulse_store::ConversationMessage {
        for _ in 0..400 {
            if let Some(m) = s
                .chat_messages(id, after)
                .unwrap()
                .into_iter()
                .find(|m| m.role == "note" && want(&m.extra))
            {
                return m;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "conversation {id} never got the note: {:#?}",
            s.chat_messages(id, 0).unwrap()
        );
    }

    /// The chat tool starts a sub-conversation in the parent's workspace,
    /// the sub-conversation's finish comes back to the parent as a report
    /// (a note from it that runs a turn there, reopening the finished
    /// parent), and the third level down is refused. Every session replays
    /// the one script, so the parent's `chat start` cascades: parent,
    /// child, grandchild - whose own start is the refused one.
    #[test]
    fn a_sub_conversation_reports_to_its_parent_and_stops_two_levels_down() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(
            home.path(),
            json!([
                {"tool": "chat", "args": {"action": "start", "title": "Docs check", "text": "Check that the docs match the code."}},
                {"tool": "finish", "args": {"summary": "Started the docs check."}},
                {"content": "Done here."}
            ]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let parent = m
            .create("", "", true, "ship the feature", "owner")
            .expect("create");
        let pid = parent.id.clone();
        let parent = wait_status(&m, &pid, &["closed", "idle"]);
        assert!(!parent.cwd.is_empty(), "the parent works in scratch space");

        // The child: the parent's row, inherited.
        let started = wait_note(&s, &pid, |x| x.get("child").is_some());
        let cid = started.extra["child"].as_str().unwrap().to_string();
        assert_eq!(started.content, "Started sub-conversation: Docs check");
        let child = m.row(&cid).expect("child row");
        assert_eq!(child.parent, pid);
        assert_eq!(child.created_by, pid);
        assert_eq!(child.title, "Docs check");
        assert_eq!(
            child.cwd, parent.cwd,
            "a sub-conversation shares its parent's workspace"
        );
        assert!(child.auto_approve);
        assert_eq!(s.chat_depth(&cid).unwrap(), 1);
        let children = s.chat_children(&pid, false).unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, cid);
        let brief = s.chat_messages(&cid, 0).unwrap().remove(0);
        assert_eq!(brief.role, "user");
        assert_eq!(brief.content, "Check that the docs match the code.");

        // The child's finish reaches the parent as a report, and the
        // parent (finished by then) runs a turn on it.
        let report = wait_note(&s, &pid, |x| x["report"]["kind"] == REPORT_FINISHED);
        assert_eq!(report.extra["report"]["child"], cid);
        assert_eq!(report.extra["report"]["title"], "Docs check");
        assert!(
            report.content.starts_with("Started the docs check."),
            "{}",
            report.content
        );
        assert!(report.content.contains("Done here."), "{}", report.content);
        let parent = wait_status(&m, &pid, &["idle"]);
        assert_eq!(
            parent.status, "idle",
            "the report reopened the finished parent"
        );
        let msgs = s.chat_messages(&pid, 0).unwrap();
        let after = msgs.iter().position(|x| x.n == report.n).unwrap();
        assert!(
            msgs[after + 1..].iter().any(|x| x.role == "assistant"),
            "the parent's agent took the report up: {msgs:#?}"
        );

        // The grandchild: started by the child (one level down is fine),
        // refused its own start (two levels down).
        let g_started = wait_note(&s, &cid, |x| x.get("child").is_some());
        let gid = g_started.extra["child"].as_str().unwrap().to_string();
        let grandchild = wait_status(&m, &gid, &["closed", "idle"]);
        assert_eq!(grandchild.parent, cid);
        assert_eq!(s.chat_depth(&gid).unwrap(), 2);
        assert!(s.chat_children(&gid, false).unwrap().is_empty());
        let steps = s.harness_steps(&grandchild.session_id, -1).unwrap();
        assert!(
            steps
                .iter()
                .any(|st| st.content.contains("levels deep at most")),
            "{steps:#?}"
        );
        // The grandchild's finish reaches the child.
        wait_note(&s, &cid, |x| x["report"]["child"] == gid);

        // The parent's agent steers the child: a note from the parent in
        // the child's transcript, a turn there, and its reply back.
        wait_idle(&m, &cid);
        wait_idle(&m, &pid);
        let before = s.chat_messages(&pid, 0).unwrap().last().unwrap().n;
        assert!(m.send_from_parent(&cid, "someone-else", "x").is_err());
        m.send_from_parent(&cid, &pid, "Also check the README.")
            .expect("send from parent");
        let steer = wait_note(&s, &cid, |x| x["from"] == pid);
        assert_eq!(steer.content, "Also check the README.");
        wait_idle(&m, &cid);
        wait_note_after(&s, &pid, before, |x| {
            x["report"]["kind"] == REPORT_REPLIED && x["report"]["child"] == cid
        });
        let listing = m.read_children(&pid, "").unwrap();
        assert!(
            listing.contains(&cid) && listing.contains("Docs check"),
            "{listing}"
        );
        let one = m.read_children(&pid, &cid).unwrap();
        assert!(one.contains("[you] Also check the README."), "{one}");
        assert!(m.read_children(&pid, &gid).is_err(), "not this one's child");
        wait_idle(&m, &pid);
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    /// A sub-conversation is refused for an archived parent, and one the
    /// owner closes reports that to its parent.
    #[test]
    fn sub_conversations_need_a_live_parent_and_report_the_owners_close() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(home.path(), json!([{"content": "Ok."}, {"content": "Ok."}]));
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        let parent = m.create("", "", true, "plan", "owner").expect("create");
        let pid = parent.id.clone();
        wait_idle(&m, &pid);
        assert!(m.create_child(&pid, "", "  ").is_err(), "an empty brief");
        let child = m
            .create_child(&pid, "", "Review the plan for gaps.")
            .expect("child");
        assert_eq!(child.title, "Review the plan for gaps.");
        assert_eq!(child.parent, pid);
        wait_idle(&m, &child.id);
        wait_note(&s, &pid, |x| x["report"]["kind"] == REPORT_REPLIED);
        wait_idle(&m, &pid);
        m.close(&child.id).expect("close");
        let closed = wait_note(&s, &pid, |x| x["report"]["kind"] == REPORT_FINISHED);
        assert_eq!(closed.content, "The owner finished it.");
        wait_idle(&m, &pid);
        m.archive(&pid).expect("archive");
        assert!(
            m.create_child(&pid, "", "another").is_err(),
            "archived parent"
        );
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }

    /// The owner closes: an idle conversation at once, a working one by
    /// stopping its turn; what waited behind is noted as unsent. The effort
    /// the owner picks restarts the session like a model change and reaches
    /// the agent.
    #[test]
    fn the_owner_closes_idle_and_working_conversations_and_picks_the_effort() {
        let _env = pulse_testutil::env_lock();
        pulse_provider_fake::register();
        let home = tempfile::tempdir().unwrap();
        let s = store(home.path());
        let script = script(
            home.path(),
            json!([{"content": "First."}, {"command": "sleep 5"}, {"content": "Never reached."}]),
        );
        unsafe { std::env::set_var("PULSE_HARNESS_FAKE_SCRIPT", &script) };
        let m = Manager::new(&home.path().display().to_string(), s.clone())
            .with_provider("fake", "fake-1");
        assert!(
            m.create_with("", "", "", "extreme", true, "hi", "owner")
                .is_err(),
            "an effort outside the list is refused"
        );
        let row = m
            .create_with("", "", "", "default", true, "one", "owner")
            .expect("create");
        let id = row.id.clone();
        assert_eq!(row.reasoning, "", "default reads as the provider's own");
        wait_idle(&m, &id);
        // Idle: closed at once, with the owner's line.
        m.close(&id).expect("close");
        let row = wait_status(&m, &id, &["closed"]);
        assert_eq!(row.status, "closed");
        assert_eq!(row.note, CLOSED_BY_OWNER_NOTE);
        let last = s.chat_messages(&id, 0).unwrap().pop().unwrap();
        assert_eq!(last.role, "note");
        assert_eq!(last.content, CLOSED_BY_OWNER_LINE);
        assert_eq!(last.extra["closed"], "owner");
        let before = s.chat_messages(&id, 0).unwrap().len();
        m.close(&id).expect("close again");
        assert_eq!(
            s.chat_messages(&id, 0).unwrap().len(),
            before,
            "closing twice adds nothing"
        );
        // A new effort restarts the session on the next turn and the agent
        // runs with it; the message reopens the conversation. Every session
        // plays the script from its start, so the reply comes at once.
        let first_session = row.session_id.clone();
        assert!(m.set_reasoning(&id, "extreme").is_err());
        m.set_reasoning(&id, "high").expect("reasoning");
        assert_eq!(m.row(&id).unwrap().reasoning, "high");
        m.send(&id, "two").expect("send");
        let row = wait_idle(&m, &id);
        assert_eq!(row.status, "idle", "reopened");
        assert_eq!(row.note, "");
        assert_ne!(row.session_id, first_session, "the session restarted");
        {
            let rt = m.running(&id).unwrap();
            let guard = rt.started.lock().unwrap();
            let started = guard.as_ref().unwrap();
            assert_eq!(started.reasoning, "high");
            let env = started.agent.lock().unwrap().environment();
            assert!(
                env.iter().any(|l| l.contains("; reasoning high")),
                "{env:#?}"
            );
        }
        assert_eq!(
            transcript(&s, &id)[before..],
            [
                ("user".to_string(), "two".to_string()),
                ("assistant".to_string(), "First.".to_string()),
            ]
        );
        // Working: the owner's close stops the turn; the message behind it
        // is noted as unsent, and the conversation stands closed.
        m.send(&id, "three").expect("send");
        for _ in 0..400 {
            if s.chat_get(&id).unwrap().unwrap().status == "running" {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        thread::sleep(Duration::from_millis(300));
        m.send(&id, "four").expect("send");
        m.close(&id).expect("close");
        let row = wait_status(&m, &id, &["closed"]);
        assert_eq!(row.note, CLOSED_BY_OWNER_NOTE);
        let msgs = s.chat_messages(&id, 0).unwrap();
        let tail: Vec<(&str, &str)> = msgs[before + 2..]
            .iter()
            .map(|m| (m.role.as_str(), m.content.as_str()))
            .collect();
        assert_eq!(
            tail,
            vec![
                ("user", "three"),
                (
                    "note",
                    "The turn stopped here: you finished the conversation."
                ),
                ("note", CLOSED_BY_OWNER_LINE),
                ("user", "four"),
                ("note", UNSENT_CLOSED_LINE),
            ],
            "{msgs:#?}"
        );
        assert!(m.live(&row).queued.is_empty());
        m.archive(&id).expect("archive");
        assert!(
            m.close(&id).is_err(),
            "an archived conversation is not closed"
        );
        thread::sleep(Duration::from_millis(100));
        unsafe { std::env::remove_var("PULSE_HARNESS_FAKE_SCRIPT") };
    }
}
