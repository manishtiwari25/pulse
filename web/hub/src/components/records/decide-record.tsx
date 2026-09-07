"use client";

import { useState } from "react";
import { BookOpenIcon, CheckIcon, ClockIcon, Loader2Icon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { showApiError } from "@/components/pulse/confirm";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { Mono, ProjectLink, StatusBadge, ToneBadge, When } from "@/components/pulse/who";
import { RecordKindBadge } from "@/components/records/records";
import { writes, type RecordDecision, type RecordRow } from "@/lib/api";
import { invalidate } from "@/lib/use-api";

// The human gates in the console (ADR-028). A Proposed ADR waits for the
// owner's yes, defer or no; each answer needs their words, which the hub
// records as the revision note (author `human`, "(via console)"). Nothing
// runs from the click itself: accepting unblocks the plan for the lead agent.

const TEXT: Record<RecordDecision, { verb: string; doing: string; done: string; placeholder: string; happens: string }> = {
  accept: {
    verb: "Accept",
    doing: "Accepting",
    done: "accepted",
    placeholder: "Approved because …",
    happens: "The decision becomes Accepted with your words in its history. Its plan is unblocked; the lead agent may start. Nothing runs from this click by itself.",
  },
  Deferred: {
    verb: "Defer",
    doing: "Deferring",
    done: "deferred",
    placeholder: "Later because …",
    happens: "Status becomes Deferred with your words. Agents will not build it; it can be reopened later.",
  },
  Rejected: {
    verb: "Decline",
    doing: "Declining",
    done: "declined",
    placeholder: "No because …",
    happens: "Status becomes Rejected with your words. The record stays for the reasoning.",
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
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-lg">
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
    const id = toast.loading(t.doing + " " + record.id + "…");
    try {
      const res = action === "accept" ? await writes.acceptRecord(record.id, trimmed) : await writes.setRecordStatus(record.id, action, trimmed);
      toast.success(record.id + " " + t.done, { id });
      invalidate(CHANGES);
      onOpenChange(false);
      onDone?.(res.record ?? record);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Could not " + t.verb.toLowerCase() + " " + record.id);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <DialogHeader>
        <DialogTitle>{t.verb + " " + record.id}</DialogTitle>
        <DialogDescription className="anywhere">
          {record.title}
          {record.project ? (
            <>
              {" · "}
              <Mono>{record.project}</Mono>
            </>
          ) : null}
        </DialogDescription>
      </DialogHeader>
      <div className="grid gap-3">
        <div className="grid gap-1.5">
          <Label htmlFor="decide-words">Your words</Label>
          <Textarea
            id="decide-words"
            value={words}
            onChange={(e) => setWords(e.target.value)}
            placeholder={t.placeholder}
            rows={3}
            autoFocus
            required
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                void submit();
              }
            }}
          />
          <p className="text-xs text-muted-foreground">Required - recorded in the record&apos;s history as your revision note. ⌘↩ submits.</p>
        </div>
        <div className="rounded-md border bg-muted/30 px-3 py-2">
          <div className="mb-1 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">What happens</div>
          <p className="text-[12.5px] leading-snug text-muted-foreground">{t.happens}</p>
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
export function RecordDecisionButtons({ record, size = "sm", onDone }: { record: RecordRow; size?: "xs" | "sm"; onDone?: (r: RecordRow) => void }) {
  // The action stays put while the dialog animates out, so the title does not flip.
  const [action, setAction] = useState<RecordDecision>("accept");
  const [open, setOpen] = useState(false);
  if (!awaitsDecision(record)) return null;
  const start = (a: RecordDecision) => {
    setAction(a);
    setOpen(true);
  };
  return (
    <span className="inline-flex flex-wrap items-center gap-1.5">
      <Button size={size} onClick={() => start("accept")} aria-label={"Accept " + record.id}>
        <CheckIcon /> Accept
      </Button>
      <Button size={size} variant="outline" onClick={() => start("Deferred")} aria-label={"Defer " + record.id}>
        <ClockIcon /> Defer
      </Button>
      <Button size={size} variant="ghost" className="text-bad hover:text-bad" onClick={() => start("Rejected")} aria-label={"Decline " + record.id}>
        <XIcon /> Decline
      </Button>
      <DecideRecordDialog record={record} action={action} open={open} onOpenChange={setOpen} onDone={onDone} />
    </span>
  );
}

// One Proposed ADR as an inbox card on the hub: what it is and where, one
// line saying what it waits for, Read to open the record and the three
// answers in reach. The title opens the record too, like every record card.
export function ProposedRecordCard({ r, onOpen }: { r: RecordRow; onOpen: (r: RecordRow) => void }) {
  const areas = r.areas || [];
  return (
    <ItemCard highlight onOpen={() => onOpen(r)} ariaLabel={r.id + " " + r.title}>
      <ItemCardHeader>
        <RecordKindBadge kind={r.kind} />
        <Mono className="font-semibold">{r.id}</Mono>
        <StatusBadge status={r.status} />
      </ItemCardHeader>
      <ItemCardTitle chevron>{r.title}</ItemCardTitle>
      <ItemCardMeta>
        <ProjectLink slug={r.project} />
        {r.tier !== null && r.tier !== undefined ? <ToneBadge tone="neutral">{"Tier " + r.tier}</ToneBadge> : null}
        {areas.length ? <span>{areas.join(", ")}</span> : null}
        <When d={r.updated_at || r.date} />
      </ItemCardMeta>
      <ItemCardBody clamp={1}>Waiting for your yes, defer or no</ItemCardBody>
      <ItemCardActions>
        <Button variant="ghost" size="xs" className="text-muted-foreground" onClick={() => onOpen(r)} aria-label={"Read " + r.id}>
          <BookOpenIcon /> Read
        </Button>
        <span className="ml-auto shrink-0">
          <RecordDecisionButtons record={r} />
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}
