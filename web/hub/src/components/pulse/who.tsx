"use client";

import Link from "next/link";
import { BotIcon, CheckIcon, ShieldAlertIcon, ThumbsDownIcon, ThumbsUpIcon, UserIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { fmtDate, KIND_TONE, rel, statusTone, type Tone } from "@/lib/format";
import { postKindWord, statusWord, threadName } from "@/lib/words";
import { hrefs } from "@/lib/nav";
import { useNow } from "@/lib/use-api";
import type { Who as WhoRow } from "@/lib/api";
import { AgentFace } from "./mascots";

// Soft tone pills: a tinted background and the tone's ink, nothing else.
const TONE_BADGE: Record<Tone, string> = {
  neutral: "bg-muted text-muted-foreground",
  ok: "bg-ok/10 text-ok",
  warn: "bg-warn/10 text-warn",
  bad: "bg-bad/10 text-bad",
  info: "bg-info/10 text-info",
  accent: "bg-accent-2/10 text-accent-2",
  you: "bg-you-soft text-you-ink ring-1 ring-inset ring-[color-mix(in_oklch,var(--you)_45%,transparent)]",
};

export function ToneBadge({ tone = "neutral", children, className, dot, title }: { tone?: Tone; children: React.ReactNode; className?: string; dot?: boolean; title?: string }) {
  return (
    <Badge variant="outline" title={title} className={cn("h-5 gap-1.5 rounded-full border-transparent px-2 text-xs font-medium", TONE_BADGE[tone], className)}>
      {dot ? <span className="size-1.5 rounded-full bg-current" aria-hidden /> : null}
      {children}
    </Badge>
  );
}

// A state in plain words ("in_progress" reads "In progress"); the raw state stays in the tooltip.
export function StatusBadge({ status, className }: { status?: string | null; className?: string }) {
  const s = (status || "").trim();
  if (!s) return null;
  const word = statusWord(s);
  return (
    <ToneBadge tone={statusTone(s)} dot className={className} title={word.toLowerCase() !== s.replace(/_/g, " ").toLowerCase() ? s : undefined}>
      {word}
    </ToneBadge>
  );
}

export function KindBadge({ kind }: { kind?: string | null }) {
  const k = kind || "post";
  return <ToneBadge tone={KIND_TONE[k] ?? "neutral"}>{postKindWord(k)}</ToneBadge>;
}

export function VoteBadge({ vote, classification }: { vote?: string; classification?: string }) {
  if (!vote) return null;
  const up = vote === "up" || vote === "yes" || vote === "+1";
  const counted = !classification || classification === "counted";
  const Icon = up ? ThumbsUpIcon : ThumbsDownIcon;
  return (
    <ToneBadge tone={up ? "ok" : "bad"} className={cn(!counted && "opacity-60 line-through")} title={counted ? undefined : "Not counted: " + classification}>
      <Icon className="size-3" aria-hidden />
      {up ? "For" : "Against"}
    </ToneBadge>
  );
}

// Whether a post says who wrote it (runner, model, session) - agents sign their work.
// Signed posts are the norm and say nothing; only a gap gets a quiet mark.
export function Provenance({ ok, missing }: { ok?: number; missing?: string[] }) {
  if (ok === undefined || ok === 1) return null;
  const text = "Not fully signed: " + ((missing || []).join(", ") || "runner, model or session") + " missing";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span tabIndex={0} aria-label={text} className="inline-flex rounded-sm text-muted-foreground">
          <ShieldAlertIcon className="size-3.5" />
        </span>
      </TooltipTrigger>
      <TooltipContent>{text}</TooltipContent>
    </Tooltip>
  );
}

// Commands, paths and ids - the only text set in the mono face.
export function Mono({ children, className }: { children: React.ReactNode; className?: string }) {
  return <span className={cn("rounded-md bg-muted px-1.5 py-0.5 font-mono text-[12.5px] text-foreground/85 anywhere", className)}>{children}</span>;
}

export function When({ d, className, exact }: { d?: string | null; className?: string; exact?: boolean }) {
  const now = useNow();
  if (!d) return <span className={cn("text-muted-foreground", className)}>{"—"}</span>;
  return (
    <time dateTime={d} title={fmtDate(d)} className={cn("tnum whitespace-nowrap", className)}>
      {exact ? fmtDate(d) : rel(d, now)}
    </time>
  );
}

// Agents get a stable face and colour from their name; the owner is always
// the teal face with the marigold star. The name itself sits next to it.
export function Avatar({ seed, size = 22, human, className }: { seed: string; size?: number; human?: boolean; className?: string }) {
  return <AgentFace seed={seed || "?"} size={size} human={human} className={className} />;
}

// An agent identity: name (or session), with runner/model as the quiet
// second line. `compact` keeps it to one line for tables.
export function Who({ row, compact, wrap, className }: { row: WhoRow & { author?: string }; compact?: boolean; wrap?: boolean; className?: string }) {
  const name = row.agent || row.name || row.author || row.session || "(unknown)";
  const human = name === "human" || name === "owner";
  const meta = [row.runner, row.model].filter(Boolean).join(", ");
  return (
    <span className={cn("inline-flex min-w-0 gap-1.5", wrap ? "items-start" : "items-center", className)} title={meta ? name + " (" + meta + ")" : name}>
      <Avatar seed={name} size={compact ? 18 : 24} human={human} className={wrap ? "mt-px" : undefined} />
      <span className="min-w-0">
        <span className={cn("block font-medium text-foreground", wrap ? "anywhere" : "truncate", compact ? "text-[13px]" : "text-sm")}>{human ? "You" : name === "(unknown)" ? "An agent" : name}</span>
        {!compact && meta ? <span className="block truncate text-xs text-muted-foreground">{meta}</span> : null}
      </span>
    </span>
  );
}

export function AuthorBadge({ author }: { author: string }) {
  const human = author === "human" || author === "owner";
  return (
    <ToneBadge tone={human ? "info" : "neutral"} className="gap-1">
      {human ? <UserIcon className="size-3" /> : <BotIcon className="size-3" />}
      {human ? "You" : author}
    </ToneBadge>
  );
}

export function ProjectLink({ slug, brain, className }: { slug: string; brain?: boolean; className?: string }) {
  return (
    <Link href={hrefs.project(slug)} className={cn("inline-flex items-center gap-1 font-medium text-foreground underline-offset-2 hover:underline", className)}>
      {slug}
      {brain ? (
        <Badge variant="secondary" className="h-4 px-1.5 text-[10.5px]" title="PULSE's own source code">
          PULSE
        </Badge>
      ) : null}
    </Link>
  );
}

// A board thread, by its name ("Checkout latency"); the area/name key it is
// filed under rides in the tooltip. Quiet and readable, not a code chip.
export function ThreadLink({ slug, k, className }: { slug: string; k: string; className?: string }) {
  return (
    <Link href={hrefs.thread(slug, k)} title={"Thread " + k} className={cn("text-[13px] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline anywhere", className)}>
      {threadName(k)}
    </Link>
  );
}

export function PostLink({ slug, id, label, className }: { slug: string; id: string; label?: string; className?: string }) {
  return (
    <Link href={hrefs.post(slug, id)} title={id} className={cn(label ? "underline-offset-2 hover:underline anywhere" : "font-mono text-[12px] hover:underline anywhere", className)}>
      {label || id}
    </Link>
  );
}

export function Check({ on }: { on: boolean }) {
  return on ? <CheckIcon className="inline size-3.5 text-ok" aria-label="yes" /> : <span className="text-muted-foreground">{"—"}</span>;
}
