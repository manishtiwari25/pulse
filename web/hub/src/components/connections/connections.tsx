"use client";

import { useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { BookOpenIcon, PencilIcon, PlugIcon, PlugZapIcon, PlusIcon, PowerIcon, PowerOffIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { showApiError } from "@/components/pulse/confirm";
import { DataTable, type Column } from "@/components/pulse/data-table";
import { EmptyState, ErrorNotice, PageTitle, Panel, SkeletonPage } from "@/components/pulse/panel";
import { Mono, StatusBadge, ToneBadge, When } from "@/components/pulse/who";
import { ConnectDialog, McpServerDialog, refreshMcp } from "@/components/connections/mcp-dialogs";
import { SkillDialog, SkillViewDialog, refreshSkills } from "@/components/connections/skill-dialogs";
import { query, writes, type McpOptions, type McpServer, type McpServersPayload, type ProjectsPayload, type Skill, type SkillsPayload } from "@/lib/api";
import { num, truncate } from "@/lib/format";
import { DEFAULT_OPTIONS, endpoint, lastCheck, scopeLabel, sortScoped, toolNames } from "@/lib/mcp";
import { useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// Connections: what agents can reach beyond the repository. MCP servers are
// the bridge to the outside world (issues, feedback, docs, environments);
// skills are the recipes workers share. Both are scoped to a project or to
// every project, and both reach agents through their brief.

type Tab = "mcp" | "skills";

export function ConnectionsPageClient() {
  const sp = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const project = sp.get("project") || "";
  const tab: Tab = sp.get("tab") === "skills" ? "skills" : "mcp";
  const servers = useApiServers(project);
  const options = useApiOptions();
  const skills = useApiSkills(project);
  const projects = useApiProjects();
  const [addMcp, setAddMcp] = useState(false);
  const [editMcp, setEditMcp] = useState<McpServer | null>(null);
  const [connecting, setConnecting] = useState<McpServer | null>(null);
  const [addSkill, setAddSkill] = useState(false);
  const [editSkill, setEditSkill] = useState<Skill | null>(null);
  const [viewSkill, setViewSkill] = useState<Skill | null>(null);
  const go = (next: { project?: string; tab?: Tab }) => {
    const p = next.project ?? project;
    const t = next.tab ?? tab;
    router.replace(pathname + query({ project: p || undefined, tab: t === "mcp" ? undefined : t }), { scroll: false });
  };
  const opts = options.data ?? DEFAULT_OPTIONS;
  const serverRows = sortScoped(servers.data?.servers ?? []);
  const skillRows = sortScoped(skills.data?.skills ?? []);
  const enabledServers = serverRows.filter((s) => s.status === "enabled").length;
  const enabledSkills = skillRows.filter((k) => k.status === "enabled").length;

  return (
    <>
      <PageTitle
        icon={PlugIcon}
        sub="What agents can reach beyond the repository: MCP servers they call from their shell, and the skills they share."
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" variant={tab === "skills" ? "outline" : "default"} onClick={() => setAddMcp(true)}>
              <PlusIcon /> Add MCP server
            </Button>
            <Button size="sm" variant={tab === "skills" ? "default" : "outline"} onClick={() => setAddSkill(true)}>
              <PlusIcon /> Add skill
            </Button>
          </div>
        }
        bar={
          <div className="flex min-w-0 flex-wrap items-end gap-2">
            <div className="grid gap-1">
              <Label htmlFor="connections-project">Project</Label>
              <Select value={project || "all"} onValueChange={(v) => go({ project: v === "all" ? "" : v })}>
                <SelectTrigger id="connections-project" className="w-48">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">All projects</SelectItem>
                  {(projects.data?.projects ?? []).map((p) => (
                    <SelectItem key={p.slug} value={p.slug}>
                      {p.slug}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            {project ? <p className="pb-2 text-xs text-muted-foreground">Showing rows for {project} and the ones every project shares.</p> : null}
          </div>
        }
      >
        Connections
      </PageTitle>

      <Tabs value={tab} onValueChange={(v) => go({ tab: v as Tab })}>
        <TabsList>
          <TabsTrigger value="mcp">
            MCP servers <span className="tnum text-muted-foreground">{servers.data ? num(serverRows.length) : ""}</span>
          </TabsTrigger>
          <TabsTrigger value="skills">
            Skills <span className="tnum text-muted-foreground">{skills.data ? num(skillRows.length) : ""}</span>
          </TabsTrigger>
        </TabsList>

        <TabsContent value="mcp" className="mt-3">
          {servers.error && !servers.data ? <ErrorNotice error={servers.error} onRetry={() => void servers.reload()} /> : null}
          {!servers.data && !servers.error ? (
            <SkeletonPage rows={2} />
          ) : servers.data && serverRows.length === 0 ? (
            <EmptyState
              title="No MCP servers yet"
              icon={<PlugIcon className="size-4" />}
              hint="pulse mcp add --name github --transport stdio --command npx --arg -y --arg @modelcontextprotocol/server-github --auth env --auth-header GITHUB_PERSONAL_ACCESS_TOKEN --secret op://PULSE/github-mcp/token"
              action={
                <Button size="sm" onClick={() => setAddMcp(true)}>
                  <PlusIcon /> Add MCP server
                </Button>
              }
            >
              Register a server here or from the shell. Once it is enabled and connects, every worker of its project reads what it offers in the brief and calls it with <Mono>pulse mcp call</Mono>.
            </EmptyState>
          ) : servers.data ? (
            <Panel
              dense
              title={
                <span className="flex items-center gap-2">
                  MCP servers
                  <span className="text-xs font-normal text-muted-foreground">
                    {num(enabledServers)} enabled of {num(serverRows.length)}
                  </span>
                </span>
              }
            >
              <McpTable rows={serverRows} onConnect={setConnecting} onEdit={setEditMcp} />
              <p className="px-1 pt-2 text-xs text-muted-foreground">
                Agents on the local executor list tools with <Mono>pulse mcp tools NAME</Mono> and call one with <Mono>pulse mcp call NAME TOOL --args &apos;{"{...}"}&apos;</Mono>. Remote executors have no store; their workers ask for the data through a FINDING line.
              </p>
            </Panel>
          ) : null}
        </TabsContent>

        <TabsContent value="skills" className="mt-3">
          {skills.error && !skills.data ? <ErrorNotice error={skills.error} onRetry={() => void skills.reload()} /> : null}
          {!skills.data && !skills.error ? (
            <SkeletonPage rows={2} />
          ) : skills.data && skillRows.length === 0 ? (
            <EmptyState
              title="No skills yet"
              icon={<BookOpenIcon className="size-4" />}
              hint="pulse skill add --name release-check --description 'How to verify a release before tagging' --file docs/release-check.md"
              action={
                <Button size="sm" onClick={() => setAddSkill(true)}>
                  <PlusIcon /> Add skill
                </Button>
              }
            >
              A skill is a markdown recipe workers are told about in their brief and read with <Mono>pulse skill show NAME</Mono>. Agents add their own when they finish something others will repeat.
            </EmptyState>
          ) : skills.data ? (
            <Panel
              dense
              title={
                <span className="flex items-center gap-2">
                  Skills
                  <span className="text-xs font-normal text-muted-foreground">
                    {num(enabledSkills)} enabled of {num(skillRows.length)}
                  </span>
                </span>
              }
            >
              <SkillTable rows={skillRows} onView={setViewSkill} onEdit={setEditSkill} />
            </Panel>
          ) : null}
        </TabsContent>
      </Tabs>

      <McpServerDialog open={addMcp} onOpenChange={setAddMcp} options={opts} project={project} />
      <McpServerDialog open={!!editMcp} onOpenChange={(o) => (o ? null : setEditMcp(null))} options={opts} server={editMcp} />
      <ConnectDialog server={connecting} onOpenChange={(o) => (o ? null : setConnecting(null))} />
      <SkillDialog open={addSkill} onOpenChange={setAddSkill} project={project} />
      <SkillDialog open={!!editSkill} onOpenChange={(o) => (o ? null : setEditSkill(null))} skill={editSkill} />
      <SkillViewDialog
        skill={viewSkill}
        onOpenChange={(o) => (o ? null : setViewSkill(null))}
        onEdit={(k) => {
          setViewSkill(null);
          setEditSkill(k);
        }}
      />
    </>
  );
}

// ----- data ------------------------------------------------------------------

function useApiServers(project: string) {
  return useApi<McpServersPayload>("mcp" + query({ project: project || undefined }), { refresh: 30000 });
}
function useApiOptions() {
  return useApi<McpOptions>("mcp/options", { refresh: false });
}
function useApiSkills(project: string) {
  return useApi<SkillsPayload>("skills" + query({ project: project || undefined }), { refresh: 30000 });
}
function useApiProjects() {
  return useApi<ProjectsPayload>("projects", { refresh: 60000 });
}

// ----- tables ----------------------------------------------------------------

function McpTable({ rows, onConnect, onEdit }: { rows: McpServer[]; onConnect: (s: McpServer) => void; onEdit: (s: McpServer) => void }) {
  const [busy, setBusy] = useState("");
  const toggle = async (s: McpServer) => {
    const next = s.status === "enabled" ? "disabled" : "enabled";
    setBusy(s.id);
    try {
      await writes.setMcpStatus(s.id, next);
      toast.success(s.name + " " + next + ".", { description: next === "enabled" ? "Workers see it in their next brief." : "Workers stop seeing it; the row and its secret reference stay." });
      refreshMcp();
    } catch (e) {
      showApiError(e, "The status did not change");
    } finally {
      setBusy("");
    }
  };
  const columns: Column<McpServer>[] = [
    {
      key: "name",
      label: "Server",
      sort: (s) => s.name,
      render: (s) => (
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Mono>{s.name}</Mono>
            <span className="text-[11px] text-muted-foreground">{s.id}</span>
          </div>
          {s.note ? <p className="mt-0.5 max-w-md truncate text-xs text-muted-foreground">{s.note}</p> : null}
        </div>
      ),
    },
    { key: "project", label: "Project", hide: "sm", sort: (s) => s.project || "", render: (s) => (s.project ? <Mono>{s.project}</Mono> : <span className="text-muted-foreground">all projects</span>) },
    {
      key: "transport",
      label: "Transport",
      hide: "md",
      sort: (s) => s.transport,
      render: (s) => (
        <Tooltip>
          <TooltipTrigger asChild>
            <span className="inline-flex flex-col items-start gap-0.5">
              <ToneBadge tone="neutral">{s.transport}</ToneBadge>
              <span className="max-w-56 truncate font-mono text-[11px] text-muted-foreground">{truncate(endpoint(s), 48)}</span>
            </span>
          </TooltipTrigger>
          <TooltipContent className="max-w-sm font-mono text-xs anywhere">
            {endpoint(s) || "no endpoint"}
            {s.auth !== "none" ? " · auth " + s.auth + (s.auth_header ? " (" + s.auth_header + ")" : "") : " · no auth"}
          </TooltipContent>
        </Tooltip>
      ),
    },
    { key: "status", label: "Status", sort: (s) => s.status, render: (s) => <StatusBadge status={s.status} /> },
    {
      key: "check",
      label: "Last check",
      hide: "md",
      sort: (s) => s.last_check_at || "",
      render: (s) => <CheckCell server={s} />,
    },
    {
      key: "actions",
      label: <span className="sr-only">Actions</span>,
      align: "right",
      nowrap: true,
      render: (s) => (
        <div className="flex items-center justify-end gap-1">
          <Button size="xs" variant="outline" onClick={() => onConnect(s)} aria-label={"Connect " + s.name}>
            <PlugZapIcon /> Connect
          </Button>
          <Button size="icon-xs" variant="ghost" onClick={() => void toggle(s)} disabled={busy === s.id} aria-label={(s.status === "enabled" ? "Disable " : "Enable ") + s.name} title={s.status === "enabled" ? "Disable" : "Enable"}>
            {s.status === "enabled" ? <PowerOffIcon /> : <PowerIcon />}
          </Button>
          <Button size="icon-xs" variant="ghost" onClick={() => onEdit(s)} aria-label={"Edit " + s.name} title="Edit">
            <PencilIcon />
          </Button>
        </div>
      ),
    },
  ];
  return <DataTable columns={columns} rows={rows} rowKey={(s) => s.id} dense caption="MCP servers" rowClassName={(s) => (s.status === "disabled" ? "opacity-60" : undefined)} />;
}

function CheckCell({ server }: { server: McpServer }) {
  const c = lastCheck(server);
  const tools = server.last_check_ok ? toolNames(server.last_check_note) : [];
  return (
    <div className="min-w-0 text-xs">
      <div className="flex items-center gap-1.5">
        <span className={cn("size-1.5 shrink-0 rounded-full", c.tone === "ok" ? "bg-ok" : c.tone === "bad" ? "bg-bad" : "bg-muted-foreground/40")} aria-hidden />
        <span className={cn(c.tone === "bad" && "text-bad")}>{c.label}</span>
        {server.last_check_at ? (
          <span className="text-muted-foreground">
            <When d={server.last_check_at} />
          </span>
        ) : null}
      </div>
      {c.detail ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <p className="mt-0.5 max-w-64 truncate text-muted-foreground">{tools.length ? tools.length + " tool" + (tools.length === 1 ? "" : "s") + ": " + tools.join(", ") : c.detail}</p>
          </TooltipTrigger>
          <TooltipContent className="max-w-sm text-xs anywhere">{server.last_check_note}</TooltipContent>
        </Tooltip>
      ) : null}
    </div>
  );
}

function SkillTable({ rows, onView, onEdit }: { rows: Skill[]; onView: (k: Skill) => void; onEdit: (k: Skill) => void }) {
  const [busy, setBusy] = useState("");
  const toggle = async (k: Skill) => {
    const next = k.status === "enabled" ? "disabled" : "enabled";
    setBusy(k.id);
    try {
      await writes.setSkillStatus(k.id, next);
      toast.success(k.name + " " + next + ".", { description: next === "enabled" ? "Workers of " + scopeLabel(k.project) + " see it in their next brief." : "Workers stop hearing about it; the body stays." });
      refreshSkills();
    } catch (e) {
      showApiError(e, "The status did not change");
    } finally {
      setBusy("");
    }
  };
  const columns: Column<Skill>[] = [
    {
      key: "name",
      label: "Skill",
      sort: (k) => k.name,
      render: (k) => (
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Mono>{k.name}</Mono>
            <span className="text-[11px] text-muted-foreground">{k.id}</span>
          </div>
          <p className="mt-0.5 max-w-lg truncate text-xs text-muted-foreground">{k.description}</p>
        </div>
      ),
    },
    { key: "project", label: "Project", hide: "sm", sort: (k) => k.project || "", render: (k) => (k.project ? <Mono>{k.project}</Mono> : <span className="text-muted-foreground">all projects</span>) },
    { key: "uses", label: "Reads", align: "right", hide: "md", sort: (k) => k.uses, render: (k) => <span className="tnum">{num(k.uses)}</span> },
    { key: "status", label: "Status", sort: (k) => k.status, render: (k) => <StatusBadge status={k.status} /> },
    { key: "updated", label: "Updated", hide: "lg", sort: (k) => k.updated_at, render: (k) => <When d={k.updated_at} className="text-xs text-muted-foreground" /> },
    {
      key: "actions",
      label: <span className="sr-only">Actions</span>,
      align: "right",
      nowrap: true,
      render: (k) => (
        <div className="flex items-center justify-end gap-1" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => e.stopPropagation()}>
          <Button size="xs" variant="outline" onClick={() => onView(k)} aria-label={"Read " + k.name}>
            <BookOpenIcon /> Read
          </Button>
          <Button size="icon-xs" variant="ghost" onClick={() => void toggle(k)} disabled={busy === k.id} aria-label={(k.status === "enabled" ? "Disable " : "Enable ") + k.name} title={k.status === "enabled" ? "Disable" : "Enable"}>
            {k.status === "enabled" ? <PowerOffIcon /> : <PowerIcon />}
          </Button>
          <Button size="icon-xs" variant="ghost" onClick={() => onEdit(k)} aria-label={"Edit " + k.name} title="Edit">
            <PencilIcon />
          </Button>
        </div>
      ),
    },
  ];
  return <DataTable columns={columns} rows={rows} rowKey={(k) => k.id} dense caption="Skills" onRow={onView} rowClassName={(k) => (k.status === "disabled" ? "opacity-60" : undefined)} />;
}
