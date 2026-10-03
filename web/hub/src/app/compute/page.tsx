"use client";

import Link from "next/link";
import { useState } from "react";
import { ClockIcon, CloudIcon, CoinsIcon, CpuIcon, HistoryIcon, InboxIcon, ServerIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ExecutorRow, MachinePanel } from "@/components/pulse/cards";
import { HygienePanel } from "@/components/pulse/hygiene";
import { CapsPanel, ComputeRequestCard } from "@/components/compute/compute";
import { CardGrid, SectionHeading } from "@/components/pulse/item-card";
import { EmptyState, ErrorNotice, Kpi, KpiGrid, PageTitle, Panel, SkeletonPage } from "@/components/pulse/panel";
import { RemoveMenuButton } from "@/components/project/project-dialogs";
import { hrefs } from "@/lib/nav";
import { num } from "@/lib/format";
import type { Compute, ComputeRequest, Executor, Runner } from "@/lib/api";
import { useApi } from "@/lib/use-api";

type Show = "pending" | "all";

// Pending requests first (they need the owner), newest first within each group.
const rank = (r: ComputeRequest) => (r.status === "pending" ? 0 : 1);
const byUrgency = (a: ComputeRequest, b: ComputeRequest) => rank(a) - rank(b) || (b.at || "").localeCompare(a.at || "");

export default function ComputePage() {
  const q = useApi<Compute>("compute", { refresh: 10000 });
  const runner = useApi<Runner>("runner");
  const [show, setShow] = useState<Show>("pending");
  if (q.error && !q.data)
    return (
      <>
        <PageTitle icon={CpuIcon} sub="Extra machines agents asked for. Approve or deny them here.">Machines</PageTitle>
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      </>
    );
  if (!q.data) return <SkeletonPage rows={3} />;
  const c = q.data;
  const pending = c.requests.filter((r) => r.status === "pending");
  const live = c.requests.filter((r) => r.status === "approved" || r.status === "provisioning" || r.status === "ready");
  const rows = [...(show === "pending" ? pending : c.requests)].sort(byUrgency);
  // The compute route returns the raw executor rows; the runner adds the live
  // running / free counts (and the local machine), so prefer it once loaded.
  const executors: Executor[] = runner.data?.executors?.length ? runner.data.executors : (c.executors ?? []).map((e) => ({ ...e, running: e.running ?? 0, free: e.free ?? 0 }));
  const active = executors.filter((e) => e.status === "active").length;
  const chips: { id: Show; label: string; n: number }[] = [
    { id: "pending", label: "pending", n: pending.length },
    { id: "all", label: "all", n: c.requests.length },
  ];
  return (
    <>
      <PageTitle
        icon={CpuIcon}
        sub="Extra machines agents asked for. Approve or deny them here."
        bar={
          <div role="group" aria-label="Which requests" className="surface-glass inline-flex rounded-xl border p-0.5">
            {chips.map((ch) => (
              <Button key={ch.id} size="xs" variant="ghost" aria-pressed={show === ch.id} className={show === ch.id ? "bg-card text-foreground shadow-card hover:bg-card" : "text-muted-foreground hover:text-foreground"} onClick={() => setShow(ch.id)}>
                {ch.label} <span className={show === ch.id ? "tnum rounded-full bg-primary/10 px-1.5 text-[10.5px] text-primary" : "tnum rounded-full bg-muted px-1.5 text-[10.5px] text-muted-foreground"}>{num(ch.n)}</span>
              </Button>
            ))}
          </div>
        }
      >
        Machines
      </PageTitle>
      <KpiGrid>
        <Kpi n={pending.length} label="to decide" tone={pending.length ? "warn" : "neutral"} sub="pending requests" icon={InboxIcon} />
        <Kpi n={live.length} label="approved" sub="starting or ready" tone={live.length ? "ok" : "neutral"} icon={CpuIcon} />
        <Kpi n={num(c.caps.vms_now) + " / " + num(c.caps.max_vms)} label="machines" sub="now / cap" icon={ServerIcon} />
        <Kpi n={num(c.caps.hours_today) + " / " + num(c.caps.max_hours_per_day)} label="hours today" sub="used / cap" tone={c.caps.hours_today >= c.caps.max_hours_per_day && c.caps.max_hours_per_day > 0 ? "bad" : "neutral"} icon={ClockIcon} />
        <Kpi n={c.caps.budget_credits} label="budget" sub="credits" icon={CoinsIcon} />
        <Kpi n={c.caps.provider || "none"} label="provider" sub={c.caps.auto_approve === true || c.caps.auto_approve === 1 ? "auto-approve on" : "manual approval"} href={hrefs.settings("compute")} icon={CloudIcon} />
        <Kpi n={executors.length} label="work machines" sub={num(active) + " active"} tone={active ? "ok" : "neutral"} icon={ServerIcon} />
        <Kpi n={c.requests.length} label="requests" sub="all time" />
      </KpiGrid>
      <div className="grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(300px,1fr)]">
        <section aria-label="Requests">
          <SectionHeading count={rows.length} icon={show === "pending" ? InboxIcon : HistoryIcon}>{show === "pending" ? "Requests to decide" : "Past requests"}</SectionHeading>
          {rows.length ? (
            <CardGrid cols={2}>
              {rows.map((r) => (
                <ComputeRequestCard key={r.id} r={r} caps={c.caps} />
              ))}
            </CardGrid>
          ) : (
            <EmptyState title="No machine request">
              {show === "pending" ? (
                <>
                  Nothing to decide. When an agent asks for another machine, the request shows here with Approve and Deny.
                </>
              ) : (
                "No machine request has been made yet."
              )}
            </EmptyState>
          )}
        </section>
        <div className="grid gap-4 content-start">
          <Panel title="Safety limits" actions={<Link href={hrefs.settings("compute")} className="hover:underline">edit</Link>}>
            <CapsPanel caps={c.caps} />
          </Panel>
          <Panel title="Machines ready to work" dense>
            {executors.length ? (
              <ul className="grid gap-1.5 p-3">
                {executors.map((e) => (
                  // The local executor is this machine and a removed row is already gone: neither gets Remove.
                  <ExecutorRow key={e.name} e={e} action={e.name !== "local" && e.status !== "removed" ? <RemoveMenuButton kind="executor" name={e.name} compact /> : undefined} />
                ))}
              </ul>
            ) : (
              <EmptyState className="m-3">
                No extra machine is ready yet. Approve a request or add a machine; agents will use it after it checks in.
              </EmptyState>
            )}
          </Panel>
          <MachinePanel r={runner.data} />
          <HygienePanel />
        </div>
      </div>
    </>
  );
}
