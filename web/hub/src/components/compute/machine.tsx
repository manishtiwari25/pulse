"use client";

import Link from "next/link";
import { useState } from "react";
import { AlertTriangleIcon, GitMergeIcon, ServerIcon } from "lucide-react";
import { Disclosure, EmptyState, Kv, Notice, Panel } from "@/components/pulse/panel";
import { When } from "@/components/pulse/who";
import { cn } from "@/lib/utils";
import { num, plural, rel, statusTone, type Tone } from "@/lib/format";
import { statusWord } from "@/lib/words";
import { hrefs } from "@/lib/nav";
import { useNow } from "@/lib/use-api";
import type { Executor, LandingHold, Runner } from "@/lib/api";

// ----- the machine ---------------------------------------------------------

type Line = { label: string; tone: Tone; value: string; note?: React.ReactNode; title?: string };

const DOT: Record<Tone, string> = { ok: "bg-ok", warn: "bg-warn", bad: "bg-bad", info: "bg-primary", neutral: "bg-muted-foreground/50", accent: "bg-accent-2", you: "bg-you" };
const DOT_WORD: Record<Tone, string> = { ok: "Fine", warn: "Needs a look", bad: "Stopped", info: "Busy", neutral: "Quiet", accent: "Note", you: "Waits for you" };

// This machine in four plain lines (is the runner on, what are the agents
// doing, how much work waits, is anything left lying around), with the
// runner's own numbers folded under "Show details". `heading={false}` drops
// the panel's title row (the page gives the section its own heading);
// `machines={false}` leaves out the machine list when the page shows it.
export function MachinePanel({ r, heading = true, machines = true }: { r: Runner | undefined | null; heading?: boolean; machines?: boolean }) {
  const [details, setDetails] = useState(false);
  const now = useNow();
  const settings = (
    <Link href={hrefs.settings("runner")} className="text-[13px] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline">
      Runner settings
    </Link>
  );
  const shell = (body: React.ReactNode) => (
    <Panel title={heading ? "This machine" : undefined} icon={ServerIcon} actions={heading ? settings : undefined}>
      {body}
    </Panel>
  );
  if (!r) return shell(<EmptyState title="No word from the runner">{"The runner's state didn't load. It shows here once the hub can reach it."}</EmptyState>);
  if (!r.available)
    return shell(
      <EmptyState title="The runner hasn't run here yet" hint="pulse daemon">
        Start the runner on this machine and it will plan work and start agents. From a terminal:
      </EmptyState>,
    );
  const d = r.daemon;
  const stopped = !!r.runner && !r.runner.allowed;
  const on = !stopped && (d.state === "running" || d.state === "alive");
  const never = /never/i.test(d.state || d.text || "");
  const leftovers = hygieneLeftovers(r.hygiene, now);
  const busy = r.jobs.running + r.jobs.leased;
  const lines: Line[] = [
    {
      label: "Runner",
      tone: stopped ? "bad" : on ? "ok" : "warn",
      value: stopped ? "Paused" : on ? "On" : "Off",
      note: stopped ? "The stop switch is on" : never ? "It hasn't run on this machine yet" : d.at ? "Last check-in " + rel(d.at, now) : undefined,
      title: d.text || d.state,
    },
    {
      label: "Agents",
      tone: r.workers.busy ? "info" : "neutral",
      value: r.workers.busy || r.workers.idle ? num(r.workers.busy) + " working, " + num(r.workers.idle) + " idle" : "None running right now",
      note: r.workers.idle_names.length ? "Idle: " + r.workers.idle_names.join(", ") : undefined,
    },
    {
      label: "Work",
      tone: r.jobs.blocked ? "bad" : busy ? "info" : r.jobs.queued ? "warn" : "ok",
      value: (r.jobs.queued || busy ? num(r.jobs.queued) + " waiting to start, " + num(busy) + " running" : "Nothing waiting") + (r.jobs.blocked ? ", " + num(r.jobs.blocked) + " stuck" : ""),
      note: r.jobs.blocked ? "See the failed jobs on Home" : (r.jobs.cap ? num(r.jobs.runs_today) + " of " + plural(r.jobs.cap, "run") : plural(r.jobs.runs_today, "run")) + " today",
    },
    { label: "Cleanup", tone: leftovers.count ? "warn" : "ok", value: leftovers.text, title: r.hygiene },
  ];
  return shell(
    <>
      <dl className="-my-1 grid divide-y divide-border/70 text-sm">
        {lines.map((l) => (
          <div key={l.label} className="grid min-w-0 grid-cols-[5.5rem_minmax(0,1fr)] items-baseline gap-x-3 py-2.5">
            <dt className="flex items-center gap-2 text-muted-foreground">
              <span className={cn("size-2 shrink-0 translate-y-[-1px] rounded-full", DOT[l.tone])} role="img" aria-label={DOT_WORD[l.tone]} />
              {l.label}
            </dt>
            <dd className="m-0 min-w-0 anywhere" title={l.title}>
              <span className="font-medium text-foreground">{l.value}</span>
              {l.note ? <span className="block text-[13px] leading-relaxed text-muted-foreground">{l.note}</span> : null}
            </dd>
          </div>
        ))}
      </dl>
      {stopped ? (
        <Notice tone="bad" className="mt-3">
          {"The runner is paused: " + (r.runner?.reason || "the stop switch is on") + ". No new agent starts until you turn it back on."}
        </Notice>
      ) : null}
      <WorkerHold hold={r.hold} />
      <LandingHolds holds={r.landing} />
      <Disclosure summary="Show details" className="mt-3 border-t border-border/70 pt-3" contentClassName="mt-3" open={details} onToggle={setDetails}>
        <div className="grid min-w-0 gap-3 [&>*]:min-w-0">
          <Kv
            className="text-[13px]"
            rows={[
              ["Runner says", <span key="d" title={d.state}>{sentence(d.text || d.state || "nothing yet")}</span>],
              ...(d.loops !== undefined ? [["Rounds", num(d.loops) + (d.passes !== undefined ? " so far, " + num(d.passes) + " of them planned work" : " so far")] as Row] : []),
              ["Checks every", d.interval ? spanSeconds(d.interval) : "—"],
              ["Last check-in", <When key="at" d={d.at} />],
              ["Slots", num(r.hands.free) + " of " + num(r.hands.slots) + " free, on " + plural(r.hands.executors_active, "active machine")],
              ["Agents", num(r.workers.busy) + " working, " + num(r.workers.idle) + " idle; keeps " + num(r.workers.target) + " ready"],
              ["Work", num(r.jobs.queued) + " waiting to start, " + num(r.jobs.leased) + " starting, " + num(r.jobs.running) + " running" + (r.jobs.blocked ? ", " + num(r.jobs.blocked) + " stuck" : "")],
              ["Runs today", num(r.jobs.runs_today) + (r.jobs.cap ? " of " + num(r.jobs.cap) : " (no daily limit)")],
              ...(r.capacity ? [["Status line", <span key="c" className="text-muted-foreground">{r.capacity}</span>] as Row] : []),
              ...(r.hygiene ? [["Last cleanup", <span key="h" className="text-muted-foreground">{r.hygiene}</span>] as Row] : []),
            ]}
          />
          {machines && r.executors.length ? (
            <ul className="grid gap-1.5" aria-label="Machines">
              {r.executors.map((e) => (
                <ExecutorRow key={e.name} e={e} />
              ))}
            </ul>
          ) : null}
        </div>
      </Disclosure>
    </>,
  );
}

