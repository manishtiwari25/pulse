"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { ArrowUpIcon, BanIcon, CircleDashedIcon, MessageCircleQuestionIcon, MessagesSquareIcon, MoreHorizontalIcon, PanelRightOpenIcon, PencilIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { CardGrid, ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardTitle, excerpt } from "@/components/pulse/item-card";
import { EmptyState } from "@/components/pulse/panel";
import { ProjectLink, StatusBadge, ToneBadge } from "@/components/pulse/who";
import { priorityWord } from "@/lib/words";
import { hrefs } from "@/lib/nav";
import { taskProgress, writes, type Task } from "@/lib/api";
import { cn } from "@/lib/utils";
import { AmendTaskDialog } from "./task-dialogs";
import { Ago, idleLine, TaskWorkingLine } from "./task-work";

const isClosed = (t: Task) => t.status === "done" || t.status === "cancelled";

// The cancel confirmation, shared by the card menu and the task sheet.
export function CancelTaskDialog({ t, open, onOpenChange }: { t: Task; open: boolean; onOpenChange: (o: boolean) => void }) {
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Cancel this task?"
      description={
        <>
          <span className="block font-medium text-foreground anywhere">{t.title}</span>
          <span className="mt-2 block">The lead agent stops working on it and any step still waiting to start is withdrawn. A step that is already running finishes first. You can&apos;t undo this; give a new task instead.</span>
        </>
      }
      action="Cancel the task"
      dismiss="Keep the task"
      destructive
      words={{ label: "Why are you cancelling it?", placeholder: "For example: no longer needed, or replaced by TASK-012", required: true }}
      success="Task cancelled. The lead agent has been told."
      onConfirm={(reason) => writes.cancelTask(t.id, reason)}
      invalidates={(p) => p.startsWith("tasks") || p.startsWith("events")}
    />
  );
}

// The owner's three moves on a task - talk to the lead, change the task,
// cancel it - without opening the sheet first. Edit and Cancel confirm.
// `chat={false}` drops the Chat button when the card already offers one.
export function TaskQuickActions({ t, onOpen, size = "xs", showOpen, chat = true }: { t: Task; onOpen?: () => void; size?: "xs" | "sm"; showOpen?: boolean; chat?: boolean }) {
  const [amend, setAmend] = useState(false);
  const [cancel, setCancel] = useState(false);
  const closed = isClosed(t);
  return (
    <>
      {showOpen && onOpen ? (
        <Button size={size} variant="outline" onClick={onOpen}>
          <PanelRightOpenIcon /> Open
        </Button>
      ) : null}
      {chat ? (
        <Button size={size} variant="outline" asChild>
          <Link href={hrefs.chat(t.project, t.id)} aria-label={"Chat with the lead agent about " + t.title}>
            <MessagesSquareIcon /> Chat
          </Link>
        </Button>
      ) : null}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button size={size === "xs" ? "icon-xs" : "icon-sm"} variant="ghost" aria-label={"More actions for " + t.title}>
            <MoreHorizontalIcon />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-48">
          <DropdownMenuItem disabled={closed} onSelect={() => setAmend(true)}>
            <PencilIcon /> Edit the task
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" disabled={closed} onSelect={() => setCancel(true)}>
            <BanIcon /> Cancel the task
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {amend ? <AmendTaskDialog task={t} open={amend} onOpenChange={setAmend} /> : null}
      <CancelTaskDialog t={t} open={cancel} onOpenChange={setCancel} />
    </>
  );
}

// "1 of 2 steps done" over a calm bar; a failed step is said in words.
export function TaskProgressBar({ t, className }: { t: Task; className?: string }) {
  const p = taskProgress(t);
  if (!p.total) return null;
  return (
    <div className={cn("grid gap-1.5", className)}>
      <div className="flex flex-wrap items-center justify-between gap-x-3 text-[13px] text-muted-foreground">
        <span>{p.done + " of " + p.total + (p.total === 1 ? " step" : " steps") + " done"}</span>
        {p.failed ? <span className="text-bad">{p.failed === 1 ? "1 step failed" : p.failed + " steps failed"}</span> : null}
      </div>
      <Progress value={Math.min(100, (100 * p.done) / Math.max(1, p.total))} className="h-1.5" aria-label={p.done + " of " + p.total + " steps done"} />
    </div>
  );
}

