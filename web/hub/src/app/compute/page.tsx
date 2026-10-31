"use client";

import Link from "next/link";
import { CheckCircle2Icon, ServerIcon } from "lucide-react";
import { AddMachineButton, PairingRow } from "@/components/compute/add-machine";
import { ExecutorRow, MachinePanel } from "@/components/compute/machine";
import { HygienePanel } from "@/components/pulse/hygiene";
import { CapsPanel, ComputeInboxCard, ComputeRequestCard } from "@/components/compute/compute";
import { CardGrid, SectionHeading } from "@/components/pulse/item-card";
import { EmptyState, ErrorNotice, PageTitle, Panel, SkeletonPage } from "@/components/pulse/panel";
import { RemoveMenuButton } from "@/components/project/project-dialogs";
import { hrefs } from "@/lib/nav";
import { plural } from "@/lib/format";
import type { Compute, ComputeRequest, Executor, Runner } from "@/lib/api";
import { useApi } from "@/lib/use-api";

const SUB = "Where your agents run. Agents can ask for an extra machine for a few hours; nothing starts until you approve it.";

const newestFirst = (a: ComputeRequest, b: ComputeRequest) => (b.at || "").localeCompare(a.at || "");

const LIVE = new Set(["approved", "provisioning", "ready"]);

const quietLink = "text-[13px] font-medium text-muted-foreground underline-offset-2 hover:text-foreground hover:underline";

export default function ComputePage() {
  const q = useApi<Compute>("compute", { refresh: 10000 });
  const runner = useApi<Runner>("runner");
  if (q.error && !q.data)
    return (
      <>
        <PageTitle icon={ServerIcon} sub={SUB}>
          Machines
        </PageTitle>
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      </>
    );
  if (!q.data) return <SkeletonPage rows={3} />;
  const c = q.data;
  const pending = c.requests.filter((r) => r.status === "pending").sort(newestFirst);
  // Approved requests that are starting or running are live and use hours, so
  // they get their own group; "Past requests" keeps the settled ones.
  const live = c.requests.filter((r) => LIVE.has(r.status)).sort(newestFirst);
  const past = c.requests.filter((r) => r.status !== "pending" && !LIVE.has(r.status)).sort(newestFirst);
  // The compute route returns the raw executor rows; the runner adds the live
  // running / free counts (and the local machine), so prefer it once loaded.
  const executors: Executor[] = runner.data?.executors?.length ? runner.data.executors : (c.executors ?? []).map((e) => ({ ...e, running: e.running ?? 0, free: e.free ?? 0 }));
  const active = executors.filter((e) => e.status === "active").length;
  const pairings = c.pairings ?? [];
  // A finished pairing's machine is in the executors list already; the row
  // under it is the report, kept until dismissed.
  const machinesNote = executors.length ? plural(active, "machine") + " taking work right now." : "Every machine your agents can use.";
  return (
    <>
      <PageTitle icon={ServerIcon} sub={SUB}>
        Machines
      </PageTitle>

      <div className="grid min-w-0 gap-10 [&>*]:min-w-0">
        <section aria-label="Waiting for you">
          <SectionHeading count={pending.length || undefined} note={pending.length ? "An agent asked for an extra machine. It starts only if you approve." : undefined}>
            Waiting for you
          </SectionHeading>
          {pending.length ? (
            <CardGrid cols={2} className={pending.length === 1 ? "max-w-3xl" : undefined}>
              {pending.map((r) => (
                <ComputeInboxCard key={r.id} r={r} caps={c.caps} here />
              ))}
            </CardGrid>
          ) : (
            <p className="flex items-center gap-2.5 rounded-2xl border bg-card px-4 py-3.5 text-sm text-muted-foreground">
              <CheckCircle2Icon className="size-4 shrink-0 text-ok" aria-hidden />
              <span className="min-w-0 anywhere">Nothing to decide. When an agent asks for an extra machine, it shows up here.</span>
            </p>
          )}
        </section>

        {live.length ? (
          <section aria-label="In use now">
            <SectionHeading count={live.length} note="Extra machines you approved that are starting or running. Each one stops when its hours are up.">
              In use now
            </SectionHeading>
            <CardGrid cols={2} className={live.length === 1 ? "max-w-3xl" : undefined}>
              {live.map((r) => (
                <ComputeRequestCard key={r.id} r={r} />
              ))}
            </CardGrid>
          </section>
        ) : null}

        <div className="grid min-w-0 items-start gap-10 lg:grid-cols-2 lg:gap-6">
          <section aria-label="This machine" className="min-w-0">
            <SectionHeading note="The computer this hub runs on." action={<Link href={hrefs.settings("runner")} className={quietLink}>Runner settings</Link>}>
              This machine
            </SectionHeading>
            <MachinePanel r={runner.data} heading={false} machines={false} />
          </section>

          <div className="grid min-w-0 gap-10 lg:gap-6">
            <section aria-label="All machines" className="min-w-0">
              <SectionHeading count={executors.length || undefined} note={machinesNote} action={<AddMachineButton caps={c.caps} />}>
                All machines
              </SectionHeading>
              {executors.length || pairings.length ? (
                <ul className="grid gap-2">
                  {pairings.map((p) => (
                    <PairingRow key={"pairing:" + p.name + ":" + p.started} p={p} />
                  ))}
                  {executors.map((e) => (
                    // The local executor is this machine and a removed row is already gone: neither gets Remove.
                    <ExecutorRow key={e.name} e={e} action={e.name !== "local" && e.status !== "removed" ? <RemoveMenuButton kind="executor" name={e.name} compact /> : undefined} />
                  ))}
                </ul>
              ) : (
                <EmptyState>No machine is set up yet. Add one above - PULSE sets it up over ssh - or approve an agent&apos;s request; it starts taking work after its first check passes.</EmptyState>
              )}
            </section>

            <section aria-label="Limits" className="min-w-0">
              <SectionHeading note="Every request for an extra machine has to fit inside these.">Limits</SectionHeading>
              <Panel>
                <CapsPanel caps={c.caps} />
              </Panel>
            </section>
          </div>
        </div>

        <section aria-label="Past requests">
          <SectionHeading count={past.length || undefined} note={past.length ? "Earlier requests that are over, and your answers." : undefined}>
            Past requests
          </SectionHeading>
          {past.length ? (
            <CardGrid cols={2} className={past.length === 1 ? "max-w-3xl" : undefined}>
              {past.map((r) => (
                <ComputeRequestCard key={r.id} r={r} />
              ))}
            </CardGrid>
          ) : (
            <p className="text-sm text-muted-foreground">Nothing here yet. Requests you deny, and machines whose time is up, move here.</p>
          )}
        </section>

        <section aria-label="Cleanup">
          <SectionHeading note="Agents clean up after themselves; the runner cleans up what's left every 10 minutes.">Cleanup</SectionHeading>
          <HygienePanel heading={false} />
        </section>
      </div>
    </>
  );
}
