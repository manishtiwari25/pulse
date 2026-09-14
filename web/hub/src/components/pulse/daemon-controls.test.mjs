import assert from "node:assert/strict";
import { test } from "node:test";
import { canApplyServiceSetup, daemonControlState, daemonServiceState, serviceSetupAction } from "./daemon-control-state.ts";
import { HANDOVER_WAIT_MS, waitForServiceHandover } from "./service-handover.ts";
import { writes } from "../../lib/api.ts";

const snapshot = (data, error = "") => ({ data, error });
const service = ({ present = true, loaded = true, supported = true, hubPresent = true, hubLoaded = true } = {}) => ({
  service: {
    kind: "launchd", supported, installed: present && loaded,
    units: [{ name: "daemon", path: "/test/pulse.daemon.plist", present, loaded }, { name: "hub", path: "/test/pulse.hub.plist", present: hubPresent, loaded: hubLoaded }],
    binary: "/test/pulse", logs: "/test/logs",
  },
  hub_is_service: true, hub_port: 8765, hub_url: "http://127.0.0.1:8765", command: "pulse service install",
});
const runner = (state = "silent") => ({
  available: true,
  daemon: { state, pid: 25755, at: "2026-09-13T03:00:00Z", interval: 300, loops: 1 },
  runner: { allowed: true, reason: "" },
});
const settings = (stop = "0", selfUpdate = "off") => ({
  available: true, can_edit: true,
  settings: [{ name: "stop", value: stop }, { name: "daemon.self_update", value: selfUpdate }],
});
const inputs = (overrides = {}) => ({ runner: snapshot(runner()), service: snapshot(service()), settings: snapshot(settings()), ...overrides });
const preview = { dry_run: true, installed: false, handover: false, output: "preview only" };
const ready = (overrides = {}) => canApplyServiceSetup({ phase: "preview", supported: true, preview, previewError: "", disabled: false, blockedReason: "", ...overrides });
const flush = async () => { await Promise.resolve(); await Promise.resolve(); };

test("service controls distinguish missing, unloaded, loaded and unsupported units", () => {
  for (const [payload, expectedState, action] of [
    [service({ present: false, loaded: false }), "missing", "install"],
    [service({ present: true, loaded: false }), "unloaded", "start"],
    [service(), "loaded", null],
    [service({ supported: false, present: false, loaded: false }), "unsupported", null],
    [undefined, "unknown", null],
  ]) {
    assert.equal(daemonServiceState(payload), expectedState);
    assert.equal(serviceSetupAction(payload), action);
  }
  const fileRemoved = service({ present: false, loaded: true });
  assert.equal(fileRemoved.service.installed, false);
  assert.equal(serviceSetupAction(fileRemoved), null, "a loaded unit must not be started again even if its file was removed");
  const noUnit = service({ present: false, loaded: false });
  noUnit.service.units = [];
  assert.equal(daemonServiceState(noUnit), "unknown");
  assert.equal(serviceSetupAction(noUnit), null);
});

test("partial hub setup remains available when the daemon service is present and loaded", () => {
  for (const hub of [{ hubPresent: false, hubLoaded: false }, { hubPresent: true, hubLoaded: false }, { hubPresent: false, hubLoaded: true }]) {
    for (const installed of [false, true]) {
      const payload = service(hub);
      payload.service.installed = installed;
      assert.equal(daemonServiceState(payload), "loaded");
      assert.equal(serviceSetupAction(payload), "finish", "unit details must identify partial setup regardless of the aggregate installed flag");
    }
  }
  const noHubUnit = service();
  noHubUnit.service.units = noHubUnit.service.units.filter((u) => u.name !== "hub");
  assert.equal(serviceSetupAction(noHubUnit), "finish");
  assert.equal(serviceSetupAction(service()), null, "two present, loaded units need no setup action");
  assert.equal(serviceSetupAction(service({ hubLoaded: false, supported: false })), null);
});

