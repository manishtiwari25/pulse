// The API client. Every route is same-origin JSON under /api/v1/ (the pulse hub
// serves the export and the API from one origin; `next dev` proxies /api to a
// running `pulse hub`). Shapes of the existing routes come from what the
// Python dashboard returned (legacy/dashboard/src/api.ts); the new routes
// (tasks, messages, compute, records, constitution, events) follow the
// "API v1 additions" contract - the pulse hub is checked against these types.

export class ApiError extends Error {
  status: number;
  why: string;
  fix: string;
  // The whole error body: a few refusals carry more than words (the push
  // key's public half and its probes, ADR-042).
  body: Record<string, unknown>;
  constructor(message: string, status: number, why = "", fix = "", body: Record<string, unknown> = {}) {
    super(message);
    this.status = status;
    this.why = why;
    this.fix = fix;
    this.body = body;
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
    throw new ApiError(e.error || "HTTP " + r.status, r.status, e.why || "", e.fix || "", typeof data === "object" && data ? (data as Record<string, unknown>) : {});
  }
  return data as T;
}

export const enc = encodeURIComponent;

export function api<T>(path: string, init?: RequestInit): Promise<T> {
  return fetch(apiHref(path), { cache: "no-store", ...init }).then((r) => readJson<T>(r));
}

export function apiHref(path: string): string {
  return "/api/v1/" + path.replace(/^\/+/, "");
}

// The decide/settings routes kept their explicit intent headers from the
// Python server (ADR-017, ADR-025); the newer write routes need only JSON.
// The held-out writes (ADR-032) take the same guard: a `.score` file is a
// program the daemon runs, so the intent is stated in a header too; so does
// the push key (ADR-042), which decides what the daemon may push with.
const WRITE_HEADERS: Record<string, string> = { decide: "X-Pulse-Decide", settings: "X-Pulse-Settings", heldout: "X-Pulse-Heldout", cred: "X-Pulse-Cred" };

export function post<T>(path: string, payload: unknown): Promise<T> {
  const head = path.split("/")[0] ?? "";
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (WRITE_HEADERS[head]) headers[WRITE_HEADERS[head]] = "yes";
  return api<T>(path, { method: "POST", headers, body: JSON.stringify(payload) });
}

// Why a "commons" sits next to the projects. One sentence, shown wherever the
// commons appears so the owner reads the same explanation everywhere.
export const COMMONS_HINT =
  "The board every project shares. Agents post here when something concerns more than one project - a question for all of them, a pattern or a pitfall worth knowing everywhere. It is not a checkout, so it has no tasks, branch or loop of its own.";

// The registered checkouts only - for project grids and filters, where the
// shared commons board is not a project. Tolerant of a hub that sends
// `projects: null` while it warms up.
export function registeredProjects(o: Org | undefined | null): ProjectCardData[] {
  return o?.projects ?? [];
}

// Every board the org knows, commons last - for lookups by slug.
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
  participants: string[];
  last_by: string;
  last_kind: string;
  open_questions: number;
  open_handoffs: number;
  active_claims: string[];
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
  job?: string;
  job_status?: "queued" | "leased" | "blocked" | string;
  gate?: string;
  plannable?: boolean;
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

// The self-improvement gates on a project row (ADR-029). The loop for a
// project opens only with enabled public fixtures and a recorded baseline;
// the console shows both and names the next human step, never starting a
// measurement itself. Optional on both the org rows and GET /projects: the
// hub that ships with this console sends them, an older one does not.
export type EvolveLevel = "A" | "B" | "C" | "D" | "";

// One of the loop's two records as a project row reports it (ADR-030).
// Onboarding seeds a copy of the template's text unless the project already
// has the record; `seeded` stays true until someone edits that copy, so the
// console can say "still the template's text - make it yours". `seeded_from`
// names where the copy started (`amistio-template/TPL-evolution-constitution@r1`)
// and keeps naming it after edits; it is "" for the brain or an own file.
export interface EvolveRecordRef {
  id: string; // EVO-constitution-<slug>, or EVO-constitution for the brain / an imported own file
  rev: number; // current revision
  seeded: boolean; // the body is still the seeded copy of the template's text
  seeded_from: string; // template record and revision the copy started from; "" when none
  // ADR-046: the draft an agent filed against the record, waiting for the owner
  // (no body on a project row - GET records/<id> or the evolution doc carry it),
  // and the project's open drafting job. Absent on an older hub.
  draft?: RecordDraft | null;
  draft_job?: JobRef | null;
}

