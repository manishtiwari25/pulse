// The API client. Every route is same-origin JSON under /api/v1/ (the Go hub
// serves the export and the API from one origin; `next dev` proxies /api to a
// running `pulse hub`). Shapes of the existing routes come from what the
// Python dashboard returned (legacy/dashboard/src/api.ts); the new routes
// (tasks, messages, compute, records, constitution, events) follow the
// "API v1 additions" contract - the Go hub is checked against these types.

export class ApiError extends Error {
  status: number;
  why: string;
  fix: string;
  constructor(message: string, status: number, why = "", fix = "") {
    super(message);
    this.status = status;
    this.why = why;
    this.fix = fix;
  }
}

export interface ErrorBody {
  error: string;
  why?: string;
  fix?: string;
}

async function readJson<T>(r: Response): Promise<T> {
  let data: unknown;
  try {
    data = await r.json();
  } catch {
    data = { error: r.ok ? "empty response" : "HTTP " + r.status };
  }
  if (!r.ok) {
    const e = (data ?? {}) as Partial<ErrorBody>;
    throw new ApiError(e.error || "HTTP " + r.status, r.status, e.why || "", e.fix || "");
  }
  return data as T;
}

export const enc = encodeURIComponent;

export function api<T>(path: string, init?: RequestInit): Promise<T> {
  return fetch("/api/v1/" + path.replace(/^\/+/, ""), { cache: "no-store", ...init }).then((r) => readJson<T>(r));
}

// The decide/settings routes kept their explicit intent headers from the
// Python server (ADR-017, ADR-025); the newer write routes need only JSON.
const WRITE_HEADERS: Record<string, string> = { decide: "X-Pulse-Decide", settings: "X-Pulse-Settings" };

export function post<T>(path: string, payload: unknown): Promise<T> {
  const head = path.split("/")[0] ?? "";
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (WRITE_HEADERS[head]) headers[WRITE_HEADERS[head]] = "yes";
  return api<T>(path, { method: "POST", headers, body: JSON.stringify(payload) });
}

// Every project card the org knows, commons last; tolerant of a hub that
// sends `projects: null` while it warms up.
export function allProjects(o: Org | undefined | null): ProjectCardData[] {
  if (!o) return [];
  return [...(o.projects ?? []), ...(o.commons ? [o.commons] : [])];
}

export function query(params: Record<string, string | number | undefined | null>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== "") q.set(k, String(v));
  const s = q.toString();
  return s ? "?" + s : "";
}

export const projectPath = (slug: string, route: string) => "projects/" + enc(slug) + "/" + route;

export function errorText(e: unknown): { error: string; why: string; fix: string } {
  if (e instanceof ApiError) return { error: e.message, why: e.why, fix: e.fix };
  if (e instanceof Error) return { error: e.message, why: "", fix: "" };
  return { error: String(e), why: "", fix: "" };
}

// ----- existing shapes (Python-compatible bodies) -----------------------

export interface Who {
  agent?: string;
  name?: string | null;
  session?: string;
  runner?: string;
  model?: string;
}

export interface PostRef extends Who {
  id: string;
  thread_key: string;
  kind: string;
  date: string;
  summary: string;
  vote?: string;
  resolution?: string;
  provenance_ok?: number;
  provenance_missing?: string[];
  on_post?: string;
  project?: string;
  slug?: string;
  base?: string;
  expires?: string;
}

export interface Claim extends Who {
  id: string;
  thread_key: string;
  summary: string;
  expires: string;
  slug?: string;
  base?: string;
}

export interface ThreadRow {
  key: string;
  area: string;
  thread: string;
  posts: number;
  last: string;
  status: string;
  resolution: string;
  summary: string;
}

export interface Vote extends Who {
  vote: string;
  classification?: string;
  reason?: string;
  date: string;
  post_id: string;
  read_line?: string;
}

export interface Suggestion extends Who {
  id: string;
  thread_key: string;
  date: string;
  score: number;
  up: number;
  down: number;
  voters: number;
  status: string;
  summary: string;
  rank?: number;
  votes: Vote[];
  slug?: string;
  base?: string;
  quorum?: number;
  expiry_days?: number;
}

export interface Amendment extends Who {
  id: string;
  thread_key: string;
  date: string;
  status: string;
  up: number;
  down: number;
  voters: number;
  quorum: number;
  pct_up: number;
  identities: number;
  uncounted_self: number;
  uncounted_hash_mismatch: number;
  uncounted_no_read_line: number;
  text_hash: string;
  section: string;
  summary: string;
  votes: Vote[];
  slug?: string;
  base?: string;
}