test("finishing hub setup does not mistake a live managed daemon for a foreground daemon", () => {
  for (const hub of [{ hubPresent: false, hubLoaded: false }, { hubPresent: true, hubLoaded: false }]) {
    for (const heartbeat of ["running", "alive", "silent"]) {
      const state = daemonControlState(inputs({ service: snapshot(service(hub)), runner: snapshot(runner(heartbeat)) }));
      assert.equal(state.label, heartbeat);
      assert.equal(state.setup, "finish");
      assert.equal(state.canPreview, true);
      assert.equal(state.setupBlock, "");
      assert.equal(ready({ disabled: !state.canPreview, blockedReason: state.setupBlock }), true);
      assert.equal(ready({ phase: "installing", disabled: !state.canPreview, blockedReason: state.setupBlock }), false);
    }
  }
  const paused = daemonControlState(inputs({ service: snapshot(service({ hubLoaded: false })), settings: snapshot(settings("1")) }));
  assert.equal(paused.setup, "finish");
  assert.equal(paused.setupBlock, "");
  assert.equal(paused.pause, "on");
  assert.equal(paused.label, "paused", "finishing the console setup must not clear the owner pause");
});

test("partial hub setup cannot bypass guards for a foreground or unknown-managed daemon", () => {
  for (const present of [false, true]) {
    const state = daemonControlState(inputs({ service: snapshot(service({ present, loaded: false, hubLoaded: false })), runner: snapshot(runner("running")) }));
    assert.equal(state.setup, present ? "start" : "install");
    assert.match(state.setupBlock, /may already be running/);
    assert.equal(ready({ disabled: !state.canPreview, blockedReason: state.setupBlock }), false);
  }
  const missingDaemonFile = service({ present: false, loaded: true, hubLoaded: false });
  const unknownDaemonUnit = service({ hubLoaded: false });
  unknownDaemonUnit.service.units = unknownDaemonUnit.service.units.filter((u) => u.name !== "daemon");
  for (const payload of [missingDaemonFile, unknownDaemonUnit]) {
    payload.service.installed = true;
    const state = daemonControlState(inputs({ service: snapshot(payload), runner: snapshot(runner("running")) }));
    assert.equal(state.setup, null, "a loaded daemon with a missing file or unknown unit must not be rewritten to finish the hub");
    assert.equal(state.canPreview, false);
    assert.equal(ready({ disabled: !state.canPreview, blockedReason: state.setupBlock }), false);
  }
  const unknown = daemonControlState(inputs({ service: snapshot(service({ loaded: false, hubLoaded: false })), runner: snapshot(runner("unreadable")) }));
  assert.match(unknown.setupBlock, /not proof that no daemon is running/);
  assert.equal(ready({ disabled: !unknown.canPreview, blockedReason: unknown.setupBlock }), false);
});

test("finishing partial hub setup still requires fresh runner and service snapshots", () => {
  const payload = service({ hubLoaded: false });
  const staleRunner = daemonControlState(inputs({ service: snapshot(payload), runner: snapshot(runner("running"), "offline") }));
  assert.equal(staleRunner.setup, "finish");
  assert.equal(staleRunner.canPreview, false);
  assert.equal(ready({ disabled: !staleRunner.canPreview, blockedReason: staleRunner.setupBlock }), false);
  const staleService = daemonControlState(inputs({ service: snapshot(payload, "offline"), runner: snapshot(runner("running")) }));
  assert.equal(staleService.setup, null);
  assert.equal(staleService.canPreview, false);
});

test("owner pause is distinct from automatic rebuilds and a service install", () => {
  const paused = daemonControlState(inputs({ settings: snapshot(settings("1", "off")), runner: snapshot(runner("running")) }));
  assert.equal(paused.label, "paused");
  assert.equal(paused.canResume, true);
  assert.equal(paused.setup, null);
  assert.match(paused.description, /does not start a missing daemon or remove any STOP file/);
  for (const selfUpdate of ["on", "off"]) {
    const live = daemonControlState(inputs({ settings: snapshot(settings("0", selfUpdate)), runner: snapshot(runner("running")) }));
    assert.equal(live.label, "running");
    assert.equal(live.canResume, false);
  }
  assert.equal(daemonControlState(inputs({ settings: snapshot(settings("0")), runner: snapshot(runner("silent")) })).label, "silent", "saving Resume must not manufacture a running heartbeat");
  assert.equal(daemonControlState(inputs({ service: snapshot(service()), runner: snapshot(runner("never started")) })).label, "no heartbeat", "installing a service must not manufacture a heartbeat");
});