type Row = [React.ReactNode, React.ReactNode];

const sentence = (s: string) => (s ? s.charAt(0).toUpperCase() + s.slice(1) : s);

// The `pulse status` hygiene line ("last sweep <at>: removed …; held branch 3,
// worktree 1") in words: how much is kept and when the last cleanup ran.
function hygieneLeftovers(h: string | undefined, now: number): { count: number; text: string } {
  if (!h) return { count: 0, text: "Nothing left behind" };
  const heldPart = /held\s+([^;]+)/i.exec(h)?.[1] ?? "";
  const held = [...heldPart.matchAll(/([a-z]+)\s+(\d+)/gi)].reduce((n, m) => n + Number(m[2] || 0), 0);
  const swept = /last sweep\s+([^:]+(?::\d\d){0,2}Z?)\s*:/i.exec(h)?.[1];
  return held ? { count: held, text: plural(held, "leftover") + (swept ? ", last cleaned " + rel(swept, now) : "") } : { count: 0, text: "Nothing left behind" };
}

const spanSeconds = (sec: number) => (sec < 60 ? plural(sec, "second") : sec < 3600 ? plural(Math.round(sec / 60), "minute") : plural(Math.round(sec / 3600), "hour"));

// The login gate: agents on this machine wait for the owner to sign in.
function WorkerHold({ hold }: { hold: string | undefined }) {
  if (!hold) return null;
  const first = sentence(hold.split(":")[0].replace(/^workers\b/i, "agents").trim());
  return (
    <div className="mt-3 flex items-start gap-2.5 rounded-xl border border-warn/35 bg-warn/[0.08] px-3.5 py-2.5 text-sm leading-relaxed" aria-label="workers held" title={hold}>
      <AlertTriangleIcon className="mt-0.5 size-4 shrink-0 text-warn" aria-hidden />
      <span className="min-w-0 anywhere">
        {first + ". "}
        <Link href={hrefs.settings("harness")} className="font-medium text-foreground underline underline-offset-2">
          Sign in
        </Link>
      </span>
    </div>
  );
}

