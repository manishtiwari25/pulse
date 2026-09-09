"use client";

import Link from "next/link";
import { AlertTriangleIcon, ArrowUpRightIcon, InfoIcon, RefreshCwIcon } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { num } from "@/lib/format";
import type { Tone } from "@/lib/format";

// The page header sticks under the app bar so the page's actions (New task,
// Approve, filters…) are reachable without scrolling back up. The title row
// is one compact line; `sub` and `below` scroll away with the content.
// `bar` renders a second sticky row (tabs, filters) inside the same block.
export function PageTitle({ children, sub, crumb, actions, bar, className, icon: Icon, badge }: { children: React.ReactNode; sub?: React.ReactNode; crumb?: React.ReactNode; actions?: React.ReactNode; bar?: React.ReactNode; className?: string; icon?: React.ComponentType<{ className?: string }>; badge?: React.ReactNode }) {
  return (
    <>
      <div className={cn("sticky top-12 z-30 -mx-3 border-b bg-background/90 px-3 backdrop-blur-md supports-[backdrop-filter]:bg-background/75 sm:-mx-4 sm:px-4", className)}>
        <div className="flex min-h-11 items-center justify-between gap-3 py-1.5">
          <div className="flex min-w-0 items-center gap-2">
            {crumb ? <div className="hidden shrink-0 text-xs text-muted-foreground sm:block">{crumb}</div> : null}
            {Icon ? (
              <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary" aria-hidden>
                <Icon className="size-4" />
              </span>
            ) : null}
            <h1 className="min-w-0 truncate text-lg font-semibold tracking-tight sm:text-xl">{children}</h1>
            {badge ? <div className="shrink-0">{badge}</div> : null}
          </div>
          {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
        </div>
        {bar ? <div className="-mx-3 -mt-1 overflow-x-auto px-3 pt-1 pb-1.5 [scrollbar-width:none] sm:-mx-4 sm:px-4 [&::-webkit-scrollbar]:hidden">{bar}</div> : null}
      </div>
      {sub ? <p className="mt-3 mb-4 max-w-4xl text-[13px] leading-snug text-muted-foreground anywhere">{sub}</p> : <div className="mb-4" aria-hidden />}
    </>
  );
}

export function Lead({ children, className }: { children: React.ReactNode; className?: string }) {
  return <p className={cn("mb-3 max-w-4xl text-sm text-muted-foreground anywhere", className)}>{children}</p>;
}

export function Panel({ title, actions, children, className, contentClassName, dense, icon: Icon, footer }: { title?: React.ReactNode; actions?: React.ReactNode; children: React.ReactNode; className?: string; contentClassName?: string; dense?: boolean; icon?: React.ComponentType<{ className?: string }>; footer?: React.ReactNode }) {
  return (
    <Card className={cn("gap-0 py-0", className)}>
      {title !== undefined ? (
        <CardHeader className={cn("flex flex-row items-center justify-between gap-2 border-b px-4 py-3 [.border-b]:pb-3")}>
          <div className="flex min-w-0 items-center gap-2">
            {Icon ? (
              <span className="flex size-7 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary" aria-hidden>
                <Icon className="size-4" />
              </span>
            ) : null}
            <CardTitle className="truncate text-sm font-semibold tracking-tight">{title}</CardTitle>
          </div>
          {actions ? <div className="flex items-center gap-2 text-xs text-muted-foreground">{actions}</div> : null}
        </CardHeader>
      ) : null}
      <CardContent className={cn(dense ? "p-0" : "px-4 py-3", contentClassName)}>{children}</CardContent>
      {footer ? <div className="border-t bg-muted/30 px-4 py-2.5 text-xs text-muted-foreground">{footer}</div> : null}
    </Card>
  );
}

export function EmptyState({ children, hint, icon, className, title, action }: { children: React.ReactNode; hint?: string; icon?: React.ReactNode; className?: string; title?: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className={cn("flex flex-col items-center gap-2 rounded-lg border border-dashed px-6 py-8 text-center", className)}>
      <span className="flex size-10 items-center justify-center rounded-full bg-muted text-muted-foreground" aria-hidden>
        {icon ?? <InfoIcon className="size-4" />}
      </span>
      {title ? <div className="text-sm font-medium text-foreground anywhere">{title}</div> : null}
      <div className="max-w-md text-sm text-muted-foreground anywhere">{children}</div>
      {hint ? <code className="mt-1 rounded-md bg-muted px-1.5 py-0.5 font-mono text-xs text-foreground">{hint}</code> : null}
      {action ? <div className="mt-1 flex flex-wrap items-center justify-center gap-2">{action}</div> : null}
    </div>
  );
}

export function ErrorNotice({ error, onRetry, children }: { error?: string; onRetry?: () => void; children?: React.ReactNode }) {
  return (
    <div role="alert" className="flex flex-wrap items-start gap-2 rounded-lg border border-bad/40 bg-bad/10 px-3.5 py-2.5 text-sm text-foreground">
      <AlertTriangleIcon className="mt-0.5 size-4 shrink-0 text-bad" />
      <span className="min-w-0 flex-1 anywhere">{children ?? "Could not load this view: " + error}</span>
      {onRetry ? (
        <Button size="xs" variant="outline" className="ml-auto" onClick={onRetry}>
          <RefreshCwIcon /> Retry
        </Button>
      ) : null}
    </div>
  );
}

export function Notice({ tone = "info", children, className }: { tone?: Tone; children: React.ReactNode; className?: string }) {
  const toneClass: Record<Tone, string> = {
    neutral: "border-border bg-muted/40 text-foreground",
    ok: "border-ok/40 bg-ok/10 text-foreground",
    warn: "border-warn/50 bg-warn/10 text-foreground",
    bad: "border-bad/40 bg-bad/10 text-foreground",
    info: "border-info/40 bg-info/10 text-foreground",
    accent: "border-accent-2/40 bg-accent-2/10 text-foreground",
  };
  const iconClass: Record<Tone, string> = {
    neutral: "text-muted-foreground",
    ok: "text-ok",
    warn: "text-warn",
    bad: "text-bad",
    info: "text-info",
    accent: "text-accent-2",
  };
  return (
    <div className={cn("flex items-start gap-2 rounded-lg border px-3.5 py-2.5 text-sm anywhere", toneClass[tone], className)}>
      <InfoIcon className={cn("mt-0.5 size-4 shrink-0", iconClass[tone])} />
      <div className="min-w-0">{children}</div>
    </div>
  );
}

export function Stack({ children, className, gap = 3 }: { children: React.ReactNode; className?: string; gap?: 1 | 2 | 3 | 4 }) {
  return <div className={cn("grid", gap === 1 ? "gap-1" : gap === 2 ? "gap-2" : gap === 4 ? "gap-4" : "gap-3", className)}>{children}</div>;
}

export function SkeletonPage({ rows = 3 }: { rows?: number }) {
  return (
    <div className="grid gap-3" aria-busy="true" aria-live="polite">
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <Skeleton key={i} className="h-[74px] rounded-lg" />
        ))}
      </div>
      {Array.from({ length: rows }).map((_, i) => (
        <Skeleton key={i} className="h-40 rounded-lg" />
      ))}
    </div>
  );
}

