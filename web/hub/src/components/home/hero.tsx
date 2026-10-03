"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { ArrowUpRightIcon, SparklesIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/pulse/panel";
import { cn } from "@/lib/utils";
import { plural, type Tone } from "@/lib/format";
import { hrefs } from "@/lib/nav";

const PILL_TONE: Record<Tone, string> = {
  neutral: "border-border/70 bg-background/55 text-muted-foreground",
  ok: "border-ok/35 bg-ok/10 text-ok",
  warn: "border-warn/45 bg-warn/10 text-warn",
  bad: "border-bad/40 bg-bad/10 text-bad",
  info: "border-info/40 bg-info/10 text-info",
  accent: "border-accent-2/40 bg-accent-2/10 text-accent-2",
};

export interface HomeHeroProps {
  inbox: number;
  daemon: { tone: Tone; label: string; pause: "on" | "off" | "unknown"; setup: unknown };
  daemonDown: boolean;
  runnerChecking: boolean;
  inboxChecking: boolean;
  runningJobs?: number;
  fallbackWorkingAgents: number;
  activeAgents: number;
  projects: number;
  ownerAway: boolean;
  actions: React.ReactNode;
}

export function HomeHero({ inbox, daemon, daemonDown, runnerChecking, inboxChecking, runningJobs, fallbackWorkingAgents, activeAgents, projects, ownerAway, actions }: HomeHeroProps) {
  const [greeting, setGreeting] = useState("Good day");

  useEffect(() => {
    const h = new Date().getHours();
    window.requestAnimationFrame(() => setGreeting(h < 12 ? "Good morning" : h < 18 ? "Good afternoon" : "Good evening"));
  }, []);

  const working = runningJobs ?? fallbackWorkingAgents;
  const sentence = daemon.pause === "on"
    ? "PULSE is paused on this machine."
    : daemonDown
      ? "The hub can't reach the daemon."
      : inbox > 0
        ? (inbox === 1 ? "1 thing needs your yes or no." : plural(inbox, "thing") + " need your yes or no.")
        : inboxChecking
          ? "Checking what needs your yes or no…"
          : "Nothing needs you right now — " + plural(activeAgents, "agent") + " working on " + plural(projects, "project") + ".";
  const daemonText = daemon.pause === "on" ? "Daemon paused" : daemon.tone === "ok" ? "Daemon running" : daemon.label === "checking" ? "Checking daemon…" : "Daemon off";
  const agentText = runnerChecking ? "Checking agents…" : plural(working, "agent") + " working";

  return (
    <section className="mb-8 rounded-3xl surface-hero glossy p-5 shadow-pop ring-soft sm:p-6" aria-label="Home summary">
      <div className="grid min-w-0 gap-5 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center">
        <div className="min-w-0">
          <p className="mb-2 flex items-center gap-2 text-sm font-medium text-brand">
            <SparklesIcon className="size-4" aria-hidden />
            {greeting}
          </p>
          <h1 className="max-w-4xl text-3xl font-semibold tracking-tight text-balance sm:text-4xl">{sentence}</h1>
          <div className="mt-5 flex min-w-0 flex-wrap gap-2">
            <StatusPill tone={daemon.tone}>{daemonText}</StatusPill>
            <StatusPill tone={runnerChecking ? "neutral" : working ? "ok" : "neutral"}>{agentText}</StatusPill>
            <StatusPill tone="neutral">{plural(projects, "project")}</StatusPill>
          </div>
        </div>
        <div className="flex min-w-0 flex-wrap items-center gap-2 lg:justify-end">
          {inbox > 0 ? (
            <Button asChild size="lg" className="glossy rounded-xl shadow-pop">
              <a href="#for-you">
                Review <ArrowUpRightIcon />
              </a>
            </Button>
          ) : null}
          {actions}
        </div>
      </div>
      {ownerAway ? (
        <Notice tone="warn" className="mt-5 bg-background/55">
          You&apos;re marked away. Project leads can handle routine proposals, but hard stops and owner-only choices still wait here.{" "}
          <Link href={hrefs.settings("owner")} className="font-medium underline underline-offset-2">
            Change this in settings
          </Link>
          .
        </Notice>
      ) : null}
    </section>
  );
}

function StatusPill({ tone, children }: { tone: Tone; children: React.ReactNode }) {
  return <span className={cn("inline-flex min-h-9 items-center rounded-full border px-3 py-1 text-sm font-medium shadow-card", PILL_TONE[tone])}>{children}</span>;
}
