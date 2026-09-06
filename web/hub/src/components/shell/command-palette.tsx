"use client";

import { useRouter } from "next/navigation";
import { useEffect, useMemo, useState } from "react";
import { BookOpenIcon, BotIcon, CpuIcon, FolderIcon, HomeIcon, ListTodoIcon, MessagesSquareIcon, SearchIcon, SettingsIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator, CommandShortcut } from "@/components/ui/command";
import { hrefs, PROJECT_TABS } from "@/lib/nav";
import { useApi } from "@/lib/use-api";
import type { Org, RecordsPayload, TasksPayload } from "@/lib/api";

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
      <Button variant="outline" size="sm" className="hidden gap-2 text-muted-foreground lg:inline-flex" onClick={() => setOpen(true)} aria-label="Search and jump (Command K)">
        <SearchIcon className="size-3.5" />
        <span>Jump to…</span>
        <kbd className="pointer-events-none rounded border bg-muted px-1 font-mono text-[10px]">⌘K</kbd>
      </Button>
      <Button variant="ghost" size="icon" className="lg:hidden" onClick={() => setOpen(true)} aria-label="Search and jump">
        <SearchIcon className="size-4" />
      </Button>
      <CommandDialog open={open} onOpenChange={setOpen} title="Jump to" description="Projects, tasks, records and pages">
        <CommandInput placeholder="Type a project, task id, record id or page…" />
        <CommandList>
          <CommandEmpty>Nothing matches.</CommandEmpty>
          <CommandGroup heading="Pages">
            <CommandItem onSelect={() => go(hrefs.home())}>
              <HomeIcon /> Hub overview
            </CommandItem>
            <CommandItem onSelect={() => go(hrefs.tasks())}>
              <ListTodoIcon /> Tasks <CommandShortcut>all projects</CommandShortcut>
            </CommandItem>
            <CommandItem onSelect={() => go(hrefs.records())}>
              <BookOpenIcon /> Records
            </CommandItem>
            <CommandItem onSelect={() => go(hrefs.compute())}>
              <CpuIcon /> Compute
            </CommandItem>
            <CommandItem onSelect={() => go(hrefs.agents())}>
              <BotIcon /> Agents
            </CommandItem>
            <CommandItem onSelect={() => go(hrefs.settings())}>
              <SettingsIcon /> Settings
            </CommandItem>
          </CommandGroup>
          {projects.length ? (
            <>
              <CommandSeparator />
              <CommandGroup heading="Projects">
                {projects.map((p) => (
                  <CommandItem key={p.slug} value={"project " + p.slug} onSelect={() => go(hrefs.project(p.slug))}>
                    <FolderIcon /> {p.slug}
                    {p.brain ? <CommandShortcut>framework</CommandShortcut> : null}
                  </CommandItem>
                ))}
                {projects.slice(0, 1).map((p) =>
                  PROJECT_TABS.filter((t) => t.id === "chat" || t.id === "tasks").map((t) => (
                    <CommandItem key={p.slug + t.id} value={p.slug + " " + t.label} onSelect={() => go(hrefs.project(p.slug, t.id))}>
                      <MessagesSquareIcon /> {p.slug} · {t.label}
                    </CommandItem>
                  )),
                )}
              </CommandGroup>
            </>
          ) : null}
          {tasks?.tasks?.length ? (
            <>
              <CommandSeparator />
              <CommandGroup heading="Tasks">
                {tasks.tasks.slice(0, 40).map((t) => (
                  <CommandItem key={t.id} value={t.id + " " + t.title + " " + t.project} onSelect={() => go(hrefs.task(t.project, t.id))}>
                    <ListTodoIcon /> <span className="font-mono text-xs">{t.id}</span> <span className="truncate">{t.title}</span>
                    <CommandShortcut>{t.project}</CommandShortcut>
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          ) : null}
          {records?.records?.length ? (
            <>
              <CommandSeparator />
              <CommandGroup heading="Records">
                {records.records.slice(0, 60).map((r) => (
                  <CommandItem key={r.id} value={r.id + " " + r.title} onSelect={() => go(hrefs.record(r.id, r.project))}>
                    <BookOpenIcon /> <span className="font-mono text-xs">{r.id}</span> <span className="truncate">{r.title}</span>
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
