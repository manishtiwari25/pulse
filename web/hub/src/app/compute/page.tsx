"use client";

import Link from "next/link";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { ExecutorRow, MachinePanel } from "@/components/pulse/cards";
import { CapsPanel, ComputeTable } from "@/components/compute/compute";
import { EmptyState, ErrorNotice, Kpi, KpiGrid, PageTitle, Panel, SkeletonPage } from "@/components/pulse/panel";
import { Mono } from "@/components/pulse/who";
import { hrefs } from "@/lib/nav";
import { num, plural } from "@/lib/format";
import type { Compute, Runner } from "@/lib/api";
import { useApi } from "@/lib/use-api";

export default function ComputePage() {
  const q = useApi<Compute>("compute", { refresh: 10000 });
  const runner = useApi<Runner>("runner");
  const [show, setShow] = useState<"pending" | "all">("pending");
  if (q.error && !q.data)
    return (
      <>
        <PageTitle>Compute</PageTitle>
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      </>
    );
  if (!q.data) return <SkeletonPage rows={3} />;
  const c = q.data;
  const pending = c.requests.filter((r) => r.status === "pending");
  const live = c.requests.filter((r) => r.status === "approved" || r.status === "provisioning" || r.status === "ready");
  const rows = show === "pending" ? pending : c.requests;
  const executors = c.executors?.length ? c.executors : (runner.data?.executors ?? []);
  return (
    <>
      <PageTitle sub="Agents ask for machines; you approve or deny within the caps. Approved requests become executors the daemon runs jobs on.">Compute</PageTitle>
      <KpiGrid>
        <Kpi n={pending.length} label="to decide" tone={pending.length ? "warn" : "neutral"} sub="pending requests" />
        <Kpi n={live.length} label="live" sub="approved / provisioning / ready" tone={live.length ? "ok" : "neutral"} />
        <Kpi n={num(c.caps.vms_now) + " / " + num(c.caps.max_vms)} label="machines" sub="now / cap" />
        <Kpi n={num(c.caps.hours_today) + " / " + num(c.caps.max_hours_per_day)} label="hours today" sub="used / cap" tone={c.caps.hours_today >= c.caps.max_hours_per_day && c.caps.max_hours_per_day > 0 ? "bad" : "neutral"} />
        <Kpi n={c.caps.budget_credits} label="budget" sub="credits" />
        <Kpi n={c.caps.provider || "none"} label="provider" sub={c.caps.auto_approve === true || c.caps.auto_approve === 1 ? "auto-approve on" : "manual approval"} href={hrefs.settings("compute")} />
        <Kpi n={executors.length} label="executors" sub={plural(executors.filter((e) => e.status === "ready" || e.status === "online").length, "ready")} />
        <Kpi n={c.requests.length} label="requests" sub="all time" />
      </KpiGrid>
      <div className="grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(300px,1fr)]">
        <Panel
          title="Requests"
          actions={
            <div className="flex gap-1">
              <Button size="xs" variant={show === "pending" ? "default" : "ghost"} onClick={() => setShow("pending")}>
                pending ({pending.length})
              </Button>
              <Button size="xs" variant={show === "all" ? "default" : "ghost"} onClick={() => setShow("all")}>
                all ({c.requests.length})
              </Button>
            </div>
          }
          dense
        >
          {rows.length ? (
            <ComputeTable rows={rows} />
          ) : (
            <EmptyState className="m-3">
              {show === "pending" ? (
                <>
                  Nothing to decide. When an agent runs <Mono>pulse compute request</Mono> the request shows here and on the hub with Approve / Deny.
                </>
              ) : (
                "No compute request has been made yet."
              )}
            </EmptyState>
          )}
        </Panel>
        <div className="grid gap-4 content-start">
          <Panel title="Caps" actions={<Link href={hrefs.settings("compute")} className="hover:underline">edit</Link>}>
            <CapsPanel caps={c.caps} />
          </Panel>
          <Panel title="Executors" dense>
            {executors.length ? (
              <ul className="divide-y">
                {executors.map((e) => (
                  <ExecutorRow key={e.name} e={e} />
                ))}
              </ul>
            ) : (
              <EmptyState className="m-3">
                No executor registered. Approve a request or add one with <Mono>pulse executor add</Mono>; the daemon probes it and runs jobs there.
              </EmptyState>
            )}
          </Panel>
          <MachinePanel r={runner.data} />
        </div>
      </div>
    </>
  );
}