test("a stale heartbeat with a loaded service is silent, never off or a duplicate-start action", () => {
  const state = daemonControlState(inputs());
  assert.equal(state.label, "silent");
  assert.equal(state.setup, null);
  assert.equal(state.canPreview, false);
  assert.match(state.description, /may still be alive/);
  assert.match(state.description, /rather than starting another copy/);
});

test("missing and unloaded service setup previews cannot apply over a possibly live foreground daemon", () => {
  for (const present of [false, true]) {
    for (const heartbeat of ["running", "alive", "silent"]) {
      const state = daemonControlState(inputs({ service: snapshot(service({ present, loaded: false })), runner: snapshot(runner(heartbeat)) }));
      assert.equal(state.setup, present ? "start" : "install");
      assert.equal(state.canPreview, true);
      assert.match(state.setupBlock, /may already be running/);
      assert.equal(ready({ blockedReason: state.setupBlock }), false);
    }
    for (const heartbeat of ["stopped", "never started"]) {
      const state = daemonControlState(inputs({ service: snapshot(service({ present, loaded: false })), runner: snapshot(runner(heartbeat)) }));
      assert.equal(state.canPreview, true);
      assert.equal(state.setupBlock, "");
      assert.equal(ready({ disabled: !state.canPreview, blockedReason: state.setupBlock }), true);
    }
  }
});

test("STOP and other runner gates remain stopped after the owner pause is clear", () => {
  const blocked = runner("running");
  blocked.runner = { allowed: false, reason: "STOP file is present" };
  const state = daemonControlState(inputs({ runner: snapshot(blocked) }));
  assert.equal(state.label, "stopped");
  assert.equal(state.canResume, false);
  assert.match(state.description, /STOP file is present/);
  assert.match(state.description, /does not clear this stop/);
});

test("pending, unavailable, read-only and failed snapshots never enable an unsafe write", () => {
  const pending = daemonControlState(inputs({ runner: snapshot(undefined), service: snapshot(undefined), settings: snapshot(undefined) }));
  assert.equal(pending.label, "checking");
  assert.equal(pending.canResume, false);
  assert.equal(pending.canPreview, false);
  const staleRunner = daemonControlState(inputs({ runner: snapshot(runner("running"), "offline"), service: snapshot(service({ loaded: false })) }));
  assert.equal(staleRunner.label, "unknown");
  assert.equal(staleRunner.canPreview, false);
  const staleService = daemonControlState(inputs({ service: snapshot(service({ loaded: false }), "offline") }));
  assert.equal(staleService.service, "unknown");
  assert.equal(staleService.setup, null);
  const staleSettings = daemonControlState(inputs({ settings: snapshot(settings("1"), "offline") }));
  assert.equal(staleSettings.pause, "unknown");
  assert.equal(staleSettings.canResume, false);
  const readOnly = daemonControlState(inputs({ settings: snapshot({ ...settings("1"), can_edit: false }) }));
  assert.equal(readOnly.pause, "on");
  assert.equal(readOnly.canResume, false);
  const unavailable = daemonControlState(inputs({ runner: snapshot({ ...runner(), available: false }), settings: snapshot({ ...settings("1"), available: false }) }));
  assert.equal(unavailable.label, "unavailable");
  assert.equal(unavailable.canResume, false);
  assert.equal(unavailable.canPreview, false);
});

test("unreadable and unknown heartbeat states require inspection, not a guessed start", () => {
  for (const heartbeat of ["unreadable", "future-state", ""]) {
    const state = daemonControlState(inputs({ runner: snapshot(runner(heartbeat)), service: snapshot(service({ loaded: false })) }));
    assert.equal(state.label, "unknown");
    assert.match(state.setupBlock, /not proof that no daemon is running/);
    assert.equal(ready({ blockedReason: state.setupBlock }), false);
  }
});

