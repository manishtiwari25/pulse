"use client";

import { useState } from "react";
import { ArrowRightIcon, CheckIcon, LightbulbIcon, Loader2Icon, MoreHorizontalIcon, UndoIcon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ConfirmDialog, showApiError } from "@/components/pulse/confirm";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardKind, ItemCardMeta, ItemCardTitle, SectionHeading, excerpt } from "@/components/pulse/item-card";
import { Disclosure } from "@/components/pulse/panel";
import { Markdown } from "@/components/pulse/markdown";
import { ProjectLink, StatusBadge, When } from "@/components/pulse/who";
import { priorityWord, statusWord } from "@/lib/words";
import { plural } from "@/lib/format";
import { cn } from "@/lib/utils";
import { isOpenWorkSuggestion, writes, type Task, type WorkSuggestion, type WorkSuggestionsPayload } from "@/lib/api";
import { invalidate, useApi } from "@/lib/use-api";
import { BriefField, UrgencyField } from "./task-dialogs";

// Suggested work: what the agents (or you) think the project should do
// next. A different agent reviews each suggestion; an accepted one becomes
// a task, a declined one keeps its reason, and one an agent could not
// decide waits for you. The Work tab shows them above the tasks; the ones
// waiting for you also sit on For you.

const who = (name: string) => (name === "human" ? "you" : name);
const Who = (name: string) => (name === "human" ? "You" : name);
const refresh = (p: string) => p.startsWith("tasks") || p.startsWith("events");
// Those waiting for you first, then the newest.
const byUrgency = (a: WorkSuggestion, b: WorkSuggestion) => (a.status === "owner" ? 0 : 1) - (b.status === "owner" ? 0 : 1) || (a.updated_at < b.updated_at ? 1 : a.updated_at > b.updated_at ? -1 : 0);

export function SuggestedWorkPanel({ slug, onOpenTask, className }: { slug: string; onOpenTask?: (id: string) => void; className?: string }) {
  const q = useApi<WorkSuggestionsPayload>("tasks/suggested?project=" + encodeURIComponent(slug), { refresh: 10000 });
  const [decided, setDecided] = useState(false);
  // An older hub has no such route; the Work tab reads the same without it.
  if (!q.data) return null;
  const all = q.data.suggestions;
  const open = all.filter(isOpenWorkSuggestion).sort(byUrgency);
  const closed = all.filter((s) => !isOpenWorkSuggestion(s));
  const yours = open.filter((s) => s.status === "owner").length;
  return (
    <section aria-label="Suggested work" className={cn("min-w-0", className)}>
      <SectionHeading
        as="h3"
        icon={LightbulbIcon}
        count={open.length || undefined}
        note={yours ? plural(yours, "suggestion") + " could not be settled by the agents and " + (yours === 1 ? "waits" : "wait") + " for you." : "What the agents think the project needs next. Another agent reviews each one before it becomes a task."}
        action={<SuggestWorkButton project={slug} />}
      >
        Suggested next
      </SectionHeading>
      {open.length ? (
        <ul className="divide-y divide-border/70 rounded-2xl border bg-card px-4 sm:px-5">
          {open.map((s) => (
            <SuggestionRow key={s.id} s={s} onOpenTask={onOpenTask} />
          ))}
        </ul>
      ) : (
        <p className="rounded-2xl border border-dashed px-4 py-3 text-sm leading-relaxed text-muted-foreground">
          Nothing is waiting. Agents add suggestions as they work; press <b>Suggest work</b> to add one of your own, and another agent weighs it.
        </p>
      )}
      {closed.length ? (
        <Disclosure summary={decided ? "Hide the decided ones" : "Show " + plural(closed.length, "decided suggestion")} open={decided} onToggle={setDecided} className="mt-2">
          <ul className="divide-y divide-border/70 rounded-2xl border bg-card px-4 sm:px-5">
            {closed
              .slice()
              .sort(byUrgency)
              .map((s) => (
                <SuggestionRow key={s.id} s={s} onOpenTask={onOpenTask} />
              ))}
          </ul>
        </Disclosure>
      ) : null}
    </section>
  );
}

