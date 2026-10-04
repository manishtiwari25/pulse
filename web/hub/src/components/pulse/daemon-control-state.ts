import type { DaemonRestart, Runner, ServiceInstallResult, ServicePayload, SettingsPayload } from "@/lib/api";
import type { Tone } from "@/lib/format";
import type { Loaded } from "@/lib/use-api";

export type DaemonServiceState = "unknown" | "unsupported" | "missing" | "unloaded" | "loaded";
export type ServiceSetupAction = "install" | "start" | "finish";
export type ServiceSetupPhase = "preview" | "installing" | "handover" | "done" | "late";

export function daemonServiceState(payload: ServicePayload | undefined): DaemonServiceState {
  if (!payload) return "unknown";
  if (!payload.service.supported) return "unsupported";
  const unit = payload.service.units.find((u) => u.name === "daemon");
  if (unit?.loaded || (!unit && payload.service.installed)) return "loaded";
  if (!unit) return "unknown";
  return unit.present ? "unloaded" : "missing";
}

export function serviceSetupAction(payload: ServicePayload | undefined): ServiceSetupAction | null {
  const state = daemonServiceState(payload);
  if (state === "missing") return "install";
  if (state === "unloaded") return "start";
  const daemon = payload?.service.units.find((u) => u.name === "daemon");
  const hub = payload?.service.units.find((u) => u.name === "hub");
  // A missing daemon file would be rewritten and reloaded, even if its unit is still loaded.
  return state === "loaded" && daemon?.present && daemon.loaded && (!hub || !hub.present || !hub.loaded) ? "finish" : null;
}

export function canApplyServiceSetup({ phase, supported, preview, previewError, disabled, blockedReason }: { phase: ServiceSetupPhase; supported: boolean; preview: ServiceInstallResult | null; previewError: string; disabled: boolean; blockedReason: string }): boolean {
  return phase === "preview" && supported && !!preview?.dry_run && !preview.installed && !previewError && !disabled && !blockedReason;
}

type Snapshot<T> = Pick<Loaded<T>, "data" | "error">;

export interface DaemonControlSnapshots {
  runner: Snapshot<Runner>;
  service: Snapshot<ServicePayload>;
  settings: Snapshot<SettingsPayload>;
}

export interface DaemonControlState {
  label: string;
  tone: Tone;
  description: string;
  // A newer pulse is built and waits for the runs in flight; null otherwise.
  update: DaemonUpdate | null;
  pause: "on" | "off" | "unknown";
  canResume: boolean;
  service: DaemonServiceState;
  setup: ServiceSetupAction | null;
  canPreview: boolean;
  setupBlock: string;
}

export interface DaemonUpdateWait {
  agent: string;
  kind: string;
  project: string;
  job: string;
  started_at: string;
}

export interface DaemonUpdate {
  since: string | null;
  inflight: number;
  // False once the daemon stopped taking new work and only waits for the runs left.
  dispatching: boolean;
  // The runs the new build waits for, slowest first; empty when an older
  // daemon only wrote `restarting: N in flight`.
  waits: DaemonUpdateWait[];
}

// The update a live heartbeat announces, from the structured `restart` record
// when the daemon writes one, else from the older `restarting: N in flight` note.
export function pendingUpdate(daemon: Pick<Runner["daemon"], "note" | "restart">, now: number = Date.now()): DaemonUpdate | null {
  const r = daemon.restart;
  if (r && typeof r === "object" && Array.isArray(r.waits_for)) {
    const waits = r.waits_for
      .filter((w): w is DaemonRestart["waits_for"][number] => !!w && typeof w === "object")
      .map((w) => ({ agent: String(w.agent || ""), kind: String(w.kind || "run"), project: String(w.project || ""), job: String(w.job || ""), started_at: String(w.started_at || "") }))
      .sort((a, b) => (startedAt(a) ?? now) - (startedAt(b) ?? now));
    return { since: r.since || null, inflight: Number.isFinite(Number(r.inflight)) ? Number(r.inflight) : waits.length, dispatching: r.dispatching !== false, waits };
  }
  const m = /^restarting:\s*(\d+)\s+in flight/.exec(daemon.note || "");
  if (!m) return daemon.note?.startsWith("restarting") ? { since: null, inflight: 0, dispatching: true, waits: [] } : null;
  return { since: null, inflight: Number(m[1]), dispatching: true, waits: [] };
}

