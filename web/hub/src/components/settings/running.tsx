"use client";

import { CoffeeIcon, TriangleAlertIcon } from "lucide-react";
import { daemonUpdateText, pendingUpdate } from "@/components/pulse/daemon-control-state";
import { awakeText, sleptText, useAwake } from "@/components/shell/awake";
import { Disclosure, Kv } from "@/components/pulse/panel";
import { RestartNow } from "@/components/pulse/restart-now";
import { Mono, ToneBadge } from "@/components/pulse/who";
import type { RunningPayload, RunningState } from "@/lib/api";
import { fmtDate, num, rel, type Tone } from "@/lib/format";
import { useApi } from "@/lib/use-api";

// "Which PULSE is running" at the top of the This machine section (ADR-035):
// the version of this console and of the runner, whether PULSE is up to date
// with its own source, and what it will do about it. Read-only - turning
// self-update off is the row underneath. Build ids and the event stream fold
// into "Technical details".

const STATE: Record<RunningState, { tone: Tone; label: string; text: string; terminal?: string; terminalWhat?: string }> = {
  current: { tone: "ok", label: "Up to date", text: "This machine runs the latest PULSE. Nothing to do." },
  behind: { tone: "info", label: "Updating", text: "A newer PULSE has been merged. The runner rebuilds itself and restarts once its current runs finish, or now if you ask: runs in progress hold and carry on under the new build. This console follows within a minute." },
  waiting: { tone: "warn", label: "Update on hold", text: "PULSE's own source has uncommitted changes or is on another branch, so the update waits. Commit, or switch back to the default branch, and it carries on." },
  unstamped: { tone: "info", label: "Version not recorded", text: "This build doesn't record which version it came from. The runner rebuilds once after the next merge and records it from then on." },
  off: { tone: "neutral", label: "Self-update off", text: "Self-update is off, so the runner keeps the build it has until you install a new one.", terminal: "cargo install --path crates/pulse --root ~/.pulse --locked", terminalWhat: "Install a new build" },
  "no-daemon": { tone: "warn", label: "Runner not started", text: "The runner hasn't run on this machine yet. Use Keep PULSE running on the Home page to start it and this console in the background.", terminal: "pulse service install", terminalWhat: "Start the runner and this console in the background" },
  "no-brain": { tone: "neutral", label: "Installed build", text: "None of your projects is PULSE's own source, so this build stays as it is. Everything works. Only people who develop PULSE itself connect its source.", terminal: "pulse init PATH --brain", terminalWhat: "Connect PULSE's own source" },
  "no-store": { tone: "bad", label: "No data", text: "This console can't open PULSE's data on this machine." },
};

function short(rev: string): string {
  return rev.length > 12 ? rev.slice(0, 12) : rev;
}

function Version({ b }: { b: RunningPayload["hub"] | null }) {
  if (!b) return <span className="text-muted-foreground">Not started yet</span>;
  return (
    <span className="inline-flex flex-wrap items-baseline gap-x-2">
      <span className="font-medium">{b.version || "Development build"}</span>
      {b.at ? <span className="text-muted-foreground" title={fmtDate(b.at)}>checked in {rel(b.at)}</span> : null}
    </span>
  );
}

function Build({ b }: { b: RunningPayload["hub"] | null }) {
  if (!b) return <span className="text-muted-foreground">-</span>;
  return (
    <span className="inline-flex flex-wrap items-baseline gap-x-2">
      <Mono>{b.version || "dev"}</Mono>
      {b.revision && !(b.version || "").includes(short(b.revision)) ? <Mono className="text-muted-foreground">{short(b.revision)}</Mono> : null}
      {b.pid ? <span className="tnum text-muted-foreground">process {b.pid}</span> : null}
    </span>
  );
}