// ADR-046: a body an agent proposed for a record. It never changes the record
// until the owner adopts it (POST records/<id>/adopt) - then it becomes the next
// revision, authored `human`, with the drafting agent in the note. One per
// record; the latest filed wins.
export interface RecordDraft {
  author: string; // the agent that filed it
  at: string; // ISO time it was filed
  note: string; // one line on what it was based on; may be ""
  base_rev: number; // the revision it was written against
  job_id: string; // the job whose run filed it; "" when filed by hand
  body?: string; // Markdown; present on records/<id> and projects/<slug>/evolution only
}

// {id, status} of a job the console refers to (the open drafting job, the baseline job).
export interface JobRef {
  id: string;
  status: string;
}

// The daemon's baseline job for a project (ADR-031): the newest open one,
// with its queue state. `blocked` waits for a worker or a gate the daemon
// clears itself; the owner does nothing at this gate.
export interface BaselineJobRef {
  id: string;
  status: "queued" | "leased" | "blocked" | string;
}

export interface EvolveGate {
  fixtures_enabled: number; // enabled public fixtures
  baseline: boolean; // the baseline is recorded (the daemon's measurement, or a kept experiment since)
  evolve_level: EvolveLevel; // highest self-improvement level allowed right now; "" on error
  evolve_next: string; // the exact next human step as a command, or "none" (optionally "none - <why>")
  // ADR-030: null when the project has no such record, undefined when the
  // hub predates the field (so the console shows nothing rather than "no constitution").
  constitution?: EvolveRecordRef | null;
  evaluators?: EvolveRecordRef | null;
  // ADR-031: the daemon records the baseline. The newest open baseline job
  // (null when none is open), and when the newest one failed within the last
  // 24 hours - the daemon retries after that - as RFC3339, "" otherwise.
  baseline_job?: BaselineJobRef | null;
  baseline_failed_at?: string;
}

export interface ProjectCardData extends Partial<EvolveGate> {
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
  suggestions_plannable?: number;
  suggestions_needing_votes: number;
  suggestions_rejected?: number;
  amendments_open: number;
  proposals_awaiting: number;
  posts: number;
  posts_24h: number;
  agents: number;
  evolution_present: boolean;
  loop: string;
  stop_present: boolean;
  last_post: PostRef | null;
  active_claims: Claim[];
  present: boolean;
  missing: boolean;
  error: string;
}

// The gate fields of a project, from whichever payload carries them: the
// org row first, the registry row (GET /projects) second. Null when neither
// does, so the console shows nothing rather than a wrong "0 fixtures". The
// record refs (ADR-030) and the baseline job (ADR-031) travel on GET
// /projects; they are read from either row, so an org row that lacks them
// does not hide the registry's.
export function evolveGate(p: Partial<EvolveGate> | undefined | null, reg?: Partial<EvolveGate> | null): EvolveGate | null {
  for (const src of [p, reg]) {
    if (src && typeof src.evolve_level === "string" && typeof src.fixtures_enabled === "number") {
      const g: EvolveGate = { fixtures_enabled: src.fixtures_enabled, baseline: src.baseline === true, evolve_level: src.evolve_level, evolve_next: src.evolve_next || "" };
      const other = src === p ? reg : p;
      const con = firstRef(src.constitution, other?.constitution);
      const eva = firstRef(src.evaluators, other?.evaluators);
      if (con !== undefined) g.constitution = con;
      if (eva !== undefined) g.evaluators = eva;
      const job = firstJob(src.baseline_job, other?.baseline_job);
      if (job !== undefined) g.baseline_job = job;
      const failed = [src.baseline_failed_at, other?.baseline_failed_at].find((v) => typeof v === "string");
      if (failed !== undefined) g.baseline_failed_at = failed;
      return g;
    }
  }
  return null;
}

// The baseline job as the hub sent it (ADR-031): the ref, null for "none
// open", undefined when the field is absent or malformed (an older hub).
export function baselineJob(v: unknown): BaselineJobRef | null | undefined {
  if (v === null) return null;
  if (typeof v !== "object" || v === undefined) return undefined;
  const j = v as Partial<BaselineJobRef>;
  if (typeof j.id !== "string" || !j.id) return undefined;
  return { id: j.id, status: typeof j.status === "string" ? j.status : "queued" };
}

// The first source that says anything about the job - null ("none open") counts as an answer.
function firstJob(a: unknown, b: unknown): BaselineJobRef | null | undefined {
  const j = baselineJob(a);
  return j !== undefined ? j : baselineJob(b);
}

// The first source that says anything about the record - null ("none") counts as an answer.
function firstRef(a: unknown, b: unknown): EvolveRecordRef | null | undefined {
  const r = recordRef(a);
  return r !== undefined ? r : recordRef(b);
}

