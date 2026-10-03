"use client";

import { useMemo } from "react";
import { daemonControlState, type DaemonControlState } from "./daemon-control-state";
import { foldSteps, gateStep, type GateStep } from "./evolve";
import { awaitsDecision } from "@/components/records/decide-record";
import { draftState, type DraftState } from "@/components/records/draft-panel";
import { evolveGate, registeredProjects, type Compute, type ComputeRequest, type EvolveGate, type Org, type OwnerQuestion, type ProjectCardData, type ProjectsPayload, type Proposal, type RecordRow, type RecordsPayload, type Runner, type ServicePayload, type SettingsPayload } from "@/lib/api";
import { plural } from "@/lib/format";
import { sortOwnerQuestions } from "@/lib/owner-questions";
import { useApi, type Loaded } from "@/lib/use-api";

// The one "for you" count. Everything that says how many things wait for the
// owner - the home hero, the sidebar tile, a project's hero, its overview tile
// and its For-you tab - reads it from here, so no two places can disagree.
// An item is "for you" when nothing happens until the owner says yes or no: a
// proposed decision record, a proposal awaiting the owner, a question
// addressed to the owner (the loop's stop questions included), a pending
// compute request, a drafted constitution or evaluators, the one gate step a
// project waits at, and - for the whole machine - a paused or unset daemon.

export interface InboxItems {
  decisions: RecordRow[];
  proposals: Proposal[];
  questions: OwnerQuestion[];
  compute: ComputeRequest[];
  drafts: { slug: string; r: DraftState }[];
  steps: GateStep[];
  machine: boolean;
  total: number;
}

export interface Inbox extends InboxItems {
  // True once the org payload arrived - before that the count means nothing.
  ready: boolean;
  // Inputs still loading / failed, as the short phrases the inbox shows.
  checking: string[];
  down: string[];
  downWhy: string[];
  daemon: DaemonControlState;
  gateOf: (p: ProjectCardData) => EvolveGate | null;
  forProject: (slug: string) => InboxItems;
}

export interface InboxSources {
  org: Loaded<Org>;
  proposed: Loaded<RecordsPayload>;
  reg: Loaded<ProjectsPayload>;
  compute: Loaded<Compute>;
  runner: Loaded<Runner>;
  service: Loaded<ServicePayload>;
  settings: Loaded<SettingsPayload>;
}

const sum = (items: Omit<InboxItems, "total">) => items.decisions.length + items.proposals.length + items.questions.length + items.compute.length + items.drafts.length + items.steps.length + (items.machine ? 1 : 0);

export function buildInbox(q: InboxSources): Inbox {
  const o = q.org.data;
  const projects = registeredProjects(o);
  const regRows = q.reg.data?.projects ?? [];
  const brain = o?.totals.brain ?? "";
  const gateOf = (p: ProjectCardData) => evolveGate(p, regRows.find((r) => r.slug === p.slug));
  const decisions = (q.proposed.data?.records ?? []).filter(awaitsDecision);
  const proposals = (o?.proposals ?? []).filter((p) => p.status === "awaiting");
  const questions = sortOwnerQuestions(o?.questions ?? []);
  const compute = (q.compute.data?.requests ?? []).filter((r) => r.status === "pending");
  const drafts = projects.flatMap((p) => {
    const g = gateOf(p);
    return [draftState(g?.constitution), draftState(g?.evaluators)].filter((r): r is DraftState => !!r?.draft).map((r) => ({ slug: p.slug, r }));
  });
  // One held-out step can serve every project; the machine-wide count folds it, a project's own count keeps its step.
  const ownSteps = projects.map((p) => gateStep(p.slug, gateOf(p))).filter((s): s is GateStep => s !== null);
  const steps = foldSteps(ownSteps);
  const daemon = daemonControlState({ runner: q.runner, service: q.service, settings: q.settings });
  const machine = daemon.pause === "on" || !!daemon.setup;
  const all = { decisions, proposals, questions, compute, drafts, steps, machine };
  const decisionsDown = !!q.proposed.error && !q.proposed.data;
  const computeDown = !!q.compute.error && !q.compute.data;
  return {
    ...all,
    total: sum(all),
    ready: !!o,
    checking: [!q.proposed.data && !decisionsDown ? "decisions" : "", !q.compute.data && !computeDown ? "compute requests" : ""].filter(Boolean),
    down: [decisionsDown ? "decisions could not be loaded" : "", computeDown ? "compute requests could not be loaded" : ""].filter(Boolean),
    downWhy: [decisionsDown ? "decisions could not be loaded (" + q.proposed.error + ")" : "", computeDown ? "compute requests could not be loaded (" + q.compute.error + ")" : ""].filter(Boolean),
    daemon,
    gateOf,
    forProject: (slug) => {
      const mine = {
        decisions: decisions.filter((r) => r.project === slug),
        proposals: proposals.filter((p) => (p.slug || brain) === slug),
        questions: questions.filter((x) => (x.slug || brain) === slug),
        compute: compute.filter((r) => r.project === slug),
        drafts: drafts.filter((d) => d.slug === slug),
        steps: ownSteps.filter((s) => s.slug === slug),
        machine: false,
      };
      return { ...mine, total: sum(mine) };
    },
  };
}

// The breakdown as one short line: "1 proposal · 2 questions", or the fallback when nothing waits.
export function inboxBreakdown(items: InboxItems, empty = "nothing waits"): string {
  const parts = [
    items.decisions.length ? plural(items.decisions.length, "decision") : "",
    items.proposals.length ? plural(items.proposals.length, "proposal") : "",
    items.questions.length ? plural(items.questions.length, "question") : "",
    items.compute.length ? plural(items.compute.length, "request") : "",
    items.drafts.length ? plural(items.drafts.length, "draft") : "",
    items.steps.length + (items.machine ? 1 : 0) ? plural(items.steps.length + (items.machine ? 1 : 0), "step") : "",
  ].filter(Boolean);
  return parts.length ? parts.join(" · ") : empty;
}

// Every path below is shared through useApi's cache, so a second caller on
// the same screen costs no extra request.
export function useInbox(): Inbox {
  const org = useApi<Org>("org");
  const proposed = useApi<RecordsPayload>("records?kind=adr&status=Proposed", { refresh: 60000 });
  const reg = useApi<ProjectsPayload>("projects", { refresh: 60000 });
  const compute = useApi<Compute>("compute");
  const runner = useApi<Runner>("runner");
  const service = useApi<ServicePayload>("service", { refresh: 60000 });
  const settings = useApi<SettingsPayload>("settings");
  return useMemo(() => buildInbox({ org, proposed, reg, compute, runner, service, settings }), [org, proposed, reg, compute, runner, service, settings]);
}