export interface Proposal extends Who {
  id: string;
  thread_key: string;
  date: string;
  status: string;
  decided_id: string;
  decided_date: string;
  summary: string;
  decided_summary: string;
  proposal_text: string;
  can_decide: boolean;
  slug?: string;
  base?: string;
  kind?: string;
  decided_agent?: string;
  decided_name?: string;
}

export interface Bucket {
  t: string;
  n: number;
  kinds?: Record<string, number>;
}

export interface Timeline {
  now: string;
  hours: Bucket[];
  days: Bucket[];
  total: number;
  before_window: number;
  unparsed: number;
  series?: (Timeline & { slug: string; kind: string; base: string })[];
}

export interface ProjectCardData {
  slug: string;
  kind: string;
  brain: boolean;
  base: string;
  path: string;
  name: string;
  branch: string;
  head: string;
  dirty: boolean;
  built_at: string;
  board_present: boolean;
  claims_active: number;
  threads_open: number;
  questions_open: number;
  suggestions_eligible: number;
  suggestions_needing_votes: number;
  amendments_open: number;
  proposals_awaiting: number;
  posts: number;
  posts_24h: number;
  agents: number;
  evolution_present: boolean;
  loop: string;
  stop_present: boolean;
  baseline: string;
  last_post: PostRef | null;
  active_claims: Claim[];
  present: boolean;
  missing: boolean;
  error: string;
}

export interface Totals {
  projects: number;
  projects_present: number;
  agents: number;
  agents_active: number;
  votes_open: number;
  proposals_awaiting: number;
  questions_open: number;
  posts_24h: number;
  posts: number;
  brain: string;
  brains: string[];
}

export interface NoticeBallot {
  slug: string;
  base: string;
  name: string;
  brain: boolean;
  suggestions: number;
  amendments: number;
  proposals: number;
}

export interface Notices {
  ballot: NoticeBallot[];
  votes_open: number;
  proposals_awaiting: number;
  brain: string;
  brains: string[];
  commons_present: boolean | null;
  commons_base: string;
  commons_questions: number | null;
}

export interface Org {
  generated_at: string;
  registry: string;
  tally_mode: string;
  can_decide: boolean;
  commons_dir: string;
  projects: ProjectCardData[];
  commons: ProjectCardData | null;
  stamp: string;
  notices: Notices;
  totals: Totals;
  active_claims: Claim[];
  proposals: Proposal[];
  // Go hub metadata (absent on the Python server).
  store?: string;
  schema_version?: string;
  records_version?: string;
  binary_version?: string;
}

export interface Executor {
  name: string;
  kind: string;
  status: string;
  address: string;
  workdir?: string;
  workers: number;
  projects: string;
  last_probe_at: string;
  probe_note: string;
  errors?: number;
  backoff_minutes?: number;
  next_probe_at?: string;
  runs_done?: number;
  runs_failed?: number;
  created_at?: string;
  created_by?: string;
  // Only the runner route reports live occupancy; the compute route omits it.
  running?: number;
  free?: number;
}

export interface Runner {
  available: boolean;
  now?: string;
  daemon: { state: string; text?: string; pid: number; at: string; loops?: number; interval: number; note?: string };
  runner?: { allowed: boolean; reason: string };
  hands: { free: number; slots: number; executors_active: number; executors: number; any_project: boolean };
  executors: Executor[];
  workers: { pool: number; idle: number; busy: number; target: number; idle_names: string[]; prune_after_days: number };
  jobs: { queued: number; leased: number; blocked: number; running: number; runs_today: number; cap: number };
  capacity?: string;
}

export type SettingKind = "count" | "count-or-none" | "credits" | "fraction" | "switch" | "choice" | "command" | "text" | "identifier";

// One of the owner's caps as the store holds it (ADR-025). `secret` marks
// keys whose values the API never returns (compute credentials never live
// here - Azure auth is `az login` on the hub machine - but the flag is the
// contract for anything the Go hub decides to hide).
export interface Setting {
  name: string;
  value: string;
  default: string;
  kind: SettingKind;
  min: number | null;
  choices: string[] | null;
  description: string;
  updated_at: string;
  updated_by: string;
  secret?: boolean;
}

export interface SettingsPayload {
  available: boolean;
  can_edit: boolean;
  settings: Setting[];
}

export interface RosterRow extends Who {
  agent: string;
  names?: string[];
  kind: string;
  status: string;
  agent_status?: string;
  sessions: string[];
  session_count: number;
  jobs_done: number;
  jobs_failed: number;
  posts: number;
  pruned_reason?: string;
  last_useful_at: string;
  votes: number;
  claims: number;
  questions: number;
  answers: number;
  first_seen: string;
  last_seen: string;
  active_claims: Claim[];
  projects: string[];
  days_since: number;
  approximate: boolean;
  name_shared: boolean;
  shared_with: string[];
}

