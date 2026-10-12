"use client";

import { useRouter } from "next/navigation";
import { useCallback, useEffect, useMemo, useRef, useSyncExternalStore } from "react";
import { ArrowDownIcon, ArrowUpIcon, BookOpenIcon, BotIcon, CornerDownLeftIcon, FolderIcon, HistoryIcon, HomeIcon, InboxIcon, ListTodoIcon, MessagesSquareIcon, PlugIcon, ScrollTextIcon, SearchIcon, ServerIcon, SettingsIcon, SparklesIcon, VoteIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator, CommandShortcut } from "@/components/ui/command";
import { hrefs, PROJECT_TABS, type ProjectCanonicalTab } from "@/lib/nav";
import { useApi } from "@/lib/use-api";
import { recordKindWord } from "@/lib/words";
import { cn } from "@/lib/utils";
import type { Org, RecordsPayload, TasksPayload } from "@/lib/api";

const itemIcon = "size-4 text-muted-foreground";
const groupStyle = "**:[[cmdk-group-heading]]:font-medium **:[[cmdk-group-heading]]:normal-case";
const itemStyle = "min-h-10 gap-3 rounded-lg px-2.5 py-2";
const shortcutStyle = "max-w-24 shrink-0 truncate tracking-normal sm:max-w-36";
const keyStyle = "inline-flex h-5 min-w-5 items-center justify-center rounded border border-border/70 bg-background px-1 font-mono text-[10px]";
const projectTabIcon: Record<ProjectCanonicalTab, typeof HomeIcon> = {
  overview: HomeIcon,
  "for-you": InboxIcon,
  work: ListTodoIcon,
  board: MessagesSquareIcon,
  evolution: SparklesIcon,
  history: HistoryIcon,
};
const projectTabs = PROJECT_TABS as { id: ProjectCanonicalTab; label: string }[];

// One palette per page. The host (mounted once by the shell) owns the dialog
// and the ⌘K / Ctrl-K shortcut; any number of triggers - the sidebar's search
// field, the phone header's magnifier - open it. Two hosts would each toggle
// on the same key press and close one another.
const listeners = new Set<() => void>();
let isOpen = false;
let opener: HTMLElement | null = null;
function setPaletteOpen(open: boolean, from?: HTMLElement | null) {
  if (open && !isOpen) opener = from ?? (document.activeElement instanceof HTMLElement && document.activeElement !== document.body ? document.activeElement : null);
  isOpen = open;
  listeners.forEach((l) => l());
}
function subscribe(cb: () => void) {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}
const usePaletteOpen = () => useSyncExternalStore(subscribe, () => isOpen, () => false);

// The trigger: `sidebar` is the full-width search field in the desktop
// sidebar; `icon` is the magnifier in the phone header.
export function CommandPalette({ variant = "sidebar" }: { variant?: "sidebar" | "icon" }) {
  const open = usePaletteOpen();
  return (
    <Button
      data-palette-trigger=""
      variant={variant === "icon" ? "ghost" : "outline"}
      size={variant === "icon" ? "icon" : "default"}
      className={cn(variant === "icon" ? "size-10 text-muted-foreground" : "h-9 w-full justify-start gap-2 px-2.5 font-normal text-muted-foreground")}
      onClick={(event) => setPaletteOpen(true, event.currentTarget)}
      aria-label="Search (Control or Command K)"
      aria-haspopup="dialog"
      aria-expanded={open}
      aria-keyshortcuts="Control+k Meta+k"
    >
      <SearchIcon aria-hidden="true" className="size-4" />
      {variant === "sidebar" ? (
        <>
          <span>Search</span>
          <kbd aria-hidden="true" className="pointer-events-none ml-auto inline-flex shrink-0 items-center rounded border bg-muted px-1.5 py-0.5 font-mono text-[10.5px] text-muted-foreground">⌘K</kbd>
        </>
      ) : null}
    </Button>
  );
}

