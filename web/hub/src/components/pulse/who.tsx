"use client";

import Link from "next/link";
import { BotIcon, CheckIcon, ShieldAlertIcon, ShieldCheckIcon, UserIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { fmtDate, hashHue, initials, KIND_TONE, rel, statusTone, type Tone } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { useNow } from "@/lib/use-api";
import type { Who as WhoRow } from "@/lib/api";

const TONE_BADGE: Record<Tone, string> = {
  neutral: "border-transparent bg-muted/55 text-foreground",
  ok: "border-transparent bg-ok/10 text-ok",
  warn: "border-transparent bg-warn/10 text-[color-mix(in_oklch,var(--warn),black_28%)] dark:text-warn",
  bad: "border-transparent bg-bad/10 text-bad",
  info: "border-transparent bg-info/10 text-info",
  accent: "border-transparent bg-accent-2/10 text-accent-2",
};

export function ToneBadge({ tone = "neutral", children, className, dot, title }: { tone?: Tone; children: React.ReactNode; className?: string; dot?: boolean; title?: string }) {
  return (
    <Badge variant="outline" title={title} className={cn("h-5 gap-1 rounded-full px-2 text-[11.5px] font-medium shadow-xs", TONE_BADGE[tone], className)}>
      {dot ? <span className="size-1.5 rounded-full bg-current" aria-hidden /> : null}
      {children}
    </Badge>
  );
}

export function StatusBadge({ status, className }: { status?: string | null; className?: string }) {
  const s = (status || "").trim();
  if (!s) return null;
  return (
    <ToneBadge tone={statusTone(s)} dot className={className}>
      {s.replace(/_/g, " ")}
    </ToneBadge>
  );
}

export function KindBadge({ kind }: { kind?: string | null }) {
  const k = kind || "post";
  return <ToneBadge tone={KIND_TONE[k] ?? "neutral"}>{k}</ToneBadge>;
}

export function VoteBadge({ vote, classification }: { vote?: string; classification?: string }) {
  if (!vote) return null;
  const up = vote === "up" || vote === "yes" || vote === "+1";
  const counted = !classification || classification === "counted";
  return (
    <ToneBadge tone={up ? "ok" : "bad"} className={cn(!counted && "opacity-60 line-through")} title={counted ? undefined : "not counted: " + classification}>
      {up ? "▲" : "▼"} {vote}
    </ToneBadge>
  );
}

export function Provenance({ ok, missing }: { ok?: number; missing?: string[] }) {
  if (ok === undefined) return null;
  const good = ok === 1;
  const text = good ? "provenance complete" : "provenance missing: " + ((missing || []).join(", ") || "runner, model or session");
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span aria-label={text} className={cn("inline-flex", good ? "text-ok" : "text-warn")}>
          {good ? <ShieldCheckIcon className="size-3.5" /> : <ShieldAlertIcon className="size-3.5" />}
        </span>
      </TooltipTrigger>
      <TooltipContent>{text}</TooltipContent>
    </Tooltip>
  );
}

export function Mono({ children, className }: { children: React.ReactNode; className?: string }) {
  return <span className={cn("ring-soft rounded-md bg-muted/45 px-1.5 py-0.5 font-mono text-[12px] anywhere", className)}>{children}</span>;
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

export function Avatar({ seed, size = 22, human, className }: { seed: string; size?: number; human?: boolean; className?: string }) {
  const hue = hashHue(seed || "?");
  return (
    <span
      aria-hidden
      className={cn("inline-flex shrink-0 items-center justify-center rounded-full font-semibold text-white shadow-[inset_0_1px_0_oklch(1_0_0/.28),0_1px_4px_oklch(0_0_0/.16)] ring-2 ring-background outline outline-1 outline-primary/15", human && "bg-primary text-primary-foreground", className)}
      style={{ width: size, height: size, fontSize: Math.max(9, size * 0.42), background: human ? undefined : `linear-gradient(135deg, oklch(0.72 0.13 ${hue}), oklch(0.52 0.16 ${hue + 24}))`, color: human ? undefined : "white" }}
    >
      {human ? <UserIcon style={{ width: size * 0.55, height: size * 0.55 }} /> : initials(seed || "?")}
    </span>
  );
}

// An agent identity: name (or session), with runner/model as the quiet
// second line. `compact` keeps it to one line for tables.
export function Who({ row, compact, className }: { row: WhoRow & { author?: string }; compact?: boolean; className?: string }) {
  const name = row.agent || row.name || row.author || row.session || "(unknown)";
  const human = name === "human" || name === "owner";
  const meta = [row.runner, row.model].filter(Boolean).join(" · ");
  return (
    <span className={cn("inline-flex min-w-0 items-center gap-1.5", className)} title={meta ? name + " · " + meta : name}>
      <Avatar seed={name} size={compact ? 18 : 22} human={human} />
      <span className="min-w-0">
        <span className={cn("block truncate font-medium", compact ? "text-[12.5px]" : "text-[13px]")}>{human ? "you" : name}</span>
        {!compact && meta ? <span className="block truncate text-[11px] text-muted-foreground">{meta}</span> : null}
      </span>
    </span>
  );
}

export function AuthorBadge({ author }: { author: string }) {
  const human = author === "human" || author === "owner";
  return (
    <ToneBadge tone={human ? "info" : "accent"} className="gap-1">
      {human ? <UserIcon className="size-3" /> : <BotIcon className="size-3" />}
      {human ? "you" : author}
    </ToneBadge>
  );
}

export function ProjectLink({ slug, brain, className }: { slug: string; brain?: boolean; className?: string }) {
  return (
    <Link href={hrefs.project(slug)} className={cn("inline-flex items-center gap-1 font-medium hover:underline", className)}>
      {slug}
      {brain ? (
        <Badge variant="secondary" className="h-4 px-1 text-[10px]">
          framework
        </Badge>
      ) : null}
    </Link>
  );
}

export function ThreadLink({ slug, k, className }: { slug: string; k: string; className?: string }) {
  return (
    <Link href={hrefs.thread(slug, k)} className={cn("font-mono text-[12px] hover:underline anywhere", className)}>
      {k}
    </Link>
  );
}

export function PostLink({ slug, id, label, className }: { slug: string; id: string; label?: string; className?: string }) {
  return (
    <Link href={hrefs.post(slug, id)} title={id} className={cn(label ? "hover:underline anywhere" : "font-mono text-[12px] hover:underline anywhere", className)}>
      {label || id}
    </Link>
  );
}

export function Check({ on }: { on: boolean }) {
  return on ? <CheckIcon className="inline size-3.5 text-ok" aria-label="yes" /> : <span className="text-muted-foreground">{"—"}</span>;
}