export function RunningStrip() {
  const q = useApi<RunningPayload>("running", { refresh: 30000 });
  const awake = useAwake();
  const d = q.data;
  if (!d) {
    return <div className="px-4 py-3 text-sm text-muted-foreground sm:px-5">{q.error ? "Couldn't check which PULSE is running: " + q.error : "Checking which PULSE is running…"}</div>;
  }
  const st = STATE[d.state] ?? STATE["no-store"];
  const rows: [React.ReactNode, React.ReactNode][] = [
    ["This console", <Version key="h" b={d.hub} />],
    ["Runner", <Version key="d" b={d.daemon} />],
  ];
  // A newer runner is built and waits for runs; a newer console waits for
  // conversations. Either way the owner can take the update now.
  const update = d.daemon ? pendingUpdate(d.daemon) : null;
  const waits = !!update || !!d.hub.restart_pending;
  if (waits) {
    rows.push([
      "Update waiting",
      <span key="u" className="grid gap-1.5">
        <span className="leading-relaxed">
          {update ? daemonUpdateText(update) + "." : ""}
          {d.hub.restart_pending ? (update ? " " : "") + "This console has a newer build and waits for the conversations working." : ""}
        </span>
        <RestartNow ask={d.restart_ask} />
      </span>,
    ]);
  } else if (d.restart_ask?.pending) {
    rows.push(["Restart asked", <span key="ra" title={fmtDate(d.restart_ask.at)}>{rel(d.restart_ask.at)}; the runner holds its runs and restarts.</span>]);
  }
  if (d.last) {
    rows.push([
      "Last update",
      <span key="l" className="inline-flex flex-wrap items-baseline gap-x-2">
        <span title={fmtDate(d.last.at)}>{rel(d.last.at)}</span>
        {d.last.inflight && !waits ? <span className="text-muted-foreground">restarts after {num(d.last.inflight)} {d.last.inflight === 1 ? "run" : "runs"} finish</span> : null}
      </span>,
    ]);
  }
  // Keep-awake (daemon.keep_awake): what holds this machine up, and the
  // last time it slept under PULSE anyway.
  rows.push([
    "Staying awake",
    <span key="aw" className="leading-relaxed">
      {awake.holding ? <CoffeeIcon className="mr-1.5 -mt-0.5 inline size-3.5 text-awake" aria-hidden /> : null}
      {awakeText(awake)}
    </span>,
  ]);
  const slept = awake.slept ?? d.daemon?.slept ?? null;
  if (slept?.seconds) {
    rows.push(["Last sleep", <span key="sl" title={fmtDate(slept.until)}>{sleptText(slept)}</span>]);
  }
  const tech: [React.ReactNode, React.ReactNode][] = [
    ["Console build", <Build key="hb" b={d.hub} />],
    ["Runner build", <Build key="db" b={d.daemon} />],
  ];
  if (d.brain) {
    tech.push([
      "PULSE source",
      <span key="b" className="inline-flex flex-wrap items-baseline gap-x-2">
        <span>{d.brain.slug}</span>
        <Mono className="text-muted-foreground">{d.brain.branch}</Mono>
        <Mono>{short(d.brain.head) || "no commits"}</Mono>
        {d.brain.dirty ? <span className="text-warn">uncommitted changes</span> : null}
        {d.brain.branch !== d.brain.default_branch ? <span className="text-warn">not on {d.brain.default_branch}</span> : null}
      </span>,
    ]);
  }
  if (d.last) {
    tech.push([
      "Last rebuild",
      <span key="lr" className="inline-flex flex-wrap items-baseline gap-x-2">
        <Mono className="text-muted-foreground">{short(d.last.from) || "unrecorded"}</Mono>
        <span className="text-muted-foreground">to</span>
        <Mono>{short(d.last.to)}</Mono>
      </span>,
    ]);
  }
  if (d.signed === true) tech.push(["macOS signature", <span key="sg">Signed, so macOS keeps your folder answers across updates</span>]);
  // Commands for people who'd rather type them, kept out of the sentences above.
  const terminal: [string, string][] = [];
  if (st.terminal) terminal.push([st.terminalWhat ?? "Do it from a terminal", st.terminal]);
  if (d.signed === false && st.terminal !== "pulse service install") terminal.push(["Sign PULSE so macOS remembers your folder answers", "pulse service install"]);
  const bus = d.daemon?.bus;
  if (bus) {
    tech.push([
      "Live updates",
      <span key="bus" className="inline-flex flex-wrap items-baseline gap-x-2">
        <span className="tnum">{num(bus.subscribers)} listening</span>
        <ToneBadge tone={bus.dropped > 0 ? "warn" : "neutral"}>{num(bus.dropped) + " dropped"}</ToneBadge>
        <span className="tnum text-muted-foreground">{num(bus.deduped) + " merged"}</span>
        {bus.dropped > 0 ? <span className="text-warn">slow listeners missed some updates</span> : null}
      </span>,
    ]);
  }
  return (
    <div className="grid gap-3 px-4 py-4 text-sm sm:px-5" data-testid="running-strip">
      <div className="flex min-w-0 flex-wrap items-start gap-x-3 gap-y-1.5">
        <ToneBadge tone={st.tone} dot className="mt-0.5">
          {st.label}
        </ToneBadge>
        <p className="min-w-0 flex-1 basis-64 leading-relaxed text-muted-foreground anywhere">{st.text}</p>
      </div>
      <Kv rows={rows} className="rounded-xl bg-muted/50 px-4 py-3" />
      {d.signed === false ? (
        <p className="flex min-w-0 max-w-2xl items-start gap-2 leading-relaxed anywhere">
          <TriangleAlertIcon className="mt-[3px] size-4 shrink-0 text-warn" aria-hidden />
          <span>PULSE isn&apos;t signed yet, so macOS asks again for your Desktop and Documents folders after each update. Signing it once makes macOS remember your answer.</span>
        </p>
      ) : null}
      <div className="grid gap-1">
        <Disclosure summary="Technical details">
          <Kv rows={tech} className="text-[13px]" />
        </Disclosure>
        {terminal.length ? (
          <Disclosure summary="Prefer the terminal?">
            <ul className="grid gap-1.5 text-[13px] text-muted-foreground">
              {terminal.map(([what, cmd]) => (
                <li key={cmd} className="anywhere">
                  {what}: <Mono>{cmd}</Mono>
                </li>
              ))}
            </ul>
          </Disclosure>
        ) : null}
      </div>
    </div>
  );
}