// One suggestion as a row: state, title, who and when, the why a click away,
// the verdict once there is one, and the owner's moves.
function SuggestionRow({ s, onOpenTask }: { s: WorkSuggestion; onOpenTask?: (id: string) => void }) {
  const open = isOpenWorkSuggestion(s);
  const yours = s.status === "owner";
  return (
    <li className={cn("grid gap-1.5 py-3", yours && "-mx-4 px-4 sm:-mx-5 sm:px-5 bg-you-soft/40")}>
      <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
        <StatusBadge status={s.status} />
        <h4 className="min-w-0 flex-1 text-sm font-medium leading-snug text-foreground anywhere">{s.title}</h4>
        {open ? <RowActions s={s} onOpenTask={onOpenTask} /> : null}
      </div>
      <p className="text-[13px] leading-relaxed text-muted-foreground anywhere">
        <span className="font-mono text-xs">{s.id}</span>
        {" · suggested by " + who(s.suggested_by) + " "}
        <When d={s.created_at} />
        {s.priority >= 30 ? " · " + priorityWord(s.priority).toLowerCase() : ""}
        {s.status === "reviewing" ? <> · being reviewed{s.job_agent ? " by " + s.job_agent : ""}</> : null}
        {s.status === "suggested" && s.reviews > 0 ? " · " + plural(s.reviews, "review") + " ended without a verdict" : ""}
      </p>
      {s.brief.trim() ? (
        <Disclosure summary="Why">
          <div className="rounded-lg border bg-background/60 px-3 py-2 text-sm">
            <Markdown source={s.brief} />
          </div>
        </Disclosure>
      ) : null}
      <VerdictLine s={s} onOpenTask={onOpenTask} />
    </li>
  );
}

function VerdictLine({ s, onOpenTask }: { s: WorkSuggestion; onOpenTask?: (id: string) => void }) {
  if (s.status === "accepted")
    return (
      <p className="flex min-w-0 flex-wrap items-center gap-x-2 text-[13px] leading-relaxed text-muted-foreground anywhere">
        <span>{"Accepted by " + who(s.reviewer) + (s.verdict ? ": " + s.verdict : ".")}</span>
        {s.task_id ? (
          onOpenTask ? (
            <button type="button" className="inline-flex items-center gap-1 font-medium text-primary underline-offset-2 hover:underline" onClick={() => onOpenTask(s.task_id)}>
              Now {s.task_id} <ArrowRightIcon className="size-3.5" aria-hidden />
            </button>
          ) : (
            <span className="font-mono text-xs">{s.task_id}</span>
          )
        ) : null}
      </p>
    );
  if (s.status === "declined") return <p className="text-[13px] leading-relaxed text-muted-foreground anywhere">{"Declined by " + who(s.reviewer) + (s.verdict ? ": " + s.verdict : ".")}</p>;
  if (s.status === "withdrawn") return <p className="text-[13px] leading-relaxed text-muted-foreground anywhere">{"Withdrawn" + (s.verdict ? ": " + s.verdict : ".")}</p>;
  if (s.status === "owner")
    return (
      <p className="text-[13px] leading-relaxed text-foreground anywhere">
        <span className="font-medium">{s.reviewer ? Who(s.reviewer) + " handed it to you" : "Your call"}</span>
        {s.verdict ? ": " + s.verdict : "."}
      </p>
    );
  return null;
}

// Accept and Decline on the row for the ones that wait for you; for the
// rest they sit behind the menu, with Withdraw on your own suggestions.
function RowActions({ s, onOpenTask }: { s: WorkSuggestion; onOpenTask?: (id: string) => void }) {
  const [accept, setAccept] = useState(false);
  const [decline, setDecline] = useState(false);
  const [withdraw, setWithdraw] = useState(false);
  const yours = s.status === "owner";
  return (
    <span className="flex shrink-0 items-center gap-1.5">
      {yours ? (
        <>
          <Button size="xs" onClick={() => setAccept(true)}>
            <CheckIcon /> Accept
          </Button>
          <Button size="xs" variant="outline" onClick={() => setDecline(true)}>
            <XIcon /> Decline
          </Button>
        </>
      ) : null}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button size="icon-xs" variant="ghost" aria-label={"More actions for " + s.title}>
            <MoreHorizontalIcon />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          {!yours ? (
            <>
              <DropdownMenuItem onSelect={() => setAccept(true)}>
                <CheckIcon /> Accept it now
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => setDecline(true)}>
                <XIcon /> Decline it
              </DropdownMenuItem>
            </>
          ) : null}
          {s.suggested_by === "human" ? (
            <>
              {!yours ? <DropdownMenuSeparator /> : null}
              <DropdownMenuItem onSelect={() => setWithdraw(true)}>
                <UndoIcon /> Withdraw my suggestion
              </DropdownMenuItem>
            </>
          ) : null}
          {yours && s.suggested_by !== "human" ? <DropdownMenuItem disabled>Accept or decline it above</DropdownMenuItem> : null}
        </DropdownMenuContent>
      </DropdownMenu>
      <AcceptSuggestionDialog s={s} open={accept} onOpenChange={setAccept} onAccepted={(t) => onOpenTask?.(t.id)} />
      <DeclineSuggestionDialog s={s} open={decline} onOpenChange={setDecline} />
      <WithdrawSuggestionDialog s={s} open={withdraw} onOpenChange={setWithdraw} />
    </span>
  );
}