// The ask on a task in review: the lead got stuck and waits in the chat,
// where its last message offers answers to tap.
// `onChat` opens the conversation in place (the task sheet) instead of
// following the link to the project's Work tab.
export function TaskNeedsYou({ t, className, onChat }: { t: Task; className?: string; onChat?: () => void }) {
  return (
    <div className={cn("surface-you flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2 rounded-xl border px-3.5 py-3", className)}>
      <p className="flex min-w-0 flex-1 basis-52 items-start gap-2 text-sm leading-relaxed text-foreground">
        <MessageCircleQuestionIcon className="mt-0.5 size-4 shrink-0 text-you-ink" aria-hidden />
        <span className="min-w-0 anywhere">The lead agent stopped and waits for your decision. Pick an answer in the chat, or write one.</span>
      </p>
      {onChat ? (
        <Button size="sm" onClick={onChat}>
          <MessagesSquareIcon /> Open the chat
        </Button>
      ) : (
        <Button size="sm" asChild>
          <Link href={hrefs.chat(t.project, t.id)}>
            <MessagesSquareIcon /> Open the chat
          </Link>
        </Button>
      )}
    </div>
  );
}

export function TaskCard({ t, showProject, onOpen, active, compact }: { t: Task; showProject?: boolean; onOpen?: (t: Task) => void; active?: boolean; compact?: boolean }) {
  const router = useRouter();
  const urgent = t.priority >= 30;
  const review = t.status === "review";
  const idle = idleLine(t);
  // Always a button, never a link: the card carries links and buttons of its
  // own, and an anchor cannot nest them.
  const open = onOpen ? () => onOpen(t) : () => router.push(hrefs.task(t.project, t.id));
  // The title and brief are the owner's own words: shown as typed.
  const body = excerpt(t.brief, compact ? 120 : 200);
  return (
    <ItemCard onOpen={open} active={active} highlight={review} ariaLabel={"Task: " + t.title} className="gap-3">
      <ItemCardTitle chevron>{t.title}</ItemCardTitle>
      <ItemCardHeader className="-mt-1">
        <StatusBadge status={t.status} />
        {showProject ? <ProjectLink slug={t.project} className="font-normal text-muted-foreground" /> : null}
        {urgent ? <ToneBadge tone={t.priority >= 50 ? "bad" : "neutral"} title={"Priority " + t.priority}><ArrowUpIcon className="size-3" aria-hidden />{priorityWord(t.priority)}</ToneBadge> : null}
      </ItemCardHeader>
      {body && !compact ? <ItemCardBody>{body}</ItemCardBody> : null}
      <TaskProgressBar t={t} />
      <TaskWorkingLine t={t} compact={compact} />
      {idle ? (
        <p className="flex min-w-0 items-start gap-2 text-[13px] leading-relaxed text-muted-foreground">
          <CircleDashedIcon className="mt-[3px] size-3.5 shrink-0" aria-hidden />
          <span className="min-w-0 anywhere">{idle}</span>
        </p>
      ) : null}
      {review ? <TaskNeedsYou t={t} /> : null}
      <ItemCardActions className="pt-1">
        <span className="min-w-0 text-[13px] text-muted-foreground" title={t.id + (t.current_rev > 1 ? ", version " + t.current_rev : "")}>
          {"Updated "}
          <Ago d={t.updated_at} />
        </span>
        <span className="ml-auto flex items-center gap-1.5">
          <TaskQuickActions t={t} onOpen={open} chat={!review} />
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}

export function TaskCards({ tasks, showProject, onOpen, activeId, cols = 2, compact, empty, emptyTitle, emptyAction }: { tasks: Task[]; showProject?: boolean; onOpen?: (t: Task) => void; activeId?: string | null; cols?: 1 | 2 | 3; compact?: boolean; empty?: React.ReactNode; emptyTitle?: React.ReactNode; emptyAction?: React.ReactNode }) {
  if (!tasks.length)
    return (
      <EmptyState title={emptyTitle} action={emptyAction}>
        {empty ?? "No tasks here."}
      </EmptyState>
    );
  return (
    <CardGrid cols={cols}>
      {tasks.map((t) => (
        <TaskCard key={t.id} t={t} showProject={showProject} onOpen={onOpen} active={activeId === t.id} compact={compact} />
      ))}
    </CardGrid>
  );
}