export interface Votes {
  suggestions: Suggestion[];
  amendments: Amendment[];
}

export interface Health {
  ok: boolean;
  time: string;
  projects: string[];
  errors: Record<string, string>;
}

export interface BoardParams {
  quorum?: number;
  expiry_days?: number;
  stale_days?: number;
  amend_quorum?: number;
  amend_majority?: string;
  protected_sections?: string;
  constitution_present?: boolean;
  stop_present?: boolean;
}

export interface ProjectInfo {
  schema_version?: string;
  built_at: string;
  now?: string;
  repo: string;
  repo_name: string;
  remote?: string;
  branch: string;
  head: string;
  dirty: string;
  board_present: string;
  evolution_present?: string;
  board_params?: BoardParams;
  last_error: string;
}

export interface EvolutionSummary {
  evolution_present: string;
  stop_present: string;
  missing_files: string[] | string;
  loop: string;
  limits: string;
  limits_note: string;
  baseline: string;
  count_baselines: string;
  count_variants: string;
  count_incidents: string;
  product_verdict: string;
  hygiene_git: string;
  last: string;
}

export interface Overview {
  board_present: boolean;
  posts_without_kind: number;
  quorum: number;
  claims_active: number;
  threads_open: number;
  threads_stale: number;
  posts: number;
  posts_without_provenance: number;
  suggestions_eligible: number;
  suggestions_needing_votes: number;
  amendments_open: number;
  amendments_passed: number;
  proposals_awaiting: number;
  active_claims: Claim[];
  stale_claims: Claim[];
  open_threads: ThreadRow[];
  recent_posts: PostRef[];
  evolution: Partial<EvolutionSummary>;
  kinds: Record<string, number>;
  agents: (Who & { posts: number; last: string })[];
}

export interface ThreadDetail {
  thread: ThreadRow;
  posts: PostRef[];
}

export interface PostDetail extends PostRef {
  rel_path: string;
  body: string;
  frontmatter: Record<string, string>;
  responses: PostRef[];
  area?: string;
  thread?: string;
  branch?: string;
  text_hash?: string;
  has_read_line?: number;
}

// GET projects/<slug>/evolution - SQL-native (D-L). The three docs are the
// EVO-* records the store indexed (null when not imported); `archive.rows`
// is the live evolution_archive table (empty until the loop has run) and
// `archive.record/body` the historical EVO-archive record when one exists.
// `loop` carries the evolve.* settings; only the *presence* of the owner's
// held-out directory is exposed, never its path.
export interface EvolutionDoc {
  record: RecordRow;
  body: string; // Markdown
}

export interface EvolutionArchiveRow {
  id: string;
  project: string;
  suggestion_post: string;
  hypothesis: string;
  branch: string;
  baseline_score: number;
  variant_score: number;
  tolerance: number;
  kept: 0 | 1 | boolean;
  sandbox: string;
  run_id: string;
  at: string;
  note: string;
}

export interface Evolution {
  constitution: EvolutionDoc | null;
  evaluators: EvolutionDoc | null;
  archive: { record?: RecordRow | null; body?: string; rows: EvolutionArchiveRow[] };
  loop: { loop: string; limits: string; tolerance: string; stop: string; heldout_present: boolean };
}

// ----- new routes (API v1 additions) -------------------------------------

export type TaskStatus = "open" | "planned" | "in_progress" | "review" | "done" | "cancelled";
export const TASK_STATUSES: TaskStatus[] = ["open", "planned", "in_progress", "review", "done", "cancelled"];

export interface Task {
  id: string; // TASK-NNN
  project: string;
  title: string;
  status: TaskStatus;
  priority: number; // higher runs sooner (decide jobs are 40, onboarding 20)
  lead: string; // agent name, "<slug>-lead" once assigned, else ""
  job_id: string;
  created_by: string;
  created_at: string;
  updated_at: string;
  current_rev: number;
  brief: string; // Markdown, the current revision's brief
}

export interface TaskRevision {
  rev: number;
  title: string;
  brief: string;
  note: string;
  author: string;
  at: string;
}

export interface TaskJob {
  id: string;
  kind: string;
  title: string;
  status: string;
  agent: string;
  branch: string;
  project?: string;
  tier?: number;
  priority?: number;
  source?: string;
  result?: string;
  executor?: string;
  created_at?: string;
  updated_at?: string;
  leased_at?: string;
  finished_at?: string;
}

