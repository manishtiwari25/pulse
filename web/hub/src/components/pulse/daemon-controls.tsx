"use client";

import Link from "next/link";
import { useState } from "react";
import { CirclePauseIcon, Loader2Icon, PlayIcon, PowerIcon, RefreshCwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "./confirm";
import { daemonControlState, daemonHeadline, daemonUpdateText } from "./daemon-control-state";
import { CommandBox, TerminalAlternative } from "./evolve";
import { Disclosure, ErrorNotice, Kv, Notice, Panel } from "./panel";
import { ServiceControl } from "./service";
import { Mono, When } from "./who";
import { writes, type Runner, type ServicePayload, type SettingsPayload } from "@/lib/api";
import { cn } from "@/lib/utils";
import { hrefs } from "@/lib/nav";
import type { Loaded } from "@/lib/use-api";

const DOT = { ok: "bg-ok", warn: "bg-warn", bad: "bg-bad", info: "bg-info", accent: "bg-accent-2", neutral: "bg-muted-foreground/50", you: "bg-you" } as const;

// The runner (the daemon) on this machine: one sentence on where it stands,
// the one action that fits (Resume, Start the runner, Keep PULSE running,
// Finish service setup) with why in plain words, and everything technical -
// heartbeat states, PIDs, the stop setting, service units, logs - folded
// under "Technical details". Every guard of the old controls stays: Resume
// only clears the owner pause, setup is previewed first and refused while a
// runner may already be running.
export function DaemonControls({ runner, service, settings }: { runner: Loaded<Runner>; service: Loaded<ServicePayload>; settings: Loaded<SettingsPayload> }) {
  const [resumeOpen, setResumeOpen] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const state = daemonControlState({ runner, service, settings });
  const r = runner.data;
  const payload = service.data;
  const headline = daemonHeadline(state);
  const refresh = async () => {
    setRefreshing(true);
    try {
      await Promise.all([runner.reload(), service.reload(), settings.reload()]);
    } finally {
      setRefreshing(false);
    }
  };

  // Why the action is the right one, in a sentence.
  const why =
    state.pause === "on"
      ? state.canResume
        ? "Resume lets agents start new work again. Your limits, project stops and approvals still apply."
        : "This console can't change the pause. Resume from PULSE on the machine that runs it."
      : state.setup === "finish"
        ? "The runner already runs as a background service, but this console doesn't yet. Finish the setup so the console keeps going after you close the terminal too."
        : state.setup === "start"
          ? "The runner's background service is installed but not loaded. Start it so agents can work; you'll see what changes first."
          : state.setup === "install"
            ? "PULSE runs in a terminal right now and stops when you close it. Set it up as a background service so the runner and this console keep going and start again when you log in. You'll see what changes before anything is installed."
            : state.service === "loaded"
              ? "Nothing to do here. The runner runs as a background service and restarts on its own."
              : state.service === "unsupported"
                ? "This machine has no supported background service manager, so the runner has to run in a terminal you keep open."
                : "";

  return (
    <Panel
      title="Runner controls"
      icon={PowerIcon}
      actions={
        <>
          <span className="inline-flex items-center gap-1.5 font-medium text-foreground" title={"Runner state: " + state.label}>
            <span className={cn("size-2 rounded-full", DOT[state.tone])} aria-hidden />
            {headline}
          </span>
          <Button size="xs" variant="outline" disabled={refreshing} onClick={() => void refresh()}>
            {refreshing ? <Loader2Icon className="animate-spin" /> : <RefreshCwIcon />} Refresh
          </Button>
        </>
      }
    >
      <div className="grid gap-4">
        <div className="grid max-w-3xl gap-1.5">
          <p className="text-base font-semibold leading-snug text-foreground anywhere">{state.sentence}</p>
          {state.update ? <p className="text-sm text-muted-foreground anywhere">{daemonUpdateText(state.update)}.</p> : null}
          {why ? <p className="text-sm leading-relaxed text-muted-foreground anywhere">{why}</p> : null}
          {r?.daemon.at ? <p className="text-[13px] text-muted-foreground">Last checked in <When d={r.daemon.at} /></p> : null}
        </div>

        {runner.error ? <ErrorNotice error={runner.error} onRetry={() => void runner.reload()}>Couldn&apos;t refresh the runner&apos;s status: {runner.error}</ErrorNotice> : null}
        {settings.error ? <ErrorNotice error={settings.error} onRetry={() => void settings.reload()}>Couldn&apos;t read whether work is paused: {settings.error}</ErrorNotice> : null}
        {service.error ? <ErrorNotice error={service.error} onRetry={() => void service.reload()}>Couldn&apos;t check the background services: {service.error}</ErrorNotice> : null}
        {r?.daemon.disk ? <Notice tone="bad">{r.daemon.disk.replace(/\.$/, "")}. Free some disk space; restarting won&apos;t clear this.</Notice> : null}
        {r?.daemon.note && !state.update ? <Notice tone="warn">{r.daemon.note}</Notice> : null}
        {state.pause === "unknown" && !settings.error && settings.data ? <Notice tone="warn">PULSE couldn&apos;t confirm whether work is paused. Refresh before resuming; setting up the service never changes the pause.</Notice> : null}
        {state.setup && state.setupBlock ? <Notice tone="warn">{state.setupBlock}</Notice> : null}
        {state.setup && !state.canPreview && !state.setupBlock ? <Notice tone="warn">PULSE needs a fresh runner and service status before it can show the setup. Refresh above.</Notice> : null}
        {state.service === "unsupported" ? (
          <div className="grid gap-2">
            <p className="text-[13px] leading-relaxed text-muted-foreground">First check <Mono>pulse status</Mono>. Only if no runner is running, start one in a terminal and keep that terminal open (run <Mono>pulse hub</Mono> in another for this console):</p>
            <CommandBox command="pulse daemon" copy />
          </div>
        ) : null}

        <div className="flex flex-wrap items-center gap-2">
          {state.pause === "on" ? (
            <Button disabled={!state.canResume || refreshing} onClick={() => setResumeOpen(true)}>
              <PlayIcon /> Resume
            </Button>
          ) : null}
          {payload ? <ServiceControl payload={payload} disabled={!state.canPreview || refreshing} blockedReason={state.setupBlock} variant={state.pause === "on" ? "outline" : "default"} size="default" /> : null}
          {state.pause !== "on" ? (
            <Button asChild variant="outline">
              <Link href={hrefs.settings("stop")} title="Pause all agent work from Settings">
                <CirclePauseIcon /> Pause all work
              </Link>
            </Button>
          ) : null}
        </div>
        {state.pause === "on" ? <TerminalAlternative command="pulse start" /> : state.setup && payload ? <TerminalAlternative command={state.setupBlock ? "pulse status" : payload.command} /> : null}

        <Disclosure summary="Technical details" className="border-t pt-3 text-[13px]" contentClassName="mt-3 grid gap-3 text-muted-foreground">
            <p className="leading-relaxed text-foreground/85 anywhere">{state.description}</p>
            <Kv
              className="text-[13px]"
              rows={[
                ["State", <Mono key="s">{state.label}</Mono>],
                ["Owner pause", state.pause === "on" ? <span key="p">On (<Mono>stop = 1</Mono>)</span> : state.pause === "off" ? <span key="p">Off (<Mono>stop = 0</Mono>)</span> : "Unknown"],
                ["Last heartbeat", <span key="h"><Mono>{r?.daemon.state || "none"}</Mono>{r?.daemon.at ? <> <When d={r.daemon.at} /></> : null}</span>],
                ...(r?.daemon.pid ? [["Recorded PID", <Mono key="pid">{r.daemon.pid}</Mono>] as [React.ReactNode, React.ReactNode]] : []),
                ...(payload && !service.error
                  ? [["Services", <span key="u" className="flex flex-wrap gap-x-3 gap-y-1">{payload.service.units.map((u) => <span key={u.name}><Mono>{u.name}</Mono> {u.loaded ? "loaded" : u.present ? "not loaded" : "missing"}</span>)}{payload.service.kind ? <span>under <Mono>{payload.service.kind}</Mono></span> : null}</span>] as [React.ReactNode, React.ReactNode]]
                  : []),
                ...(payload?.service.logs ? [["Logs", <Mono key="l">{payload.service.logs}</Mono>] as [React.ReactNode, React.ReactNode]] : []),
              ]}
            />
            {state.pause === "on" ? <p className="leading-relaxed">Resume clears only <Mono>stop</Mono>. It leaves STOP files, project stops and limits in place, and the runner reads it on its next loop. Service setup does not clear the pause.</p> : null}
            {state.setup ? (
              <p className="leading-relaxed">
                {state.setup === "finish" ? "The runner is already a service; the console's service is missing, not loaded or unconfirmed, and Finish service setup applies the existing installer to complete it." : state.setup === "start" ? "The runner's service file exists, but its unit is not loaded; Start the runner reapplies the existing installer to load it." : "The runner is not installed as a user service; Keep PULSE running previews the setup that starts the runner and console at login and keeps them alive."}
                {" "}Unchanged, loaded units are left alone; a console started by hand hands over when needed.
              </p>
            ) : state.service === "loaded" ? (
              <p className="leading-relaxed">The runner&apos;s service is loaded under <Mono>{payload?.service.kind}</Mono>. Loaded does not prove a fresh heartbeat; no duplicate start or automatic restart is offered here.</p>
            ) : state.service === "unknown" && !service.error ? (
              <p className="leading-relaxed">{payload ? "The service response did not identify the runner's unit. Refresh, or inspect it in a terminal." : "Checking this machine's service manager…"}</p>
            ) : null}
            <p className="leading-relaxed"><Mono>pulse status</Mono> reports the heartbeat and work gates; <Mono>pulse service status</Mono> reports the service units. Automatic rebuilds are separate: <Mono>daemon.self_update = off</Mono> keeps the current binary and does not turn the runner off.</p>
        </Disclosure>
      </div>

      <ConfirmDialog
        open={resumeOpen}
        onOpenChange={setResumeOpen}
        title="Resume the runner?"
        description={<>Agents can start new work again. This is the same as <Mono>pulse start</Mono>. It doesn&apos;t start a runner that isn&apos;t running or remove any STOP file, and your limits, project stops and approvals still apply.</>}
        action="Resume"
        success="Resumed. The runner picks this up on its next check-in."
        onConfirm={() => {
          if (!state.canResume) throw new Error("The pause can no longer be changed here. Refresh, then try again.");
          return writes.setSetting("stop", "0");
        }}
        invalidates={(p) => ["settings", "runner", "service", "running", "compute", "org", "events"].some((path) => p === path || p.startsWith(path + "?"))}
      />
    </Panel>
  );
}
