"use client";

import Link from "next/link";
import { useState } from "react";
import { CheckIcon, ExternalLinkIcon, Loader2Icon, SparklesIcon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, showApiError } from "@/components/pulse/confirm";
import { amendCommand, CommandBox, seedName } from "@/components/pulse/evolve";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardTitle } from "@/components/pulse/item-card";
import { Markdown } from "@/components/pulse/markdown";
import { Notice } from "@/components/pulse/panel";
import { Mono, ProjectLink, ToneBadge, When, Who } from "@/components/pulse/who";
import { writes, type EvolveRecordRef, type JobRef, type RecordDetail, type RecordDraft } from "@/lib/api";
import { hrefs } from "@/lib/nav";
import { invalidate, useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// Record drafts (ADR-046). A project's constitution and evaluators start as
// the template's text; the daemon has an agent read the repository and file
// the complete text as a draft, and the owner adopts it here in one click.
// The console never edits the text itself: adopting writes the draft as the
// next revision in the owner's name, with the drafting agent in the note.

const CHANGES = (p: string) => p.startsWith("records") || p.startsWith("projects") || p === "org" || p.startsWith("events") || p.startsWith("jobs");

const kindOf = (id: string): "constitution" | "evaluators" => (id.includes("evaluators") ? "evaluators" : "constitution");
const slugOf = (id: string) => id.replace(/^EVO-(constitution|evaluators)-?/, "");

// What one loop record is waiting on, from a project row or the evolution document.
export interface DraftState {
  id: string;
  rev: number;
  seeded: boolean;
  seeded_from: string;
  draft?: RecordDraft | null;
  draft_job?: JobRef | null;
}

export const draftState = (r: EvolveRecordRef | null | undefined): DraftState | null => (r ? { id: r.id, rev: r.rev, seeded: r.seeded, seeded_from: r.seeded_from, draft: r.draft, draft_job: r.draft_job } : null);

// The badge suffix a chip shows for a record: what waits, if anything.
export function draftSuffix(r: { seeded: boolean; draft?: RecordDraft | null; draft_job?: JobRef | null }): { text: string; tone: "ok" | "warn" | "info"; title: string } | null {
  if (r.draft) return { text: "draft ready", tone: "info", title: "an agent drafted this record's text from the repository (" + r.draft.author + ") - read it and adopt or discard it on the Evolution tab" };
  if (r.draft_job) return { text: "drafting…", tone: "info", title: "an agent is reading the repository and writing the text (job " + r.draft_job.id + ", " + r.draft_job.status + ")" };
  if (r.seeded) return { text: "seeded", tone: "warn", title: "still the template's text - the daemon drafts it for you, or ask for a draft now on the Evolution tab" };
  return null;
}

// "Draft it for me": queue the drafting job now instead of waiting for the planner.
export function DraftJobButton({ id, again, size = "sm", variant = "default", className }: { id: string; again?: boolean; size?: "xs" | "sm"; variant?: "default" | "outline"; className?: string }) {
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    const t = toast.loading("Queueing the drafting job…");
    try {
      const res = await writes.draftJob(id);
      toast.success(res.created ? "Drafting job queued: " + res.job.id : "A drafting job is already " + res.job.status + " (" + res.job.id + ")", { id: t, description: res.created ? "An agent reads the repository and files the text as a draft; you adopt it here." : undefined });
      invalidate(CHANGES);
    } catch (e) {
      toast.dismiss(t);
      showApiError(e, "Could not queue the drafting job");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Button onClick={() => void run()} disabled={busy} size={size} variant={variant} className={className} title="an agent reads the repository and drafts the text; nothing changes until you adopt it">
      {busy ? <Loader2Icon className="animate-spin" /> : <SparklesIcon />}
      {again ? "Draft again" : "Draft it for me"}
    </Button>
  );
}

// The draft's text, from the document when it carries it, else fetched.
function DraftBody({ id, body }: { id: string; body?: string }) {
  const detail = useApi<RecordDetail>(body === undefined ? "records/" + encodeURIComponent(id) : null, { refresh: false });
  const text = body ?? detail.data?.draft?.body;
  if (text === undefined) return <p className="text-xs text-muted-foreground">{detail.error ? "Could not load the draft: " + detail.error : "Loading the draft…"}</p>;
  return <Markdown source={text} className="max-h-[45vh] overflow-y-auto rounded-md border bg-background/60 px-3 py-2 text-[13px]" />;
}

function DraftLine({ draft, rev }: { draft: RecordDraft; rev: number }) {
  return (
    <span className="text-xs text-muted-foreground">
      <Who row={{ agent: draft.author }} compact /> {"drafted it "}
      <When d={draft.at} />
      {draft.base_rev !== rev ? " against r" + draft.base_rev + " (the record is r" + rev + " now)" : ""}
      {draft.job_id ? (
        <>
          {" · job "}
          <Mono>{draft.job_id}</Mono>
        </>
      ) : null}
      {draft.note ? " · " + draft.note : ""}
    </span>
  );
}

// Adopt and Discard, each behind a confirm with the owner's optional words.
export function DraftActions({ r, body, size = "sm", className }: { r: DraftState; body?: string; size?: "xs" | "sm"; className?: string }) {
  const [open, setOpen] = useState<"adopt" | "discard" | null>(null);
  const draft = r.draft;
  if (!draft) return null;
  const what = kindOf(r.id);
  return (
    <span className={cn("flex flex-wrap items-center gap-1.5", className)}>
      <Button size={size} onClick={() => setOpen("adopt")} title={"read the draft and make it the " + what + "'s text (revision r" + (r.rev + 1) + ", in your name)"}>
        <CheckIcon /> Read &amp; adopt
      </Button>
      <Button size={size} variant="outline" onClick={() => setOpen("discard")} title="drop the draft; the record keeps its current text and the daemon may draft again">
        <XIcon /> Discard
      </Button>
      <ConfirmDialog
        open={open === "adopt"}
        onOpenChange={(o) => setOpen(o ? "adopt" : null)}
        title={"Adopt the drafted " + what + " of " + slugOf(r.id)}
        description={"The text below becomes " + r.id + " revision r" + (r.rev + 1) + ", authored by you with " + draft.author + " named in the note. The record's frontmatter stays; loop, baseline and scope settings are untouched. Read it first - the facts are the agent's reading of the repository."}
        action="Adopting"
        success={r.id + " adopted - the " + what + " is yours now"}
        words={{ label: "Your words (optional)", placeholder: "Paths checked; the hard stops match ours …" }}
        onConfirm={(words) => writes.adoptDraft(r.id, words)}
        invalidates={CHANGES}
      >
        <DraftBody id={r.id} body={body} />
      </ConfirmDialog>
      <ConfirmDialog
        open={open === "discard"}
        onOpenChange={(o) => setOpen(o ? "discard" : null)}
        title={"Discard the drafted " + what + " of " + slugOf(r.id)}
        description={"The draft by " + draft.author + " is dropped; " + r.id + " keeps its current text (r" + r.rev + "). While the text is still the seeded copy the daemon drafts again after a day, or you ask for one now."}
        action="Discarding"
        destructive
        success={"Draft on " + r.id + " discarded"}
        words={{ label: "Why (optional)", placeholder: "Wrong product paths …" }}
        onConfirm={(words) => writes.discardDraft(r.id, words)}
        invalidates={CHANGES}
      />
    </span>
  );
}

// Under a record's panel on the Evolution tab: the draft with its actions, the
// drafting job under way, or the seeded text with the two ways to replace it.
// Nothing when the text is the project's own.
export function RecordDraftPanel({ r, body, className }: { r: DraftState; body?: string; className?: string }) {
  const what = kindOf(r.id);
  if (r.draft) {
    return (
      <Notice tone="info" className={cn("text-xs", className)}>
        <div className="flex flex-wrap items-center gap-x-1.5 gap-y-1">
          <ToneBadge tone="info">draft ready</ToneBadge>
          <DraftLine draft={r.draft} rev={r.rev} />
        </div>
        <details className="mt-2 group">
          <summary className="cursor-pointer select-none text-[12px] font-medium text-foreground/80 hover:underline">Show the draft</summary>
          <div className="mt-1.5">
            <DraftBody id={r.id} body={body} />
          </div>
        </details>
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <DraftActions r={r} body={body} size="xs" />
          <span className="text-[11.5px] text-muted-foreground">Adopting writes it as r{r.rev + 1} in your name; the console never edits the text itself.</span>
        </div>
      </Notice>
    );
  }
  if (r.draft_job) {
    return (
      <Notice tone="info" className={cn("text-xs", className)}>
        <div className="flex flex-wrap items-center gap-x-1.5 gap-y-1">
          <ToneBadge tone="info">drafting…</ToneBadge>
          <span>
            {"An agent is reading the repository and writing the " + what + " - job "}
            <Mono>{r.draft_job.id}</Mono>
            {" is " + r.draft_job.status + ". The draft appears here for you to read and adopt; the record does not change until you do."}
          </span>
        </div>
      </Notice>
    );
  }
  if (!r.seeded) return null;
  const from = r.seeded_from;
  return (
    <Notice tone="warn" className={cn("text-xs", className)}>
      <div className="flex flex-wrap items-center gap-x-1.5 gap-y-1">
        <ToneBadge tone="warn">seeded</ToneBadge>
        <span>
          {from ? (
            <span title={from}>
              {"still the template's text, seeded from "}
              <Mono>{seedName(from)}</Mono>
            </span>
          ) : (
            "still the template's text"
          )}
          {" · the loop stays off until the " + what + " is this project's own."}
        </span>
      </div>
      <div className="mt-2 flex flex-wrap items-center gap-2">
        <DraftJobButton id={r.id} size="xs" />
        <span className="text-[11.5px] text-muted-foreground">An agent reads the repository (README, manifests, CI, checks) and drafts the complete text; you read and adopt it here. The daemon queues this by itself within a day.</span>
      </div>
      <details className="mt-2">
        <summary className="cursor-pointer select-none text-[11.5px] text-muted-foreground hover:underline">Or write it yourself in a terminal</summary>
        <CommandBox command={amendCommand(r.id)} copy className="mt-1.5 bg-background/60" />
        <p className="mt-1 text-[11.5px] text-muted-foreground">Run on the hub machine with your text in FILE. The console shows the record and never edits it.</p>
      </details>
    </Notice>
  );
}

// The inbox card: one drafted record, ready for the owner's read and yes or no.
export function DraftCard({ slug, r }: { slug: string; r: DraftState }) {
  const draft = r.draft;
  if (!draft) return null;
  const what = kindOf(r.id);
  return (
    <ItemCard highlight ariaLabel={"Adopt the drafted " + what + " of " + slug}>
      <ItemCardHeader>
        <ToneBadge tone="accent">self-improvement</ToneBadge>
        <ToneBadge tone="info">draft ready</ToneBadge>
        <ProjectLink slug={slug} className="text-xs" />
      </ItemCardHeader>
      <ItemCardTitle>{"Adopt the drafted " + what + " of " + slug}</ItemCardTitle>
      <ItemCardBody clamp={3}>
        {draft.author + " read the repository and wrote the " + what + " with every placeholder filled in" + (draft.note ? " - " + draft.note : "") + ". Read it and adopt it as r" + (r.rev + 1) + " in your name, or discard it. The record keeps the template's text until you do."}
      </ItemCardBody>
      <ItemCardActions>
        <span className="flex min-w-0 flex-wrap items-center gap-x-1 gap-y-0.5">
          <Button asChild variant="ghost" size="xs" className="text-muted-foreground">
            <Link href={hrefs.project(slug, "evolution")} title={"self-improvement for " + slug}>
              self-improvement <ExternalLinkIcon />
            </Link>
          </Button>
        </span>
        <DraftActions r={r} size="xs" className="ml-auto shrink-0" />
      </ItemCardActions>
    </ItemCard>
  );
}