export function SkeletonRows({ n = 5, className }: { n?: number; className?: string }) {
  return (
    <div className={cn("grid gap-2", className)} aria-busy="true">
      {Array.from({ length: n }).map((_, i) => (
        <Skeleton key={i} className="h-9 rounded-lg" />
      ))}
    </div>
  );
}

const KPI_TONE: Record<Tone, string> = {
  neutral: "text-foreground",
  ok: "text-ok",
  warn: "text-warn",
  bad: "text-bad",
  info: "text-info",
  accent: "text-accent-2",
};

const KPI_ICON_TONE: Record<Tone, string> = {
  neutral: "bg-muted text-muted-foreground",
  ok: "bg-ok/10 text-ok",
  warn: "bg-warn/10 text-warn",
  bad: "bg-bad/10 text-bad",
  info: "bg-info/10 text-info",
  accent: "bg-accent-2/10 text-accent-2",
};

// One compact stat: number and label on a line, an optional quiet second
// line. Half the height of a card so eight of them fit in one row and the
// content that matters (proposals, tasks) stays above the fold.
export function Kpi({ n, label, sub, href, tone = "neutral", spark, icon: Icon }: { n: number | string; label: string; sub?: React.ReactNode; href?: string; tone?: Tone; spark?: React.ReactNode; icon?: React.ComponentType<{ className?: string }> }) {
  // Stacked stat card: label + icon on top, big number, then the sub line at full width.
  const body = (
    <div className="flex h-full min-h-[58px] flex-col justify-between gap-1.5">
      <div className="flex items-start justify-between gap-2">
        <span className="min-w-0 text-xs font-medium leading-tight text-muted-foreground anywhere">{label}</span>
        <span className="flex shrink-0 items-center gap-1 text-muted-foreground">
          {spark}
          {href ? <ArrowUpRightIcon className="size-3 opacity-0 transition-opacity group-hover/kpi:opacity-60" aria-hidden /> : null}
          {Icon ? (
            <span className={cn("-mr-0.5 -mt-0.5 flex size-6 items-center justify-center rounded-md", KPI_ICON_TONE[tone])} aria-hidden>
              <Icon className="size-3.5" />
            </span>
          ) : null}
        </span>
      </div>
      <div className="min-w-0">
        <div className={cn("tnum text-xl font-semibold leading-none tracking-tight", KPI_TONE[tone])}>{typeof n === "number" ? num(n) : n}</div>
        {sub ? <div className="mt-1 truncate text-[11.5px] leading-tight text-muted-foreground">{sub}</div> : null}
      </div>
    </div>
  );
  const cls = "group/kpi block min-w-[9.5rem] shrink-0 snap-start rounded-lg border bg-card px-3 py-2.5 shadow-card transition-all sm:min-w-0";
  return href ? (
    <Link href={href} className={cn(cls, "hover:border-foreground/15 hover:shadow-pop focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring")}>
      {body}
    </Link>
  ) : (
    <div className={cls}>{body}</div>
  );
}

// A single strip of stats: one scrollable row on phones, a grid from sm up.
export function KpiGrid({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("-mx-3 mb-4 flex snap-x gap-2 overflow-x-auto px-3 pb-0.5 [scrollbar-width:none] sm:mx-0 sm:grid sm:grid-cols-4 sm:overflow-visible sm:px-0 xl:grid-cols-8 [&::-webkit-scrollbar]:hidden", className)}>{children}</div>;
}

export function Toolbar({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("mb-3 flex flex-wrap items-center gap-2", className)}>{children}</div>;
}

export function Count({ children }: { children: React.ReactNode }) {
  return <span className="ml-auto whitespace-nowrap text-xs text-muted-foreground tnum">{children}</span>;
}

export function Kv({ rows, className }: { rows: [React.ReactNode, React.ReactNode][]; className?: string }) {
  return (
    <dl className={cn("grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[13px]", className)}>
      {rows.map(([k, v], i) => (
        <div key={i} className="contents">
          <dt className="text-muted-foreground">{k}</dt>
          <dd className="m-0 min-w-0 text-foreground anywhere">{v}</dd>
        </div>
      ))}
    </dl>
  );
}
