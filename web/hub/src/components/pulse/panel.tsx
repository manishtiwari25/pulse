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
// Approve, filters…) stay reachable. Actions can wrap on narrow screens;
// the supporting description scrolls away with the content.
// `bar` renders a second sticky row (tabs, filters) inside the same block.
export function PageTitle({ children, sub, crumb, actions, bar, className, icon: Icon, badge }: { children: React.ReactNode; sub?: React.ReactNode; crumb?: React.ReactNode; actions?: React.ReactNode; bar?: React.ReactNode; className?: string; icon?: React.ComponentType<{ className?: string }>; badge?: React.ReactNode }) {
  return (
    <>
      <div className={cn("sticky top-[calc(4rem+1px+env(safe-area-inset-top))] z-30 min-w-0 border-b border-border/70 surface-glass", className)}>
        <div className="flex min-h-16 flex-wrap items-center justify-between gap-x-4 gap-y-2 py-3 sm:py-4">
          <div className="flex min-w-0 max-w-full flex-1 basis-48 items-center gap-2.5">
            {crumb ? <div className="hidden shrink-0 text-xs text-muted-foreground sm:block">{crumb}</div> : null}
            {Icon ? (
              <span className="flex size-9 shrink-0 items-center justify-center rounded-xl border border-primary/10 bg-primary/8 text-primary" aria-hidden>
                <Icon className="size-[18px]" />
              </span>
            ) : null}
            <h1 className="min-w-0 truncate text-xl font-semibold leading-tight tracking-tight sm:text-2xl" title={typeof children === "string" ? children : undefined}>{children}</h1>
            {badge ? <div className="flex min-w-0 max-w-full shrink-0 flex-wrap items-center">{badge}</div> : null}
          </div>
          {actions ? <div className="flex min-w-0 max-w-full flex-wrap items-center gap-2 sm:ml-auto">{actions}</div> : null}
        </div>
        {bar ? <div className="min-w-0 overflow-x-auto pt-1 pb-3 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">{bar}</div> : null}
      </div>
      {sub ? <div className="mt-3 mb-6 min-w-0 max-w-4xl text-[13px] leading-relaxed text-muted-foreground anywhere">{sub}</div> : <div className="mb-6" aria-hidden />}
    </>
  );
}

export function Lead({ children, className }: { children: React.ReactNode; className?: string }) {
  return <p className={cn("mb-4 max-w-4xl text-sm leading-relaxed text-muted-foreground anywhere", className)}>{children}</p>;
}

export function Panel({ title, actions, children, className, contentClassName, dense, icon: Icon, footer }: { title?: React.ReactNode; actions?: React.ReactNode; children: React.ReactNode; className?: string; contentClassName?: string; dense?: boolean; icon?: React.ComponentType<{ className?: string }>; footer?: React.ReactNode }) {
  return (
    <Card className={cn("min-w-0 gap-0 rounded-2xl py-0", className)}>
      {title !== undefined ? (
        <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-x-4 gap-y-3 border-b border-border/70 bg-muted/20 px-4 py-3.5 [.border-b]:pb-3.5 sm:px-5">
          <div className="flex min-w-0 flex-1 basis-36 items-center gap-2.5">
            {Icon ? (
              <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground" aria-hidden>
                <Icon className="size-4" />
              </span>
            ) : null}
            <CardTitle className="min-w-0 text-sm font-semibold leading-snug tracking-tight anywhere">{title}</CardTitle>
          </div>
          {actions ? <div className="ml-auto flex min-w-0 max-w-full flex-wrap items-center justify-end gap-2 text-xs leading-relaxed text-muted-foreground anywhere">{actions}</div> : null}
        </CardHeader>
      ) : null}
      <CardContent className={cn("min-w-0", dense ? "p-0" : "px-4 py-4 sm:px-5", contentClassName)}>{children}</CardContent>
      {footer ? <div className="border-t border-border/70 bg-muted/20 px-4 py-3 text-xs leading-relaxed text-muted-foreground anywhere sm:px-5">{footer}</div> : null}
    </Card>
  );
}

export function EmptyState({ children, hint, icon, className, title, action }: { children: React.ReactNode; hint?: string; icon?: React.ReactNode; className?: string; title?: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className={cn("flex min-w-0 flex-col items-center gap-2.5 rounded-xl border border-dashed bg-card/50 px-5 py-9 text-center", className)}>
      <span className="mb-1 flex size-11 items-center justify-center rounded-2xl border bg-card text-muted-foreground shadow-card" aria-hidden>
        {icon ?? <InfoIcon className="size-[18px]" />}
      </span>
      {title ? <div className="text-sm font-medium text-foreground anywhere">{title}</div> : null}
      <div className="max-w-md text-sm leading-relaxed text-muted-foreground anywhere">{children}</div>
      {hint ? <code className="mt-1 max-w-full rounded-md bg-muted px-2 py-1 font-mono text-xs text-foreground anywhere">{hint}</code> : null}
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

// The value remains in the accessible text even when a long identifier is
// visually clamped; its full text is also available on hover.
export function Kpi({ n, label, sub, href, tone = "neutral", spark, icon: Icon }: { n: number | string; label: string; sub?: React.ReactNode; href?: string; tone?: Tone; spark?: React.ReactNode; icon?: React.ComponentType<{ className?: string }> }) {
  const value = typeof n === "number" ? num(n) : n;
  const body = (
    <div className="flex h-full min-h-20 min-w-0 flex-col justify-between gap-3">
      <div className="flex min-w-0 items-start justify-between gap-2">
        <span className="min-w-0 text-xs font-medium leading-relaxed text-muted-foreground anywhere">{label}</span>
        <span className="flex shrink-0 items-center gap-1 text-muted-foreground">
          {spark}
          {href ? <ArrowUpRightIcon className="size-3.5 opacity-45 transition-opacity group-hover/kpi:opacity-100 group-focus-visible/kpi:opacity-100 motion-reduce:transition-none" aria-hidden /> : null}
          {Icon ? (
            <span className={cn("-mt-1 flex size-7 items-center justify-center rounded-lg", KPI_ICON_TONE[tone])} aria-hidden>
              <Icon className="size-4" />
            </span>
          ) : null}
        </span>
      </div>
      <div className="min-w-0">
        <div title={value} className={cn("tnum line-clamp-2 text-2xl font-semibold leading-tight tracking-tight anywhere", KPI_TONE[tone])}>{value}</div>
        {sub ? <div title={typeof sub === "string" ? sub : undefined} className="mt-1.5 text-xs leading-relaxed text-muted-foreground anywhere">{sub}</div> : null}
      </div>
    </div>
  );
  const cls = "group/kpi block w-44 min-w-0 shrink-0 snap-start rounded-xl border bg-card p-4 shadow-card sm:w-auto";
  return href ? (
    <Link href={href} className={cn(cls, "transition-[border-color,box-shadow,background-color] duration-200 hover:border-primary/30 hover:bg-accent/30 hover:shadow-pop focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background motion-reduce:transition-none")}>
      {body}
    </Link>
  ) : (
    <div className={cls}>{body}</div>
  );
}

// Keep a compact strip on phones; four columns leave room for text beside
// the desktop navigation rail instead of squeezing eight tiles into a row.
export function KpiGrid({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("-mx-1 mb-6 flex min-w-0 snap-x gap-3 overflow-x-auto p-1 [scrollbar-width:none] sm:mx-0 sm:grid sm:grid-cols-2 sm:overflow-visible sm:p-0 md:grid-cols-4 [&::-webkit-scrollbar]:hidden", className)}>{children}</div>;
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
