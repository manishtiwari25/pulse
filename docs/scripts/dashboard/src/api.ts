// The API client. Every route is same-origin JSON under /api/v1/ (the hub) or
// <base>/api/v1/ (one project or the commons); the server stamps the page
// with where it sits. Types are written from the JSON the routes return
// today - the contract ADR-023 leaves unchanged.

export type Mode = 'org' | 'project' | 'commons';

export interface Place {
  mode: Mode;
  base: string;
  hub: boolean;
  missing: string;
}

export function readPlace(): Place {
  const d = document.body.dataset;
  const mode = d.mode === 'org' || d.mode === 'commons' ? d.mode : 'project';
  return { mode, base: d.base || '', hub: d.hub === '1', missing: d.missing || '' };
}

export const PLACE: Place = readPlace();

export class ApiError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
  }
}

async function readJson<T>(r: Response): Promise<T> {
  let data: unknown;
  try {
    data = await r.json();
  } catch {
    data = { error: 'bad JSON' };
  }
  if (!r.ok) {
    const err = (data as { error?: string }).error;
    throw new ApiError(err || 'HTTP ' + r.status, r.status);
  }
  return data as T;
}

export function api<T>(route: string, base: string = PLACE.base): Promise<T> {
  return fetch(base + '/api/v1/' + route, { cache: 'no-store' }).then((r) => readJson<T>(r));
}

export function hubApi<T>(route: string): Promise<T> {
  return api<T>(route, '');
}

export function postJson<T>(route: string, payload: unknown): Promise<T> {
  return fetch('/api/v1/' + route, {
    method: 'POST',
    cache: 'no-store',
    headers: { 'Content-Type': 'application/json', 'X-Pulse-Decide': 'yes' },
    body: JSON.stringify(payload),
  }).then((r) => readJson<T>(r));
}

export const enc = encodeURIComponent;

// ----- shapes -----------------------------------------------------------

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

export interface Notices {
  ballot: { slug: string; base: string; name: string; brain: boolean; suggestions: number; amendments: number; proposals: number }[];
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
  running: number;
  free: number;
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

export interface ProjectInfo {
  built_at: string;
  repo: string;
  repo_name: string;
  branch: string;
  head: string;
  dirty: string;
  board_present: string;
  board_params?: { quorum?: number; expiry_days?: number; stale_days?: number };
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
}

export interface ArchiveTable {
  columns: string[];
  rows: { cells: string[] }[];
}

export interface Evolution {
  summary: EvolutionSummary;
  decisions: Record<string, number>;
  origins: Record<string, { tried: number; kept: number }>;
  archive: Record<string, ArchiveTable>;
  scope: { file_class: string; paths: string; stage: string; autonomy: string; clean_cycles_so_far: string; clean_cycles_required: string; incidents: string }[];
  product_notes: string[];
  hygiene: { kind: string; value: string }[];
}
