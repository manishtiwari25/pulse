"use client";

import Link from "next/link";
import { useState } from "react";
import { BookOpenIcon, CheckIcon, FileTextIcon, Loader2Icon, SparklesIcon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, showApiError } from "@/components/pulse/confirm";
import { amendCommand, CommandBox, seedName } from "@/components/pulse/evolve";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardKind, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { Markdown } from "@/components/pulse/markdown";
import { Disclosure, Notice } from "@/components/pulse/panel";
import { ProjectLink, When, Who } from "@/components/pulse/who";
import { writes, type EvolveRecordRef, type JobRef, type RecordDetail, type RecordDraft } from "@/lib/api";
import { hrefs } from "@/lib/nav";
import { invalidate, useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";
import { sentence } from "@/lib/format";
import { statusWord } from "@/lib/words";

// Record drafts (ADR-046). A project's constitution and evaluators start as
// the template's text; the runner has an agent read the repository and file
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

// "a new constitution" / "new evaluators": the thing drafted, in a sentence.
const aNew = (what: "constitution" | "evaluators") => (what === "evaluators" ? "new evaluators" : "a new constitution");
// The drafting agent's note reads "from the README, …": make it a sentence.
const noteWords = (note: string) => (/^from\s/i.test(note.trim()) ? "Drafted " + note.trim() : sentence(note));
const theWhat = (what: "constitution" | "evaluators") => "the " + what;

// The badge suffix a chip shows for a record: what waits, if anything.
export function draftSuffix(r: { seeded: boolean; draft?: RecordDraft | null; draft_job?: JobRef | null }): { text: string; tone: "ok" | "warn" | "info"; title: string } | null {
  if (r.draft) return { text: "Draft to review", tone: "info", title: r.draft.author + " drafted this text from the repository. Read it, then adopt or discard it on the Self-improvement tab." };
  if (r.draft_job) return { text: "Being drafted", tone: "info", title: "An agent is reading the repository and writing the text (job " + r.draft_job.id + ", " + statusWord(r.draft_job.status).toLowerCase() + ")." };
  if (r.seeded) return { text: "Template text", tone: "warn", title: "Still the template's text. The runner has an agent draft it for you, or ask for a draft now on the Self-improvement tab." };
  return null;
}

// "Draft it for me": queue the drafting job now instead of waiting for the planner.
export function DraftJobButton({ id, again, size = "sm", variant = "default", className }: { id: string; again?: boolean; size?: "xs" | "sm"; variant?: "default" | "outline"; className?: string }) {
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    const t = toast.loading("Asking an agent to draft it…");
    try {
      const res = await writes.draftJob(id);
      toast.success(res.created ? "Asked an agent to draft it." : "An agent is already on it (" + statusWord(res.job.status).toLowerCase() + ").", { id: t, description: res.created ? "The agent reads the repository and writes the text. You adopt or discard it here." : undefined });
      invalidate(CHANGES);
    } catch (e) {
      toast.dismiss(t);
      showApiError(e, "Couldn't ask for a draft");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Button onClick={() => void run()} disabled={busy} size={size} variant={variant} className={className} title="An agent reads the repository and drafts the text. Nothing changes until you adopt it.">
      {busy ? <Loader2Icon className="animate-spin" /> : <SparklesIcon />}
      {again ? "Draft again" : "Draft it for me"}
    </Button>
  );
}

// The draft's text, from the document when it carries it, else fetched.
function DraftBody({ id, body }: { id: string; body?: string }) {
  const detail = useApi<RecordDetail>(body === undefined ? "records/" + encodeURIComponent(id) : null, { refresh: false });
  const text = body ?? detail.data?.draft?.body;
  if (text === undefined) return <p className="text-[13px] text-muted-foreground">{detail.error ? "The draft didn't load: " + detail.error : "Loading the draft…"}</p>;
  return <Markdown source={text} className="max-h-[45vh] overflow-y-auto rounded-xl border bg-card px-4 py-3 text-sm" />;
}

// Who drafted it and when; the job id rides in the tooltip.
function DraftLine({ draft, rev }: { draft: RecordDraft; rev: number }) {
  return (
    <span className="inline-flex flex-wrap items-center gap-x-1.5 gap-y-1 text-[13px] text-muted-foreground" title={draft.job_id ? "Drafted by job " + draft.job_id : undefined}>
      <Who row={{ agent: draft.author }} compact />
      <span>
        {"drafted it "}
        <When d={draft.at} />
        {draft.base_rev !== rev ? ", from revision " + draft.base_rev + " (the record is at revision " + rev + " now)" : ""}
      </span>
    </span>
  );
}

// Discard and Adopt, each behind a confirm with the owner's optional words.
// The "yes" comes last, on the right.
export function DraftActions({ r, body, size = "sm", className }: { r: DraftState; body?: string; size?: "xs" | "sm" | "default"; className?: string }) {
  const [open, setOpen] = useState<"adopt" | "discard" | null>(null);
  const draft = r.draft;
  if (!draft) return null;
  const what = kindOf(r.id);
  const slug = slugOf(r.id);
  return (
    <span className={cn("flex flex-wrap items-center gap-2", className)}>
      <Button size={size} variant="outline" onClick={() => setOpen("discard")} title={"Drop the draft. " + sentence(theWhat(what)) + " keeps its current text (" + r.id + ")."}>
        <XIcon /> Discard
      </Button>
      <Button size={size} onClick={() => setOpen("adopt")} title={"Read the draft and make it your version: revision " + (r.rev + 1) + " of " + r.id + ", in your name"}>
        <CheckIcon /> Read &amp; adopt
      </Button>
      <ConfirmDialog
        open={open === "adopt"}
        onOpenChange={(o) => setOpen(o ? "adopt" : null)}
        title={"Adopt the drafted " + what + (slug ? " for " + slug : "") + "?"}
        description={"This text becomes revision " + (r.rev + 1) + " of " + theWhat(what) + ", in your name, with " + draft.author + " credited in the note. Its settings stay as they are. Read it first: the facts are the agent's reading of the repository."}
        action="Adopt"
        success={"Adopted. " + sentence(theWhat(what)) + (slug ? " for " + slug : "") + " is yours now."}
        words={{ label: "Your words (optional)", placeholder: "Checked the paths; the hard stops match ours …" }}
        onConfirm={(words) => writes.adoptDraft(r.id, words)}
        invalidates={CHANGES}
      >
        <DraftBody id={r.id} body={body} />
      </ConfirmDialog>
      <ConfirmDialog
        open={open === "discard"}
        onOpenChange={(o) => setOpen(o ? "discard" : null)}
        title={"Discard the drafted " + what + (slug ? " for " + slug : "") + "?"}
        description={draft.author + "'s draft is dropped and " + theWhat(what) + " keeps its current text (revision " + r.rev + "). While it is still the template's text, an agent drafts again after a day, or you can ask for a new draft."}
        action="Discard"
        destructive
        success="Draft discarded."
        words={{ label: "Why? (optional)", placeholder: "The product paths are wrong …" }}
        onConfirm={(words) => writes.discardDraft(r.id, words)}
        invalidates={CHANGES}
      />
    </span>
  );
}

// Under a record's panel on the Self-improvement tab: the draft with its
// actions, the drafting under way, or the template text with the two ways to
// replace it. Nothing when the text is the project's own.
export function RecordDraftPanel({ r, body, className }: { r: DraftState; body?: string; className?: string }) {
  const what = kindOf(r.id);
  if (r.draft) {
    return (
      <div className={cn("surface-you rounded-xl border px-4 py-3 text-sm", className)}>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <span className="inline-flex items-center gap-1.5 font-medium text-foreground">
            <FileTextIcon className="size-4 shrink-0 text-you-ink" aria-hidden />
            A draft is ready for you to review.
          </span>
          <DraftLine draft={r.draft} rev={r.rev} />
        </div>
        {r.draft.note ? <p className="mt-1 text-[13px] text-muted-foreground anywhere">{noteWords(r.draft.note)}</p> : null}
        <Disclosure summary="Read the draft" className="mt-2">
          <DraftBody id={r.id} body={body} />
        </Disclosure>
        <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-2">
          <DraftActions r={r} body={body} size="sm" />
          <span className="text-xs text-muted-foreground">{"Adopting saves it as revision " + (r.rev + 1) + " in your name. PULSE never edits the text itself."}</span>
        </div>
      </div>
    );
  }
  if (r.draft_job) {
    return (
      <Notice tone="info" className={cn("text-[13px]", className)}>
        <span title={"Job " + r.draft_job.id + ": " + statusWord(r.draft_job.status)}>
          {"An agent is reading the repository and writing " + theWhat(what) + ". The draft shows up here for you to read; nothing changes until you adopt it."}
        </span>
      </Notice>
    );
  }
  if (!r.seeded) return null;
  const from = r.seeded_from;
  return (
    <Notice tone="warn" className={cn("text-[13px]", className)}>
      <p className="text-sm text-foreground">
        <span title={from || undefined}>{"This is still the template's text" + (from ? " (from " + seedName(from) + ")" : "") + "."}</span>
        {" Self-improvement stays off until " + theWhat(what) + " is this project's own."}
      </p>
      <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-2">
        <DraftJobButton id={r.id} size="sm" />
        <span className="text-xs text-muted-foreground">An agent reads the repository (README, manifests, CI, checks) and drafts the full text for you to review. PULSE also asks for this by itself within a day.</span>
      </div>
      <Disclosure summary="Prefer the terminal? Write it yourself" className="mt-2" contentClassName="mt-1.5">
        <CommandBox command={amendCommand(r.id)} copy className="bg-background/60" />
        <p className="mt-1 text-xs text-muted-foreground">Run it on the hub machine with your text in FILE. The console shows the record and never edits it.</p>
      </Disclosure>
    </Notice>
  );
}

// The inbox card: one drafted record, ready for the owner's read and yes or
// no. The record and job ids live in tooltips only.
export function DraftCard({ slug, r }: { slug: string; r: DraftState }) {
  const draft = r.draft;
  if (!draft) return null;
  const what = kindOf(r.id);
  return (
    <ItemCard highlight ariaLabel={"Draft to review: " + draft.author + " drafted " + aNew(what) + " for " + slug}>
      <ItemCardHeader right={<When d={draft.at} />}>
        <span title={r.id + (draft.job_id ? ", drafted by job " + draft.job_id : "")}>
          <ItemCardKind icon={FileTextIcon} tone="you">Draft to review</ItemCardKind>
        </span>
        <ProjectLink slug={slug} className="font-normal text-muted-foreground" />
      </ItemCardHeader>
      <ItemCardTitle>{draft.author + " drafted " + aNew(what) + " for " + slug}</ItemCardTitle>
      <ItemCardBody clamp={2}>{"Read it, then adopt it as your version (revision " + (r.rev + 1) + ") or discard it."}</ItemCardBody>
      {draft.note || draft.base_rev !== r.rev ? (
        <ItemCardMeta>
          {draft.note ? <span className="anywhere">{noteWords(draft.note)}</span> : null}
          {draft.base_rev !== r.rev ? <span>{"Written against revision " + draft.base_rev + "; the record has changed since."}</span> : null}
        </ItemCardMeta>
      ) : null}
      <ItemCardActions>
        <Button asChild variant="outline">
          <Link href={hrefs.project(slug, "evolution")} title={"Self-improvement for " + slug}>
            <BookOpenIcon /> Read it
          </Link>
        </Button>
        <DraftActions r={r} size="default" className="w-full justify-start sm:ml-auto sm:w-auto sm:justify-end" />
      </ItemCardActions>
    </ItemCard>
  );
}
