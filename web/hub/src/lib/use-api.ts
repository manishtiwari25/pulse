"use client";

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { api, errorText, type StoreEvent } from "./api";

// A small SWR-style cache: one entry per API path, shared by every component
// that asks for it, refreshed on an interval while the tab is visible, and
// invalidated by the event stream. Nothing here knows about React beyond the
// hooks at the bottom, so the mock server and tests can drive it too.

interface Entry {
  data: unknown;
  error: string;
  loading: boolean;
  updatedAt: number;
  inflight: Promise<void> | null;
  subscribers: Set<() => void>;
  refresh: number;
  version: number; // bumped on every change so useSyncExternalStore sees it
}

const cache = new Map<string, Entry>();

function entry(path: string): Entry {
  let e = cache.get(path);
  if (!e) {
    e = { data: undefined, error: "", loading: true, updatedAt: 0, inflight: null, subscribers: new Set(), refresh: 0, version: 0 };
    cache.set(path, e);
  }
  return e;
}

function notify(e: Entry) {
  e.version++;
  for (const fn of e.subscribers) fn();
}

export function load(path: string, force = false): Promise<void> {
  const e = entry(path);
  if (e.inflight && !force) return e.inflight;
  const p = api<unknown>(path)
    .then((data) => {
      e.data = data;
      e.error = "";
    })
    .catch((err: unknown) => {
      e.error = errorText(err).error;
    })
    .finally(() => {
      e.loading = false;
      e.updatedAt = Date.now();
      e.inflight = null;
      notify(e);
    });
  e.inflight = p;
  return p;
}

// Refetch every subscribed path whose key matches. `invalidate()` with no
// argument refreshes everything on screen.
export function invalidate(match?: string | RegExp | ((path: string) => boolean)) {
  for (const [path, e] of cache) {
    if (!e.subscribers.size) continue;
    const hit = match === undefined || (typeof match === "string" ? path.startsWith(match) : match instanceof RegExp ? match.test(path) : match(path));
    if (hit) void load(path, true);
  }
}

export function peek<T>(path: string): T | undefined {
  return cache.get(path)?.data as T | undefined;
}

export interface Loaded<T> {
  data: T | undefined;
  error: string;
  loading: boolean;
  updatedAt: number;
  reload: () => Promise<void>;
}

const DEFAULT_REFRESH = 30000;