// The For-you card: a suggestion the agents could not settle. Accept makes
// it a task; Decline closes it with your reason.
export function WorkSuggestionCard({ s, slug, onOpenTask }: { s: WorkSuggestion; slug?: string; onOpenTask?: (id: string) => void }) {
  const [accept, setAccept] = useState(false);
  const [decline, setDecline] = useState(false);
  const why = excerpt(s.brief, 260);
  return (
    <ItemCard highlight ariaLabel={"Suggested work for you: " + s.title}>
      <ItemCardHeader right={<When d={s.reviewed_at || s.updated_at} />}>
        <ItemCardKind icon={LightbulbIcon} tone="you">
          Suggested work
        </ItemCardKind>
        {!slug || s.project !== slug ? <ProjectLink slug={s.project} className="font-normal text-muted-foreground" /> : null}
      </ItemCardHeader>
      <ItemCardTitle>{s.title}</ItemCardTitle>
      <ItemCardBody clamp={0}>
        {why ? <p className="line-clamp-3">{why}</p> : null}
        <p className={cn("text-foreground", why && "mt-1.5")}>
          <span className="font-medium">{s.reviewer ? Who(s.reviewer) + " handed it to you" : "The agents could not settle it"}</span>
          {s.verdict ? ": " + s.verdict : "."}
        </p>
      </ItemCardBody>
      <ItemCardMeta>
        <span>{"Suggested by " + who(s.suggested_by) + " as " + s.id}</span>
        {s.priority >= 30 ? <span>{priorityWord(s.priority)}</span> : null}
      </ItemCardMeta>
      <ItemCardActions>
        <Button size="sm" onClick={() => setAccept(true)}>
          <CheckIcon /> Accept, make it a task
        </Button>
        <Button size="sm" variant="outline" onClick={() => setDecline(true)}>
          <XIcon /> Decline
        </Button>
      </ItemCardActions>
      <AcceptSuggestionDialog s={s} open={accept} onOpenChange={setAccept} onAccepted={(t) => onOpenTask?.(t.id)} />
      <DeclineSuggestionDialog s={s} open={decline} onOpenChange={setDecline} />
    </ItemCard>
  );
}

export function SuggestWorkButton({ project, size = "sm", variant = "outline" }: { project: string; size?: "xs" | "sm" | "default"; variant?: "default" | "outline" }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size={size} variant={variant} onClick={() => setOpen(true)}>
        <LightbulbIcon /> Suggest work
      </Button>
      <SuggestWorkDialog project={project} open={open} onOpenChange={setOpen} />
    </>
  );
}

