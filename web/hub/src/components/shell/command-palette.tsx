"use client";

import { useRouter } from "next/navigation";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowDownIcon, ArrowUpIcon, BookOpenIcon, BotIcon, CornerDownLeftIcon, CpuIcon, FolderIcon, HistoryIcon, HomeIcon, InboxIcon, ListTodoIcon, MessagesSquareIcon, PlugIcon, SearchIcon, SettingsIcon, SparklesIcon, TerminalIcon, VoteIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator, CommandShortcut } from "@/components/ui/command";
import { hrefs, PROJECT_TABS, type ProjectCanonicalTab } from "@/lib/nav";
import { useApi } from "@/lib/use-api";
import type { Org, RecordsPayload, TasksPayload } from "@/lib/api";

const itemIcon = "size-4 text-muted-foreground";
const groupStyle = "**:[[cmdk-group-heading]]:font-semibold **:[[cmdk-group-heading]]:tracking-wide **:[[cmdk-group-heading]]:normal-case";
const itemStyle = "min-h-11 gap-3 rounded-xl px-3 py-2";
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

// ⌘K / Ctrl-K: jump to a project (and its tabs), a task, a record, or a page.
export function CommandPalette() {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const router = useRouter();
  const { data: org, error: orgError, loading: orgLoading } = useApi<Org>("org", { refresh: 60000 });
  const { data: records, error: recordsError, loading: recordsLoading } = useApi<RecordsPayload>(open ? "records" : null, { refresh: false });
  const { data: tasks, error: tasksError, loading: tasksLoading } = useApi<TasksPayload>(open ? "tasks" : null, { refresh: false });

  const openPalette = useCallback((source?: HTMLElement) => {
    returnFocusRef.current = source ?? (document.activeElement instanceof HTMLElement && document.activeElement !== document.body ? document.activeElement : triggerRef.current);
    setOpen(true);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.isComposing && !e.repeat && (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        if (open) setOpen(false);
        else openPalette();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, openPalette]);

  useEffect(() => {
    if (open || !returnFocusRef.current) return;
    const target = returnFocusRef.current;
    returnFocusRef.current = null;
    (target.isConnected ? target : triggerRef.current)?.focus({ preventScroll: true });
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
      <Button ref={triggerRef} variant="outline" size="sm" className="h-11 w-11 gap-2 rounded-xl px-0 text-muted-foreground sm:w-44 sm:justify-start sm:px-3 xl:w-64" onClick={(event) => openPalette(event.currentTarget)} aria-label="Search workspace (Control or Command K)" aria-haspopup="dialog" aria-expanded={open} aria-keyshortcuts="Control+k Meta+k">
        <SearchIcon aria-hidden="true" className="size-[18px]" />
        <span className="hidden sm:inline">Search<span className="hidden xl:inline"> workspace</span></span>
        <kbd aria-hidden="true" className="pointer-events-none ml-auto hidden shrink-0 items-center rounded-md border border-border/70 bg-background/80 px-1.5 py-0.5 font-mono text-[10px] sm:inline-flex">⌘K</kbd>
      </Button>
      <CommandDialog open={open} onOpenChange={setOpen} title="Search workspace" description="Find projects, tasks, records or pages. Use the arrow keys to choose a result and Enter to open it." showCloseButton className="surface-glass rounded-2xl! shadow-pop sm:max-w-2xl [&_[data-slot=command-input-wrapper]]:pr-14">
        <CommandInput aria-label="Search projects, tasks, records and pages" placeholder="Search projects, tasks, records and pages..." autoComplete="off" spellCheck={false} />
        <CommandList aria-label="Search results" className="scroll-thin max-h-[min(28rem,55dvh)] scroll-py-2 p-1">
          <CommandEmpty className="px-6 py-10">
            <SearchIcon aria-hidden="true" className="mx-auto mb-3 size-6 text-muted-foreground/60" />
            <p className="font-medium">{loading ? "Loading destinations..." : "No matching destinations"}</p>
            <p className="mt-1 text-xs text-muted-foreground">{loading ? "Results will appear as they arrive." : errors.length ? "Some sources are unavailable. Details are shown below." : "Try a project name, task ID, record ID or page."}</p>
          </CommandEmpty>
          <CommandGroup heading="Go to" className={groupStyle}>
            <CommandItem value="page home overview" className={itemStyle} onSelect={() => go(hrefs.home())}>
              <HomeIcon aria-hidden="true" className={itemIcon} /> Home
            </CommandItem>
            <CommandItem value="page tasks all projects" className={itemStyle} onSelect={() => go(hrefs.tasks())}>
              <ListTodoIcon aria-hidden="true" className={itemIcon} /> Tasks <CommandShortcut className={shortcutStyle}>all projects</CommandShortcut>
            </CommandItem>
            <CommandItem value="page votes" className={itemStyle} onSelect={() => go(hrefs.votes())}>
              <VoteIcon aria-hidden="true" className={itemIcon} /> Votes
            </CommandItem>
            <CommandItem value="page records" className={itemStyle} onSelect={() => go(hrefs.records())}>
              <BookOpenIcon aria-hidden="true" className={itemIcon} /> Records
            </CommandItem>
          </CommandGroup>
          <CommandSeparator />
          <CommandGroup heading="Actions" className={groupStyle}>
            <CommandItem value="page compute" className={itemStyle} onSelect={() => go(hrefs.compute())}>
              <CpuIcon aria-hidden="true" className={itemIcon} /> Compute
            </CommandItem>
            <CommandItem value="page agents" className={itemStyle} onSelect={() => go(hrefs.agents())}>
              <BotIcon aria-hidden="true" className={itemIcon} /> Agents
            </CommandItem>
            <CommandItem value="page harness" className={itemStyle} onSelect={() => go(hrefs.harness())}>
              <TerminalIcon aria-hidden="true" className={itemIcon} /> Harness
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
                    <CommandShortcut className={shortcutStyle}>{p.slug === org?.commons?.slug ? "shared board" : p.brain ? "framework" : "project"}</CommandShortcut>
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
                      <Icon aria-hidden="true" className={itemIcon} /> <span className="truncate">{p.slug} · {t.label}</span>
                      <CommandShortcut className={shortcutStyle}>{t.id}</CommandShortcut>
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
              <CommandGroup heading="Actions" className={groupStyle}>
                {tasks.tasks.slice(0, 40).map((t) => (
                  <CommandItem key={t.project + ":" + t.id} value={t.id + " " + t.title + " " + t.project} className={itemStyle} onSelect={() => go(hrefs.task(t.project, t.id))}>
                    <ListTodoIcon aria-hidden="true" className={itemIcon} /> <span className="max-w-24 shrink-0 truncate font-mono text-xs sm:max-w-40">{t.id}</span> <span className="truncate">{t.title}</span>
                    <CommandShortcut className={shortcutStyle}>{t.project}</CommandShortcut>
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          ) : null}
          {records?.records?.length ? (
            <>
              <CommandSeparator />
              <CommandGroup heading="Actions" className={groupStyle}>
                {records.records.slice(0, 60).map((r) => (
                  <CommandItem key={r.project + ":" + r.kind + ":" + r.id} value={r.id + " " + r.title + " " + r.project + " " + r.kind} className={itemStyle} onSelect={() => go(hrefs.record(r.id, r.project))}>
                    <BookOpenIcon aria-hidden="true" className={itemIcon} /> <span className="max-w-24 shrink-0 truncate font-mono text-xs sm:max-w-40">{r.id}</span> <span className="truncate">{r.title}</span>
                    <CommandShortcut className={shortcutStyle}>{r.kind}</CommandShortcut>
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
        ) : loading ? <p role="status" className="shrink-0 border-t border-border/50 px-4 py-2 text-xs text-muted-foreground">Loading project, task and record results...</p> : null}
        <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-t border-border/50 px-4 py-3 text-[11px] text-muted-foreground">
          <span className="inline-flex items-center gap-1.5"><kbd aria-label="Arrow keys" className={keyStyle}><ArrowUpIcon aria-hidden="true" className="size-3" /><ArrowDownIcon aria-hidden="true" className="size-3" /></kbd> Navigate</span>
          <span className="inline-flex items-center gap-1.5"><kbd aria-label="Enter" className={keyStyle}><CornerDownLeftIcon aria-hidden="true" className="size-3" /></kbd> Open</span>
          <span className="ml-auto inline-flex items-center gap-1.5"><kbd className={keyStyle}>Esc</kbd> Close</span>
        </div>
      </CommandDialog>
    </>
  );
}