test("apply requires a successful dry-run preview and is unavailable while pending or after completion", () => {
  assert.equal(ready(), true);
  for (const override of [
    { preview: null }, { previewError: "preview refused" }, { supported: false }, { disabled: true },
    { blockedReason: "daemon may be alive" }, { preview: { ...preview, dry_run: false } },
    { preview: { ...preview, installed: true } },
    ...["installing", "handover", "done", "late"].map((phase) => ({ phase })),
  ]) assert.equal(ready(override), false, JSON.stringify(override));
});

test("existing service preview/apply and owner resume retain their guarded request shapes", async (t) => {
  const requests = [];
  t.mock.method(globalThis, "fetch", async (url, init) => {
    requests.push({ url, ...init, body: JSON.parse(init.body) });
    return Response.json({ ok: true });
  });
  await writes.serviceInstall(true);
  await writes.serviceInstall(false);
  await writes.setSetting("stop", "0");
  assert.deepEqual(requests.map(({ url, method, body }) => ({ url, method, body })), [
    { url: "/api/v1/service", method: "POST", body: { dry_run: true } },
    { url: "/api/v1/service", method: "POST", body: { dry_run: false } },
    { url: "/api/v1/settings", method: "POST", body: { name: "stop", value: "0" } },
  ]);
  assert.equal(requests[2].headers["X-Pulse-Settings"], "yes");
  for (const request of requests) assert.equal(request.headers["Content-Type"], "application/json");
});

test("handover waits for the service's hub, not a still-answering foreground hub", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const replies = [{ hub_is_service: false }, { hub_is_service: true }];
  let loads = 0;
  let completed = 0;
  let signal;
  const stop = waitForServiceHandover({
    load: (s) => { signal = s; loads++; return Promise.resolve(replies.shift()); },
    onReady: () => { completed++; },
    onLate: () => assert.fail("handover should finish"),
  });
  t.after(stop);
  t.mock.timers.tick(1499);
  assert.equal(loads, 0);
  t.mock.timers.tick(1);
  await flush();
  assert.equal(completed, 0);
  t.mock.timers.tick(1000);
  await flush();
  assert.equal(completed, 1);
  assert.equal(signal.aborted, true);
  t.mock.timers.tick(HANDOVER_WAIT_MS);
  assert.equal(loads, 2);
});

test("handover retries connection errors and reports the last error when its deadline expires", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let loads = 0;
  const late = [];
  const stop = waitForServiceHandover({
    load: () => { loads++; return Promise.reject(new Error("console is reconnecting")); },
    onReady: () => assert.fail("an error is not a service handover"),
    onLate: (note) => late.push(note),
  });
  t.after(stop);
  t.mock.timers.tick(1500);
  await flush();
  t.mock.timers.tick(1000);
  await flush();
  assert.equal(loads, 2);
  t.mock.timers.tick(HANDOVER_WAIT_MS - 2500);
  await flush();
  assert.deepEqual(late, ["console is reconnecting"]);
});

test("a hung handover request is aborted at the deadline and a late answer cannot trigger reload", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let finish;
  let signal;
  const late = [];
  const stop = waitForServiceHandover({
    load: (s) => { signal = s; return new Promise((resolve) => { finish = resolve; }); },
    onReady: () => assert.fail("late handover must not reload"),
    onLate: (note) => late.push(note),
  });
  t.after(stop);
  t.mock.timers.tick(1500);
  t.mock.timers.tick(HANDOVER_WAIT_MS - 1500);
  assert.equal(signal.aborted, true);
  assert.equal(late.length, 1);
  assert.match(late[0], /has not answered/);
  finish({ hub_is_service: true });
  await flush();
  assert.equal(late.length, 1);
});

test("leaving the page cancels an in-flight handover without a reload or another poll", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let finish;
  let signal;
  let loads = 0;
  const stop = waitForServiceHandover({
    load: (s) => { signal = s; loads++; return new Promise((resolve) => { finish = resolve; }); },
    onReady: () => assert.fail("an unmounted page must not reload"),
    onLate: () => assert.fail("an unmounted page must not change phase"),
  });
  t.after(stop);
  t.mock.timers.tick(1500);
  stop();
  assert.equal(signal.aborted, true);
  finish({ hub_is_service: true });
  await flush();
  t.mock.timers.tick(HANDOVER_WAIT_MS);
  assert.equal(loads, 1);
});
