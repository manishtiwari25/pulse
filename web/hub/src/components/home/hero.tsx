"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { ArrowDownIcon, CoffeeIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { AwayCard } from "@/components/home/away-card";
import { PulseLine } from "@/components/shell/pulse-mark";
import { awakeLabel, awakeText, type AwakeView } from "@/components/shell/awake";
import { PulseMascot, type PipMood } from "@/components/pulse/mascots";
import { daemonHeadline, daemonUpdateText, type DaemonUpdate } from "@/components/pulse/daemon-control-state";
import { cn } from "@/lib/utils";
import { plural, type Tone } from "@/lib/format";
import { hrefs } from "@/lib/nav";

const DOT: Record<Tone, string> = {
  neutral: "bg-muted-foreground/50",
  ok: "bg-ok",
  warn: "bg-warn",
  bad: "bg-bad",
  info: "bg-info",
  accent: "bg-accent-2",
  you: "bg-you",
};

export interface HomeHeroProps {
  inbox: number;
  daemon: { tone: Tone; label: string; pause: "on" | "off" | "unknown"; setup: unknown; update?: DaemonUpdate | null };
  daemonDown: boolean;
  runnerChecking: boolean;
  inboxChecking: boolean;
  runningJobs?: number;
  fallbackWorkingAgents: number;
  activeAgents: number;
  projects: number;
  ownerAway: boolean;
  // Whether this reader may change settings (the away card's "I'm back").
  canEdit?: boolean;
  // Keep-awake (daemon.keep_awake): one line under the headline while it is on.
  awake?: AwakeView | null;
  // Opens "Details for this machine" and scrolls to the runner controls.
  onOpenRunner?: () => void;
  actions: React.ReactNode;
}

// The page opens like a briefing: a greeting, one sentence that says whether
// anything needs you, and how the machine is doing, in words. Pip sits
// beside it in the mood of the moment: asleep when work is paused, worried
// when the runner is gone, holding a note when something waits, typing on
// a laptop when all is well - with a coffee while the runner holds the
// machine awake, and the laptop on its charger while that hold has AC power.
export function HomeHero({ inbox, daemon, daemonDown, runnerChecking, inboxChecking, runningJobs, fallbackWorkingAgents, activeAgents, projects, ownerAway, canEdit, awake, onOpenRunner, actions }: HomeHeroProps) {
  const [greeting, setGreeting] = useState("Hello");

  useEffect(() => {
    const h = new Date().getHours();
    window.requestAnimationFrame(() => setGreeting(h < 5 ? "Good evening" : h < 12 ? "Good morning" : h < 18 ? "Good afternoon" : "Good evening"));
  }, []);

  const working = runningJobs ?? fallbackWorkingAgents;
  const paused = daemon.pause === "on";
  const headline = paused
    ? "Work is paused on this machine."
    : daemonDown
      ? "PULSE can't reach its runner right now."
      : inbox > 0
        ? inbox === 1
          ? "1 thing is waiting for your answer."
          : plural(inbox, "thing") + " are waiting for your answer."
        : inboxChecking
          ? "Checking what's waiting for you…"
          : "You're all caught up.";
  const subline = paused
    ? "Agents finish what they're doing and start nothing new until you resume."
    : daemonDown
      ? "The page will update on its own once the runner answers again."
      : inbox > 0
        ? "Each one stays put until you say yes or no. Everything else carries on meanwhile."
        : inboxChecking
          ? ""
          : "Nothing needs your answer. " + (activeAgents ? plural(activeAgents, "agent") + (activeAgents === 1 ? " is" : " are") + " active across " + plural(projects, "project") + "." : "Give a task when you're ready.");
  // A pending self-update is not a stopped daemon: the hero names the run the new build waits for.
  const updateText = !paused && daemon.update ? daemonUpdateText(daemon.update) : "";
  const agentText = runnerChecking ? "Checking agents…" : working ? plural(working, "agent") + " working" : "No agents working";
  const mood: PipMood = paused ? "asleep" : daemonDown ? "worried" : inbox > 0 ? "waiting" : inboxChecking ? "busy" : "working";
  const coffee = !!awake?.holding;
  const charging = coffee && awake?.power === "ac";

  return (
    <section className="pt-8 pb-2 sm:pt-12" aria-label="Summary">
      <p className="flex items-center gap-3 text-[15px] font-medium text-muted-foreground">
        <PulseLine className="h-6 w-[110px]" />
        <span>{greeting}</span>
      </p>
      <div className="mt-4 flex min-w-0 items-start justify-between gap-4 sm:gap-8">
        <div className="min-w-0 flex-1">
          <h1 className="enter max-w-3xl text-[32px] font-semibold leading-[1.12] tracking-[-0.02em] sm:text-[40px]">{headline}</h1>
          {subline ? <p className="mt-3 max-w-2xl text-base leading-relaxed text-muted-foreground sm:text-[17px]">{subline}</p> : null}
        </div>
        <PulseMascot key={mood} mood={mood} coffee={coffee} charging={charging} size={112} className="pop float mt-1 size-[72px] shrink-0 sm:mr-6 sm:size-32" />
      </div>

      <ul className="mt-5 flex min-w-0 flex-wrap items-center gap-x-6 gap-y-2 text-sm" aria-label="This machine">
        <li>
          <a
            href="#daemon-controls"
            onClick={(e) => {
              if (!onOpenRunner) return;
              e.preventDefault();
              onOpenRunner();
            }}
            className="inline-flex items-center gap-2 rounded-md font-medium underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            title="The runner plans work and starts agents on this machine. Open its controls."
          >
            <span className={cn("size-2 rounded-full", DOT[daemon.tone])} aria-hidden />
            {daemonHeadline(daemon)}
          </a>
        </li>
        {updateText ? (
          <li className="inline-flex items-center gap-2 text-muted-foreground">
            <span className="size-2 rounded-full bg-info" aria-hidden />
            {updateText}
          </li>
        ) : null}
        <li className="inline-flex items-center gap-2 text-muted-foreground">
          <span className={cn("size-2 rounded-full", runnerChecking ? DOT.neutral : working ? "bg-primary" : DOT.neutral)} aria-hidden />
          {agentText}
        </li>
        <li className="inline-flex items-center gap-2 text-muted-foreground">
          <span className="size-2 rounded-full bg-muted-foreground/50" aria-hidden />
          {plural(projects, "project")}
        </li>
        {awake?.on ? (
          <li className="inline-flex items-center gap-2 text-muted-foreground">
            <Link href={hrefs.settings("daemon")} className="inline-flex items-center gap-2 rounded-md underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" title={awakeText(awake)}>
              <CoffeeIcon className={cn("size-3.5", awake.holding ? "text-awake" : "")} aria-hidden />
              {awake.holding ? "Staying awake for its agents" : awakeLabel(awake)}
            </Link>
          </li>
        ) : null}
      </ul>

      <div className="mt-6 flex min-w-0 flex-wrap items-center gap-2">
        {inbox > 0 ? (
          <Button asChild size="lg">
            <a href="#for-you">
              <ArrowDownIcon /> See what&apos;s waiting
            </a>
          </Button>
        ) : null}
        {actions}
      </div>

      {ownerAway ? <AwayCard canEdit={!!canEdit} className="enter mt-6 max-w-3xl" /> : null}
    </section>
  );
}
