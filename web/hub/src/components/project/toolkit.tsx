"use client";

import Link from "next/link";
import { useState } from "react";
import { BookOpenIcon, CheckIcon, ExternalLinkIcon, KeyRoundIcon, Loader2Icon, PenLineIcon, PlugZapIcon, PlusIcon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { showApiError } from "@/components/pulse/confirm";
import { Disclosure, Panel } from "@/components/pulse/panel";
import { Mono, ToneBadge } from "@/components/pulse/who";
import { McpServerDialog, refreshMcp } from "@/components/connections/mcp-dialogs";
import { SkillDialog, refreshSkills } from "@/components/connections/skill-dialogs";
import { RoleMascot } from "@/components/project/lead-mascots";
import { writes, type McpAuth, type McpOptions, type McpTransport, type SkillSuggestion, type ToolSuggestion, type ToolkitPayload } from "@/lib/api";
import { plural } from "@/lib/format";
import { DEFAULT_OPTIONS, TRANSPORT_SHORT, type McpForm, type SkillForm } from "@/lib/mcp";
import { hrefs } from "@/lib/nav";
import { invalidate, useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// The toolkit: what a project's checkout asks for and does not have yet - a
// tool connection (an MCP server) for a service its manifests name, a skill
// drafted from its shape (how to set it up, check its UI, ship a release).
// One illustrated card each; a click picks, the footer says what it is and
// why, and one more click adds it the way `pulse mcp add` or `pulse skill
// add` would. A tool that needs a secret opens the form instead, filled in,
// so the owner adds the secret reference. The daemon asks about the same
// things on the board, one at a time; a yes there adds it too.

export const refreshToolkit = () => invalidate((p) => p === "toolkit" || p.startsWith("toolkit?"));

export type ToolkitPick = { kind: "tool"; tool: ToolSuggestion } | { kind: "skill"; skill: SkillSuggestion };

export function ToolkitPanel({ slug, canDecide = true }: { slug: string; canDecide?: boolean }) {
  const q = useApi<ToolkitPayload>(slug === "commons" ? null : "toolkit?project=" + encodeURIComponent(slug), { refresh: 120000 });
  if (slug === "commons") return null;
  const d = q.data;
  const tools = d?.tools ?? [];
  const skills = d?.skills ?? [];
  const count = tools.length + skills.length;
  const summary = d ? (count ? [tools.length ? plural(tools.length, "tool") : "", skills.length ? plural(skills.length, "skill") : ""].filter(Boolean).join(", ") + " suggested" : "Nothing missing") : undefined;
  return (
    <Panel title="Toolkit" icon={PlugZapIcon} actions={summary ? <span>{summary}</span> : null}>
      {!d ? (
        <p className="text-[13px] text-muted-foreground">{q.error ? "The toolkit didn't load (" + q.error + "). It refreshes on its own every two minutes." : "Reading the checkout…"}</p>
      ) : count === 0 ? (
        <div className="grid gap-2">
          <p className="text-sm leading-relaxed text-muted-foreground">
            {slug} has the tools and skills its shape asks for. PULSE reads the checkout again as it changes - a new dependency, a compose file, a UI - and suggests what fits here and on the board.
          </p>
          <p className="text-[12.5px] leading-relaxed text-muted-foreground">
            Everything connected is on the{" "}
            <Link href={hrefs.connections({ project: slug })} className="underline decoration-foreground/25 underline-offset-4 hover:decoration-foreground/60">
              Connections
            </Link>{" "}
            page. From a shell: <Mono>{"pulse toolkit --project " + slug}</Mono>.
          </p>
        </div>
      ) : (
        <div className="grid gap-3">
          <p className="text-sm leading-relaxed text-muted-foreground">
            From what {slug} shows - its manifests, files and remote - PULSE suggests {count === 1 ? "one thing" : num(count) + " things"} agents could use. Pick a card to read why.
          </p>
          <ToolkitGallery project={slug} tools={tools} skills={skills} canDecide={canDecide} />
          <p className="text-[12.5px] leading-relaxed text-muted-foreground">
            From a shell: <Mono>{"pulse toolkit --project " + slug}</Mono> shows the same, <Mono>{"pulse toolkit add tool|skill NAME --project " + slug}</Mono> adds one. The daemon asks about these on the board too, one at a time; a yes there adds it, a declined thread rests for a month.
          </p>
        </div>
      )}
    </Panel>
  );
}

function num(n: number): string {
  return ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine"][n] ?? String(n);
}

// The cards and the footer for one project's suggestions. `only` narrows
// to one kind (the Connections page shows tools under Tools, skills under
// Skills); without it both kinds share the grid, tools first.
export function ToolkitGallery({ project, tools, skills, canDecide, only, onAdded }: { project: string; tools: ToolSuggestion[]; skills: SkillSuggestion[]; canDecide: boolean; only?: "tool" | "skill"; onAdded?: () => void }) {
  const options = useApi<McpOptions>("mcp/options", { refresh: false });
  const [pick, setPick] = useState("");
  const [busy, setBusy] = useState("");
  const [form, setForm] = useState<ToolkitPick | null>(null);
  const picks: ToolkitPick[] = [...(only === "skill" ? [] : tools.map((tool) => ({ kind: "tool", tool }) as const)), ...(only === "tool" ? [] : skills.map((skill) => ({ kind: "skill", skill }) as const))];
  const chosen = picks.find((p) => keyOf(p) === pick) ?? null;
  const choose = (k: string) => setPick((p) => (p === k ? "" : k));
  const done = () => {
    refreshToolkit();
    refreshMcp();
    refreshSkills();
    onAdded?.();
  };
  const add = async (p: ToolkitPick) => {
    const k = keyOf(p);
    const name = p.kind === "tool" ? p.tool.title : p.skill.name;
    setBusy(k);
    const id = toast.loading("Adding " + name + "…");
    try {
      const r = await writes.addToolkit({ project, kind: p.kind, name: p.kind === "tool" ? p.tool.name : p.skill.name });
      if (p.kind === "tool") {
        const after = p.tool.after.trim();
        toast.success(name + " added" + (r.server ? " as " + r.server.id : "") + ".", { id, description: after ? after[0].toUpperCase() + after.slice(1) + (after.endsWith(".") ? "" : ".") : "Agents see it in their brief while it is on." });
      } else {
        toast.success(name + " added" + (r.skill ? " as " + r.skill.id : "") + ".", { id, description: "Agents read it before work it covers. Edit it on the Connections page any time." });
      }
      setPick("");
      done();
    } catch (e) {
      toast.dismiss(id);
      showApiError(e);
    } finally {
      setBusy("");
    }
  };
  if (!picks.length) return null;
  return (
    <div className="grid gap-3" role="group" aria-label="Suggested tools and skills">
      <ul className="grid grid-cols-2 gap-2.5 sm:grid-cols-3 md:grid-cols-4 xl:grid-cols-6">
        {picks.map((p, i) => (
          <PickCard key={keyOf(p)} index={i} pick={p} picked={pick === keyOf(p)} onPick={() => choose(keyOf(p))} />
        ))}
      </ul>
      {chosen ? (
        <PickDetail
          key={keyOf(chosen)}
          project={project}
          pick={chosen}
          busy={busy === keyOf(chosen)}
          canDecide={canDecide}
          onAdd={() => void add(chosen)}
          onForm={() => setForm(chosen)}
          onClose={() => setPick("")}
        />
      ) : null}
      {form?.kind === "tool" ? (
        <McpServerDialog
          open
          onOpenChange={(o) => {
            if (!o) {
              setForm(null);
              done();
            }
          }}
          options={options.data ?? DEFAULT_OPTIONS}
          initial={toolForm(form.tool)}
        />
      ) : form?.kind === "skill" ? (
        <SkillDialog
          open
          onOpenChange={(o) => {
            if (!o) {
              setForm(null);
              done();
            }
          }}
          initial={skillForm(form.skill)}
        />
      ) : null}
    </div>
  );
}

function keyOf(p: ToolkitPick): string {
  return p.kind === "tool" ? "tool:" + p.tool.name : "skill:" + p.skill.name;
}

// A tool that signs in or needs no credential adds on a click; one that
// needs a secret opens the form with everything but the secret filled in.
function needsSecret(t: ToolSuggestion): boolean {
  return t.auth !== "none" && t.auth !== "oauth";
}

function toolForm(t: ToolSuggestion): Partial<McpForm> {
  return {
    name: t.name,
    project: t.scope === "shared" ? "" : t.project,
    transport: t.transport as McpTransport,
    command: t.command,
    args: t.args.join("\n"),
    url: t.url,
    auth: t.auth as McpAuth,
    auth_header: t.auth_header,
    note: t.about + " Suggested by pulse toolkit for " + t.project + ".",
  };
}

function skillForm(k: SkillSuggestion): Partial<SkillForm> {
  return { name: k.name, project: k.project, description: k.description, body: k.body };
}

// A steady hue per name, so the plugs in a row are not all the same colour.
function hueOf(name: string): string {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) % 360;
  return "oklch(0.78 0.12 " + h + ")";
}

function PickCard({ index, pick: p, picked, onPick }: { index: number; pick: ToolkitPick; picked: boolean; onPick: () => void }) {
  const tool = p.kind === "tool" ? p.tool : null;
  const title = tool ? tool.title : p.kind === "skill" ? p.skill.name : "";
  const sub = tool ? (tool.auth === "oauth" ? "signs in once" : needsSecret(tool) ? "needs a secret" : TRANSPORT_SHORT[tool.transport as McpTransport] ?? tool.transport) : "a drafted skill";
  return (
    <li className="min-w-0 animate-in fade-in-0 slide-in-from-bottom-2 fill-mode-both duration-300" style={{ animationDelay: Math.min(index, 14) * 45 + "ms" }}>
      <button
        type="button"
        onClick={onPick}
        aria-pressed={picked}
        data-picked={picked}
        className={cn(
          "mascot-card lift relative flex h-full w-full flex-col items-center gap-1.5 rounded-2xl border bg-card px-2 pt-4 pb-3 text-center outline-none transition-[transform,border-color,box-shadow] duration-300 ease-[var(--ease-out-soft)] focus-visible:ring-2 focus-visible:ring-ring/60",
          picked ? "-translate-y-0.5 border-primary shadow-md ring-2 ring-primary/25" : "hover:-translate-y-0.5",
        )}
        style={{ "--mascot-delay": (index % 5) * 0.9 + "s" } as React.CSSProperties}
      >
        {picked ? (
          <span className="absolute top-2 right-2 flex size-5 items-center justify-center rounded-full bg-primary text-primary-foreground animate-in zoom-in-50 duration-200" aria-hidden>
            <CheckIcon className="size-3" />
          </span>
        ) : tool && needsSecret(tool) ? (
          <span className="absolute top-2 right-2 flex size-5 items-center justify-center rounded-full bg-muted text-muted-foreground" title="Needs a secret you hold" aria-label="Needs a secret">
            <KeyRoundIcon className="size-3" />
          </span>
        ) : null}
        <RoleMascot role={p.kind} size={64} mood={picked ? "picked" : "idle"} title={title} color={tool ? hueOf(tool.name) : undefined} />
        <span className="mt-0.5 text-[13px] leading-tight font-medium text-foreground">{title}</span>
        <span className="text-[11.5px] text-muted-foreground">{sub}</span>
      </button>
    </li>
  );
}

function PickDetail({ project, pick: p, busy, canDecide, onAdd, onForm, onClose }: { project: string; pick: ToolkitPick; busy: boolean; canDecide: boolean; onAdd: () => void; onForm: () => void; onClose: () => void }) {
  if (p.kind === "tool") {
    const t = p.tool;
    const secret = needsSecret(t);
    const reach = t.transport === "stdio" ? [t.command, ...t.args].join(" ") : t.url;
    return (
      <div className="grid gap-3 rounded-xl border bg-card px-4 py-3 animate-in fade-in-0 slide-in-from-bottom-1 duration-300">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-sm font-medium text-foreground">{t.title}</span>
              <ToneBadge tone="neutral">{t.scope === "shared" ? "every project" : project}</ToneBadge>
              <ToneBadge tone={secret ? "warn" : "ok"} dot>
                {t.auth === "oauth" ? "signs in once" : secret ? "needs a secret" : "no credential"}
              </ToneBadge>
            </div>
            <p className="mt-1 text-sm leading-relaxed text-foreground">{t.about}</p>
            <p className="mt-1 text-[13px] leading-relaxed text-muted-foreground">
              <span className="text-you-ink">Why here:</span> {t.why}
            </p>
            <p className="mt-1.5 text-[12.5px] leading-relaxed text-muted-foreground">
              {TRANSPORT_SHORT[t.transport as McpTransport] ?? t.transport}
              {": "}
              <Mono>{reach}</Mono>
              {secret ? (
                <>
                  {" · the secret is "}
                  <span className="text-foreground/80">{t.secret}</span>
                  {t.auth === "env" ? (
                    <>
                      {", passed as "}
                      <Mono>{t.auth_header}</Mono>
                    </>
                  ) : null}
                </>
              ) : null}
              {t.docs ? (
                <>
                  {" · "}
                  <a href={t.docs} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 underline decoration-foreground/25 underline-offset-4 hover:decoration-foreground/60">
                    docs <ExternalLinkIcon className="size-3" />
                  </a>
                </>
              ) : null}
            </p>
          </div>
          <Button size="icon" variant="ghost" aria-label="Close" onClick={onClose}>
            <XIcon />
          </Button>
        </div>
        <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
          <p className="min-w-0 text-[12.5px] leading-relaxed text-muted-foreground">
            From a shell: <Mono>{t.add}</Mono>
            {t.after ? " - then " + t.after + "." : ""}
          </p>
          {canDecide ? (
            secret ? (
              <Button size="sm" disabled={busy} onClick={onForm}>
                <KeyRoundIcon /> Open the form
              </Button>
            ) : (
              <Button size="sm" disabled={busy} onClick={onAdd}>
                {busy ? <Loader2Icon className="animate-spin" /> : <PlusIcon />} {t.scope === "shared" ? "Add for every project" : "Add to " + project}
              </Button>
            )
          ) : null}
        </div>
      </div>
    );
  }
  const k = p.skill;
  return (
    <div className="grid gap-3 rounded-xl border bg-card px-4 py-3 animate-in fade-in-0 slide-in-from-bottom-1 duration-300">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-medium text-foreground">{k.name}</span>
            <ToneBadge tone="neutral">{project}</ToneBadge>
            <ToneBadge tone="info">
              <BookOpenIcon className="size-3" /> skill
            </ToneBadge>
          </div>
          <p className="mt-1 text-sm leading-relaxed text-foreground">{k.description}.</p>
          <p className="mt-1 text-[13px] leading-relaxed text-muted-foreground">
            <span className="text-you-ink">Why here:</span> {k.why}
          </p>
        </div>
        <Button size="icon" variant="ghost" aria-label="Close" onClick={onClose}>
          <XIcon />
        </Button>
      </div>
      <Disclosure summary="Read the draft" contentClassName="min-w-0">
        <pre className="max-h-72 overflow-auto rounded-lg border bg-muted/40 px-3 py-2 font-mono text-[12px] leading-relaxed whitespace-pre-wrap text-foreground/85">{k.body}</pre>
      </Disclosure>
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <p className="min-w-0 text-[12.5px] leading-relaxed text-muted-foreground">
          Drafted from what {project} shows; agents read it before work it covers. From a shell: <Mono>{k.add}</Mono>
        </p>
        {canDecide ? (
          <div className="flex shrink-0 flex-wrap items-center gap-1.5">
            <Button size="sm" variant="outline" disabled={busy} onClick={onForm}>
              <PenLineIcon /> Edit first
            </Button>
            <Button size="sm" disabled={busy} onClick={onAdd}>
              {busy ? <Loader2Icon className="animate-spin" /> : <PlusIcon />} Add the skill
            </Button>
          </div>
        ) : null}
      </div>
    </div>
  );
}
