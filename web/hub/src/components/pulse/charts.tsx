"use client";

import { useMemo, useState } from "react";
import { Area, AreaChart, Bar, BarChart, CartesianGrid, Legend, ResponsiveContainer, Tooltip as ChartTooltip, XAxis, YAxis } from "recharts";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Panel } from "./panel";
import { KINDS, num, parseDate } from "@/lib/format";
import type { Bucket, Timeline } from "@/lib/api";

export type Range = "24h" | "7d" | "30d";
export type By = "total" | "kind" | "project";

const COLORS = ["var(--chart-1)", "var(--chart-2)", "var(--chart-3)", "var(--chart-4)", "var(--chart-5)", "oklch(0.7 0.1 200)", "oklch(0.65 0.12 30)", "oklch(0.55 0.1 120)"];

export function lastDays(tl: Timeline, n: number): number[] {
  return (tl.days || []).slice(-n).map((b) => b.n);
}

function buckets(tl: Timeline, range: Range): Bucket[] {
  if (range === "24h") return tl.hours || [];
  return (tl.days || []).slice(range === "7d" ? -7 : -30);
}

function label(t: string, range: Range): string {
  const d = parseDate(t);
  if (!d) return t;
  if (range === "24h") return d.toLocaleTimeString(undefined, { hour: "2-digit", hour12: false }) + "h";
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

// Posts over time. `by` splits the total by post kind or (on the hub) by
// project series; the range switches between the 24 hourly and the daily
// buckets the API returns.
export function ActivityChart({ tl, bys, title = "Activity", height = 200 }: { tl: Timeline; bys: By[]; title?: string; height?: number }) {
  const [range, setRange] = useState<Range>("7d");
  const [by, setBy] = useState<By>(bys[0] ?? "total");
  const { data, keys } = useMemo(() => {
    const rows = buckets(tl, range);
    if (by === "project" && tl.series?.length) {
      const keys = tl.series.map((s) => s.slug);
      const byT = new Map<string, Record<string, number | string>>();
      for (const b of rows) byT.set(b.t, { t: label(b.t, range) });
      for (const s of tl.series) for (const b of buckets(s, range)) {
        const row = byT.get(b.t);
        if (row) row[s.slug] = b.n;
      }
      return { data: [...byT.values()], keys };
    }
    if (by === "kind") {
      const present = new Set<string>();
      for (const b of rows) for (const k of Object.keys(b.kinds || {})) present.add(k);
      const keys = [...present].sort((a, b) => KINDS.indexOf(a) - KINDS.indexOf(b));
      return { data: rows.map((b) => ({ t: label(b.t, range), ...Object.fromEntries(keys.map((k) => [k, b.kinds?.[k] ?? 0])) })), keys };
    }
    return { data: rows.map((b) => ({ t: label(b.t, range), posts: b.n })), keys: ["posts"] };
  }, [tl, range, by]);
  const total = buckets(tl, range).reduce((a, b) => a + b.n, 0);
  return (
    <Panel
      title={title}
      actions={
        <div className="flex items-center gap-2">
          <span className="tnum hidden sm:inline">{num(total) + " posts"}</span>
          {bys.length > 1 ? (
            <Tabs value={by} onValueChange={(v) => setBy(v as By)}>
              <TabsList className="h-7">
                {bys.map((b) => (
                  <TabsTrigger key={b} value={b} className="h-6 px-2 text-xs">
                    {b}
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>
          ) : null}
          <Tabs value={range} onValueChange={(v) => setRange(v as Range)}>
            <TabsList className="h-7">
              {(["24h", "7d", "30d"] as Range[]).map((r) => (
                <TabsTrigger key={r} value={r} className="h-6 px-2 text-xs">
                  {r}
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
        </div>
      }
    >
      <div style={{ height }} className="text-xs">
        <ResponsiveContainer width="100%" height="100%">
          {by === "total" ? (
            <AreaChart data={data} margin={{ top: 8, right: 8, left: -18, bottom: 0 }}>
              <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" vertical={false} />
              <XAxis dataKey="t" tick={{ fill: "var(--muted-foreground)", fontSize: 11 }} tickLine={false} axisLine={false} minTickGap={24} />
              <YAxis tick={{ fill: "var(--muted-foreground)", fontSize: 11 }} tickLine={false} axisLine={false} allowDecimals={false} />
              <ChartTooltip contentStyle={{ background: "var(--popover)", border: "1px solid var(--border)", borderRadius: 8, fontSize: 12 }} labelStyle={{ color: "var(--muted-foreground)" }} cursor={{ stroke: "var(--border)" }} />
              <Area type="monotone" dataKey="posts" stroke="var(--chart-1)" fill="var(--chart-1)" fillOpacity={0.18} strokeWidth={2} isAnimationActive={false} />
            </AreaChart>
          ) : (
            <BarChart data={data} margin={{ top: 8, right: 8, left: -18, bottom: 0 }}>
              <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" vertical={false} />
              <XAxis dataKey="t" tick={{ fill: "var(--muted-foreground)", fontSize: 11 }} tickLine={false} axisLine={false} minTickGap={24} />
              <YAxis tick={{ fill: "var(--muted-foreground)", fontSize: 11 }} tickLine={false} axisLine={false} allowDecimals={false} />
              <ChartTooltip contentStyle={{ background: "var(--popover)", border: "1px solid var(--border)", borderRadius: 8, fontSize: 12 }} labelStyle={{ color: "var(--muted-foreground)" }} cursor={{ fill: "var(--muted)", opacity: 0.5 }} />
              <Legend iconType="circle" iconSize={8} wrapperStyle={{ fontSize: 11 }} />
              {keys.map((k, i) => (
                <Bar key={k} dataKey={k} stackId="a" fill={COLORS[i % COLORS.length]} isAnimationActive={false} radius={i === keys.length - 1 ? [3, 3, 0, 0] : 0} />
              ))}
            </BarChart>
          )}
        </ResponsiveContainer>
      </div>
    </Panel>
  );
}

export function Sparkline({ values, w = 64, h = 18, className }: { values: number[]; w?: number; h?: number; className?: string }) {
  if (!values.length) return null;
  const max = Math.max(1, ...values);
  const step = values.length > 1 ? w / (values.length - 1) : w;
  const pts = values.map((v, i) => `${(i * step).toFixed(1)},${(h - (v / max) * (h - 2) - 1).toFixed(1)}`).join(" ");
  return (
    <svg width={w} height={h} viewBox={`0 0 ${w} ${h}`} className={className} aria-hidden>
      <polyline points={pts} fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" strokeLinecap="round" className="text-info" />
    </svg>
  );
}

export function StackedBar({ parts }: { parts: { label: string; n: number; color: string }[] }) {
  const total = parts.reduce((a, p) => a + p.n, 0) || 1;
  return (
    <div>
      <div className="flex h-2.5 w-full overflow-hidden rounded-full bg-muted" role="img" aria-label={parts.map((p) => p.label + " " + p.n).join(", ")}>
        {parts.map((p) => (
          <div key={p.label} style={{ width: (100 * p.n) / total + "%", background: p.color }} title={p.label + " " + num(p.n)} />
        ))}
      </div>
      <div className="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-[12px]">
        {parts.map((p) => (
          <span key={p.label} className="inline-flex items-center gap-1.5">
            <span className="size-2 rounded-full" style={{ background: p.color }} />
            {p.label} <span className="tnum text-muted-foreground">{num(p.n)}</span>
          </span>
        ))}
      </div>
    </div>
  );
}

export const kindColor = (k: string) => COLORS[Math.max(0, KINDS.indexOf(k)) % COLORS.length]!;

export function Bar2({ value, max, className }: { value: number; max: number; className?: string }) {
  const pct = Math.min(100, Math.round((100 * value) / Math.max(1, max)));
  return (
    <span className={"inline-block h-1.5 w-16 overflow-hidden rounded-full bg-muted align-middle " + (className ?? "")} role="progressbar" aria-valuenow={value} aria-valuemax={max}>
      <span className="block h-full bg-ok" style={{ width: pct + "%" }} />
    </span>
  );
}