// A record ref as the hub sent it: the object, null for "no such record",
// undefined when the field is absent or malformed (an older hub).
function recordRef(v: unknown): EvolveRecordRef | null | undefined {
  if (v === null) return null;
  if (typeof v !== "object" || v === undefined) return undefined;
  const r = v as Partial<EvolveRecordRef>;
  if (typeof r.id !== "string" || !r.id) return undefined;
  const ref: EvolveRecordRef = { id: r.id, rev: typeof r.rev === "number" ? r.rev : 0, seeded: r.seeded === true, seeded_from: typeof r.seeded_from === "string" ? r.seeded_from : "" };
  // ADR-046: the waiting draft and the open drafting job, when the hub reports them.
  if (r.draft && typeof r.draft === "object" && typeof r.draft.author === "string") {
    ref.draft = { author: r.draft.author, at: str(r.draft.at), note: str(r.draft.note), base_rev: typeof r.draft.base_rev === "number" ? r.draft.base_rev : 0, job_id: str(r.draft.job_id) };
  } else if (r.draft === null) ref.draft = null;
  const job = baselineJob(r.draft_job);
  if (job !== undefined) ref.draft_job = job;
  return ref;
}

const str = (v: unknown) => (typeof v === "string" ? v : "");

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
  owner_presence?: "present" | "away";
  commons_dir: string;
  projects: ProjectCardData[];
  commons: ProjectCardData | null;
  stamp: string;
  notices: Notices;
  totals: Totals;
  active_claims: Claim[];
  proposals: Proposal[];
  // pulse hub metadata (absent on the Python server).
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

// ----- project lifecycle (ADR-027) --------------------------------------
// GET /projects lists every registered checkout, inactive ones included; the
// org payload only carries the active ones. Rows may carry the ADR-029 gate
// fields and the ADR-030 record refs too (see EvolveGate).
export interface RegisteredProject extends Partial<EvolveGate> {
  slug: string;
  name: string;
  path: string;
  remote: string;
  brain: boolean;
  status: "active" | "inactive" | string;
  verify_cmd: string;
  default_branch: string;
  added_at: string;
  added_by: string;
  last_seen: string;
  present: boolean;
}

export interface ProjectsPayload {
  projects: RegisteredProject[] | null;
  brain: string;
}

export interface RegisterProjectInput {
  path: string;
  name?: string;
  verify?: string;
}

export const MAX_PROJECT_GOAL_CHARS = 4000;

export interface ProjectGoal {
  text: string;
  source: "owner" | "generated";
  updated_at: string;
  updated_by: string;
}

export interface ProjectGoalPayload {
  project: string;
  goal: ProjectGoal;
  can_edit: boolean;
}

// What `pulse init` did: the row, whether it was new, the onboarding job it
// queued (or found queued), and the environment record it imported, if any.
export interface InitResult {
  project: RegisteredProject;
  created: boolean;
  job: { id: string; project: string; kind: string; title: string; status: string; created_at?: string } | null;
  job_queued: boolean;
  environment_record: string;
  store: string;
}

export interface RemoveItem {
  label: string;
  detail: string;
}

// What `pulse remove` shows before (dry_run) and records after (applied):
// what changes, what stays, and the commands that are the human's to run.
export interface RemovePreview {
  kind: "project" | "executor" | string;
  name: string;
  dry_run: boolean;
  applied: boolean;
  changes: RemoveItem[] | null;
  stays: RemoveItem[] | null;
  yours: string[] | null;
  cancelled_jobs: string[] | null;
  leased_jobs: string[] | null;
  executors: string[] | null;
}

// One level of the machine's folders, for the Add project picker (WU-023)
// and the held-out folder step (ADR-032). Directories only, hidden entries
// skipped; the hub marks git checkouts and the projects already registered
// on those paths.
export interface FolderEntry {
  name: string;
  path: string;
  checkout: boolean;
  registered: string;
}

export interface FoldersPayload {
  path: string;
  parent: string;
  home: string;
  roots: { label: string; path: string }[];
  checkout: boolean;
  registered: string;
  // ADR-032: the registered project the listed folder sits inside ("" when
  // none) - a private set may not live there. Absent on an older hub.
  inside?: string;
  entries: FolderEntry[];
  truncated: boolean;
}

// ----- the owner's held-out fixtures (ADR-032) -------------------------------

// One of the six built-in effect judges, named for the form; `label` is the
// plain sentence the owner picks by ("Nothing changed and no commit was made").
export interface HeldoutJudge {
  name: string;
  label: string;
}