// ⌘K / Ctrl-K: jump to a project (and its tabs), a task, a record, or a page.
export function CommandPaletteHost() {
  const open = usePaletteOpen();
  const setOpen = useCallback((o: boolean) => setPaletteOpen(o), []);
  const router = useRouter();
  const { data: org, error: orgError, loading: orgLoading } = useApi<Org>("org", { refresh: 60000 });
  const { data: records, error: recordsError, loading: recordsLoading } = useApi<RecordsPayload>(open ? "records" : null, { refresh: false });
  const { data: tasks, error: tasksError, loading: tasksLoading } = useApi<TasksPayload>(open ? "tasks" : null, { refresh: false });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.isComposing && !e.repeat && (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen(!isOpen);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // On close, back to whatever opened it, or to the trigger that is on screen.
  const wasOpen = useRef(false);
  useEffect(() => {
    if (open) {
      wasOpen.current = true;
      return;
    }
    if (!wasOpen.current) return;
    wasOpen.current = false;
    const target = opener;
    opener = null;
    const visibleTrigger = [...document.querySelectorAll<HTMLElement>("[data-palette-trigger]")].find((el) => el.offsetParent !== null);
    (target?.isConnected ? target : visibleTrigger)?.focus({ preventScroll: true });
  }, [open]);

  const projects = useMemo(() => [...(org?.projects ?? []), ...(org?.commons ? [org.commons] : [])], [org]);
  const loading = orgLoading || recordsLoading || tasksLoading;
  const errors = [
    { source: "Projects", error: orgError },
    { source: "Tasks", error: tasksError },
    { source: "Records", error: recordsError },
  ].filter((source) => source.error);
  const go = (href: string) => {
    setOpen(false);
    router.push(href);
  };

  return (
    <>
      <CommandDialog open={open} onOpenChange={setOpen} title="Search" description="Find a project, task, record or page. Use the arrow keys to choose and Enter to open." showCloseButton className="rounded-2xl! shadow-pop sm:max-w-2xl [&_[data-slot=command-input-wrapper]]:pr-14">
        <CommandInput aria-label="Search projects, tasks, records and pages" placeholder="Search projects, tasks, records and pages…" autoComplete="off" spellCheck={false} />
        <CommandList aria-label="Search results" className="scroll-thin max-h-[min(28rem,55dvh)] scroll-py-2 p-1">
          <CommandEmpty className="px-6 py-10">
            <SearchIcon aria-hidden="true" className="mx-auto mb-3 size-6 text-muted-foreground/60" />
            <p className="font-medium">{loading ? "Looking…" : "Nothing matches that"}</p>
            <p className="mt-1 text-xs text-muted-foreground">{loading ? "Results appear as they arrive." : errors.length ? "Some results couldn't load. See below." : "Try a project name, a task like TASK-002, a record like ADR-004, or a page."}</p>
          </CommandEmpty>
          <CommandGroup heading="Pages" className={groupStyle}>
            <CommandItem value="page home overview" className={itemStyle} onSelect={() => go(hrefs.home())}>
              <HomeIcon aria-hidden="true" className={itemIcon} /> Home
            </CommandItem>
            <CommandItem value="page tasks all projects" className={itemStyle} onSelect={() => go(hrefs.tasks())}>
              <ListTodoIcon aria-hidden="true" className={itemIcon} /> Tasks
            </CommandItem>
            <CommandItem value="page votes" className={itemStyle} onSelect={() => go(hrefs.votes())}>
              <VoteIcon aria-hidden="true" className={itemIcon} /> Votes
            </CommandItem>
            <CommandItem value="page records" className={itemStyle} onSelect={() => go(hrefs.records())}>
              <BookOpenIcon aria-hidden="true" className={itemIcon} /> Records
            </CommandItem>
          </CommandGroup>
          <CommandSeparator />
          <CommandGroup heading="Behind the scenes" className={groupStyle}>
            <CommandItem value="page agents" className={itemStyle} onSelect={() => go(hrefs.agents())}>
              <BotIcon aria-hidden="true" className={itemIcon} /> Agents
            </CommandItem>
            <CommandItem value="page runs harness transcripts" className={itemStyle} onSelect={() => go(hrefs.harness())}>
              <ScrollTextIcon aria-hidden="true" className={itemIcon} /> Runs
            </CommandItem>
            <CommandItem value="page machines compute executors" className={itemStyle} onSelect={() => go(hrefs.compute())}>
              <ServerIcon aria-hidden="true" className={itemIcon} /> Machines
            </CommandItem>
            <CommandItem value="page connections mcp servers skills" className={itemStyle} onSelect={() => go(hrefs.connections())}>
              <PlugIcon aria-hidden="true" className={itemIcon} /> Connections
            </CommandItem>
            <CommandItem value="page settings" className={itemStyle} onSelect={() => go(hrefs.settings())}>
              <SettingsIcon aria-hidden="true" className={itemIcon} /> Settings
            </CommandItem>
          </CommandGroup>
          {projects.length ? (
            <>
              <CommandSeparator />
              <CommandGroup heading="Projects" className={groupStyle}>
                {projects.map((p) => (
                  <CommandItem key={p.slug} value={"project " + p.slug} keywords={p.slug === org?.commons?.slug ? ["commons", "shared board"] : undefined} className={itemStyle} onSelect={() => go(hrefs.project(p.slug))}>
                    <FolderIcon aria-hidden="true" className={itemIcon} /> <span className="truncate">{p.slug}</span>
                    <CommandShortcut className={shortcutStyle}>{p.slug === org?.commons?.slug ? "Shared board" : p.brain ? "PULSE itself" : "Project"}</CommandShortcut>
                  </CommandItem>
                ))}
              </CommandGroup>
              <CommandSeparator />
              <CommandGroup heading="Project tabs" className={groupStyle}>
                {projects.map((p) =>
                  projectTabs.map((t) => {
                    const Icon = projectTabIcon[t.id];
                    return (
                    <CommandItem key={p.slug + t.id} value={p.slug + " " + t.label} className={itemStyle} onSelect={() => go(hrefs.project(p.slug, t.id))}>
                      <Icon aria-hidden="true" className={itemIcon} /> <span className="truncate">{(p.slug === org?.commons?.slug ? "Shared board" : p.slug) + ": " + t.label}</span>
                    </CommandItem>
                    );
                  }),
                )}
              </CommandGroup>
            </>
          ) : null}
          {tasks?.tasks?.length ? (
            <>
              <CommandSeparator />
              <CommandGroup heading="Tasks" className={groupStyle}>
                {tasks.tasks.slice(0, 40).map((t) => (
                  <CommandItem key={t.project + ":" + t.id} value={t.id + " " + t.title + " " + t.project} className={itemStyle} onSelect={() => go(hrefs.task(t.project, t.id))}>
                    <ListTodoIcon aria-hidden="true" className={itemIcon} /> <span className="truncate">{t.title}</span> <span className="shrink-0 font-mono text-xs text-muted-foreground">{t.id}</span>
                    <CommandShortcut className={shortcutStyle}>{t.project}</CommandShortcut>
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          ) : null}
          {records?.records?.length ? (
            <>
              <CommandSeparator />
              <CommandGroup heading="Records" className={groupStyle}>
                {records.records.slice(0, 60).map((r) => (
                  <CommandItem key={r.project + ":" + r.kind + ":" + r.id} value={r.id + " " + r.title + " " + r.project + " " + r.kind} className={itemStyle} onSelect={() => go(hrefs.record(r.id, r.project))}>
                    <BookOpenIcon aria-hidden="true" className={itemIcon} /> <span className="truncate">{r.title}</span> <span className="shrink-0 font-mono text-xs text-muted-foreground">{r.id}</span>
                    <CommandShortcut className={shortcutStyle}>{recordKindWord(r.kind)}</CommandShortcut>
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          ) : null}
        </CommandList>
        {errors.length ? (
          <div role="status" className="max-h-24 shrink-0 overflow-y-auto border-t border-border/50 px-4 py-2 text-xs break-words text-destructive">
            {errors.map(({ source, error }) => <p key={source}>{source}: {error}</p>)}
          </div>
        ) : loading ? <p role="status" className="shrink-0 border-t border-border/50 px-4 py-2 text-xs text-muted-foreground">Loading projects, tasks and records…</p> : null}
        <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-t border-border/50 px-4 py-3 text-[11px] text-muted-foreground">
          <span className="inline-flex items-center gap-1.5"><kbd aria-label="Arrow keys" className={keyStyle}><ArrowUpIcon aria-hidden="true" className="size-3" /><ArrowDownIcon aria-hidden="true" className="size-3" /></kbd> Navigate</span>
          <span className="inline-flex items-center gap-1.5"><kbd aria-label="Enter" className={keyStyle}><CornerDownLeftIcon aria-hidden="true" className="size-3" /></kbd> Open</span>
          <span className="ml-auto inline-flex items-center gap-1.5"><kbd className={keyStyle}>Esc</kbd> Close</span>
        </div>
      </CommandDialog>
    </>
  );
}
