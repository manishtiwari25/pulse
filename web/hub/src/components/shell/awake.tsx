"use client";

import Link from "next/link";
import { useEffect } from "react";
import { CoffeeIcon } from "lucide-react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { DaemonAwake, DaemonSlept, Runner } from "@/lib/api";
import { fmtDate, rel, span } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// Keep-awake (daemon.keep_awake): while the runner holds this machine awake
// a hairline of amber runs along the top of the console (`data-awake` on
// <html>, globals.css) and a small steaming cup in the shell says since
// when. The cup follows a live runner only: a dead daemon's last heartbeat
// may still say "holding", and the machine is free to sleep then.

export interface AwakeView {
  // The setting is on (the owner asked for the hold), whatever the runner does with it.
  on: boolean;
  // The runner is alive and holds the machine awake right now.
  holding: boolean;
  // `on` while the machine runs on battery: the runner holds nothing until
  // the charger is back (the display and idle assertions would drain it).
  waiting: boolean;
  // Where the machine draws its power from, as the runner last read it.
  power: "ac" | "battery" | "unknown";
  awake: DaemonAwake | null;
  slept: DaemonSlept | null;
}

export function awakeView(r: Runner | undefined): AwakeView {
  const d = r?.daemon;
  const awake = d?.awake ?? null;
  const live = d?.state === "alive";
  const on = !!awake && awake.mode !== "off";
  const held = live && !!awake?.holding;
  // Older daemons kept a hold that did nothing on battery and said so with
  // `holding` true, `effective` false; newer ones say `waiting` outright.
  const waiting = live && on && (awake?.waiting === true || (held && awake?.effective === false));
  return { on, holding: held && !waiting, waiting, power: awake?.power ?? "unknown", awake, slept: d?.slept ?? null };
}

export function useAwake(): AwakeView {
  const q = useApi<Runner>("runner", { refresh: 30000 });
  return awakeView(q.data);
}

// One or two sentences for the state, used by the shell's cup, the hero and
// the Settings strip. The display sentence comes from the runner (macOS:
// the hold's display assertion is what the screen-saver daemon checks, so
// the saver stays off too), the lid caveat likewise.
export function awakeText(v: AwakeView): string {
  const a = v.awake;
  if (!a || a.mode === "off") return "The machine sleeps as its power settings say.";
  if (v.holding) {
    return (
      "Keeping this machine awake" +
      (a.mode === "always" ? ", on battery too" : " while it's plugged in") +
      (a.since ? ", since " + rel(a.since) : "") +
      "." +
      (a.display ? " " + capital(a.display) + "." : "") +
      (a.lid ? " " + capital(a.lid) + "." : "")
    );
  }
  if (v.waiting) return "Keep-awake is on, but the machine runs on battery, so nothing holds it until the charger is back. Set it to always to hold on battery too.";
  if (a.note) return "Keep-awake is on, but nothing holds the machine right now: " + a.note + ".";
  return "Keep-awake is on; the runner takes the hold on its next check-in.";
}

// The short label next to the cup.
export function awakeLabel(v: AwakeView): string {
  return v.holding ? "Staying awake" : v.waiting ? "Awake once plugged in" : "Keep-awake on, not holding";
}

function capital(s: string): string {
  return s ? s[0].toUpperCase() + s.slice(1) : s;
}

export function sleptText(s: DaemonSlept | null): string {
  if (!s || !s.seconds) return "";
  return "The machine slept for about " + span(s.seconds) + " until " + rel(s.until) + ".";
}

// Sets the theme attribute on <html>. Mounted once, in the shell.
export function AwakeTheme() {
  const v = useAwake();
  const value = v.holding ? "holding" : v.on ? "on" : "";
  useEffect(() => {
    const el = document.documentElement;
    if (value) el.setAttribute("data-awake", value);
    else el.removeAttribute("data-awake");
    return () => el.removeAttribute("data-awake");
  }, [value]);
  return null;
}

// The cup in the sidebar footer and the compact header: shown only while the
// setting is on, steaming while the hold is held.
export function AwakeStatus({ compact }: { compact?: boolean }) {
  const v = useAwake();
  if (!v.on) return null;
  const text = awakeText(v);
  const label = awakeLabel(v) + ". " + text;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Link
          href={hrefs.settings("daemon")}
          aria-label={label + " Open the machine settings."}
          data-holding={v.holding}
          className={cn(
            "awake-cup inline-flex items-center gap-1.5 rounded-lg text-[13px] text-muted-foreground outline-none transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
            compact ? "size-10 justify-center" : "h-8 px-2",
          )}
        >
          <span className="relative inline-flex size-[18px] items-center justify-center" aria-hidden>
            <CoffeeIcon className={cn("size-[18px] lg:size-4", v.holding ? "text-awake" : "")} />
            {v.holding ? (
              <span className="awake-steam pointer-events-none absolute -top-2 left-1/2 flex -translate-x-1/2 gap-[3px]">
                <i />
                <i />
                <i />
              </span>
            ) : null}
          </span>
          {compact ? null : <span>{v.holding ? "Awake" : v.waiting ? "Awake: battery" : "Awake: off"}</span>}
        </Link>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-xs">
        {text}
        {v.awake?.since && v.holding ? <span className="block text-muted-foreground">{fmtDate(v.awake.since)}</span> : null}
      </TooltipContent>
    </Tooltip>
  );
}