// "Suggest work" is the softer sibling of "New task": you say what and
// why, and an agent weighs it before anybody starts. A New task starts now.
export function SuggestWorkDialog({ project, open, onOpenChange }: { project: string; open: boolean; onOpenChange: (o: boolean) => void }) {
  const [title, setTitle] = useState("");
  const [brief, setBrief] = useState("");
  const [priority, setPriority] = useState("10");
  const [busy, setBusy] = useState(false);
  const valid = title.trim().length >= 3;
  const submit = async () => {
    setBusy(true);
    const id = toast.loading("Suggesting…");
    try {
      const r = await writes.suggestWork({ project, title: title.trim(), brief: brief.trim(), priority: Number(priority) });
      toast.success("Suggested. An agent reviews it and an accepted one becomes a task; you'll see the verdict here.", { id, description: r.suggestion.id });
      invalidate(refresh);
      onOpenChange(false);
      setTitle("");
      setBrief("");
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "The suggestion wasn't saved");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Suggest work for {project}</DialogTitle>
          <DialogDescription>Not sure it is worth doing yet? Say what and why. A different agent weighs it against what the project needs; if it agrees, the suggestion becomes a task. To start work right away, give a task instead.</DialogDescription>
        </DialogHeader>
        <div className="grid gap-4">
          <div className="grid gap-1.5">
            <Label htmlFor="sw-title">What should the project do next?</Label>
            <Input id="sw-title" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="For example: Cache the feed so the home page loads once" maxLength={200} autoFocus />
            <p className="text-xs text-muted-foreground">One short line. It becomes the task&apos;s name if accepted.</p>
          </div>
          <BriefField id="sw-brief" value={brief} onChange={setBrief} rows={6} label="Why" hint="A failing check, something you do by hand, a promise the README makes. The reviewing agent judges this; Markdown works." placeholder={"What you noticed and why it matters.\n\nHow you'd know it's done:\n- the check that proves it"} />
          <UrgencyField id="sw-urgency" value={priority} onChange={setPriority} />
        </div>
        <DialogFooter className="items-center gap-2">
          <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={() => void submit()} disabled={busy || !valid} title={valid ? undefined : "Say what the project should do (3 letters or more)."}>
            {busy ? <Loader2Icon className="animate-spin" /> : null}
            Suggest it
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// Accept: the suggestion becomes a task the lead plans. The title and the
// urgency can be sharpened on the way; the brief travels as it is, with a
// line saying where it came from.
export function AcceptSuggestionDialog({ s, open, onOpenChange, onAccepted }: { s: WorkSuggestion; open: boolean; onOpenChange: (o: boolean) => void; onAccepted?: (t: Task) => void }) {
  const [title, setTitle] = useState(s.title);
  const [reason, setReason] = useState("");
  const [priority, setPriority] = useState(String(s.priority));
  const [busy, setBusy] = useState(false);
  const valid = title.trim().length >= 3;
  const submit = async () => {
    setBusy(true);
    const id = toast.loading("Accepting…");
    try {
      const input: Parameters<typeof writes.acceptSuggestion>[1] = { reason: reason.trim() };
      if (title.trim() !== s.title) input.title = title.trim();
      if (Number(priority) !== s.priority) input.priority = Number(priority);
      const r = await writes.acceptSuggestion(s.id, input);
      toast.success("Accepted. The lead agent plans it like any task.", { id, description: r.task ? r.task.id + ": " + r.task.title : s.id });
      invalidate(refresh);
      onOpenChange(false);
      if (r.task) onAccepted?.(r.task);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "The suggestion wasn't accepted");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Make it a task?</DialogTitle>
          <DialogDescription>{"Suggested by " + who(s.suggested_by) + " as " + s.id + ". The task keeps the suggestion's details and a line saying where it came from; the lead agent plans it next."}</DialogDescription>
        </DialogHeader>
        <div className="grid gap-4">
          <div className="grid gap-1.5">
            <Label htmlFor="as-title">The task&apos;s name</Label>
            <Input id="as-title" value={title} onChange={(e) => setTitle(e.target.value)} maxLength={200} />
            <p className="text-xs text-muted-foreground">Sharpen it if you like; the suggestion&apos;s own title stays on record.</p>
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="as-reason">Why accept it? (optional)</Label>
            <Input id="as-reason" value={reason} onChange={(e) => setReason(e.target.value)} placeholder="For example: measured it, 400ms on every load" maxLength={300} />
            <p className="text-xs text-muted-foreground">The suggester and the lead agent read this.</p>
          </div>
          <UrgencyField id="as-urgency" value={priority} onChange={setPriority} />
        </div>
        <DialogFooter className="items-center gap-2">
          <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={() => void submit()} disabled={busy || !valid}>
            {busy ? <Loader2Icon className="animate-spin" /> : <CheckIcon />}
            Accept, make it a task
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function DeclineSuggestionDialog({ s, open, onOpenChange }: { s: WorkSuggestion; open: boolean; onOpenChange: (o: boolean) => void }) {
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Decline this suggestion?"
      description={
        <>
          <span className="block font-medium text-foreground anywhere">{s.title}</span>
          <span className="mt-2 block">{"It closes with your reason, which " + who(s.suggested_by) + " can read. Nothing is started. The same title can be suggested again later with new evidence."}</span>
        </>
      }
      action="Decline it"
      dismiss="Keep it open"
      words={{ label: "Why not?", placeholder: "For example: already covered by TASK-012, or not worth the agents' time this month", required: s.suggested_by !== "human" }}
      success="Declined. The suggester has been told."
      onConfirm={(reason) => writes.declineSuggestion(s.id, reason)}
      invalidates={refresh}
    />
  );
}

export function WithdrawSuggestionDialog({ s, open, onOpenChange }: { s: WorkSuggestion; open: boolean; onOpenChange: (o: boolean) => void }) {
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Withdraw your suggestion?"
      description={
        <>
          <span className="block font-medium text-foreground anywhere">{s.title}</span>
          <span className="mt-2 block">{statusWord(s.status) === "Being reviewed" ? "The agent reviewing it stops; nothing else changes." : "It leaves the list; nothing else changes."}</span>
        </>
      }
      action="Withdraw it"
      dismiss="Keep it"
      words={{ label: "A word on why (optional)", placeholder: "For example: found it is already done" }}
      success="Withdrawn."
      onConfirm={(reason) => writes.withdrawSuggestion(s.id, reason)}
      invalidates={refresh}
    />
  );
}
