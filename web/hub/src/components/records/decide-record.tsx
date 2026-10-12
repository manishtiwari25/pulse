"use client";

import { useState } from "react";
import { BookOpenIcon, CheckIcon, ClockIcon, Loader2Icon, ScaleIcon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { showApiError } from "@/components/pulse/confirm";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardKind, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { ProjectLink, When } from "@/components/pulse/who";
import { tierWord } from "@/lib/words";
import { writes, type RecordDecision, type RecordRow } from "@/lib/api";
import { invalidate } from "@/lib/use-api";
import { sentence } from "@/lib/format";

// The human gates in the console (ADR-028). A Proposed ADR waits for the
// owner's yes, defer or no; each answer needs their words, which the hub
// records as the revision note (author `human`, "(via console)"). Nothing
// runs from the click itself: accepting unblocks the plan for the lead agent.

const TEXT: Record<RecordDecision, { verb: string; title: string; doing: string; done: string; label: string; placeholder: string; happens: string }> = {
  accept: {
    verb: "Accept",
    title: "Accept this decision?",
    doing: "Accepting the decision…",
    done: "Accepted. The lead agent may now start on its plan.",
    label: "Why do you accept it?",
    placeholder: "I accept because …",
    happens: "The decision becomes Accepted, with your words in its history. Its plan is unblocked, so the lead agent may start on it. Nothing runs from this click by itself.",
  },
  Deferred: {
    verb: "Defer",
    title: "Defer this decision?",
    doing: "Deferring the decision…",
    done: "Deferred. Agents won't build it for now.",
    label: "Why not now?",
    placeholder: "Later, because …",
    happens: "The decision is put on hold, with your words in its history. Agents won't build it, and it can be reopened later.",
  },
  Rejected: {
    verb: "Decline",
    title: "Decline this decision?",
    doing: "Declining the decision…",
    done: "Declined. The record keeps your reasoning.",
    label: "Why do you decline it?",
    placeholder: "No, because …",
    happens: "The decision is turned down, with your words in its history. The record stays, so the reasoning isn't lost.",
  },
};

const CHANGES = (p: string) => p.startsWith("records") || p === "org" || p.startsWith("events");

export const awaitsDecision = (r: RecordRow) => r.kind === "adr" && r.status === "Proposed";

function DecisionIcon({ action, className }: { action: RecordDecision; className?: string }) {
  if (action === "accept") return <CheckIcon className={className} />;
  if (action === "Deferred") return <ClockIcon className={className} />;
  return <XIcon className={className} />;
}

export function DecideRecordDialog({ record, action, open, onOpenChange, onDone }: { record: RecordRow; action: RecordDecision; open: boolean; onOpenChange: (o: boolean) => void; onDone?: (r: RecordRow) => void }) {
  // The busy flag lives here so the dialog cannot be dismissed mid-write; the
  // words live in the body, which mounts with the content, so every opening
  // starts from an empty field.
  const [busy, setBusy] = useState(false);
  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="sm:max-w-lg">
        <DecideBody record={record} action={action} busy={busy} setBusy={setBusy} onOpenChange={onOpenChange} onDone={onDone} />
      </DialogContent>
    </Dialog>
  );
}

