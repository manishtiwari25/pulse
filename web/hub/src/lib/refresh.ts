// What the event stream refreshes, and how often. Pure: no React and no DOM,
// so refresh.test.mjs can drive it. use-api.ts owns the cache and the fetches.
//
// The store emits one `harness.step` per agent step, one to two a second while
// agents work - 488k of the first 670k events. A console with several tabs
// open must not answer each of them by refetching every view, so each action
// names the cached paths it can change, the stream's requests are coalesced
// into one refresh per quiet window, and a hidden tab refreshes nothing until
// it is looked at again.

export type PathMatch = (path: string) => boolean;

// `starts("tasks")` matches tasks, tasks?project=x and tasks/ID - never taskstats.
export const starts =
  (...prefixes: string[]): PathMatch =>
  (p) =>
    prefixes.some((x) => p === x || p.startsWith(x + "?") || p.startsWith(x + "/"));

// A per-project board view, projects/<slug>/<route>; with no routes, any of them.
export const projectView =
  (...routes: string[]): PathMatch =>
  (p) => {
    const m = /^projects\/[^/?]+\/([^/?]+)/.exec(p);
    return !!m && (routes.length === 0 || routes.includes(m[1]));
  };

// The project registry, GET /projects (and nothing under it).
export const registry: PathMatch = (p) => p === "projects" || p.startsWith("projects?");

const any =
  (...ms: PathMatch[]): PathMatch =>
  (p) =>
    ms.some((m) => m(p));

const never: PathMatch = () => false;

// Views that summarise the whole hub: the org card list and its counters.
const SUMMARY = starts("org", "notices", "activity", "health");
// Everything a board post can change: counters, timelines, ballots, agents, every project view.
const BOARD = any(SUMMARY, starts("timeline", "votes", "roster"), projectView());
const RECORDS = any(starts("records", "constitution", "votes"), SUMMARY, registry, projectView("evolution", "amendments", "overview", "notices"));
const DAEMON = starts("runner", "running", "service", "hygiene", "roster");

// Which cached paths an event action touches, first match wins. Live feeds
// (events?…) are absent on purpose: they merge the stream's own buffer, so a
// refetch would only repeat what the tab already has.
export const TOUCHES: [RegExp, PathMatch][] = [
  // The Transcript panel consumes harness.step from the live buffer and fetches ?after=N itself; the sessions list polls.
  [/^harness\.step$/, never],
  [/^harness\./, starts("harness/sessions", "roster")],
  [/^(job|run)\.|^decide\.job$/, any(starts("runner", "jobs/failed", "roster", "tasks", "code-deliveries", "org"), registry, projectView("overview", "evolution"))],
  [/^task\./, starts("tasks", "messages", "code-deliveries", "roster")],
  [/^(message|msg)\.|^mention$/, starts("messages", "tasks")],
  [/^post(\.|$)|^question\./, BOARD],
  [/^decide(\.|$)/, any(BOARD, starts("compute", "records", "constitution", "tasks", "heldout"))],
  [/^(record|records|constitution)\./, RECORDS],
  [/^(evolve|evolution|fixture|metadata)\./, any(RECORDS, starts("heldout"))],
  [/^compute\./, starts("compute", "executors", "runner")],
  [/^executor\./, starts("compute", "executors", "runner", "roster")],
  [/^setting(s\.|$)/, starts("settings", "org", "runner", "running", "service", "harness", "heldout", "compute")],
  [/^cred\./, starts("cred", "settings", "runner", "harness")],
  [/^land(ing)?\./, any(starts("org", "code-deliveries", "runner", "tasks"), registry, projectView("overview", "project", "timeline", "evolution"))],
  [/^project\./, any(SUMMARY, registry, projectView(), starts("tasks", "timeline"))],
  [/^hook\./, any(starts("org", "settings"), registry, projectView("project", "overview"))],
  [/^schedule\./, any(starts("runner", "tasks", "org"), projectView("overview"))],
  [/^agent\./, any(starts("roster", "org", "runner", "hygiene"), projectView("overview"))],
  [/^(daemon|worktree|auto|rolled)\./, DAEMON],
];

// An action the table does not know refreshes only the summary views: a new
// kind of event never leaves the hub's headline counters stale, and never
// costs more than three requests.
export const UNKNOWN: PathMatch = starts("org", "notices", "timeline");

export function touches(action: string): PathMatch {
  return TOUCHES.find(([rx]) => rx.test(action))?.[1] ?? UNKNOWN;
}

// An action no view needs to hear about (harness.step): not even queued.
export function isSilent(action: string): boolean {
  return touches(action) === never;
}

// One predicate for a batch of actions, so a flush walks the cache once.
export function touchedBy(actions: Iterable<string>): PathMatch {
  const ms = Array.from(new Set(Array.from(actions, touches)));
  return (p) => ms.some((m) => m(p));
}

export interface FlushWindow {
  // How long the stream must be quiet before pending actions flush...
  quietMs: number;
  // ...and the longest a steady burst may postpone the flush.
  maxMs: number;
}

export const STREAM_WINDOW: FlushWindow = { quietMs: 1500, maxMs: 5000 };

// Trailing-edge debounce with a ceiling, pausable. Actions accumulate as a
// set (so a thousand job events in a burst are one entry), flush at most once per
// window, and wait while paused (the tab is hidden) for one flush on resume.
export class Coalescer {
  private readonly flush: (actions: Set<string>, lastAt: number) => void;
  private readonly window: FlushWindow;
  private actions = new Set<string>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private firstAt = 0;
  private lastAt = 0;
  private paused = false;

  constructor(flush: (actions: Set<string>, lastAt: number) => void, window: FlushWindow = STREAM_WINDOW) {
    this.flush = flush;
    this.window = window;
  }

  get pending(): number {
    return this.actions.size;
  }

  add(action: string, now = Date.now()): void {
    if (isSilent(action)) return;
    this.actions.add(action);
    this.lastAt = now;
    this.arm(now);
  }

  pause(): void {
    this.paused = true;
    this.clear();
  }

  // Back from hidden: whatever queued flushes after one quiet window, so the
  // reconnecting stream's replay and the hooks' own wake-up loads settle first.
  resume(now = Date.now()): void {
    this.paused = false;
    this.firstAt = 0;
    if (this.actions.size) this.arm(now);
  }

  private arm(now: number): void {
    if (this.paused) return;
    if (!this.firstAt) this.firstAt = now;
    const due = Math.min(now + this.window.quietMs, this.firstAt + this.window.maxMs);
    this.clear();
    this.timer = setTimeout(() => this.fire(), Math.max(0, due - now));
  }

  private clear(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }

  private fire(): void {
    this.clear();
    const actions = this.actions;
    const lastAt = this.lastAt;
    this.actions = new Set();
    this.firstAt = 0;
    if (actions.size) this.flush(actions, lastAt);
  }
}