export function useApi<T>(path: string | null, opts: { refresh?: number | false } = {}): Loaded<T> {
  const refresh = opts.refresh === false ? 0 : (opts.refresh ?? DEFAULT_REFRESH);
  const subscribe = useCallback(
    (cb: () => void) => {
      if (!path) return () => {};
      const e = entry(path);
      e.subscribers.add(cb);
      if (e.updatedAt === 0 || Date.now() - e.updatedAt > (refresh || DEFAULT_REFRESH)) void load(path);
      return () => {
        e.subscribers.delete(cb);
      };
    },
    [path, refresh],
  );
  const snapshot = useCallback(() => (path ? (cache.get(path)?.version ?? 0) : -1), [path]);
  const version = useSyncExternalStore(subscribe, snapshot, () => -1);
  const e = path ? cache.get(path) : undefined;

  // Visibility-aware polling: skip while hidden, refetch once when the tab
  // comes back if the entry aged past the interval.
  useEffect(() => {
    if (!path || !refresh) return;
    const tick = () => {
      if (typeof document !== "undefined" && document.hidden) return;
      const cur = cache.get(path);
      if (cur && Date.now() - cur.updatedAt >= refresh - 250) void load(path);
    };
    const id = setInterval(tick, refresh);
    const onVis = () => {
      if (!document.hidden) tick();
    };
    document.addEventListener("visibilitychange", onVis);
    return () => {
      clearInterval(id);
      document.removeEventListener("visibilitychange", onVis);
    };
  }, [path, refresh]);

  const reload = useCallback(() => (path ? load(path, true) : Promise.resolve()), [path]);
  return useMemo(
    () => ({ data: (e?.data as T | undefined) ?? undefined, error: e?.error ?? "", loading: path ? (e?.loading ?? true) : false, updatedAt: e?.updatedAt ?? 0, reload }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [version, e, path, reload],
  );
}

// ----- the event stream --------------------------------------------------

// Which cached paths an event touches. Anything unmatched refreshes the
// whole screen after a short debounce, so a new event action never leaves a
// stale view; matched actions refresh only what changed.
const TOUCHES: [RegExp, (path: string) => boolean][] = [
  [/^task\./, (p) => p.startsWith("tasks") || p.startsWith("messages") || p.startsWith("events")],
  [/^(message|msg)\./, (p) => p.startsWith("messages") || p.startsWith("tasks/") || p.startsWith("events")],
  [/^compute\./, (p) => p.startsWith("compute") || p.startsWith("executors") || p.startsWith("runner") || p.startsWith("proposals") || p.startsWith("events")],
  // Transcript panels consume harness.step from live.events and fetch ?after=N, so do not full-refetch every step.
  [/^harness\.(?!step$)/, (p) => p.startsWith("harness/sessions") || p.startsWith("harness") || p.startsWith("events")],
  [/^settings\./, (p) => p.startsWith("settings") || p.startsWith("compute") || p.startsWith("runner") || p.startsWith("jobs/failed") || p.startsWith("events")],
  [/^record/, (p) => p.startsWith("records") || p.startsWith("constitution") || p.startsWith("events")],
  [/^constitution\./, (p) => p.startsWith("constitution") || p.startsWith("records") || /amendments/.test(p) || p.startsWith("events")],
  [/^executor\./, (p) => p.startsWith("executors") || p.startsWith("runner") || p.startsWith("compute") || p.startsWith("events")],
  [/^(job|run|daemon)\./, (p) => p.startsWith("runner") || p.startsWith("jobs/failed") || p.startsWith("tasks") || p.startsWith("events") || /\/(overview|timeline)$/.test(p)],
];

export type StreamState = "connecting" | "live" | "polling";

interface LiveStore {
  state: StreamState;
  events: StoreEvent[];
  lastId: number;
  subscribers: Set<() => void>;
}

const live: LiveStore = { state: "connecting", events: [], lastId: 0, subscribers: new Set() };
const LIVE_MAX = 200;

function liveNotify() {
  for (const fn of live.subscribers) fn();
}

let pending = new Set<string>();
let pendingAll = false;
let flushTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleFlush() {
  if (flushTimer) return;
  flushTimer = setTimeout(() => {
    flushTimer = null;
    if (pendingAll) invalidate();
    else {
      const actions = [...pending];
      invalidate((path) => actions.some((a) => TOUCHES.find(([rx]) => rx.test(a))?.[1](path) ?? false) || /^(org|activity|timeline|notices|health)/.test(path) || /^projects\//.test(path));
    }
    pending = new Set();
    pendingAll = false;
  }, 400);
}

export function pushEvent(ev: StoreEvent) {
  if (ev.id && ev.id <= live.lastId) return;
  live.lastId = Math.max(live.lastId, ev.id || 0);
  live.events = [ev, ...live.events].slice(0, LIVE_MAX);
  liveNotify();
  if (TOUCHES.some(([rx]) => rx.test(ev.action))) pending.add(ev.action);
  else pendingAll = true;
  scheduleFlush();
}

let streamAbort: AbortController | null = null;
let streamUsers = 0;
let pollTimer: ReturnType<typeof setInterval> | null = null;
let retryMs = 1000;

function setState(s: StreamState) {
  if (live.state !== s) {
    live.state = s;
    liveNotify();
  }
}

function startPolling() {
  if (pollTimer) return;
  pollTimer = setInterval(async () => {
    if (document.hidden) return;
    try {
      const r = await api<{ events: StoreEvent[] }>("events?since=" + live.lastId + "&limit=100");
      for (const ev of [...(r.events || [])].sort((a, b) => a.id - b.id)) pushEvent(ev);
    } catch {
      // the polling fallback stays quiet; useApi's own intervals still run
    }
  }, 10000);
}

function stopPolling() {
  if (pollTimer) clearInterval(pollTimer);
  pollTimer = null;
}

// One SSE frame: `id:` / `event:` / `data:` lines, blank line ends it.
function handleFrame(frame: string) {
  let id = "";
  let type = "";
  const data: string[] = [];
  for (const line of frame.split("\n")) {
    if (!line || line.startsWith(":")) continue;
    const i = line.indexOf(":");
    const field = i < 0 ? line : line.slice(0, i);
    const value = i < 0 ? "" : line.slice(i + 1).replace(/^ /, "");
    if (field === "id") id = value;
    else if (field === "event") type = value;
    else if (field === "data") data.push(value);
  }
  if (!data.length) return;
  try {
    const ev = JSON.parse(data.join("\n")) as StoreEvent;
    if (typeof ev.id !== "number" && id) ev.id = Number(id) || 0;
    if (!ev.action && type) ev.action = type;
    pushEvent(ev);
  } catch {
    // malformed frames are ignored
  }
}

// The stream is read with fetch rather than EventSource so every `event:`
// name reaches the handler (EventSource only fires listeners you named) and
// so `since` can be sent on reconnect. Falls back to polling while down.
async function readStream(signal: AbortSignal) {
  setState("connecting");
  try {
    const r = await fetch("/api/v1/events/stream" + (live.lastId ? "?since=" + live.lastId : ""), { signal, headers: { accept: "text/event-stream" }, cache: "no-store" });
    if (!r.ok || !r.body) throw new Error("stream " + r.status);
    setState("live");
    stopPolling();
    retryMs = 1000;
    const reader = r.body.getReader();
    const dec = new TextDecoder();
    let buf = "";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true }).replace(/\r\n/g, "\n");
      let i: number;
      while ((i = buf.indexOf("\n\n")) >= 0) {
        handleFrame(buf.slice(0, i));
        buf = buf.slice(i + 2);
      }
    }
  } catch {
    // fall through to the retry below
  }
  if (signal.aborted) return;
  setState("polling");
  startPolling();
  const wait = retryMs;
  retryMs = Math.min(retryMs * 2, 30000);
  setTimeout(() => {
    if (!signal.aborted) void readStream(signal);
  }, wait);
}