function DecideBody({ record, action, busy, setBusy, onOpenChange, onDone }: { record: RecordRow; action: RecordDecision; busy: boolean; setBusy: (b: boolean) => void; onOpenChange: (o: boolean) => void; onDone?: (r: RecordRow) => void }) {
  const t = TEXT[action];
  const [words, setWords] = useState("");
  const trimmed = words.trim();
  const ready = trimmed.length > 0 && !busy;

  const submit = async () => {
    if (!ready) return;
    setBusy(true);
    const id = toast.loading(t.doing);
    try {
      const res = action === "accept" ? await writes.acceptRecord(record.id, trimmed) : await writes.setRecordStatus(record.id, action, trimmed);
      toast.success(t.done, { id, description: sentence(record.title) });
      invalidate(CHANGES);
      onOpenChange(false);
      onDone?.(res.record ?? record);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Couldn't " + t.verb.toLowerCase() + " this decision (" + record.id + ")");
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <DialogHeader>
        <DialogTitle>{t.title}</DialogTitle>
        <DialogDescription asChild>
          <div className="grid gap-0.5">
            <span className="text-[15px] font-medium text-foreground anywhere">{sentence(record.title)}</span>
            <span className="text-[13px]" title={"Record " + record.id}>
              {(record.project ? "In " + record.project + ", " : "") + record.id}
            </span>
          </div>
        </DialogDescription>
      </DialogHeader>
      <div className="grid gap-4">
        <div className="grid gap-1.5">
          <Label htmlFor="decide-words">{t.label}</Label>
          <Textarea
            id="decide-words"
            value={words}
            onChange={(e) => setWords(e.target.value)}
            placeholder={t.placeholder}
            rows={3}
            autoFocus
            required
            aria-describedby="decide-words-help"
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                void submit();
              }
            }}
          />
          <p id="decide-words-help" className="text-xs text-muted-foreground">
            A few words are required. They go into the decision&apos;s history as your note. Press ⌘ Enter to send.
          </p>
        </div>
        <div className="rounded-xl bg-muted/60 px-3.5 py-3">
          <div className="mb-0.5 text-[13px] font-medium text-foreground">What happens next</div>
          <p className="text-[13px] leading-relaxed text-muted-foreground">{t.happens}</p>
        </div>
      </div>
      <DialogFooter className="items-center gap-2">
        <Button variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
          Cancel
        </Button>
        <Button onClick={() => void submit()} disabled={!ready} variant={action === "Rejected" ? "destructive" : "default"}>
          {busy ? <Loader2Icon className="animate-spin" /> : <DecisionIcon action={action} />}
          {t.verb}
        </Button>
      </DialogFooter>
    </>
  );
}

// Accept / Defer / Decline for one Proposed ADR; nothing for any other
// record. Each opens the dialog with its action. `sm` by default for cards,
// `xs` for the sheet header.
export function RecordDecisionButtons({ record, size = "sm", onDone }: { record: RecordRow; size?: "xs" | "sm" | "default"; onDone?: (r: RecordRow) => void }) {
  // The action stays put while the dialog animates out, so the title does not flip.
  const [action, setAction] = useState<RecordDecision>("accept");
  const [open, setOpen] = useState(false);
  if (!awaitsDecision(record)) return null;
  const start = (a: RecordDecision) => {
    setAction(a);
    setOpen(true);
  };
  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      <Button size={size} variant="ghost" className="text-bad hover:bg-bad/10 hover:text-bad" onClick={() => start("Rejected")} aria-label={"Decline " + record.id}>
        <XIcon /> Decline
      </Button>
      <Button size={size} variant="outline" onClick={() => start("Deferred")} aria-label={"Defer " + record.id}>
        <ClockIcon /> Defer
      </Button>
      <Button size={size} onClick={() => start("accept")} aria-label={"Accept " + record.id}>
        <CheckIcon /> Accept
      </Button>
      <DecideRecordDialog record={record} action={action} open={open} onOpenChange={setOpen} onDone={onDone} />
    </span>
  );
}

// One Proposed ADR as an inbox card on the hub: a decision to make, what it
// is about, Read to open the record and the three answers in reach. The
// title opens the record too, like every record card. The record id rides in
// the tooltip and the accessible name, not in the owner's way.
export function ProposedRecordCard({ r, onOpen }: { r: RecordRow; onOpen: (r: RecordRow) => void }) {
  const areas = r.areas || [];
  const tier = r.tier !== null && r.tier !== undefined ? tierWord(r.tier) : null;
  return (
    <ItemCard highlight onOpen={() => onOpen(r)} ariaLabel={"Decision " + r.id + ": " + r.title}>
      <ItemCardHeader right={<When d={r.updated_at || r.date} />}>
        <ItemCardKind icon={ScaleIcon} tone="you">Decision to make</ItemCardKind>
        <ProjectLink slug={r.project} className="font-normal text-muted-foreground" />
      </ItemCardHeader>
      <ItemCardTitle chevron>{sentence(r.title)}</ItemCardTitle>
      <ItemCardBody clamp={2}>The agents wrote this decision and wait for yours. Read it, then accept, defer or decline it.</ItemCardBody>
      {areas.length || tier ? (
        <ItemCardMeta>
          {areas.length ? <span>{"About " + areas.join(", ")}</span> : null}
          {tier?.short ? <span title={tier.long}>{tier.short}</span> : null}
          <span title={"Record " + r.id}>{r.id}</span>
        </ItemCardMeta>
      ) : null}
      <ItemCardActions>
        <Button variant="outline" onClick={() => onOpen(r)} aria-label={"Read " + r.id}>
          <BookOpenIcon /> Read it
        </Button>
        <span className="flex w-full justify-start sm:ml-auto sm:w-auto sm:justify-end">
          <RecordDecisionButtons record={r} size="default" />
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}