// The private set as counts only: whether evolve.heldout_dir is set, whether
// the folder exists on this machine, how many fixtures are ready and how
// many template pairs still wait for text, and one sentence when the set is
// invalid. The path, the stems and the text never travel (ADR-012).
export interface HeldoutPayload {
  configured: boolean;
  present: boolean;
  count: number;
  pending: number;
  // ADR-033: how many of the ready fixtures the daemon wrote sealed (omitted by an older hub).
  generated?: number;
  problem?: string;
  judges: HeldoutJudge[];
  // On a write: template files the setup created (4 the first time, 0 after).
  created?: number;
}

// `pulse evolve setup DIR` from the console: an absolute path or ~, outside
// every registered checkout and not under ~/.pulse; created 0700 if missing.
export interface HeldoutSetupInput {
  dir: string;
}

// One fixture written by the hub, never read back: the request, and either a
// built-in judge by name or the owner's own scorer script - exactly one.
// `slot` rewrites the fixture at that 1-based position; without it the first
// pending template pair is filled, else a new pair is added.
export interface HeldoutFixtureInput {
  request: string;
  judge?: string;
  scorer?: string;
  slot?: number;
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
  // Capacity is the CLI-compatible status line; hold is the structured login gate sentence from the same heartbeat.
  capacity?: string;
  hold?: string;
  // Projects whose verified branches cannot land right now and why (ADR-024:
  // the checkout is dirty or off its default branch, a lock, an unreadable
  // folder). Written by the daemon once per change; empty when everything lands.
  landing?: LandingHold[];
}

export interface LandingHold {
  project: string;
  at: string;
  note: string;
  stale: boolean;
}

export type SettingKind = "count" | "count-or-none" | "credits" | "number" | "fraction" | "switch" | "choice" | "command" | "text" | "identifier" | "reference";

// One of the owner's caps as the store holds it (ADR-025). `secret` marks
// keys whose values the API never returns (compute credentials never live
// here - each provider's auth is its CLI login on the hub machine - but the flag is the
// contract for anything the pulse hub decides to hide).
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

// ----- the daemon's push key (ADR-041, ADR-042) --------------------------

// Where the key can live. The hub lists these; `available` is false for a
// provider this machine lacks (`unavailable` says why), `needs_token` for
// 1Password's service-account token, `browse` when the hub can list its
// items, `generate` when the hub can mint a key of the daemon's own.
export interface CredProvider {
  id: "op" | "keychain" | "file" | string;
  title: string;
  blurb: string;
  available: boolean;
  unavailable?: string;
  needs_token: boolean;
  browse: boolean;
  generate: boolean;
  fields: { name: string; label: string; placeholder: string }[];
  example: string;
}

// The state `pulse cred status` prints: the reference (never the key), where
// the token was found ("none" when it was not), and the fingerprint the last
// successful set recorded.
export interface CredReport {
  reference: string;
  scheme?: string;
  landing_push: string;
  token: string;
  set_at?: string;
  set_by?: string;
  fingerprint?: string;
}

export interface CredPayload {
  available: boolean;
  can_edit: boolean;
  os: string;
  report: CredReport;
  providers: CredProvider[];
  shapes: string;
}

export interface CredVault {
  id: string;
  name: string;
}

export interface CredItem {
  id: string;
  title: string;
  category: string;
  updated_at?: string;
  reference: string;
}

// One remote asked whether the key may push: `ok`, or `skipped` (no
// upstream), or `why` (the remote's refusal, redacted).
export interface CredProbe {
  project: string;
  branch: string;
  remote?: string;
  ok: boolean;
  skipped?: string;
  why?: string;
}

export interface CredSetResult {
  reference: string;
  fingerprint: string;
  public_key: string;
  source: string;
  probed: boolean;
  probes: CredProbe[];
  landing_push: string;
}

export interface CredGenerated {
  path: string;
  reference: string;
  fingerprint: string;
  public_key: string;
  existed: boolean;
}

export interface CredCleared {
  was: string;
  token_removed: string[];
}

// ----- native harness ----------------------------------------------------

export type HarnessKind = "native" | "copilot-cli";

export interface HarnessLogin {
  logged_in: boolean;
  user: string;
  source: string;
  plan: string;
  expires_at: string;
  detail: string;
}

export interface HarnessProviderInfo {
  name: string;
  label: string;
  device_login: boolean;
  token_login: boolean;
  login?: HarnessLogin;
}

