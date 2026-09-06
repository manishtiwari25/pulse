"use client";

import { Suspense, useCallback, useMemo, useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import { DataTable } from "@/components/pulse/data-table";
import { Count, EmptyState, ErrorNotice, PageTitle, SkeletonPage, SkeletonRows, Toolbar } from "@/components/pulse/panel";
import { Mono } from "@/components/pulse/who";
import { RECORD_COLUMNS, RecordSheet, kindLabel } from "@/components/records/records";
import { allProjects, RECORD_KINDS, type Org, type RecordsPayload } from "@/lib/api";
import { plural } from "@/lib/format";
import { useApi } from "@/lib/use-api";

export default function RecordsPage() {
  return (
    <Suspense fallback={<SkeletonPage rows={3} />}>
      <RecordsScreen />
    </Suspense>
  );
}

function RecordsScreen() {
  const sp = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const kind = sp.get("kind") || "";
  const status = sp.get("status") || "";
  const project = sp.get("project") || "";
  const id = sp.get("id");
  const [text, setText] = useState("");
  const [area, setArea] = useState("");

  const setParams = useCallback(
    (patch: Record<string, string | null | undefined>) => {
      const q = new URLSearchParams(sp.toString());
      for (const [k, v] of Object.entries(patch)) {
        if (v === null || v === undefined || v === "") q.delete(k);
        else q.set(k, v);
      }
      const s = q.toString();
      router.push(pathname + (s ? "?" + s : ""), { scroll: false });
    },
    [sp, router, pathname],
  );

  const q = useApi<RecordsPayload>("records" + (project ? "?project=" + encodeURIComponent(project) : ""), { refresh: 60000 });
  const org = useApi<Org>("org", { refresh: 60000 });
  const projects = useMemo(() => (org.data ? allProjects(org.data).map((p) => p.slug) : project ? [project] : []), [org.data, project]);
  const all = useMemo(() => q.data?.records ?? [], [q.data]);
  const statuses = useMemo(() => Array.from(new Set(all.map((r) => r.status).filter(Boolean))).sort(), [all]);
  const areas = useMemo(() => Array.from(new Set(all.flatMap((r) => r.areas || []))).sort(), [all]);
  const kinds = useMemo(() => {
    const present = new Set(all.map((r) => r.kind));
    return RECORD_KINDS.filter((k) => present.has(k)).concat(Array.from(present).filter((k) => !(RECORD_KINDS as string[]).includes(k)) as typeof RECORD_KINDS);
  }, [all]);
  const rows = useMemo(() => {
    const t = text.trim().toLowerCase();
    return all.filter((r) => (!kind || r.kind === kind) && (!status || r.status === status) && (!area || (r.areas || []).includes(area)) && (!t || r.id.toLowerCase().includes(t) || r.title.toLowerCase().includes(t) || (r.tags || []).some((x) => x.toLowerCase().includes(t))));
  }, [all, kind, status, area, text]);

  return (
    <>
      <PageTitle sub="Every ADR, plan, work unit, feature, memory, workflow, skill, guide and context record the store indexed from docs/. Click a row to read it; each has a revision history.">Records</PageTitle>
      <Toolbar>
        <Select value={project || "__all"} onValueChange={(v) => setParams({ project: v === "__all" ? null : v })}>
          <SelectTrigger aria-label="Project" className="h-8 w-44">
            <SelectValue placeholder="project" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all">all projects</SelectItem>
            {projects.map((p) => (
              <SelectItem key={p} value={p}>
                {p}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={kind || "__all"} onValueChange={(v) => setParams({ kind: v === "__all" ? null : v })}>
          <SelectTrigger aria-label="Kind" className="h-8 w-40">
            <SelectValue placeholder="kind" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all">all kinds</SelectItem>
            {kinds.map((k) => (
              <SelectItem key={k} value={k}>
                {kindLabel(k)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={status || "__all"} onValueChange={(v) => setParams({ status: v === "__all" ? null : v })}>
          <SelectTrigger aria-label="Status" className="h-8 w-40">
            <SelectValue placeholder="status" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all">all statuses</SelectItem>
            {statuses.map((s) => (
              <SelectItem key={s} value={s}>
                {s}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {areas.length ? (
          <Select value={area || "__all"} onValueChange={(v) => setArea(v === "__all" ? "" : v)}>
            <SelectTrigger aria-label="Area" className="h-8 w-40">
              <SelectValue placeholder="area" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all">all areas</SelectItem>
              {areas.map((a) => (
                <SelectItem key={a} value={a}>
                  {a}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : null}
        <Input aria-label="Search records" placeholder="id, title or tag…" value={text} onChange={(e) => setText(e.target.value)} className="h-8 w-56" />
        {kind || status || project || area || text ? (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setText("");
              setArea("");
              setParams({ kind: null, status: null, project: null });
            }}
          >
            clear
          </Button>
        ) : null}
        <Count>{q.data ? plural(rows.length, "record") : ""}</Count>
      </Toolbar>
      {q.error && !q.data ? (
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      ) : !q.data ? (
        <SkeletonRows n={10} />
      ) : rows.length ? (
        <div className="rounded-lg border bg-card">
          <DataTable columns={RECORD_COLUMNS(!project)} rows={rows} rowKey={(r) => r.id} onRow={(r) => setParams({ id: r.id })} initialSort="updated" caption="Records" />
        </div>
      ) : (
        <EmptyState>{all.length ? "Nothing matches these filters." : <>No record indexed yet. <Mono>pulse init</Mono> indexes docs/ (ADRs, plans, work units, memory…) into the store; agents add records as they work.</>}</EmptyState>
      )}
      <RecordSheet id={id} project={project || undefined} onClose={() => setParams({ id: null })} />
    </>
  );
}
