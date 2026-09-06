"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { Suspense, useMemo } from "react";
import { ActivityIcon, BotIcon, BookOpenIcon, ChevronDownIcon, CircleDotIcon, CpuIcon, HomeIcon, ListTodoIcon, SettingsIcon, WifiIcon, WifiOffIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { hrefs } from "@/lib/nav";
import { useApi, useEventStream } from "@/lib/use-api";
import type { Org } from "@/lib/api";
import { CommandPalette } from "./command-palette";
import { ThemeToggle } from "./theme-toggle";

const NAV: { href: string; label: string; icon: React.ComponentType<{ className?: string }>; match: (p: string) => boolean }[] = [
  { href: "/", label: "Hub", icon: HomeIcon, match: (p) => p === "/" },
  { href: "/tasks/", label: "Tasks", icon: ListTodoIcon, match: (p) => p.startsWith("/tasks") },
  { href: "/records/", label: "Records", icon: BookOpenIcon, match: (p) => p.startsWith("/records") },
  { href: "/compute/", label: "Compute", icon: CpuIcon, match: (p) => p.startsWith("/compute") },
  { href: "/agents/", label: "Agents", icon: BotIcon, match: (p) => p.startsWith("/agents") },
  { href: "/settings/", label: "Settings", icon: SettingsIcon, match: (p) => p.startsWith("/settings") },
];

function LiveDot() {
  const { state } = useEventStream();
  const label = state === "live" ? "live: the hub streams every store event" : state === "polling" ? "polling: the event stream is reconnecting, refreshing every 10 s" : "connecting to the hub";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-muted-foreground" aria-live="polite" aria-label={label}>
          {state === "live" ? <WifiIcon className="size-3.5 text-ok" /> : state === "polling" ? <WifiOffIcon className="size-3.5 text-warn" /> : <CircleDotIcon className="size-3.5 animate-pulse" />}
          <span className="hidden sm:inline">{state}</span>
        </span>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

function ProjectSwitcher() {
  const params = useSearchParams();
  const pathname = usePathname();
  const current = pathname.startsWith("/project") ? params.get("slug") || "" : "";
  const { data } = useApi<Org>("org", { refresh: 30000 });
  const projects = useMemo(() => {
    const list = [...(data?.projects ?? [])];
    if (data?.commons) list.push(data.commons);
    return list;
  }, [data]);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="sm" className="max-w-[220px] gap-1.5" aria-label="Switch project">
          <ActivityIcon className="size-3.5 text-muted-foreground" />
          <span className="truncate">{current || "Projects"}</span>
          <ChevronDownIcon className="size-3.5 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-72">
        <DropdownMenuLabel>Projects on this machine</DropdownMenuLabel>
        {projects.length === 0 ? <div className="px-2 py-1.5 text-xs text-muted-foreground">{data ? "No project is registered yet: run pulse init in a checkout." : "Loading…"}</div> : null}
        {projects.map((p) => (
          <DropdownMenuItem key={p.slug} asChild>
            <Link href={hrefs.project(p.slug)} className="flex items-center justify-between gap-2">
              <span className="truncate">{p.slug}</span>
              <span className="flex items-center gap-1">
                {p.brain ? <Badge variant="secondary">framework</Badge> : null}
                {p.proposals_awaiting ? <Badge>{p.proposals_awaiting} for you</Badge> : null}
                {!p.present ? <Badge variant="destructive">missing</Badge> : null}
              </span>
            </Link>
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem asChild>
          <Link href="/">All projects</Link>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function AppShell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();
  return (
    <>
      <a href="#main" className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-primary focus:px-3 focus:py-1.5 focus:text-primary-foreground">
        Skip to content
      </a>
      <header className="sticky top-0 z-40 border-b bg-background/85 backdrop-blur supports-[backdrop-filter]:bg-background/70">
        <div className="mx-auto flex h-12 max-w-[1560px] items-center gap-2 px-3 sm:px-4">
          <Link href="/" className="mr-1 flex items-center gap-2 font-semibold tracking-tight" aria-label="PULSE hub">
            <span className="inline-flex size-6 items-center justify-center rounded-md bg-primary text-primary-foreground">
              <ActivityIcon className="size-3.5" />
            </span>
            <span className="hidden sm:inline">PULSE</span>
          </Link>
          <Suspense fallback={<div className="h-7 w-32 rounded-md bg-muted" />}>
            <ProjectSwitcher />
          </Suspense>
          <nav aria-label="Primary" className="ml-1 hidden items-center gap-0.5 md:flex">
            {NAV.map((n) => {
              const active = n.match(pathname);
              return (
                <Link key={n.href} href={n.href} aria-current={active ? "page" : undefined} className={cn("inline-flex h-8 items-center gap-1.5 rounded-md px-2.5 text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground", active && "bg-muted text-foreground")}>
                  <n.icon className="size-3.5" />
                  {n.label}
                </Link>
              );
            })}
          </nav>
          <div className="ml-auto flex items-center gap-1">
            <LiveDot />
            <Suspense fallback={null}>
              <CommandPalette />
            </Suspense>
            <ThemeToggle />
          </div>
        </div>
        <nav aria-label="Primary (compact)" className="flex gap-0.5 overflow-x-auto border-t px-2 py-1 md:hidden">
          {NAV.map((n) => {
            const active = n.match(pathname);
            return (
              <Link key={n.href} href={n.href} aria-current={active ? "page" : undefined} className={cn("inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground", active && "bg-muted text-foreground")}>
                <n.icon className="size-3.5" />
                {n.label}
              </Link>
            );
          })}
        </nav>
      </header>
      <main id="main" tabIndex={-1} className="mx-auto w-full max-w-[1560px] flex-1 px-3 py-4 outline-none sm:px-4">
        {children}
      </main>
      <footer className="border-t px-4 py-3 text-center text-xs text-muted-foreground">
        PULSE hub · 127.0.0.1 only · the owner writes here: tasks, messages, decisions, settings, compute; agents write with the CLI.
      </footer>
    </>
  );
}