export interface HarnessStatus {
  harness: HarnessKind;
  provider: HarnessProviderInfo;
  providers: HarnessProviderInfo[];
  models: { worker: string; light: string };
  limits: { max_steps: number; step_timeout_seconds: number; output_limit: number; minutes_per_job: number; max_cost?: number; keep_observations?: number };
  sessions: { running: number; today: number; failed_today: number; submitted_today: number; cost_today: number };
  settings: Setting[];
}

export interface HarnessPrompt {
  template: string;
  default: string;
  custom: boolean;
  rendered: string;
  error?: string;
}

export interface HarnessModel {
  id: string;
  name: string;
  vendor: string;
  chat: boolean;
  tool_calls: boolean;
  premium: number;
  preview: boolean;
  context_window: number;
  max_output: number;
  default: boolean;
}

export interface HarnessModelsResponse {
  models: HarnessModel[];
  fetched_at: string;
}

export interface HarnessDeviceFlow {
  device_code: string;
  user_code: string;
  verification_uri: string;
  interval: number;
  expires_in: number;
}

export interface HarnessPollResponse {
  status: "pending" | "slow_down" | "complete" | "expired" | "denied";
  login?: HarnessLogin;
}

export type HarnessSessionStatus = "running" | "submitted" | "step_limit" | "time_limit" | "format_error" | "provider_error" | "not_logged_in" | "cancelled" | "lost";

export interface HarnessSession {
  id: string;
  run_id: string;
  job_id: string;
  job_title: string;
  agent: string;
  project: string;
  // Executor that ran or mirrored this harness session; empty when the hub did not know.
  executor: string;
  provider: string;
  model: string;
  cwd: string;
  status: HarnessSessionStatus;
  exit_status: string;
  submission: string;
  error: string;
  steps: number;
  calls: number;
  prompt_tokens: number;
  completion_tokens: number;
  cost: number;
  started_at: string;
  updated_at: string;
  ended_at: string;
  trajectory: string;
  cancel_requested_at: string;
}

export type HarnessStepKind = "assistant" | "command" | "observation" | "note" | "error" | "exit";

export interface HarnessStep {
  n: number;
  at: string;
  kind: HarnessStepKind;
  content: string;
  command: string;
  exit_code: number | null;
  duration_ms: number;
  prompt_tokens: number;
  completion_tokens: number;
  model: string;
}

export interface HarnessSessionsResponse {
  sessions: HarnessSession[];
  counts: { running: number; today: number; failed_today: number; submitted_today: number; cost_today: number };
  next_before?: string;
}

export interface HarnessSessionResponse {
  session: HarnessSession;
  steps: HarnessStep[];
  after?: number;
}

// GET /api/v1/running (ADR-035): which pulse runs where, against the brain's
// HEAD. `state` is one word: off, no-daemon, no-brain, current, unstamped,
// behind, waiting (see internal/hub/running.go). `daemon` comes from the
// heartbeat and is null until the daemon has run once; `last` is the newest
// rebuild the daemon recorded, null when it never rebuilt.
export type RunningState = "off" | "no-store" | "no-daemon" | "no-brain" | "current" | "unstamped" | "behind" | "waiting";

export interface RunningBuild {
  version: string;
  revision: string;
  pid: number | null;
  at?: string;
}

export interface RunningPayload {
  available: boolean;
  self_update: string;
  retry_minutes: number;
  hub: RunningBuild;
  daemon: (RunningBuild & { bus?: { subscribers: number; dropped: number; deduped: number } | null }) | null;
  brain: { slug: string; default_branch: string; branch: string; head: string; dirty: boolean } | null;
  last: { at: string; from: string; to: string; inflight: number } | null;
  state: RunningState;
  // macOS only (null elsewhere): the running binary carries pulse's stable signing requirement, so
  // the owner's folder answers survive rebuilds (ADR-035 rule 8).
  signed?: boolean | null;
}

// GET /service (ADR-038): the user services that keep PULSE alive on this
// machine - launchd or systemd --user - as `pulse service status` reads them.
// `installed` is the daemon's unit present and loaded; `hub_is_service` says
// whether the hub answering is the service's own (a foreground `pulse hub`
// on the service's port hands over when it installs).
export interface ServiceUnit {
  name: string;
  path: string;
  present: boolean;
  loaded: boolean;
}

export interface ServiceState {
  kind: string;
  supported: boolean;
  installed: boolean;
  units: ServiceUnit[];
  binary: string;
  logs: string;
}

export interface ServicePayload {
  service: ServiceState;
  hub_is_service: boolean;
  hub_port: number;
  hub_url: string;
  command: string;
}

