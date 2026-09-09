"use client";

import { useRouter } from "next/navigation";
import { useEffect, useMemo, useState } from "react";
import { BookOpenIcon, BotIcon, CpuIcon, FolderIcon, HomeIcon, ListTodoIcon, MessagesSquareIcon, SearchIcon, SettingsIcon, VoteIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator, CommandShortcut } from "@/components/ui/command";
import { hrefs, PROJECT_TABS } from "@/lib/nav";
import { useApi } from "@/lib/use-api";
import type { Org, RecordsPayload, TasksPayload } from "@/lib/api";

const itemIcon = "size-4 text-muted-foreground";

// ⌘K / Ctrl-K: jump to a project (and its tabs), a task, a record, or a page.
export function CommandPalette() {
  const [open, setOpen] = useState(false);
  const router = useRouter();
  const { data: org } = useApi<Org>("org", { refresh: 60000 });
  const { data: records } = useApi<RecordsPayload>(open ? "records" : null, { refresh: false });
  const { data: tasks } = useApi<TasksPayload>(open ? "tasks" : null, { refresh: false });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setOpen((o) => !o);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const projects = useMemo(() => [...(org?.projects ?? []), ...(org?.commons ? [org.commons] : [])], [org]);
  const go = (href: string) => {
    setOpen(false);
    router.push(href);
  };

  return (
    <>
      <Button variant="outline" size="sm" className="hidden gap-2 rounded-full border-transparent bg-muted/50 text-muted-foreground shadow-none hover:bg-muted hover:text-foreground lg:inline-flex" onClick={() => setOpen(true)} aria-label="Search and jump (Command K)">
        <SearchIcon className="size-3.5" />
        <span>Search…</span>
        <kbd className="pointer-events-none rounded border bg-background px-1 font-mono text-[10px] text-muted-foreground">⌘K</kbd>
      </Button>
      <Button variant="ghost" size="icon" className="rounded-full bg-muted/50 text-muted-foreground hover:bg-muted hover:text-foreground lg:hidden" onClick={() => setOpen(true)} aria-label="Search and jump">
        <SearchIcon className="size-4" />
      </Button>
      <CommandDialog open={open} onOpenChange={setOpen} title="Jump to" description="Projects, tasks, records and pages" className="rounded-xl">
        <CommandInput placeholder="Type a project, task id, record id or page…" />
        <CommandList className="scroll-thin">
          <CommandEmpty>Nothing matches.</CommandEmpty>
          <CommandGroup heading="Pages" className="**:[[cmdk-group-heading]]:font-semibold **:[[cmdk-group-heading]]:tracking-tight">
            <CommandItem onSelect={() => go(hrefs.home())}>
              <HomeIcon className={itemIcon} /> Hub overview
            </CommandItem>
            <CommandItem onSelect={() => go(hrefs.tasks())}>
              <ListTodoIcon className={itemIcon} /> Tasks <CommandShortcut>all projects</CommandShortcut>
            </CommandItem>
            <CommandItem onSelect={() => go(hrefs.votes())}>
              <VoteIcon className={itemIcon} /> Votes
            </CommandItem>
            <CommandItem onSelect={() => go(hrefs.records())}>
              <BookOpenIcon className={itemIcon} /> Records
            </CommandItem>
            <CommandItem onSelect={() => go(hrefs.compute())}>
              <CpuIcon className={itemIcon} /> Compute
            </CommandItem>
            <CommandItem onSelect={() => go(hrefs.agents())}>
              <BotIcon className={itemIcon} /> Agents
            </CommandItem>
            <CommandItem onSelect={() => go(hrefs.settings())}>
              <SettingsIcon className={itemIcon} /> Settings
            </CommandItem>
          </CommandGroup>
          {projects.length ? (
            <>
              <CommandSeparator />
              <CommandGroup heading="Projects" className="**:[[cmdk-group-heading]]:font-semibold **:[[cmdk-group-heading]]:tracking-tight">
                {projects.map((p) => (
                  <CommandItem key={p.slug} value={"project " + p.slug} onSelect={() => go(hrefs.project(p.slug))}>
                    <FolderIcon className={itemIcon} /> <span className="truncate">{p.slug}</span>
                    <CommandShortcut>{p.brain ? "framework" : "project"}</CommandShortcut>
                  </CommandItem>
                ))}
                {projects.slice(0, 1).map((p) =>
                  PROJECT_TABS.filter((t) => t.id === "chat" || t.id === "tasks").map((t) => (
                    <CommandItem key={p.slug + t.id} value={p.slug + " " + t.label} onSelect={() => go(hrefs.project(p.slug, t.id))}>
                      <MessagesSquareIcon className={itemIcon} /> <span className="truncate">{p.slug} · {t.label}</span>
                      <CommandShortcut>{t.id}</CommandShortcut>
                    </CommandItem>
                  )),
                )}
              </CommandGroup>
            </>
          ) : null}
          {tasks?.tasks?.length ? (
            <>
              <CommandSeparator />
              <CommandGroup heading="Tasks" className="**:[[cmdk-group-heading]]:font-semibold **:[[cmdk-group-heading]]:tracking-tight">
                {tasks.tasks.slice(0, 40).map((t) => (
                  <CommandItem key={t.id} value={t.id + " " + t.title + " " + t.project} onSelect={() => go(hrefs.task(t.project, t.id))}>
                    <ListTodoIcon className={itemIcon} /> <span className="font-mono text-xs">{t.id}</span> <span className="truncate">{t.title}</span>
                    <CommandShortcut>{t.project}</CommandShortcut>
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          ) : null}
          {records?.records?.length ? (
            <>
              <CommandSeparator />
              <CommandGroup heading="Records" className="**:[[cmdk-group-heading]]:font-semibold **:[[cmdk-group-heading]]:tracking-tight">
                {records.records.slice(0, 60).map((r) => (
                  <CommandItem key={r.id} value={r.id + " " + r.title} onSelect={() => go(hrefs.record(r.id, r.project))}>
                    <BookOpenIcon className={itemIcon} /> <span className="font-mono text-xs">{r.id}</span> <span className="truncate">{r.title}</span>
                    <CommandShortcut>{r.kind}</CommandShortcut>
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          ) : null}
        </CommandList>
      </CommandDialog>
    </>
  );
}
