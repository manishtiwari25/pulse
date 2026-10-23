"use client";

import Link from "next/link";
import { useState } from "react";
import { Loader2Icon, UndoIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { showApiError } from "@/components/pulse/confirm";
import { PulseMascot } from "@/components/pulse/mascots";
import { writes } from "@/lib/api";
import { hrefs } from "@/lib/nav";
import { invalidate } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// The owner is marked away (owner.presence = away): instead of a warning
// triangle, a small scene says what that means - the sign on your door
// sways, and Pip ticks off the routine proposals on your behalf - with the
// one action that matters most, "I'm back", right there. Animations run
// only when the system allows motion (globals.css, away-scene).
export function AwayCard({ canEdit, className }: { canEdit: boolean; className?: string }) {
  const [busy, setBusy] = useState(false);
  const back = async () => {
    setBusy(true);
    const id = toast.loading("Marking you as here…");
    try {
      await writes.setSetting("owner.presence", "present");
      toast.success("Welcome back. Proposals and requests wait for your answer again.", { id });
      invalidate((p) => ["org", "settings", "events", "inbox"].some((path) => p === path || p.startsWith(path + "?") || p.startsWith(path + "/")));
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "PULSE couldn't change your presence. Try again, or change it in Settings.");
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className={cn("away-card flex flex-col gap-4 overflow-hidden rounded-2xl border border-you/40 bg-you-soft p-4 sm:flex-row sm:items-center sm:gap-6 sm:p-5", className)} aria-label="You are marked as away">
      <AwayScene className="mx-auto w-[220px] shrink-0 sm:mx-0" />
      <div className="min-w-0 flex-1">
        <h2 className="text-base font-semibold tracking-tight">You&apos;re marked as away.</h2>
        <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
          Each project&apos;s lead agent answers routine proposals for you and says so on the board. Hard stops, money and the choices only you can make still wait here for you.
        </p>
        <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-2">
          {canEdit ? (
            <Button size="sm" onClick={back} disabled={busy}>
              {busy ? <Loader2Icon className="animate-spin" /> : <UndoIcon />} I&apos;m back
            </Button>
          ) : null}
          <Link href={hrefs.settings("owner")} className="text-sm font-medium underline-offset-4 hover:underline">
            {canEdit ? "Change this in Settings" : "See this in Settings"}
          </Link>
        </div>
      </div>
    </section>
  );
}

const LINE = "oklch(0.55 0.09 262)";
const PAPER = "oklch(1 0 0)";
// The sign's lettering and the pen: a fixed brown ink, dark enough on the
// marigold sign in both themes (the theme's --you-ink goes light in the dark).
const SIGN_INK = "oklch(0.38 0.07 65)";

// Your door with its swaying "Away" sign on the left; Pip at a clipboard on
// the right, check marks popping in as the routine things get decided.
function AwayScene({ className }: { className?: string }) {
  return (
    <div className={cn("away-scene relative", className)} aria-hidden>
      <svg viewBox="0 0 220 120" className="block h-auto w-full overflow-visible">
        {/* the door and its frame */}
        <rect x={14} y={10} width={64} height={110} rx={6} fill="color-mix(in oklch, var(--card) 70%, var(--you-soft))" stroke={LINE} strokeWidth={1.6} />
        <rect x={22} y={20} width={48} height={40} rx={3} fill="none" stroke={LINE} strokeWidth={1.2} opacity={0.55} />
        <rect x={22} y={68} width={48} height={44} rx={3} fill="none" stroke={LINE} strokeWidth={1.2} opacity={0.55} />
        <circle cx={70} cy={66} r={2.6} fill={LINE} />
        {/* the hook and the sign that hangs from it */}
        <circle cx={46} cy={30} r={2.2} fill={LINE} />
        <g className="away-sign" style={{ ["--sign-origin" as string]: "46px 30px" }}>
          <path d="M46 30 L36 44 M46 30 L56 44" stroke={LINE} strokeWidth={1.3} fill="none" />
          <rect x={30} y={44} width={32} height={18} rx={3} fill="var(--you)" stroke={SIGN_INK} strokeWidth={1.4} />
          <text x={46} y={56.5} textAnchor="middle" fontSize={9.5} fontWeight={800} letterSpacing={0.6} fill={SIGN_INK} fontFamily="ui-sans-serif, system-ui">
            AWAY
          </text>
        </g>
        {/* the floor */}
        <path d="M6 119 H214" stroke={LINE} strokeWidth={1.2} strokeLinecap="round" opacity={0.5} />
        {/* the clipboard Pip works through */}
        <g transform="translate(142 22)">
          <rect x={0} y={0} width={70} height={92} rx={5} fill={PAPER} stroke={LINE} strokeWidth={1.6} />
          <rect x={22} y={-5} width={26} height={10} rx={3} fill={LINE} />
          <g stroke={LINE} strokeWidth={2} strokeLinecap="round" opacity={0.55}>
            <path d="M26 24 h30" />
            <path d="M26 46 h30" />
            <path d="M26 68 h22" />
          </g>
          <g stroke={LINE} strokeWidth={1.4} fill="none">
            <rect x={8} y={17} width={12} height={12} rx={2.5} />
            <rect x={8} y={39} width={12} height={12} rx={2.5} />
            <rect x={8} y={61} width={12} height={12} rx={2.5} />
          </g>
          <g stroke="var(--ok)" strokeWidth={2.6} strokeLinecap="round" strokeLinejoin="round" fill="none">
            <path className="away-check" d="M10.5 23.5 l3 3 l6 -7" />
            <path className="away-check" d="M10.5 45.5 l3 3 l6 -7" style={{ animationDelay: "1.6s" }} />
            <path className="away-check" d="M10.5 67.5 l3 3 l6 -7" style={{ animationDelay: "3.2s" }} />
          </g>
          {/* the pen, scribbling beside the last line */}
          <g className="away-pen">
            <path d="M54 74 l10 -14" stroke={SIGN_INK} strokeWidth={3} strokeLinecap="round" />
            <path d="M54 74 l-2 4 l4 -1.5 z" fill={SIGN_INK} />
          </g>
        </g>
      </svg>
      <PulseMascot mood="busy" size={72} decorative className="float absolute bottom-0 left-[88px] size-[64px]" />
    </div>
  );
}
