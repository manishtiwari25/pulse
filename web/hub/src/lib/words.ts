// The console's vocabulary. The store speaks in states and ids (`in_progress`,
// `eligible`, `leased`, `step_limit`); the owner reads plain words. Every
// status badge and every place that names a technical concept goes through
// here, so the same thing is called the same everywhere. Pure functions, no
// DOM - safe to import from tests.

const STATUS: Record<string, string> = {
  // tasks
  open: "Open",
  planned: "Planned",
  in_progress: "In progress",
  review: "Needs you",
  done: "Done",
  cancelled: "Cancelled",
  // jobs and runs
  queued: "Waiting to start",
  leased: "Starting",
  running: "Running",
  blocked: "Blocked",
  failed: "Failed",
  succeeded: "Finished",
  submitted: "Finished",
  cancelling: "Cancelling",
  step_limit: "Hit the step limit",
  time_limit: "Hit the time limit",
  format_errors: "Kept answering in the wrong format",
  provider_error: "Model provider error",
  not_signed_in: "Not signed in",
  lost: "Lost track of it",
  landed: "Merged",
  // votes
  eligible: "Agreed",
  "needs votes": "Needs votes",
  needs_votes: "Needs votes",
  rejected: "Turned down",
  passed: "Passed",
  // the owner's answers
  awaiting: "Waiting for you",
  pending: "Waiting for you",
  approved: "Approved",
  denied: "Denied",
  accepted: "Accepted",
  declined: "Declined",
  deferred: "Deferred",
  proposed: "Proposed",
  superseded: "Replaced",
  draft: "Draft",
  // agents and machines
  online: "Online",
  idle: "Idle",
  working: "Working",
  dead: "Offline",
  quiet: "Quiet",
  unknown: "Unknown",
  legacy: "Older identity",
  pruned: "Removed",
  retired: "Retired",
  active: "Active",
  inactive: "Inactive",
  drained: "Not taking work",
  provisioning: "Starting up",
  ready: "Ready",
  unreachable: "Can't reach it",
  // connections
  enabled: "On",
  disabled: "Off",
  // threads
  closed: "Closed",
  answered: "Answered",
  stale: "Gone quiet",
  // code delivery
  delivered: "Delivered",
  partially: "Partly delivered",
};

// "in_progress" -> "In progress", "eligible" -> "Agreed"; unknown states read in sentence case.
export function statusWord(s?: string | null): string {
  const raw = (s || "").trim();
  if (!raw) return "";
  const key = raw.toLowerCase();
  if (STATUS[key]) return STATUS[key];
  const spaced = key.replace(/[_-]+/g, " ");
  if (STATUS[spaced]) return STATUS[spaced];
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

// What a post on the board is, as a person would say it.
const POST_KIND: Record<string, string> = {
  claim: "Picked up",
  handoff: "Handover",
  finding: "Finding",
  question: "Question",
  answer: "Answer",
  suggestion: "Suggestion",
  opinion: "Opinion",
  vote: "Vote",
  amendment: "Rule change",
  result: "Result",
  notice: "Notice",
  proposal: "Proposal",
  post: "Post",
};

export function postKindWord(k?: string | null): string {
  const key = (k || "post").toLowerCase();
  return POST_KIND[key] ?? key.charAt(0).toUpperCase() + key.slice(1);
}

// Record kinds (pulse record): the label on a record's badge and filter.
export const RECORD_KIND: Record<string, string> = {
  adr: "Decision",
  plan: "Plan",
  wu: "Work item",
  feature: "Feature",
  memory: "Memory",
  evolution: "Self-improvement",
  guide: "Guide",
  skill: "Skill",
  workflow: "Workflow",
  prompt: "Prompt",
  template: "Template",
  env: "Environment",
  constitution: "Constitution",
  context: "Context",
  architecture: "Architecture",
  instruction: "Instruction",
  usage: "Usage note",
};

export function recordKindWord(k?: string | null): string {
  const key = (k || "").toLowerCase();
  return RECORD_KIND[key] ?? (key ? key.charAt(0).toUpperCase() + key.slice(1) : "Record");
}

// A board thread's key ("product/checkout-latency") as a title people read
// ("Checkout latency"); the key itself stays in tooltips.
const ACRONYMS = new Set(["api", "ui", "ux", "ci", "cd", "cli", "mcp", "url", "id", "ids", "sql", "http", "https", "sse", "ssh", "vm", "vms", "db", "pr", "prs", "adr", "json", "csv", "html", "css", "js", "ts", "llm", "ai", "os", "io"]);

export function threadName(key?: string | null): string {
  const k = (key || "").trim();
  const name = k.includes("/") ? k.split("/").slice(1).join("/") : k;
  const words = name.replace(/[-_/]+/g, " ").replace(/\s+/g, " ").trim().split(" ").map((w) => (ACRONYMS.has(w.toLowerCase()) ? w.toUpperCase() : w)).join(" ");
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : k;
}

// Change tiers: how much say the owner has before an agent acts.
export function tierWord(t?: string | number | null): { short: string; long: string } {
  const n = String(t ?? "").replace(/^t(ier)?\s*/i, "");
  if (n === "0") return { short: "Quick fix", long: "Tier 0: a small fix agents make directly" };
  if (n === "1") return { short: "Planned change", long: "Tier 1: a change agents plan, then make" };
  if (n === "2") return { short: "Big change", long: "Tier 2: a big change agents propose first and make only after a decision" };
  if (n === "3") return { short: "Owner-only change", long: "Tier 3: a change only you can make" };
  return { short: n ? "Tier " + n : "", long: n ? "Tier " + n : "" };
}

// Task priorities, in the words the New task dialog uses.
export function priorityWord(p: number): string {
  if (p >= 50) return "Urgent";
  if (p >= 30) return "High";
  if (p >= 10) return "Normal";
  return "Low";
}

// Plain names for the concepts the store is built on, with the term the CLI
// and the README use - shown as a hint where an owner may meet the term.
export const TERMS = {
  daemon: { word: "background runner", term: "daemon", hint: "The background runner (pulse daemon) plans work and starts agents on this machine." },
  executor: { word: "machine", term: "executor", hint: "A machine that runs agents: this one, or one PULSE reaches over ssh." },
  heartbeat: { word: "check-in", term: "heartbeat", hint: "Agents check in every few minutes; a missed check-in means the agent is offline." },
  claim: { word: "picked up", term: "claim", hint: "An agent says “I'm on this” for a few hours, so others leave it alone." },
  quorum: { word: "votes needed", term: "quorum", hint: "How many agents must vote before a suggestion is decided." },
  amendment: { word: "rule change", term: "amendment", hint: "A change to a project's constitution, the rules its self-improvement loop follows." },
  heldout: { word: "private tests", term: "held-out fixtures", hint: "Test cases the agents never see, so self-improvement cannot game them." },
  harness: { word: "harness", term: "harness", hint: "The built-in agent that runs every job: it talks to the model provider and runs commands." },
  commons: { word: "shared board", term: "commons", hint: "The board for work that touches more than one project." },
  landing: { word: "merging", term: "landing", hint: "Merging an agent's finished, verified branch into the project's default branch." },
} as const;