// POST /service: `dry_run` true previews the launchers and units, false
// writes and loads them. `handover` means this hub stops serving so the
// service's hub can take its port; the page reconnects.
export interface ServiceInstallResult {
  dry_run: boolean;
  installed: boolean;
  kind: string;
  daemon_path: string;
  hub_path: string;
  output: string;
  handover: boolean;
  hub_url: string;
}

// Whether the inbox asks the owner to keep PULSE running: a service manager
// exists and the daemon's unit is not installed yet. Nothing to ask elsewhere.
export function serviceStep(p: ServicePayload | undefined | null): boolean {
  return !!p && p.service.supported && !p.service.installed;
}

export type AgentPresence = "online" | "idle" | "dead" | "unknown" | "legacy" | "pruned";

export interface RosterRow extends Who {
  agent: string;
  names?: string[];
  kind: string;
  status: string;
  agent_status?: string;
  presence?: AgentPresence;
  presence_reason?: string;
  last_heartbeat_at?: string;
  // Where the latest heartbeat came from: execution, cli, hook, lease, manual, activity.
  heartbeat_source?: string;
  heartbeat_age_seconds?: number | null;
  // Seconds of silence after which the server counts this agent as dead.
  dead_after_seconds?: number;
  presence_session?: string;
  presence_checked_at?: string;
  presence_expires_at?: string;
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
  current_job: string;
  current_job_kind: string;
  current_job_title: string;
  current_task?: string;
  current_task_title?: string;
  current_project?: string;
  current_source?: string;
  current_thread: string;
  executor: string;
  focus_since: string;
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
  suggestions_plannable?: number;
  suggestions_needing_votes: number;
  suggestions_rejected?: number;
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
  // ADR-030: whether the body is still the seeded copy of the template's
  // text and where that copy started (see EvolveRecordRef). Absent on an older hub.
  seeded?: boolean;
  seeded_from?: string;
  // ADR-046: the waiting draft (with its body) and the open drafting job, or null.
  draft?: RecordDraft | null;
  draft_job?: JobRef | null;
}

