"use client";

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { api, errorText, type StoreEvent } from "./api";
import { Coalescer, touchedBy } from "./refresh";

// A small SWR-style cache: one entry per API path, shared by every component
// that asks for it, refreshed on an interval while the tab is visible, and
// invalidated by the event stream. Nothing here knows about React beyond the
// hooks at the bottom, so the mock server and tests can drive it too.
//
// Every path has at most one request in flight. A refetch asked for during
// that flight runs once after it, so a burst of invalidations costs two
// requests at most, never a pile of parallel ones. The stream's own
// invalidations go through refresh.ts: each event action names the paths it
// can change, they coalesce into one refresh per quiet window, and nothing
// refreshes while the tab is hidden.

interface Entry {
  data: unknown;
  error: string;
  loading: boolean;
  updatedAt: number;
  startedAt: number; // when the latest request began; a request that began after a change already reflects it
  inflight: Promise<void> | null;
  queued: Promise<void> | null; // the one follow-up fetch promised to callers who invalidated mid-flight
  subscribers: Set<() => void>;
  refresh: number;
  version: number; // bumped on every change so useSyncExternalStore sees it
}

const cache = new Map<string, Entry>();

function entry(path: string): Entry {
  let e = cache.get(path);
  if (!e) {
    e = { data: undefined, error: "", loading: true, updatedAt: 0, startedAt: 0, inflight: null, queued: null, subscribers: new Set(), refresh: 0, version: 0 };
    cache.set(path, e);
  }
  return e;
}

function notify(e: Entry) {
  e.version++;
  for (const fn of e.subscribers) fn();
}

function run(e: Entry, path: string): Promise<void> {
  e.startedAt = Date.now();
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

// Single flight per path. `force` while a request is in flight queues one
// follow-up (shared by everyone who asks during the flight) and resolves
// when that follow-up lands, so `await reload()` always sees fresh data.
export function load(path: string, force = false): Promise<void> {
  const e = entry(path);
  if (!e.inflight) return run(e, path);
  if (!force) return e.inflight;
  if (!e.queued) {
    e.queued = e.inflight.then(() => {
      e.queued = null;
      return run(e, path);
    });
  }
  return e.queued;
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

// The stream's invalidations: one refresh per quiet window of the paths the
// pending actions touch (refresh.ts), skipping paths nobody shows and paths
// whose latest request began after the last event arrived - that request
// already reflects the change, whether it has landed or is still in flight.
const streamRefresh = new Coalescer((actions, lastAt) => {
  const touched = touchedBy(actions);
  for (const [path, e] of cache) {
    if (!e.subscribers.size || e.startedAt > lastAt || !touched(path)) continue;
    void load(path, true);
  }
});

export function pushEvent(ev: StoreEvent) {
  if (ev.id && ev.id <= live.lastId) return;
  live.lastId = Math.max(live.lastId, ev.id || 0);
  live.events = [ev, ...live.events].slice(0, LIVE_MAX);
  liveNotify();
  streamRefresh.add(ev.action);
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

// The hub names its build on the event stream (`X-Pulse-Version`). When a
// tab that was opened on one build reconnects to another - PULSE updated
// itself, or the owner installed a new binary - the pages it holds are
// stale, so it reloads itself; a tab with words half-typed waits until the
// box is left.
let serverVersion = "";
let reloadTimer: ReturnType<typeof setTimeout> | null = null;

function noteServerVersion(v: string | null) {
  if (!v) return;
  if (!serverVersion) {
    serverVersion = v;
    return;
  }
  if (v !== serverVersion && !reloadTimer) scheduleReload();
}

function typingSomewhere(): boolean {
  const el = document.activeElement;
  return (el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement) && el.value.trim().length > 0;
}

function scheduleReload() {
  if (typeof window === "undefined") return;
  if (!typingSomewhere()) {
    window.location.reload();
    return;
  }
  reloadTimer = setTimeout(() => {
    reloadTimer = null;
    scheduleReload();
  }, 5000);
}

// The stream is read with fetch rather than EventSource so every `event:`
// name reaches the handler (EventSource only fires listeners you named) and
// so `since` can be sent on reconnect. Falls back to polling while down.
async function readStream(signal: AbortSignal) {
  // Reopening after a hidden spell keeps "live" rather than flashing "reconnecting".
  if (live.state !== "live") setState("connecting");
  try {
    const r = await fetch("/api/v1/events/stream" + (live.lastId ? "?since=" + live.lastId : ""), { signal, headers: { accept: "text/event-stream" }, cache: "no-store" });
    if (!r.ok || !r.body) throw new Error("stream " + r.status);
    noteServerVersion(r.headers.get("x-pulse-version"));
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

// A hidden tab stops refreshing at once and, after a minute, drops its
// stream connection too: a background transcript would otherwise keep
// answering every harness.step with a fetch. Coming back reopens the stream
// with `since`, so the hub replays what was missed, and flushes once.
const HIDDEN_CLOSE_MS = 60000;
let hiddenTimer: ReturnType<typeof setTimeout> | null = null;

function onVisibility() {
  if (document.hidden) {
    streamRefresh.pause();
    if (!hiddenTimer) hiddenTimer = setTimeout(closeStream, HIDDEN_CLOSE_MS);
    return;
  }
  if (hiddenTimer) clearTimeout(hiddenTimer);
  hiddenTimer = null;
  if (streamUsers) openStream();
  streamRefresh.resume();
}

// Mount once in the shell: opens the SSE connection, keeps the last 200
// events for the live feed, and invalidates the queries each event touches.
export function useEventStream(): { state: StreamState; events: StoreEvent[] } {
  useEffect(() => {
    streamUsers++;
    if (streamUsers === 1) document.addEventListener("visibilitychange", onVisibility);
    onVisibility();
    return () => {
      streamUsers--;
      if (streamUsers === 0) {
        document.removeEventListener("visibilitychange", onVisibility);
        if (hiddenTimer) clearTimeout(hiddenTimer);
        hiddenTimer = null;
        closeStream();
      }
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
