"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { BanIcon, MessagesSquareIcon, MoreHorizontalIcon, PencilIcon, PanelRightOpenIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { CardGrid, ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardTitle, excerpt } from "@/components/pulse/item-card";
import { EmptyState } from "@/components/pulse/panel";
import { Mono, ProjectLink, StatusBadge, ToneBadge, When, Who } from "@/components/pulse/who";
import { prioLabel } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { writes, type Task } from "@/lib/api";
import { cn } from "@/lib/utils";
import { AmendTaskDialog } from "./task-dialogs";

const isClosed = (t: Task) => t.status === "done" || t.status === "cancelled";
const STATUS_CUE: Record<Task["status"], string> = {
  open: "border-l-primary/50",
  planned: "border-l-muted-foreground/40",
  in_progress: "border-l-info",
  review: "border-l-warn",
  done: "border-l-ok/60",
  cancelled: "border-l-border",
};

// The owner's three moves on a task - talk to the lead, amend the brief,
// cancel - without opening the sheet first. Amend and Cancel confirm.
export function TaskQuickActions({ t, onOpen, size = "xs", showOpen }: { t: Task; onOpen?: () => void; size?: "xs" | "sm"; showOpen?: boolean }) {
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
      <Button size={size} variant="outline" asChild>
        <Link href={hrefs.chat(t.project, t.id)}>
          <MessagesSquareIcon /> Chat
        </Link>
      </Button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button size={size === "xs" ? "icon-xs" : "icon-sm"} variant="ghost" aria-label={"More actions for " + t.id}>
            <MoreHorizontalIcon />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-48">
          <DropdownMenuItem disabled={closed} onSelect={() => setAmend(true)}>
            <PencilIcon /> Amend the brief
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" disabled={closed} onSelect={() => setCancel(true)}>
            <BanIcon /> Cancel task
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {amend ? <AmendTaskDialog task={t} open={amend} onOpenChange={setAmend} /> : null}
      <ConfirmDialog
        open={cancel}
        onOpenChange={setCancel}
        title={"Cancel " + t.id + "?"}
        description="The task is marked cancelled and its queued job is withdrawn. A job already running finishes and the lead is told. This cannot be undone - open a new task instead."
        action="Cancel the task"
        destructive
        words={{ label: "Reason", placeholder: "e.g. superseded by TASK-012", required: true }}
        success={t.id + " cancelled."}
        onConfirm={(reason) => writes.cancelTask(t.id, reason)}
        invalidates={(p) => p.startsWith("tasks") || p.startsWith("events")}
      />
    </>
  );
}

export function TaskCard({ t, showProject, onOpen, active, compact }: { t: Task; showProject?: boolean; onOpen?: (t: Task) => void; active?: boolean; compact?: boolean }) {
  const router = useRouter();
  const prio = prioLabel(t.priority);
  // Always a button, never a link: the card carries links and buttons of its
  // own, and an anchor cannot nest them.
  const open = onOpen ? () => onOpen(t) : () => router.push(hrefs.task(t.project, t.id));
  const body = excerpt(t.brief, compact ? 120 : 220);
  return (
    <ItemCard onOpen={open} active={active} highlight={t.status === "review"} ariaLabel={t.id + " " + t.title} className={cn("border-l-2", STATUS_CUE[t.status])}>
      <ItemCardHeader
        right={
          <>
            {t.lead ? <Who row={{ agent: t.lead }} compact /> : <span className="text-muted-foreground" title="the runner keeps one lead per active project and wakes it for this task">no lead yet</span>}
            <When d={t.updated_at} />
          </>
        }
      >
        <Mono className="font-semibold">{t.id}</Mono>
        <StatusBadge status={t.status} />
        {prio === "urgent" || prio === "high" ? <ToneBadge tone={prio === "urgent" ? "bad" : "warn"}>{prio}</ToneBadge> : null}
        {showProject ? <ProjectLink slug={t.project} className="text-xs" /> : null}
      </ItemCardHeader>
      <ItemCardTitle chevron>{t.title}</ItemCardTitle>
      {body && !compact ? <ItemCardBody>{body}</ItemCardBody> : null}
      <ItemCardActions>
        <span className="text-[11.5px] text-muted-foreground anywhere">
          {"rev " + t.current_rev}
          {t.job_id ? (
            <>
              {" · job "}
              <Mono>{t.job_id}</Mono>
            </>
          ) : null}
          {t.status === "review" ? <span className="ml-1 text-warn">· the lead could not finish this without you - read the conversation</span> : null}
        </span>
        <span className="ml-auto flex items-center gap-1.5">
          <TaskQuickActions t={t} onOpen={open} />
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}

export function TaskCards({ tasks, showProject, onOpen, activeId, cols = 2, compact, empty, emptyTitle, emptyAction }: { tasks: Task[]; showProject?: boolean; onOpen?: (t: Task) => void; activeId?: string | null; cols?: 1 | 2 | 3; compact?: boolean; empty?: React.ReactNode; emptyTitle?: React.ReactNode; emptyAction?: React.ReactNode }) {
  if (!tasks.length)
    return (
      <EmptyState title={emptyTitle} action={emptyAction}>
        {empty ?? "No task here."}
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