export interface Message {
  id: string;
  project: string;
  task_id: string | null;
  author: string; // "human" for the owner, else the agent name
  body: string; // Markdown
  at: string;
  read_by?: string[];
}

export interface TaskDetail {
  task: Task;
  revisions: TaskRevision[];
  messages: Message[];
  jobs: TaskJob[];
}

export interface TasksPayload {
  tasks: Task[];
}

export interface MessagesPayload {
  messages: Message[];
}

export interface NewTaskInput {
  project: string;
  title: string;
  brief: string;
  priority: number;
}

export interface AmendTaskInput {
  title?: string;
  brief: string;
  note: string;
}

export type ComputeStatus = "pending" | "approved" | "provisioning" | "ready" | "denied" | "released" | "failed";

export interface ComputeRequest {
  id: string;
  project: string;
  requested_by: string;
  size: string; // provider VM size, e.g. Standard_D4s_v5
  hours: number;
  reason: string;
  status: ComputeStatus;
  decided_by: string;
  executor: string;
  at: string;
  updated_at: string;
}

export interface ComputeCaps {
  provider: string;
  max_vms: number;
  max_hours_per_day: number;
  auto_approve: boolean | 0 | 1;
  budget_credits: number;
  vms_now: number;
  hours_today: number;
}

export interface Compute {
  requests: ComputeRequest[];
  executors: Executor[];
  caps: ComputeCaps;
}

export type RecordKind =
  | "adr"
  | "plan"
  | "wu"
  | "feature"
  | "memory"
  | "workflow"
  | "skill"
  | "guide"
  | "context"
  | "architecture"
  | "evolution"
  | "instruction"
  | "template"
  | "usage"
  | "prompt";

export const RECORD_KINDS: RecordKind[] = ["adr", "plan", "wu", "feature", "memory", "workflow", "skill", "guide", "context", "architecture", "evolution", "instruction", "template", "usage", "prompt"];

export interface RecordRow {
  id: string; // ADR-027, PLAN-026, WU-022, FEAT-016, MEM-product, EVO-constitution, ...
  project: string;
  kind: RecordKind;
  number: number | null;
  slug: string;
  title: string;
  status: string;
  tier: number | null;
  areas: string[];
  tags: string[];
  delivery: string;
  delivered: string;
  date: string;
  path: string;
  updated_at: string;
  current_rev: number;
  created_at?: string;
  created_by?: string;
  origin?: string;
}

export interface RecordRevisionRef {
  rev: number;
  author: string;
  at: string;
  note: string;
}

export interface RecordDetail {
  record: RecordRow;
  frontmatter: Record<string, unknown>;
  body: string; // Markdown after the frontmatter
  revisions: RecordRevisionRef[];
}

export interface RecordsPayload {
  records: RecordRow[];
}

export interface ConstitutionAmendment {
  id: string;
  status: string;
  votes_up: number;
  votes_down: number;
  quorum: number;
  passed: boolean;
  protected: boolean;
}

export interface Constitution {
  record: RecordRow | null;
  body: string;
  amendments: ConstitutionAmendment[];
}

export interface StoreEvent {
  id: number;
  at: string;
  actor: string;
  action: string; // e.g. post.created, task.amended, settings.set, compute.approved
  subject: string;
  detail: unknown;
}

export interface EventsPayload {
  events: StoreEvent[];
}

// ----- writes (one CLI command each; every one logs the same event) -------

export const writes = {
  // `id` is the contract field; `post_id` is what the Python hub read - the Go hub accepts both.
  decide: (id: string, answer: "yes" | "no", words: string) => post<{ ok?: boolean; decided?: unknown }>("decide", { id, post_id: id, answer, words }),
  setSetting: (name: string, value: string) => post<{ ok?: boolean; setting?: Setting }>("settings", { name, value }),
  newTask: (input: NewTaskInput) => post<{ task: Task }>("tasks", input),
  amendTask: (id: string, input: AmendTaskInput) => post<{ task: Task }>("tasks/" + enc(id) + "/amend", input),
  cancelTask: (id: string, reason: string) => post<{ task: Task }>("tasks/" + enc(id) + "/cancel", { reason }),
  sendMessage: (project: string, body: string, task_id?: string | null) => post<{ message: Message }>("messages", task_id ? { project, task_id, body } : { project, body }),
  approveCompute: (id: string, words: string) => post<{ request: ComputeRequest }>("compute/" + enc(id) + "/approve", { words }),
  denyCompute: (id: string, words: string) => post<{ request: ComputeRequest }>("compute/" + enc(id) + "/deny", { words }),
};
