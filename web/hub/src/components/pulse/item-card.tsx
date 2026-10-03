"use client";

import Link from "next/link";
import { createContext, useContext } from "react";
import { ChevronRightIcon } from "lucide-react";
import { cn } from "@/lib/utils";

// Card primitives for the lists the owner reads most (tasks, requests,
// threads, agents, records). One card = one thing with its badges, a title,
// a line or two of context and its actions in reach - no horizontal scroll
// to find a button in the last column of a table.

// Cap the column count while letting the available width, not the viewport,
// decide when a card needs its own row (including beside the dashboard rail).
const COLS = {
  1: "grid-cols-1",
  2: "grid-cols-[repeat(auto-fit,minmax(min(100%,max(18rem,calc((100%_-_1rem)/2))),1fr))]",
  3: "grid-cols-[repeat(auto-fit,minmax(min(100%,max(18rem,calc((100%_-_2rem)/3))),1fr))]",
  4: "grid-cols-[repeat(auto-fit,minmax(min(100%,max(18rem,calc((100%_-_3rem)/4))),1fr))]",
} as const;

export function CardGrid({ children, cols = 2, className }: { children: React.ReactNode; cols?: keyof typeof COLS; className?: string }) {
  return <div className={cn("grid min-w-0 gap-4", COLS[cols], className)}>{children}</div>;
}

// A click that starts on a control inside the card (a link, a button, a menu
// item rendered through a portal) belongs to that control, not to the card.
const CONTROL = "a,button,input,select,textarea,summary,[role=button],[role=menuitem],[role=option]";
function opensCard(e: React.MouseEvent<HTMLElement>) {
  const target = e.target as HTMLElement;
  if (!e.currentTarget.contains(target)) return false;
  const hit = target.closest(CONTROL);
  return !hit || hit === e.currentTarget;
}

// A card that can be opened. With `href` it is a link - use it only when the
// card holds no links or buttons of its own (nested anchors are invalid HTML).
// With `onOpen` the card's title (ItemCardTitle) becomes its real button for
// keyboard and assistive tech, and the surface adds a pointer shortcut that
// ignores clicks on inner controls, so those stay proper controls of their own.
const OpenContext = createContext<{ open: () => void; label?: string } | null>(null);

export function ItemCard({ children, href, onOpen, active, highlight, className, ariaLabel }: { children: React.ReactNode; href?: string; onOpen?: () => void; active?: boolean; highlight?: boolean; className?: string; ariaLabel?: string }) {
  const cls = cn(
    "group/item surface-raised relative flex min-w-0 flex-col gap-3 rounded-2xl border p-5 text-left",
    (href || onOpen) && "lift cursor-pointer hover:border-primary/30 motion-reduce:transition-none",
    href && !onOpen && "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
    onOpen && "has-[[data-open]:focus-visible]:ring-2 has-[[data-open]:focus-visible]:ring-ring",
    highlight && "glossy border-primary/35 bg-[linear-gradient(180deg,color-mix(in_oklch,var(--primary)_8%,var(--card)),color-mix(in_oklch,var(--brand-sky)_4%,var(--card)))] after:absolute after:inset-y-0 after:left-0 after:w-1 after:bg-[linear-gradient(180deg,var(--brand-mint),var(--brand-sky),var(--brand-violet))] after:content-['']",
    active && "ring-2 ring-ring",
    className,
  );
  if (href && !onOpen)
    return (
      <Link href={href} className={cls} aria-label={ariaLabel}>
        {children}
      </Link>
    );
  if (onOpen)
    return (
      <OpenContext.Provider value={{ open: onOpen, label: ariaLabel }}>
        <article
          className={cls}
          onClick={(e) => {
            if (opensCard(e)) onOpen();
          }}
        >
          {children}
        </article>
      </OpenContext.Provider>
    );
  return (
    <article className={cls} aria-label={ariaLabel}>
      {children}
    </article>
  );
}

// Top line: badges on the left, the quiet meta (who, when) on the right.
export function ItemCardHeader({ children, right, className }: { children: React.ReactNode; right?: React.ReactNode; className?: string }) {
  return (
    <div className={cn("flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1.5 text-xs leading-relaxed anywhere", className)}>
      {children}
      {right ? <span className="ml-auto flex min-w-0 max-w-full flex-wrap items-center gap-x-2 gap-y-1 text-muted-foreground">{right}</span> : null}
    </div>
  );
}

