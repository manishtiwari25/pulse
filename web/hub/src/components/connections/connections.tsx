"use client";

import { useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { BookOpenIcon, KeyRoundIcon, LogOutIcon, PencilIcon, PlugIcon, PlugZapIcon, PlusIcon, PowerIcon, PowerOffIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { showApiError } from "@/components/pulse/confirm";
import { CardGrid, ItemCard, ItemCardActions, ItemCardBody, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { Disclosure, EmptyState, ErrorNotice, PageTitle, SkeletonRows } from "@/components/pulse/panel";
import { Mono, StatusBadge, When } from "@/components/pulse/who";
import { ConnectDialog, McpServerDialog, SignInDialog, refreshMcp } from "@/components/connections/mcp-dialogs";
import { SkillDialog, SkillViewDialog, refreshSkills } from "@/components/connections/skill-dialogs";
import { ToolkitGallery } from "@/components/project/toolkit";
import { query, skillRepoPath, writes, type McpOptions, type McpServer, type McpServersPayload, type ProjectsPayload, type Skill, type SkillsPayload, type ToolkitPayload, type ToolkitProject } from "@/lib/api";
import { num, plural } from "@/lib/format";
import { DEFAULT_OPTIONS, TRANSPORT_SHORT, authLabel, checkReason, endpoint, lastCheck, scopeLabel, scopeTitle, sortScoped, toolNames } from "@/lib/mcp";
import { useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// Connections: what agents can reach beyond the repository. A tool
// connection (an MCP server) is the bridge to another system - issues,
// feedback, docs, environments; a skill is a reusable set of instructions.
// Both are for every project unless one project is named - the Add dialogs
// start at "All projects" whatever the page filter says - and both reach
// agents through their brief.

type Tab = "mcp" | "skills";

// A quiet fold for the terminal side of things.
function Fold({ summary, children }: { summary: string; children: React.ReactNode }) {
  return (
    <Disclosure summary={summary} contentClassName="grid max-w-3xl gap-2 text-sm leading-relaxed text-muted-foreground anywhere">
      {children}
    </Disclosure>
  );
}

// What the projects' checkouts ask for and do not have yet, one illustrated
// card each; the page filter narrows it to one project.
function Suggested({ project, kind }: { project: string; kind: "tool" | "skill" }) {
  const q = useApi<ToolkitPayload>(project ? "toolkit?project=" + encodeURIComponent(project) : "toolkit", { refresh: 120000 });
  const d = q.data;
  if (!d) return null;
  const groups: ToolkitProject[] = d.projects ?? [{ project: d.project, tools: d.tools ?? [], skills: d.skills ?? [] }];
  const rows = groups.filter((g) => (kind === "tool" ? g.tools.length : g.skills.length) > 0);
  if (!rows.length) return null;
  const total = rows.reduce((n, g) => n + (kind === "tool" ? g.tools.length : g.skills.length), 0);
  return (
    <section className="grid gap-3 rounded-2xl border border-dashed bg-card/40 p-4" aria-label={"Suggested " + (kind === "tool" ? "tools" : "skills")}>
      <div>
        <p className="text-sm font-medium text-foreground">{kind === "tool" ? "Tools PULSE suggests" : "Skills PULSE suggests"}</p>
        <p className="text-[13px] leading-relaxed text-muted-foreground">
          {plural(total, kind === "tool" ? "tool connection" : "drafted skill") + " from what " + (rows.length === 1 ? rows[0].project + " shows" : plural(rows.length, "project") + " show")}
          {kind === "tool" ? " - a service the manifests name, with the client that reaches it." : " - how to set it up, check it, ship it, written from the files."} Pick a card to read why.
        </p>
      </div>
      {rows.map((g) => (
        <div key={g.project} className="grid gap-2">
          {rows.length > 1 ? <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">{g.project}</p> : null}
          <ToolkitGallery project={g.project} tools={g.tools} skills={g.skills} canDecide only={kind} />
        </div>
      ))}
    </section>
  );
}

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
  const [signingIn, setSigningIn] = useState<McpServer | null>(null);
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
  const scopeNote = project ? " Showing " + project + "'s own and the ones every project shares." : "";

  return (
    <Tabs value={tab} onValueChange={(v) => go({ tab: v as Tab })} className="min-w-0 gap-0">
      <PageTitle
        icon={PlugIcon}
        sub="Tools and skills your agents can use. A tool connection (MCP server) lets agents reach another system; a skill is a reusable set of instructions."
        actions={
          <>
            <Button variant={tab === "mcp" ? "default" : "outline"} onClick={() => setAddMcp(true)}>
              <PlusIcon /> Add a tool
            </Button>
            <Button variant={tab === "skills" ? "default" : "outline"} onClick={() => setAddSkill(true)}>
              <PlusIcon /> Add a skill
            </Button>
          </>
        }
        bar={
          <TabsList variant="line" aria-label="Connections" className="w-full justify-start">
            <TabsTrigger value="mcp" className="flex-none gap-1.5">
              Tools <span className="tnum text-muted-foreground">{servers.data ? num(serverRows.length) : ""}</span>
            </TabsTrigger>
            <TabsTrigger value="skills" className="flex-none gap-1.5">
              Skills <span className="tnum text-muted-foreground">{skills.data ? num(skillRows.length) : ""}</span>
            </TabsTrigger>
          </TabsList>
        }
      >
        Connections
      </PageTitle>
      <div className="mb-4 flex min-w-0">
        <Select value={project || "all"} onValueChange={(v) => go({ project: v === "all" ? "" : v })}>
          <SelectTrigger id="connections-project" aria-label="Project" className="w-full sm:w-48">
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

      <TabsContent value="mcp" className="grid min-w-0 gap-4">
        {servers.error && !servers.data ? (
          <ErrorNotice error={servers.error} onRetry={() => void servers.reload()}>
            {"The tool connections didn't load (" + servers.error + "). Check that PULSE is running, then try again."}
          </ErrorNotice>
        ) : null}
        {!servers.data && !servers.error ? (
          <SkeletonRows n={4} />
        ) : servers.data && serverRows.length === 0 ? (
          <>
            <EmptyState
              title={project ? "No tools for " + project + " yet" : "No tools connected yet"}
              icon={<PlugIcon className="size-[18px]" />}
              action={
                <Button size="sm" onClick={() => setAddMcp(true)}>
                  <PlusIcon /> Add a tool
                </Button>
              }
            >
              Connect a tool so agents can reach another system, like your issue tracker, user feedback or docs. Once it is on and connects, agents see what it offers in their brief.
            </EmptyState>
            <Fold summary="Prefer the terminal?">
              <p>Add a tool from a shell on this machine, for example GitHub:</p>
              <Mono className="block w-fit max-w-full">pulse mcp add --name github --transport stdio --command npx --arg -y --arg @modelcontextprotocol/server-github --auth env --auth-header GITHUB_PERSONAL_ACCESS_TOKEN --secret op://PULSE/github-mcp/token</Mono>
            </Fold>
          </>
        ) : servers.data ? (
          <>
            <p className="max-w-3xl text-sm leading-relaxed text-muted-foreground">
              {enabledServers === serverRows.length ? (serverRows.length === 1 ? "It is on." : "All " + num(serverRows.length) + " are on.") : num(enabledServers) + " of " + num(serverRows.length) + " are on."} Agents see the tools that are on in their brief.{scopeNote}
            </p>
            <CardGrid cols={2}>
              {serverRows.map((s) => (
                <ServerCard key={s.id} s={s} onConnect={setConnecting} onEdit={setEditMcp} onSignIn={setSigningIn} />
              ))}
            </CardGrid>
            <Fold summary="How agents use these">
              <p>
                Agents on this machine list what a tool offers with <Mono>pulse mcp tools NAME</Mono> and use it with <Mono>pulse mcp call NAME TOOL --args &apos;{"{...}"}&apos;</Mono>.
              </p>
              <p>Agents on other machines can&apos;t reach these tools directly. When they need something from one, they ask their lead agent for it in a finding.</p>
            </Fold>
          </>
        ) : null}
        <Suggested project={project} kind="tool" />
      </TabsContent>

      <TabsContent value="skills" className="grid min-w-0 gap-4">
        {skills.error && !skills.data ? (
          <ErrorNotice error={skills.error} onRetry={() => void skills.reload()}>
            {"The skills didn't load (" + skills.error + "). Check that PULSE is running, then try again."}
          </ErrorNotice>
        ) : null}
        {!skills.data && !skills.error ? (
          <SkeletonRows n={3} />
        ) : skills.data && skillRows.length === 0 ? (
          <>
            <EmptyState
              title={project ? "No skills for " + project + " yet" : "No skills yet"}
              icon={<BookOpenIcon className="size-[18px]" />}
              action={
                <Button size="sm" onClick={() => setAddSkill(true)}>
                  <PlusIcon /> Add a skill
                </Button>
              }
            >
              A skill is a set of instructions agents read before work it covers: a setup, a debugging path, a release check. Agents also add their own when they finish something others will repeat.
            </EmptyState>
            <Fold summary="Prefer the terminal?">
              <p>Add a skill from a shell on this machine:</p>
              <Mono className="block w-fit max-w-full">pulse skill add --name release-check --description &apos;How to verify a release before tagging&apos; --file docs/release-check.md</Mono>
            </Fold>
          </>
        ) : skills.data ? (
          <>
            <p className="max-w-3xl text-sm leading-relaxed text-muted-foreground">
              {enabledSkills === skillRows.length ? (skillRows.length === 1 ? "It is on." : "All " + num(skillRows.length) + " are on.") : num(enabledSkills) + " of " + num(skillRows.length) + " are on."} Agents are told about the skills that are on and read one before work it covers.{scopeNote}
            </p>
            <CardGrid cols={2}>
              {skillRows.map((k) => (
                <SkillCard key={k.id} k={k} onView={setViewSkill} onEdit={setEditSkill} />
              ))}
            </CardGrid>
            <Fold summary="How agents use these">
              <p>
                Agents read a skill with <Mono>pulse skill show NAME</Mono>; each read is counted. They add their own with <Mono>pulse skill add</Mono>.
              </p>
            </Fold>
          </>
        ) : null}
        <Suggested project={project} kind="skill" />
      </TabsContent>

      <McpServerDialog open={addMcp} onOpenChange={setAddMcp} options={opts} />
      <McpServerDialog open={!!editMcp} onOpenChange={(o) => (o ? null : setEditMcp(null))} options={opts} server={editMcp} />
      <ConnectDialog
        server={connecting}
        onOpenChange={(o) => (o ? null : setConnecting(null))}
        onSignIn={(s) => {
          setConnecting(null);
          setSigningIn(s);
        }}
      />
      <SignInDialog
        server={signingIn}
        onOpenChange={(o) => (o ? null : setSigningIn(null))}
        onSignedIn={(s) => {
          // Signed in: the natural next step is the check that lists its tools.
          setSigningIn(null);
          setConnecting(s);
        }}
      />
      <SkillDialog open={addSkill} onOpenChange={setAddSkill} />
      <SkillDialog open={!!editSkill} onOpenChange={(o) => (o ? null : setEditSkill(null))} skill={editSkill} />
      <SkillViewDialog
        skill={viewSkill}
        onOpenChange={(o) => (o ? null : setViewSkill(null))}
        onEdit={
          viewSkill && skillRepoPath(viewSkill)
            ? undefined
            : (k) => {
                setViewSkill(null);
                setEditSkill(k);
              }
        }
      />
    </Tabs>
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

// ----- cards -----------------------------------------------------------------

function CardHead({ icon: Icon, name, id, status, off }: { icon: React.ComponentType<{ className?: string }>; name: React.ReactNode; id: string; status: string; off: boolean }) {
  return (
    <div className="flex min-w-0 items-start justify-between gap-3">
      <div className="flex min-w-0 items-center gap-3">
        <span className={cn("flex size-9 shrink-0 items-center justify-center rounded-lg", off ? "bg-muted text-muted-foreground" : "bg-primary/10 text-primary")} aria-hidden>
          <Icon className="size-[18px]" />
        </span>
        <div className="min-w-0">
          <ItemCardTitle className={cn(off && "text-muted-foreground")}>{name}</ItemCardTitle>
          <p className="text-xs text-muted-foreground" title={"Id " + id}>
            {id}
          </p>
        </div>
      </div>
      <StatusBadge status={status} className="mt-0.5 shrink-0" />
    </div>
  );
}

function ServerCard({ s, onConnect, onEdit, onSignIn }: { s: McpServer; onConnect: (s: McpServer) => void; onEdit: (s: McpServer) => void; onSignIn: (s: McpServer) => void }) {
  const [busy, setBusy] = useState(false);
  const [signingOut, setSigningOut] = useState(false);
  const on = s.status === "enabled";
  const oauth = s.auth === "oauth";
  const signedIn = !!s.sign_in?.signed_in;
  const signOut = async () => {
    setSigningOut(true);
    try {
      const r = await writes.logoutMcp(s.id);
      toast.success("Signed out of " + s.name + ".", { description: r.note || (r.revoked ? "The provider was told to forget the tokens too." : "The tokens on this machine are gone.") });
      refreshMcp();
    } catch (e) {
      showApiError(e, "Couldn't sign out of " + s.name);
    } finally {
      setSigningOut(false);
    }
  };
  const toggle = async () => {
    const next = on ? "disabled" : "enabled";
    setBusy(true);
    try {
      await writes.setMcpStatus(s.id, next);
      toast.success(s.name + (next === "enabled" ? " is on." : " is off."), { description: next === "enabled" ? "Agents see it from their next job." : "Agents stop seeing it. Its settings and secret reference stay." });
      refreshMcp();
    } catch (e) {
      showApiError(e, next === "enabled" ? s.name + " wasn't turned on" : s.name + " wasn't turned off");
    } finally {
      setBusy(false);
    }
  };
  const where = endpoint(s);
  const auth = s.auth !== "none" ? authLabel(s.auth) + (s.auth_header ? " (" + s.auth_header + ")" : "") : authLabel("none");
  return (
    <ItemCard ariaLabel={"Tool " + s.name} className="gap-3">
      <CardHead icon={PlugIcon} name={s.name} id={s.id} status={s.status} off={!on} />
      <ItemCardBody clamp={2}>{s.note || "No description yet. Edit it to tell agents what this tool is for and when to use it."}</ItemCardBody>
      <CheckLine server={s} />
      {oauth ? <SignInLine server={s} /> : null}
      <ItemCardMeta>
        <span>{scopeTitle(s.project)}</span>
        <span className="flex min-w-0 max-w-full items-center gap-1.5" title={TRANSPORT_SHORT[s.transport] + ": " + (where || "no address") + ". Sign-in: " + auth + (s.secret_ref ? ", secret from " + s.secret_ref : "") + "."}>
          <span className="shrink-0">{TRANSPORT_SHORT[s.transport]}</span>
          <span className="min-w-0 truncate font-mono text-xs">{where || "no address"}</span>
        </span>
      </ItemCardMeta>
      <ItemCardActions className="border-t border-border/70 pt-3">
        {oauth && !signedIn ? (
          <Button size="sm" onClick={() => onSignIn(s)} aria-label={"Sign in to " + s.name}>
            <KeyRoundIcon /> Sign in
          </Button>
        ) : null}
        <Button size="sm" variant="outline" onClick={() => onConnect(s)} aria-label={"Check the connection to " + s.name}>
          <PlugZapIcon /> {s.last_check_at ? "Check now" : "Connect now"}
        </Button>
        <span className="ml-auto flex flex-wrap gap-1">
          {oauth && signedIn ? (
            <Button size="sm" variant="ghost" onClick={() => void signOut()} disabled={signingOut} aria-label={"Sign out of " + s.name}>
              <LogOutIcon /> Sign out
            </Button>
          ) : null}
          <Button size="sm" variant="ghost" onClick={() => onEdit(s)} aria-label={"Edit " + s.name}>
            <PencilIcon /> Edit
          </Button>
          <Button size="sm" variant="ghost" onClick={() => void toggle()} disabled={busy} aria-label={(on ? "Turn off " : "Turn on ") + s.name}>
            {on ? <PowerOffIcon /> : <PowerIcon />} {on ? "Turn off" : "Turn on"}
          </Button>
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}

// "Connected 1 month ago. 12 tools, including list_issues, get_issue and
// create_issue." / "Failed 1 month ago: PULSE couldn't read the secret.";
// the raw note in the tooltip.
// The sign-in an `auth: oauth` server has on this machine, from the grant
// file the daemon reads - not from the last check, which may be older.
function SignInLine({ server }: { server: McpServer }) {
  const signedIn = !!server.sign_in?.signed_in;
  // The daemon's note names the terminal command when nothing is signed in;
  // here the Sign in button is the step, so the line only states the fact.
  const raw = server.sign_in?.note || "";
  const note = signedIn ? raw.charAt(0).toUpperCase() + raw.slice(1) : /grant unreadable/.test(raw) ? "The saved sign-in can't be read. Sign in again." : "Not signed in on this machine yet.";
  return (
    <p className="flex min-w-0 items-start gap-2 text-sm leading-relaxed" title={raw || undefined}>
      <KeyRoundIcon className={cn("mt-[3px] size-3.5 shrink-0", signedIn ? "text-ok" : "text-muted-foreground/60")} aria-hidden />
      <span className={cn("min-w-0 anywhere", signedIn ? "text-foreground" : "text-muted-foreground")}>{(note || "Signed in on this machine").replace(/[.\s]*$/, ".")}</span>
    </p>
  );
}

function CheckLine({ server }: { server: McpServer }) {
  const c = lastCheck(server);
  const tools = server.last_check_ok ? toolNames(server.last_check_note) : [];
  const shown = tools.slice(0, 3);
  const rest = tools.length - shown.length;
  const reason = c.tone === "bad" ? checkReason(c.detail) : "";
  return (
    <p className="flex min-w-0 items-start gap-2 text-sm leading-relaxed" title={server.last_check_note || undefined}>
      <span className={cn("mt-[7px] size-2 shrink-0 rounded-full", c.tone === "ok" ? "bg-ok" : c.tone === "bad" ? "bg-bad" : "bg-muted-foreground/40")} aria-hidden />
      <span className="min-w-0 anywhere">
        {server.last_check_at ? (
          <>
            <span className={cn("font-medium", c.tone === "bad" ? "text-bad" : "text-foreground")}>
              {c.label} <When d={server.last_check_at} />
              {reason ? ":" : "."}
            </span>
            {tools.length ? <span className="text-muted-foreground">{" " + plural(tools.length, "tool") + (rest > 0 ? ", including " : ": ") + andList(shown) + "."}</span> : reason ? <span className="text-muted-foreground">{" " + reason.replace(/[.\s]*$/, ".")}</span> : null}
          </>
        ) : (
          <span className="text-muted-foreground">Not checked yet. Connect it to see what it offers.</span>
        )}
      </span>
    </p>
  );
}

const andList = (xs: string[]) => (xs.length < 2 ? xs.join("") : xs.slice(0, -1).join(", ") + " and " + xs[xs.length - 1]);

function SkillCard({ k, onView, onEdit }: { k: Skill; onView: (k: Skill) => void; onEdit: (k: Skill) => void }) {
  const [busy, setBusy] = useState(false);
  const on = k.status === "enabled";
  const toggle = async () => {
    const next = on ? "disabled" : "enabled";
    setBusy(true);
    try {
      await writes.setSkillStatus(k.id, next);
      toast.success(k.name + (next === "enabled" ? " is on." : " is off."), {
        description: next === "enabled" ? "Agents of " + scopeLabel(k.project) + " hear about it from their next job." : "Agents stop hearing about it. The instructions stay.",
      });
      refreshSkills();
    } catch (e) {
      showApiError(e, next === "enabled" ? k.name + " wasn't turned on" : k.name + " wasn't turned off");
    } finally {
      setBusy(false);
    }
  };
  const by = k.created_by === "human" || k.created_by === "owner" ? "you" : k.created_by;
  const repoPath = skillRepoPath(k);
  return (
    <ItemCard onOpen={() => onView(k)} ariaLabel={"Read the skill " + k.name} className="gap-3">
      <CardHead icon={BookOpenIcon} name={k.name} id={k.id} status={k.status} off={!on} />
      <ItemCardBody clamp={2}>{k.description}</ItemCardBody>
      <ItemCardMeta>
        <span>{scopeTitle(k.project)}</span>
        <span>{k.uses ? "Read " + (k.uses === 1 ? "once" : num(k.uses) + " times") : "Not read yet"}</span>
        {repoPath ? <span className="anywhere">{"Kept by the repository: " + repoPath}</span> : by ? <span>{"Added by " + by}</span> : null}
        <span>
          {"Updated "}
          <When d={k.updated_at} />
        </span>
      </ItemCardMeta>
      <ItemCardActions className="border-t border-border/70 pt-3">
        <Button size="sm" variant="outline" onClick={() => onView(k)} aria-label={"Read " + k.name}>
          <BookOpenIcon /> Read
        </Button>
        <span className="ml-auto flex flex-wrap gap-1">
          {repoPath ? null : (
            <Button size="sm" variant="ghost" onClick={() => onEdit(k)} aria-label={"Edit " + k.name}>
              <PencilIcon /> Edit
            </Button>
          )}
          <Button size="sm" variant="ghost" onClick={() => void toggle()} disabled={busy} aria-label={(on ? "Turn off " : "Turn on ") + k.name}>
            {on ? <PowerOffIcon /> : <PowerIcon />} {on ? "Turn off" : "Turn on"}
          </Button>
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}