// One archived measurement. The daemon's baseline (ADR-031) is a row like
// any other - hypothesis "baseline", the default branch, equal scores, kept -
// and the console shows it as the baseline, never as a kept experiment.
export interface EvolutionArchiveRow {
  id: string;
  project: string;
  suggestion_post: string;
  hypothesis: string; // "baseline" for the daemon's measurement
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

// The daemon's baseline measurement rather than an experiment (ADR-031).
export const isBaselineRow = (r: Pick<EvolutionArchiveRow, "hypothesis">): boolean => r.hypothesis === "baseline";

export interface Evolution {
  constitution: EvolutionDoc | null;
  evaluators: EvolutionDoc | null;
  archive: { record?: RecordRow | null; body?: string; rows: EvolutionArchiveRow[] };
  loop: { loop: string; limits: string; tolerance: string; stop: string; heldout_present: boolean };
  // ADR-029: the gates the loop checks before it may run, and the fixtures
  // it measures against. `gates` is null when the hub could not compute
  // them, with `gates_error` saying why. Both are absent on an older hub.
  gates?: EvolveGates | null;
  gates_error?: string;
  fixtures?: EvolveFixture[] | null;
}

// Every gate between a project and its self-improvement loop, as the hub
// evaluates them: the level allowed right now, why, and the one human step
// that opens the next gate. The held-out set is counted, never named.
export interface EvolvePlanning {
  state: "gated" | "blocked" | "waiting" | "carding" | "ready" | "queued" | "leased" | "finalizing" | string;
  reason: string;
  job?: string;
  suggestion?: string;
}

export interface EvolveGates {
  project: string;
  stop: boolean;
  loop: boolean;
  limits: string;
  tolerance: number;
  constitution: boolean;
  evaluators: boolean;
  baseline: number | null; // the recorded baseline score, null when none
  baseline_id?: string;
  // ADR-031: the daemon's baseline job while it measures, and the time of a
  // failure within the last 24 hours ("" when none) - see EvolveGate.
  baseline_job?: BaselineJobRef | null;
  baseline_failed_at?: string;
  public_fixtures: number;
  // ADR-033: `generated` counts the ready fixtures the daemon wrote sealed;
  // an older hub omits it.
  heldout: { count: number; pending: number; generated?: number; problem?: string };
  kept: number;
  discarded: number;
  voted: { tried: number; kept: number; rate: number };
  signal: { tried: number; kept: number; rate: number };
  product_scope: { allowed: boolean; reason: string };
  // The constitution's Editable scope table as the loop reads it: how many
  // file classes it has, which ones the loop may edit now (Stage 2, or Stage 4
  // for product code, with paths present in the checkout) and which staged
  // classes name absent paths. Empty `editable` is a gate; an older hub omits it.
  scope?: { classes: number; editable: string[]; absent?: string[]; reason: string };
  loop_started_at?: string;
  cycles: number;
  hours: number;
  cap_reached: boolean;
  limit_problem?: string;
  level: EvolveLevel | string;
  reason: string;
  next_human_step: string; // a command, or "none" - optionally "none - <what the daemon does instead>"
  planning?: EvolvePlanning; // absent on an older hub; readiness alone is not dispatch or progress
}

// One fixture as the console may see it: name, kind, state and provenance.
// The request and scorer text stay in the store on purpose.
export interface EvolveFixture {
  name: string;
  kind: "public" | "signal" | string;
  enabled: boolean;
  weight: number;
  created_by: string;
  at: string;
  starter: boolean;
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
  jobs_total?: number;
  jobs_queued?: number;
  jobs_blocked?: number;
  jobs_running?: number;
  jobs_done?: number;
  jobs_failed?: number;
  working?: TaskWorker[];
}

export interface TaskWorker {
  job: string;
  kind: string;
  title: string;
  agent: string;
  since: string;
  executor: string;
  harness_session?: string;
}

export function taskProgress(t: Task): { total: number; done: number; failed: number; running: number; queued: number; blocked: number; working: TaskWorker[] } {
  return {
    total: t.jobs_total ?? 0,
    done: t.jobs_done ?? 0,
    failed: t.jobs_failed ?? 0,
    running: t.jobs_running ?? 0,
    queued: t.jobs_queued ?? 0,
    blocked: t.jobs_blocked ?? 0,
    working: t.working ?? [],
  };
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
  harness_session?: string;
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

export interface CodeDeliveryScope {
  project: string | null;
  limit: number;
  scanned: number;
  has_more: boolean;
  next_before: number | null;
}

export interface CodeDeliverySummary {
  code_deliveries: number;
  code_files: number;
  docs_only: number;
  non_code: number;
  unknown: number;
  duplicates: number;
}

export interface CodeDeliveryJob {
  id: string;
  title: string;
  kind: string;
  status: string;
}

export interface CodeDeliveryTask {
  id: string;
  title: string;
}

export interface CodeDeliveryWorker {
  name: string;
  runner: string;
  model: string;
}

export interface CodeDeliveryVotes {
  up: number;
  down: number;
  score: number;
}

export interface CodeDeliverySuggestion {
  id: string;
  summary: string;
  area: string;
  thread: string;
  votes: CodeDeliveryVotes | null;
}

export interface CodeDelivery {
  event_seq: number;
  project: string;
  at: string;
  commit: string;
  code_files: number;
  files_changed: number;
  code_paths: string[];
  paths_truncated: boolean;
  evidence: "recorded" | "git";
  job: CodeDeliveryJob | null;
  tasks: CodeDeliveryTask[];
  worker: CodeDeliveryWorker | null;
  suggestion: CodeDeliverySuggestion | null;
}

export interface CodeDeliveryUnavailable {
  event_seq: number;
  project: string;
  at: string;
  reason: string;
}

export interface CodeDeliveriesPayload {
  scope: CodeDeliveryScope;
  summary: CodeDeliverySummary;
  deliveries: CodeDelivery[];
  unavailable: CodeDeliveryUnavailable[];
}

export function codeDeliveriesPath({ project, limit, before }: { project?: string | null; limit?: number; before?: number | null } = {}): string {
  return "code-deliveries" + query({ project, limit, before });
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
  draft?: RecordDraft | null; // ADR-046: the waiting draft with its body, or null
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
  action: string; // e.g. post.question, mention, hook.failed, task.amended, settings.set, compute.approved
  subject: string;
  detail: unknown;
  payload?: unknown;
}

export interface EventsPayload {
  events: StoreEvent[];
}

// ----- writes (one CLI command each; every one logs the same event) -------

// The human gates in the console (ADR-028): a Proposed ADR is accepted,
// declined or deferred with the owner's words; the hub records them as the
// revision note, author `human`, suffixed "(via console)".
export type RecordDecision = "accept" | "Rejected" | "Deferred";

export const writes = {
  acceptRecord: (id: string, words: string) => post<{ record: RecordRow }>("records/" + enc(id) + "/accept", { words }),
  setRecordStatus: (id: string, status: "Rejected" | "Deferred", words: string) => post<{ record: RecordRow }>("records/" + enc(id) + "/status", { status, words }),
  // Record drafts (ADR-046): the owner adopts or discards what an agent drafted, or asks for a draft now.
  adoptDraft: (id: string, words: string) => post<{ record: RecordRow }>("records/" + enc(id) + "/adopt", words ? { words } : {}),
  discardDraft: (id: string, words: string) => post<{ record: RecordRow; discarded: boolean }>("records/" + enc(id) + "/discard", words ? { words } : {}),
  draftJob: (id: string) => post<{ job: JobRef; created: boolean; reason: string }>("records/" + enc(id) + "/draft-job", {}),
  // `id` is the contract field; `post_id` is what the Python hub read - the pulse hub accepts both.
  decide: (id: string, answer: "yes" | "no", words: string) => post<{ ok?: boolean; decided?: unknown }>("decide", { id, post_id: id, answer, words }),
  setSetting: (name: string, value: string) => post<{ ok?: boolean; setting?: Setting }>("settings", { name, value }),
  harnessLogin: () => post<HarnessDeviceFlow>("harness/login", {}),
  harnessLoginPoll: (device_code: string) => post<HarnessPollResponse>("harness/login/poll", { device_code }),
  harnessLoginToken: (token: string) => post<{ login: HarnessLogin }>("harness/login/token", { token }),
  harnessLogout: () => post<{ ok: boolean; login: HarnessLogin }>("harness/logout", {}),
  harnessCancel: (id: string) => post<{ cancelled: boolean; session: HarnessSession }>("harness/sessions/" + enc(id) + "/cancel", {}),
  newTask: (input: NewTaskInput) => post<{ task: Task }>("tasks", input),
  amendTask: (id: string, input: AmendTaskInput) => post<{ task: Task }>("tasks/" + enc(id) + "/amend", input),
  cancelTask: (id: string, reason: string) => post<{ task: Task }>("tasks/" + enc(id) + "/cancel", { reason }),
  sendMessage: (project: string, body: string, task_id?: string | null) => post<{ message: Message }>("messages", task_id ? { project, task_id, body } : { project, body }),
  approveCompute: (id: string, words: string) => post<{ request: ComputeRequest }>("compute/" + enc(id) + "/approve", { words }),
  denyCompute: (id: string, words: string) => post<{ request: ComputeRequest }>("compute/" + enc(id) + "/deny", { words }),
  // Project lifecycle (ADR-027) - the same functions `pulse init` and `pulse remove` run.
  registerProject: (input: RegisterProjectInput) => post<InitResult>("projects", input),
  setProjectGoal: (slug: string, text: string) => post<ProjectGoalPayload>(projectPath(slug, "goal"), { text }),
  resetProjectGoal: (slug: string) => post<ProjectGoalPayload>(projectPath(slug, "goal"), { reset: true }),
  // dry_run:true previews; the hub applies only on an explicit false.
  removeProject: (slug: string, dryRun: boolean) => post<RemovePreview>("projects/" + enc(slug) + "/remove", { dry_run: dryRun }),
  restoreProject: (slug: string) => post<InitResult>("projects/" + enc(slug) + "/restore", {}),
  removeExecutor: (name: string, dryRun: boolean) => post<RemovePreview>("compute/executors/" + enc(name) + "/remove", { dry_run: dryRun }),
  // The owner's held-out fixtures (ADR-032): `pulse evolve setup DIR`, then one
  // write per fixture. The hub answers with counts only and never reads a fixture back.
  heldoutSetup: (dir: string) => post<HeldoutPayload>("heldout", { dir } satisfies HeldoutSetupInput),
  heldoutFixture: (input: HeldoutFixtureInput) => post<HeldoutPayload>("heldout/fixtures", input),
  // Keep PULSE running (ADR-038): `pulse service install` from the console. dry_run:true previews.
  serviceInstall: (dryRun: boolean) => post<ServiceInstallResult>("service", { dry_run: dryRun }),
  // The daemon's push key (ADR-042): the same steps as `pulse cred`. The token
  // is posted once and kept by the hub machine; a set probes every upstream
  // and saves only when they all answer; a refusal carries the public key.
  credToken: (token: string) => post<{ stored_in: string; report: CredReport }>("cred/token", { token }),
  credVaults: () => post<{ vaults: CredVault[] }>("cred/op/vaults", {}),
  credItems: (vault: string) => post<{ vault: string; items: CredItem[] }>("cred/op/items", { vault }),
  credSet: (reference: string, probe = true) => post<{ result: CredSetResult; report: CredReport }>("cred/set", { reference, probe }),
  credClear: (token: boolean) => post<{ cleared: CredCleared; report: CredReport }>("cred/clear", { token }),
  credGenerate: (replace: boolean) => post<{ generated: CredGenerated }>("cred/generate", { replace }),
};
