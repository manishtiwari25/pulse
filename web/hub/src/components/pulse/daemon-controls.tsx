"use client";

import Link from "next/link";
import { useState } from "react";
import { Loader2Icon, PowerIcon, RefreshCwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "./confirm";
import { daemonControlState, daemonUpdateText } from "./daemon-control-state";
import { CommandBox, TerminalAlternative } from "./evolve";
import { ErrorNotice, Notice, Panel } from "./panel";
import { ServiceControl } from "./service";
import { Mono, ToneBadge, When } from "./who";
import { writes, type Runner, type ServicePayload, type SettingsPayload } from "@/lib/api";
import { hrefs } from "@/lib/nav";
import type { Loaded } from "@/lib/use-api";

export function DaemonControls({ runner, service, settings }: { runner: Loaded<Runner>; service: Loaded<ServicePayload>; settings: Loaded<SettingsPayload> }) {
  const [resumeOpen, setResumeOpen] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const state = daemonControlState({ runner, service, settings });
  const r = runner.data;
  const payload = service.data;
  const checked = r?.daemon.at ? (
    <>
      , checked <When d={r.daemon.at} />
    </>
  ) : null;
  const statusLine = state.pause === "on" ? (
    "Paused by you"
  ) : state.label === "running" || state.label === "alive" ? (
    <>Running on this machine{state.update ? " — " + daemonUpdateText(state.update).replace(/^New build/, "a newer build") : ""}{checked}</>
  ) : state.label === "silent" ? (
    <>Heartbeat stale — read its log before starting another copy{checked}</>
  ) : state.label === "checking" ? (
    "Checking this machine"
  ) : state.label === "unknown" || state.label === "unavailable" ? (
    "Status unavailable — refresh before starting anything"
  ) : (
    "Not running — start it to let agents work"
  );
  const refresh = async () => {
    setRefreshing(true);
    try {
      await Promise.all([runner.reload(), service.reload(), settings.reload()]);
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <Panel
      title="PULSE on this machine"
      icon={PowerIcon}
      actions={
        <>
          <ToneBadge tone={state.tone}>{state.label}</ToneBadge>
          <Button size="xs" variant="outline" disabled={refreshing} onClick={() => void refresh()}>
            {refreshing ? <Loader2Icon className="animate-spin" /> : <RefreshCwIcon />} Refresh status
          </Button>
        </>
      }
    >
      <div className="grid gap-4">
        <p className="surface-glass rounded-xl border px-3 py-2 text-sm font-medium leading-relaxed text-foreground">{statusLine}</p>
        <p className="text-sm leading-relaxed text-muted-foreground">{state.description}</p>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-muted-foreground">
          <span>Owner pause: <strong className="font-medium text-foreground">{state.pause === "on" ? "on" : state.pause === "off" ? "clear" : "unknown"}</strong></span>
          <span>Last reported heartbeat: <Mono>{r?.daemon.state || "none"}</Mono></span>
          {r?.daemon.at ? <span>Last loop <When d={r.daemon.at} /></span> : null}
          {r?.daemon.pid ? <span>Recorded PID <Mono>{r.daemon.pid}</Mono></span> : null}
          {payload && !service.error ? payload.service.units.map((u) => (
            <ToneBadge key={u.name} tone={u.loaded ? "neutral" : "warn"}>{u.name + " service: " + (u.loaded ? "loaded" : u.present ? "not loaded" : "missing")}</ToneBadge>
          )) : null}
        </div>

        {runner.error ? <ErrorNotice error={runner.error} onRetry={() => void runner.reload()}>Could not refresh daemon status: {runner.error}</ErrorNotice> : null}
        {settings.error ? <ErrorNotice error={settings.error} onRetry={() => void settings.reload()}>Could not read the owner pause: {settings.error}</ErrorNotice> : null}
        {service.error ? <ErrorNotice error={service.error} onRetry={() => void service.reload()}>Could not refresh service status: {service.error}</ErrorNotice> : null}
        {r?.daemon.disk ? <Notice tone="bad">{r.daemon.disk} - free disk space before restarting; a restart does not clear this hold.</Notice> : null}
        {r?.daemon.note ? <Notice tone={state.update ? "info" : "warn"}>{r.daemon.note}</Notice> : null}

        <div className="flex flex-wrap items-center gap-2">
          {state.pause === "on" ? (
            <Button size="sm" disabled={!state.canResume || refreshing} onClick={() => setResumeOpen(true)}>
              <PowerIcon /> Resume
            </Button>
          ) : null}
          {payload ? <ServiceControl payload={payload} disabled={!state.canPreview || refreshing} blockedReason={state.setupBlock} /> : null}
          <Button asChild size="sm" variant="outline">
            <Link href={hrefs.settings("stop")}>Pause</Link>
          </Button>
        </div>

        {state.pause === "on" ? (
          <div className="grid gap-2">
            {!state.canResume ? <Notice tone="warn">This hub cannot edit the owner pause. Resume on the owner&apos;s machine.</Notice> : null}
            <p className="text-xs leading-relaxed text-muted-foreground">Resume clears only <Mono>stop</Mono>. It leaves file STOPs, project gates and caps intact, and the daemon reads it on its next loop. Service setup below does not clear the pause.</p>
            <TerminalAlternative command="pulse start" />
          </div>
        ) : state.pause === "unknown" ? (
          <Notice tone="warn">The owner pause has not been confirmed. Refresh settings before resuming; service setup never changes this setting.</Notice>
        ) : null}

        {state.setup ? (
          <div className="grid gap-2">
            <p className="text-sm leading-relaxed text-muted-foreground">
              {state.setup === "finish" ? "The daemon is already service-managed. The console's service setup is missing, unloaded or unconfirmed; Finish service setup previews the existing installer to complete it." : state.setup === "start" ? "The daemon's service file exists, but its unit is not loaded. Start daemon previews and reapplies the existing installer to load it." : "The daemon is not installed as a user service. Keep running previews the setup that starts the daemon and console at login and keeps them alive."}
              {" "}The installer also covers the console. Unchanged, loaded units are left alone; a foreground console hands over when needed.
            </p>
            {state.setupBlock ? <Notice tone="warn">{state.setupBlock}</Notice> : null}
            {!state.canPreview ? <Notice tone="warn">A current runner and service status is needed before setup can be previewed. Refresh status above.</Notice> : null}
            {payload ? <TerminalAlternative command={state.setupBlock ? "pulse status" : payload.command} /> : null}
          </div>
        ) : state.service === "loaded" ? (
          <p className="text-sm leading-relaxed text-muted-foreground">The daemon service is loaded under <Mono>{payload?.service.kind}</Mono>. Loaded does not prove a fresh heartbeat. No duplicate start or automatic restart is offered here.</p>
        ) : state.service === "unsupported" ? (
          <div className="grid gap-2">
            <Notice tone="warn">No supported user service manager is available here. <Mono>{payload?.command}</Mono> needs launchd on macOS or systemd --user on Linux. Use a terminal or your own supervisor instead.</Notice>
            <p className="text-xs leading-relaxed text-muted-foreground">First inspect <Mono>pulse status</Mono>. Only if no daemon process is running, start it in a terminal with the command below; keep that terminal open. Run <Mono>pulse hub</Mono> in another terminal if you also need the console.</p>
            <CommandBox command="pulse daemon" copy />
          </div>
        ) : !service.error ? (
          <Notice tone="neutral">{payload ? "The service response did not identify the daemon unit. Refresh status or inspect it in a terminal." : "Checking this machine's service manager..."}</Notice>
        ) : null}

        <div className="grid gap-2 border-t pt-3 text-xs text-muted-foreground">
          <p>For diagnosis, <Mono>pulse status</Mono> reports the heartbeat and work gates; <Mono>pulse service status</Mono> reports the service units.{payload?.service.logs ? <> Service logs: <Mono>{payload.service.logs}</Mono>.</> : null}</p>
          <p>Automatic rebuilds are separate: <Mono>daemon.self_update = off</Mono> keeps the current binary; it does not disable the daemon.</p>
        </div>
      </div>

      <ConfirmDialog
        open={resumeOpen}
        onOpenChange={setResumeOpen}
        title="Resume the daemon's work?"
        description={<>Set the owner pause to <Mono>stop = 0</Mono>, the same as <Mono>pulse start</Mono>. This does not start a missing daemon or remove any STOP file. Existing caps, project gates and approvals still apply. A fresh status, not this save, confirms progress.</>}
        action="Resume daemon"
        success="Owner pause cleared. Waiting for fresh daemon status; STOP files and caps still apply."
        onConfirm={() => {
          if (!state.canResume) throw new Error("The owner pause can no longer be changed here. Refresh settings before resuming.");
          return writes.setSetting("stop", "0");
        }}
        invalidates={(p) => ["settings", "runner", "service", "running", "compute", "org", "events"].some((path) => p === path || p.startsWith(path + "?"))}
      />
    </Panel>
  );
}