function openStream() {
  if (streamAbort || typeof window === "undefined") return;
  streamAbort = new AbortController();
  void readStream(streamAbort.signal);
}

function closeStream() {
  streamAbort?.abort();
  streamAbort = null;
  stopPolling();
}

// Mount once in the shell: opens the SSE connection, keeps the last 200
// events for the live feed, and invalidates the queries each event touches.
export function useEventStream(): { state: StreamState; events: StoreEvent[] } {
  useEffect(() => {
    streamUsers++;
    openStream();
    const onVis = () => {
      if (!document.hidden && !streamAbort) openStream();
    };
    document.addEventListener("visibilitychange", onVis);
    return () => {
      streamUsers--;
      document.removeEventListener("visibilitychange", onVis);
      if (streamUsers === 0) closeStream();
    };
  }, []);
  return useLiveEvents();
}

export function useLiveEvents(): { state: StreamState; events: StoreEvent[] } {
  const subscribe = useCallback((cb: () => void) => {
    live.subscribers.add(cb);
    return () => {
      live.subscribers.delete(cb);
    };
  }, []);
  const snap = useCallback(() => live.events, []);
  const events = useSyncExternalStore(subscribe, snap, () => live.events);
  const state = useSyncExternalStore(
    subscribe,
    () => live.state,
    () => "connecting" as StreamState,
  );
  return { state, events };
}

// A ticking "now" so relative times re-render without a full reload.
export function useNow(ms = 30000): number {
  const [now, setNow] = useState(() => Date.now());
  const ref = useRef(ms);
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), ref.current);
    return () => clearInterval(id);
  }, []);
  return now;
}