// Inside an openable card the title is the button that opens it.
export function ItemCardTitle({ children, className, chevron }: { children: React.ReactNode; className?: string; chevron?: boolean }) {
  const ctx = useContext(OpenContext);
  const inner = (
    <>
      <span className="min-w-0 flex-1">{children}</span>
      {chevron ? <ChevronRightIcon className="mt-0.5 size-4 shrink-0 opacity-40 transition-[transform,opacity] group-hover/item:opacity-70 group-focus-within/item:opacity-70 motion-safe:group-hover/item:translate-x-0.5 motion-reduce:transition-none" aria-hidden /> : null}
    </>
  );
  return (
    <h3 className={cn("min-w-0 text-[15px] font-semibold leading-snug tracking-tight anywhere", className)}>
      {ctx ? (
        <button
          type="button"
          data-open
          aria-label={ctx.label}
          className="flex w-full min-w-0 items-start gap-2 text-left focus-visible:outline-none"
          onClick={(e) => {
            e.stopPropagation();
            ctx.open();
          }}
        >
          {inner}
        </button>
      ) : (
        <span className="flex items-start gap-2">{inner}</span>
      )}
    </h3>
  );
}

export function ItemCardBody({ children, className, clamp = 2 }: { children: React.ReactNode; className?: string; clamp?: 0 | 1 | 2 | 3 | 5 }) {
  return <div className={cn("min-w-0 text-[13.5px] leading-relaxed text-muted-foreground anywhere", clamp === 1 && "line-clamp-1", clamp === 2 && "line-clamp-2", clamp === 3 && "line-clamp-3", clamp === 5 && "line-clamp-5", className)}>{children}</div>;
}

// Small facts in a row: "3 posts · quorum 2 · expires in 4 d".
export function ItemCardMeta({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5 text-xs leading-relaxed text-muted-foreground anywhere", className)}>{children}</div>;
}

// Bottom line with the card's buttons. Clicks here never open the card.
export function ItemCardActions({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <div
      className={cn("mt-auto flex min-w-0 flex-wrap items-center gap-2 pt-2", className)}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") e.stopPropagation();
      }}
    >
      {children}
    </div>
  );
}

// A compact fact tile for the inside of a card (used by project cards).
export function Fact({ n, label, tone, className }: { n: React.ReactNode; label: string; tone?: "ok" | "warn" | "bad" | "accent" | "info"; className?: string }) {
  return (
    <div className={cn("surface-glass min-w-0 rounded-xl border border-border/50 bg-muted/35 px-2 py-2.5 text-center", className)}>
      <div className={cn("tnum text-base font-semibold leading-tight tracking-tight anywhere", tone === "ok" && "text-ok", tone === "warn" && "text-warn", tone === "bad" && "text-bad", tone === "accent" && "text-accent-2", tone === "info" && "text-info")}>{n}</div>
      <div className="mt-1 text-[11px] leading-tight text-muted-foreground anywhere">{label}</div>
    </div>
  );
}

// Section heading for a card group: title, count, and an optional link.
export function SectionHeading({ children, count, action, className, icon: Icon }: { children: React.ReactNode; count?: number | string; action?: React.ReactNode; className?: string; icon?: React.ComponentType<{ className?: string }> }) {
  return (
    <div className={cn("mb-3 flex min-w-0 flex-wrap items-center justify-between gap-x-4 gap-y-2", className)}>
      <h2 className="flex min-w-0 items-center gap-2 text-lg font-semibold leading-snug tracking-tight anywhere">
        {Icon ? <Icon className="size-4 shrink-0 text-primary" aria-hidden /> : null}
        {children}
        {count !== undefined && count !== "" ? <span className="ring-soft tnum rounded-full bg-muted/45 px-2 py-0.5 text-[11px] font-medium text-muted-foreground">{count}</span> : null}
      </h2>
      {action ? <div className="min-w-0 max-w-full text-xs leading-relaxed text-muted-foreground anywhere">{action}</div> : null}
    </div>
  );
}

// Strip Markdown down to a one-paragraph excerpt for a card body.
export function excerpt(md: string, max = 220): string {
  const text = (md || "")
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/^#{1,6}\s+/gm, "")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[*_~>]+/g, "")
    .replace(/^\s*[-+]\s+/gm, "")
    .replace(/^\s*\d+\.\s+/gm, "")
    .replace(/\s+/g, " ")
    .trim();
  return text.length > max ? text.slice(0, max).replace(/\s+\S*$/, "") + "\u2026" : text;
}