// Verified branches that cannot merge and why (ADR-024): the runner writes
// one note per held project - a dirty or switched checkout, a lock, an
// unreadable folder - and clears it when the branches merge again. Nothing
// pushes; merging moves the local default branch, so the fix is the owner's:
// commit or stash, switch back, and the next round merges the queue.
export function LandingHolds({ holds }: { holds: LandingHold[] | undefined }) {
  if (!holds?.length) return null;
  return (
    <ul className="mt-3 grid gap-1.5" aria-label="landing held">
      {holds.map((h) => (
        <li key={h.project} className="flex min-w-0 items-start gap-2.5 rounded-xl border border-warn/35 bg-warn/[0.08] px-3.5 py-2.5 text-sm leading-relaxed">
          <GitMergeIcon className="mt-0.5 size-4 shrink-0 text-warn" aria-hidden />
          <span className="min-w-0 anywhere">
            <span className="font-medium">{"Can't merge into " + h.project + ". "}</span>
            {sentence(h.note)}
            {h.stale ? <span className="text-muted-foreground" title="This note may be out of date">{" (older note)"}</span> : null}
          </span>
        </li>
      ))}
    </ul>
  );
}

// What kind of machine a row is, as a person would say it.
function kindWord(e: Executor): string {
  const k = (e.kind || "").toLowerCase();
  if (k === "local" || e.name === "local") return "This machine";
  if (k === "ssh") return "Over SSH";
  if (/cloud|azure|aws|gcp|hetzner|vm/.test(k)) return "Cloud machine";
  return statusWord(k) || "Machine";
}

// One machine that runs agents: its name and kind, whether it takes work,
// and how many of its agent slots are free. `action` holds a Remove menu.
export function ExecutorRow({ e, action }: { e: Executor; action?: React.ReactNode }) {
  // A drained machine reads "info" elsewhere; here it is simply not taking work, so it stays grey.
  const raw = statusTone(e.status);
  const tone: Tone = raw === "info" ? "neutral" : raw;
  const running = e.running ?? 0;
  const free = e.free ?? 0;
  const slots = (running ? num(running) + " running, " : "") + num(free) + " of " + plural(e.workers, "slot") + " free";
  const projects = e.projects && e.projects !== "*" ? e.projects.split(",").map((p) => p.trim()).filter(Boolean).join(", ") : "";
  const reserved = Boolean(e.reserved) || /\(reserved\)/.test(e.offers || "");
  const offers = (e.offers || "").replace("(reserved)", "").trim();
  const tip = [e.address && "Address: " + e.address, e.workdir && "Folder: " + e.workdir, "Kind: " + e.kind, "Status: " + e.status, e.errors ? "Errors: " + e.errors + (e.backoff_minutes ? ", next try in " + e.backoff_minutes + " min" : "") : ""].filter(Boolean).join("\n");
  return (
    <li className="flex min-w-0 items-start gap-3 rounded-xl border bg-card px-3 py-2.5 text-sm">
      <span className={cn("mt-1.5 size-2 shrink-0 rounded-full", DOT[tone])} aria-hidden />
      <div className="min-w-0 flex-1" title={tip}>
        <div className="flex min-w-0 flex-wrap items-baseline gap-x-2">
          <span className="font-medium text-foreground anywhere">{e.name}</span>
          <span className="text-[13px] text-muted-foreground">{kindWord(e)}</span>
        </div>
        <div className="mt-0.5 flex min-w-0 flex-wrap gap-x-3 gap-y-0.5 text-[13px] leading-relaxed text-muted-foreground">
          <span className={cn("font-medium", tone === "ok" ? "text-ok" : tone === "bad" ? "text-bad" : tone === "warn" ? "text-warn" : tone === "you" ? "text-you-ink" : "text-foreground/80")}>{statusWord(e.status)}</span>
          <span>{slots}</span>
          {projects ? <span className="anywhere">{"Only " + projects}</span> : null}
          {offers && offers !== "-" ? <span className="anywhere" title="What this machine offers; a project asks for these words in its .pulse/pulse.yaml needs:">{"Offers " + offers.split(" ").join(", ")}</span> : null}
          {reserved ? <span className="text-foreground/80" title="Takes only the jobs that need something it offers">Reserved for the work that needs it</span> : null}
        </div>
      </div>
      {action ? <span className="-my-1 -mr-1 shrink-0">{action}</span> : null}
    </li>
  );
}
