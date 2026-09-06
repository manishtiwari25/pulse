"use client";

import Link from "next/link";
import { AlertTriangleIcon, InfoIcon, RefreshCwIcon } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { num } from "@/lib/format";
import type { Tone } from "@/lib/format";

export function PageTitle({ children, sub, crumb, actions, className }: { children: React.ReactNode; sub?: React.ReactNode; crumb?: React.ReactNode; actions?: React.ReactNode; className?: string }) {
  return (
    <div className={cn("mb-4 flex flex-wrap items-end justify-between gap-2", className)}>
      <div className="min-w-0">
        {crumb ? <div className="mb-0.5 text-xs text-muted-foreground">{crumb}</div> : null}
        <h1 className="text-xl font-semibold tracking-tight anywhere">{children}</h1>
        {sub ? <p className="mt-0.5 text-sm text-muted-foreground anywhere">{sub}</p> : null}
      </div>
      {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
    </div>
  );
}

export function Lead({ children, className }: { children: React.ReactNode; className?: string }) {
  return <p className={cn("mb-3 max-w-4xl text-sm text-muted-foreground anywhere", className)}>{children}</p>;
}

export function Panel({ title, actions, children, className, contentClassName, dense }: { title?: React.ReactNode; actions?: React.ReactNode; children: React.ReactNode; className?: string; contentClassName?: string; dense?: boolean }) {
  return (
    <Card className={cn("gap-0 py-0", className)}>
      {title !== undefined ? (
        <CardHeader className={cn("flex flex-row items-center justify-between gap-2 border-b px-4 py-2.5 [.border-b]:pb-2.5")}>
          <CardTitle className="text-[13px] font-semibold tracking-tight">{title}</CardTitle>
          {actions ? <div className="flex items-center gap-2 text-xs text-muted-foreground">{actions}</div> : null}
        </CardHeader>
      ) : null}
      <CardContent className={cn(dense ? "p-0" : "px-4 py-3", contentClassName)}>{children}</CardContent>
    </Card>
  );
}

export function EmptyState({ children, hint, icon, className }: { children: React.ReactNode; hint?: string; icon?: React.ReactNode; className?: string }) {
  return (
    <div className={cn("flex flex-col items-center gap-1.5 rounded-md border border-dashed px-4 py-6 text-center text-sm text-muted-foreground", className)}>
      {icon ?? <InfoIcon className="size-4 opacity-60" />}
      <div className="max-w-md anywhere">{children}</div>
      {hint ? <code className="mt-1 rounded bg-muted px-1.5 py-0.5 font-mono text-xs text-foreground">{hint}</code> : null}
    </div>
  );
}

export function ErrorNotice({ error, onRetry, children }: { error?: string; onRetry?: () => void; children?: React.ReactNode }) {
  return (
    <div role="alert" className="flex flex-wrap items-center gap-2 rounded-md border border-bad/40 bg-bad/5 px-3 py-2 text-sm">
      <AlertTriangleIcon className="size-4 text-bad" />
      <span className="anywhere">{children ?? "Could not load this view: " + error}</span>
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
    neutral: "border-border bg-muted/40",
    ok: "border-ok/40 bg-ok/5",
    warn: "border-warn/50 bg-warn/10",
    bad: "border-bad/40 bg-bad/5",
    info: "border-info/40 bg-info/5",
    accent: "border-accent-2/40 bg-accent-2/5",
  };
  return <div className={cn("rounded-md border px-3 py-2 text-sm anywhere", toneClass[tone], className)}>{children}</div>;
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
        <Skeleton key={i} className="h-8" />
      ))}
    </div>
  );
}

const KPI_TONE: Record<Tone, string> = {
  neutral: "",
  ok: "text-ok",
  warn: "text-warn",
  bad: "text-bad",
  info: "text-info",
  accent: "text-accent-2",
};

export function Kpi({ n, label, sub, href, tone = "neutral", spark }: { n: number | string; label: string; sub?: React.ReactNode; href?: string; tone?: Tone; spark?: React.ReactNode }) {
  const body = (
    <>
      <div className="flex items-start justify-between gap-2">
        <div className={cn("tnum text-2xl font-semibold leading-none tracking-tight", KPI_TONE[tone])}>{typeof n === "number" ? num(n) : n}</div>
        {spark}
      </div>
      <div className="mt-1.5 text-xs font-medium">{label}</div>
      {sub ? <div className="mt-0.5 truncate text-[11.5px] text-muted-foreground">{sub}</div> : <div className="mt-0.5 h-[15px]" />}
    </>
  );
  const cls = "block rounded-lg border bg-card px-3.5 py-3 shadow-xs transition-colors";
  return href ? (
    <Link href={href} className={cn(cls, "hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring")}>
      {body}
    </Link>
  ) : (
    <div className={cls}>{body}</div>
  );
}

export function KpiGrid({ children }: { children: React.ReactNode }) {
  return <div className="mb-4 grid grid-cols-2 gap-3 md:grid-cols-4 xl:grid-cols-8">{children}</div>;
}

export function Toolbar({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("mb-3 flex flex-wrap items-center gap-2", className)}>{children}</div>;
}

export function Count({ children }: { children: React.ReactNode }) {
  return <span className="ml-auto text-xs text-muted-foreground tnum">{children}</span>;
}

export function Kv({ rows, className }: { rows: [React.ReactNode, React.ReactNode][]; className?: string }) {
  return (
    <dl className={cn("grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[12.5px]", className)}>
      {rows.map(([k, v], i) => (
        <div key={i} className="contents">
          <dt className="text-muted-foreground">{k}</dt>
          <dd className="m-0 min-w-0 anywhere">{v}</dd>
        </div>
      ))}
    </dl>
  );
}
