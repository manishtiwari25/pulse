"use client";

import { useState } from "react";
import { CheckIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { DataTable, type Column } from "@/components/pulse/data-table";
import { Kv } from "@/components/pulse/panel";
import { Mono, ProjectLink, StatusBadge, ToneBadge, When } from "@/components/pulse/who";
import { Bar2 } from "@/components/pulse/charts";
import { writes, type ComputeCaps, type ComputeRequest } from "@/lib/api";
import { num } from "@/lib/format";

const CHANGES = (p: string) => p.startsWith("compute") || p.startsWith("executors") || p.startsWith("events") || p.startsWith("runner");

// Approve / deny buttons for one pending request; each confirms with words.
export function ComputeDecide({ r }: { r: ComputeRequest }) {
  const [which, setWhich] = useState<"approve" | "deny" | null>(null);
  if (r.status !== "pending") return null;
  return (
    <span className="inline-flex gap-1">
      <Button size="xs" onClick={() => setWhich("approve")} aria-label={"Approve " + r.id}>
        <CheckIcon /> Approve
      </Button>
      <Button size="xs" variant="outline" onClick={() => setWhich("deny")} aria-label={"Deny " + r.id}>
        <XIcon /> Deny
      </Button>
      <ConfirmDialog
        open={which !== null}
        onOpenChange={(o) => !o && setWhich(null)}
        title={(which === "approve" ? "Approve " : "Deny ") + r.id + "?"}
        description={
          <span>
            <Mono>{r.requested_by}</Mono> in <Mono>{r.project}</Mono> asks for <Mono>{r.size}</Mono> for {num(r.hours)} h: {r.reason}
            {which === "approve" ? " Approving lets the daemon provision the machine within the caps and charges the budget." : " Denying records your reason on the request; the agent sees it on its next loop."}
          </span>
        }
        action={which === "approve" ? "Approve" : "Deny"}
        destructive={which === "deny"}
        words={{ label: "Your words (recorded on the request)", placeholder: which === "approve" ? "approved - needed for the load test" : "not now - use the shared executor", required: which === "deny" }}
        success={r.id + (which === "approve" ? " approved." : " denied.")}
        onConfirm={(w) => (which === "approve" ? writes.approveCompute(r.id, w) : writes.denyCompute(r.id, w))}
        invalidates={CHANGES}
      />
    </span>
  );
}

export const COMPUTE_COLUMNS: Column<ComputeRequest>[] = [
  { key: "id", label: "request", nowrap: true, sort: (r) => r.id, render: (r) => <Mono>{r.id}</Mono> },
  { key: "status", label: "status", sort: (r) => r.status, render: (r) => <StatusBadge status={r.status} /> },
  { key: "project", label: "project", sort: (r) => r.project, render: (r) => <ProjectLink slug={r.project} /> },
  { key: "by", label: "asked by", hide: "md", render: (r) => <Mono>{r.requested_by}</Mono> },
  { key: "size", label: "size", nowrap: true, render: (r) => <Mono>{r.size}</Mono> },
  { key: "hours", label: "hours", align: "right", sort: (r) => r.hours, render: (r) => num(r.hours) },
  { key: "reason", label: "reason", render: (r) => <span className="anywhere">{r.reason}</span> },
  { key: "at", label: "asked", nowrap: true, sort: (r) => r.at, render: (r) => <When d={r.at} /> },
  {
    key: "decision",
    label: "decision",
    render: (r) => (r.status === "pending" ? <ComputeDecide r={r} /> : <span className="text-xs text-muted-foreground">{r.decided_by ? "by " + r.decided_by : ""}{r.executor ? <> {" · "}<Mono>{r.executor}</Mono></> : null}</span>),
  },
];

export function ComputeTable({ rows, dense }: { rows: ComputeRequest[]; dense?: boolean }) {
  return <DataTable columns={COMPUTE_COLUMNS} rows={rows} rowKey={(r) => r.id} initialSort="at" dense={dense} caption="Compute requests" />;
}

export function CapsPanel({ caps }: { caps: ComputeCaps }) {
  const auto = caps.auto_approve === true || caps.auto_approve === 1;
  return (
    <div className="grid gap-3">
      <Kv
        rows={[
          ["provider", <Mono key="p">{caps.provider || "(none)"}</Mono>],
          [
            "machines",
            <span key="m" className="inline-flex items-center gap-2">
              <span className="tnum">{num(caps.vms_now) + " of " + num(caps.max_vms)}</span>
              <Bar2 value={caps.vms_now} max={caps.max_vms} className="w-24" />
            </span>,
          ],
          [
            "hours today",
            <span key="h" className="inline-flex items-center gap-2">
              <span className="tnum">{num(caps.hours_today) + " of " + num(caps.max_hours_per_day)}</span>
              <Bar2 value={caps.hours_today} max={caps.max_hours_per_day} className="w-24" />
            </span>,
          ],
          ["budget", <span key="b" className="tnum">{num(caps.budget_credits) + " credits"}</span>],
          ["auto-approve", <ToneBadge key="a" tone={auto ? "warn" : "neutral"}>{auto ? "on - requests within caps start without you" : "off - every request waits for you"}</ToneBadge>],
        ]}
      />
      <p className="text-xs text-muted-foreground">
        Caps and the Azure target live in <a className="underline" href="/settings/#compute">Settings → Compute</a>.
      </p>
    </div>
  );
}