// This module is run by node straight from the `.ts` source in tests, so it
// keeps its small formatting local rather than importing `@/lib/format`.
function startedAt(w: DaemonUpdateWait): number | null {
  const t = Date.parse(w.started_at);
  return Number.isFinite(t) ? t : null;
}

function count(n: number, noun: string): string {
  return n + " " + noun + (n === 1 ? "" : "s");
}

function runAge(w: DaemonUpdateWait, now: number): string {
  const t = startedAt(w);
  if (t === null) return "";
  const min = Math.max(0, Math.round((now - t) / 60000));
  if (min < 1) return "just started";
  if (min < 60) return min + "m so far";
  return Math.floor(min / 60) + "h " + (min % 60) + "m so far";
}

function describeWait(w: DaemonUpdateWait, project: boolean, now: number): string {
  const who = (w.agent ? w.agent + "'s " : "") + w.kind + (project && w.project ? " on " + w.project : "");
  const age = runAge(w, now);
  return age ? who + " (" + age + ")" : who;
}

// The short phrase for the home hero pill and the controls' status line:
// which run the new build waits for and how long it has run. Never a count
// of minutes left - the console does not know the run's budget.
export function daemonUpdateText(update: DaemonUpdate | null | undefined, now: number = Date.now()): string {
  if (!update) return "";
  const [first] = update.waits;
  if (!first) return update.inflight > 0 ? "New build waits for " + count(update.inflight, "run") + " in flight" : "New build swapping in";
  if (update.waits.length === 1) return "New build waits for " + describeWait(first, false, now);
  return "New build waits for " + count(update.waits.length, "run") + ", the longest " + describeWait(first, false, now);
}

function updateDescription(update: DaemonUpdate, now: number): string {
  const named = update.waits.slice(0, 3).map((w) => describeWait(w, true, now));
  const more = update.waits.length - named.length;
  const list = named.length ? named.join(", ") + (more > 0 ? " and " + count(more, "more run") : "") : update.inflight > 0 ? count(update.inflight, "run") + " in flight" : "its last runs";
  const meanwhile = update.dispatching ? " Meanwhile it starts only runs that end before those do." : " It has stopped taking new work.";
  return "The daemon is running and has a newer pulse built. It swaps to the new build once the runs it supervised when that build landed finish: " + list + "." + meanwhile + " Its service brings the new build back within seconds; nothing to start by hand.";
}

export function daemonControlState({ runner, service, settings }: DaemonControlSnapshots, now: number = Date.now()): DaemonControlState {
  const stop = !settings.error && settings.data?.available ? settings.data.settings.find((s) => s.name === "stop")?.value : undefined;
  const pause = stop === "1" ? "on" : stop === "0" ? "off" : "unknown";
  const serviceState = service.error ? "unknown" : daemonServiceState(service.data);
  const setup = service.error ? null : serviceSetupAction(service.data);
  const r = runner.error || !runner.data?.available ? undefined : runner.data;
  const heartbeat = r?.daemon.state;
  // The store writes `alive` for a fresh loop heartbeat and `silent` for a stale
  // one; `running` is kept for older hubs. A live daemon whose heartbeat carries
  // a `restart` record (or an older `restarting:` note) is still running: it
  // has a newer pulse built and swaps once the runs it supervises finish.
  const live = heartbeat === "running" || heartbeat === "alive";
  const update = live && r ? pendingUpdate(r.daemon, now) : null;
  const mayBeAlive = live || heartbeat === "silent";
  const knownStopped = heartbeat === "stopped" || heartbeat === "off" || heartbeat === "exited" || heartbeat === "never started" || heartbeat === "no heartbeat" || heartbeat === "no-heartbeat";
  const startsDaemon = setup === "install" || setup === "start";
  const controls = {
    update,
    pause,
    canResume: pause === "on" && !!settings.data?.can_edit,
    service: serviceState,
    setup,
    canPreview: !!setup && !!r,
    // Finishing the hub leaves an unchanged loaded daemon alone; only daemon setup needs this guard.
    setupBlock: startsDaemon && mayBeAlive ? "A daemon may already be running outside the loaded service. Check pulse status and its recorded PID, and stop that foreground daemon yourself before applying service setup. This control will not stop it or start a second copy while its heartbeat still reports running, alive or silent." : startsDaemon && r && !knownStopped ? "The heartbeat state is not recognized. Inspect pulse status and refresh before applying service setup; an unknown state is not proof that no daemon is running." : "",
  } satisfies Omit<DaemonControlState, "label" | "tone" | "description">;

  if (pause === "on") return { ...controls, label: "paused", tone: "warn", description: "The owner's pause is on (stop = 1). Resume clears that setting only; it does not start a missing daemon or remove any STOP file." };
  if (runner.error) return { ...controls, label: "unknown", tone: "warn", description: "Daemon status could not be refreshed. Any heartbeat shown below is the last reported value, not confirmation that it is running now." };
  if (!runner.data) return { ...controls, label: "checking", tone: "neutral", description: "Checking the daemon's heartbeat and the controls available on this machine." };
  if (!r) return { ...controls, label: "unavailable", tone: "warn", description: "The runner store is unavailable on this hub. Refresh status before trying to start a daemon." };
  if (r.runner?.allowed === false) return { ...controls, label: "stopped", tone: "warn", description: "New work is stopped: " + (r.runner.reason || "the runner does not allow work") + ". Service setup does not clear this stop." };
  switch (heartbeat) {
    case "running":
      return { ...controls, label: "running", tone: "ok", description: update ? updateDescription(update, now) : "The daemon reports a recent heartbeat. Work still follows the owner's pause, STOP files, approvals and existing caps." };
    case "alive":
      return { ...controls, label: "alive", tone: "ok", description: update ? updateDescription(update, now) : "The daemon's loop heartbeat is recent. Work still follows the owner's pause, STOP files, approvals and existing caps; whether a worker can make progress is a separate question." };
    case "silent":
      return { ...controls, label: "silent", tone: "warn", description: "The loop heartbeat is stale, not simply off. The daemon may still be alive or waiting; inspect its status and logs rather than starting another copy or automatically restarting it." };
    case "exited":
      return { ...controls, label: "stopped", tone: "warn", description: "The daemon wrote its last heartbeat on exit" + (r.daemon.at ? " at " + r.daemon.at : "") + ". A service-managed daemon is started again within seconds - refresh before doing anything; a foreground daemon needs you to start it." };
    case "stopped":
    case "off":
      return { ...controls, label: "stopped", tone: "warn", description: "The daemon is reported stopped. If its service is missing or unloaded, review the service setup below; a loaded service needs inspection, not a duplicate start." };
    case "never started":
    case "no heartbeat":
    case "no-heartbeat":
      return { ...controls, label: "no heartbeat", tone: "warn", description: "No daemon loop heartbeat has been recorded. This is not an owner-disable setting, and a loaded service alone does not confirm that a loop has started." };
    default:
      return { ...controls, label: "unknown", tone: "warn", description: "The daemon heartbeat is unreadable or its state is not recognized. Refresh status or inspect pulse status before trying to start it." };
  }
}

// The one short phrase the home hero pill and the controls' status line show
// for a daemon state. Only a known stop reads "off": a fresh heartbeat is
// running (with a pending update named in its own pill), and a stale or
// unreadable one is named rather than called off, so nobody starts a second
// daemon on a guess.
export function daemonHeadline(state: Pick<DaemonControlState, "label" | "pause">): string {
  if (state.pause === "on") return "Daemon paused";
  switch (state.label) {
    case "running":
    case "alive":
      return "Daemon running";
    case "silent":
      return "Daemon silent";
    case "checking":
      return "Checking daemon…";
    case "unknown":
    case "unavailable":
      return "Daemon status unknown";
    default:
      return "Daemon off";
  }
}
