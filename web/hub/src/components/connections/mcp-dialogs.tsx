"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { CheckIcon, Loader2Icon, PlugZapIcon, PlusIcon, TrashIcon, WrenchIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { showApiError } from "@/components/pulse/confirm";
import { Notice } from "@/components/pulse/panel";
import { Mono } from "@/components/pulse/who";
import { errorText, writes, type McpAuth, type McpConnectResult, type McpOptions, type McpServer, type McpTransport, type ProjectsPayload } from "@/lib/api";
import { EMPTY_MCP_FORM, TRANSPORT_BLURB, TRANSPORT_LABEL, authExplain, authFieldLabel, authFieldPlaceholder, authsFor, formFromServer, mcpInput, mcpPatch, needsSecret, secretHint, validateMcpForm, type EnvRow, type McpForm } from "@/lib/mcp";
import { hrefs } from "@/lib/nav";
import { invalidate, useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// The owner registers an MCP server the way `pulse mcp add` does: a name,
// the project it serves (or every project), how to reach it, how it
// authenticates, and where the secret lives - a pulse-cred reference like
// the push key's, never the secret. The hub validates again and records
// mcp.added / mcp.updated.

export const refreshMcp = () => invalidate((p) => p === "mcp" || p.startsWith("mcp?") || p.startsWith("mcp/") || p.startsWith("events"));

const ALL = "__all__";

export function McpServerDialog({ open, onOpenChange, options, server, project }: { open: boolean; onOpenChange: (o: boolean) => void; options: McpOptions; server?: McpServer | null; project?: string }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto sm:max-w-2xl">
        {open ? <McpFormBody options={options} server={server ?? null} project={project ?? ""} close={() => onOpenChange(false)} /> : null}
      </DialogContent>
    </Dialog>
  );
}

// The form's state lives inside the open dialog so it starts over each time.
function McpFormBody({ options, server, project, close }: { options: McpOptions; server: McpServer | null; project: string; close: () => void }) {
  const { data: projects } = useApi<ProjectsPayload>("projects", { refresh: false });
  const [f, setF] = useState<McpForm>(() => (server ? formFromServer(server) : { ...EMPTY_MCP_FORM, project }));
  const [busy, setBusy] = useState(false);
  const [touched, setTouched] = useState(false);
  const set = <K extends keyof McpForm>(k: K, v: McpForm[K]) => setF((cur) => ({ ...cur, [k]: v }));
  const errors = validateMcpForm(f, options);
  const auths = authsFor(options, f.transport);
  const fieldLabel = authFieldLabel(f.auth);
  const slugs = (projects?.projects ?? []).filter((p) => p.status === "active").map((p) => p.slug);
  const knownProject = !f.project || slugs.includes(f.project);

  // Changing the transport can leave an auth that does not fit; fall back to none.
  const setTransport = (t: McpTransport) => {
    setF((cur) => ({ ...cur, transport: t, auth: authsFor(options, t).some((a) => a.name === cur.auth) ? cur.auth : "none" }));
  };

  const submit = async () => {
    setTouched(true);
    if (errors.length) return;
    setBusy(true);
    const id = toast.loading(server ? "Saving " + server.name + "…" : "Adding " + f.name.trim() + "…");
    try {
      if (server) {
        const patch = mcpPatch(server, f);
        if (!Object.keys(patch).length) {
          toast.dismiss(id);
          toast.message("Nothing changed.");
          close();
          return;
        }
        const r = await writes.updateMcp(server.id, patch);
        toast.success(r.server.name + " saved.", { id, description: "Connect to check that it still answers." });
      } else {
        const r = await writes.addMcp(mcpInput(f));
        toast.success(r.server.id + " added.", { id, description: "Connect it to see the tools it offers; agents see it in their brief once it is enabled." });
      }
      refreshMcp();
      close();
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, server ? "The server was not saved" : "The server was not added");
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <DialogHeader>
        <DialogTitle>{server ? "Edit " + server.name : "Add an MCP server"}</DialogTitle>
        <DialogDescription>
          Agents reach it from their shell with <Mono>pulse mcp call NAME TOOL</Mono>. The secret stays where it lives; PULSE keeps only a reference, like the push key.
        </DialogDescription>
      </DialogHeader>
      <div className="grid gap-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="grid gap-1.5">
            <Label htmlFor="mcp-name">Name</Label>
            <Input id="mcp-name" value={f.name} onChange={(e) => set("name", e.target.value)} placeholder="github" maxLength={64} spellCheck={false} autoFocus={!server} className="font-mono text-[13px]" />
            <p className="text-xs text-muted-foreground">A slug; agents use it as-is in the bridge command.</p>
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="mcp-project">Project</Label>
            <Select value={f.project || ALL} onValueChange={(v) => set("project", v === ALL ? "" : v)}>
              <SelectTrigger id="mcp-project" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL}>All projects</SelectItem>
                {!knownProject ? <SelectItem value={f.project}>{f.project}</SelectItem> : null}
                {slugs.map((slug) => (
                  <SelectItem key={slug} value={slug}>
                    {slug}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">Which workers see it in their brief. A project row with the same name wins over the global one.</p>
          </div>
        </div>

        <fieldset className="grid gap-2">
          <legend className="mb-1.5 text-sm font-medium">Transport</legend>
          <div className="grid gap-2 sm:grid-cols-3">
            {options.transports.map((t) => (
              <label key={t} className={cn("surface-glass flex cursor-pointer items-start gap-2.5 rounded-xl border px-3 py-2.5 text-left transition-colors has-[:checked]:border-primary/60 has-[:checked]:bg-primary/5", "hover:bg-muted/50")}>
                <input type="radio" name="mcp-transport" value={t} checked={f.transport === t} onChange={() => setTransport(t)} className="mt-1 size-3.5 shrink-0 accent-primary" />
                <span className="min-w-0">
                  <span className="block text-sm font-medium">{TRANSPORT_LABEL[t]}</span>
                  <span className="block text-xs leading-snug text-muted-foreground">{TRANSPORT_BLURB[t]}</span>
                </span>
              </label>
            ))}
          </div>
        </fieldset>

        {f.transport === "stdio" ? (
          <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
            <div className="grid gap-1.5">
              <Label htmlFor="mcp-command">Command</Label>
              <Input id="mcp-command" value={f.command} onChange={(e) => set("command", e.target.value)} placeholder="npx" spellCheck={false} className="font-mono text-[13px]" />
              <p className="text-xs text-muted-foreground">Run on this machine, as the daemon&apos;s user; it must be on the PATH or absolute.</p>
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="mcp-args">Arguments</Label>
              <Textarea id="mcp-args" value={f.args} onChange={(e) => set("args", e.target.value)} rows={3} placeholder={"-y\n@modelcontextprotocol/server-github"} spellCheck={false} className="min-h-[4.5rem] font-mono text-[13px]" />
              <p className="text-xs text-muted-foreground">One per line.</p>
            </div>
          </div>
        ) : (
          <div className="grid gap-1.5">
            <Label htmlFor="mcp-url">URL</Label>
            <Input id="mcp-url" value={f.url} onChange={(e) => set("url", e.target.value)} placeholder="https://mcp.example.com/mcp" spellCheck={false} className="font-mono text-[13px]" />
            <p className="text-xs text-muted-foreground">{f.transport === "http" ? "The endpoint that takes POST; the session id the server hands back is kept for the conversation." : "The event-stream URL; the server announces its POST endpoint on it."}</p>
          </div>
        )}

        <div className="grid gap-3 sm:grid-cols-2">
          <div className="grid gap-1.5">
            <Label htmlFor="mcp-auth">Auth</Label>
            <Select value={f.auth} onValueChange={(v) => set("auth", v as McpAuth)}>
              <SelectTrigger id="mcp-auth" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {auths.map((a) => (
                  <SelectItem key={a.name} value={a.name}>
                    <span className="flex flex-col items-start text-left">
                      <span className="font-medium">{a.name}</span>
                      <span className="text-xs text-muted-foreground">{a.explain}</span>
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground anywhere">{authExplain(options, f.auth)}</p>
          </div>
          {fieldLabel ? (
            <div className="grid gap-1.5">
              <Label htmlFor="mcp-auth-header">{fieldLabel}</Label>
              <Input id="mcp-auth-header" value={f.auth_header} onChange={(e) => set("auth_header", e.target.value)} placeholder={authFieldPlaceholder(f.auth)} spellCheck={false} className="font-mono text-[13px]" />
              <p className="text-xs text-muted-foreground">{f.auth === "env" ? "Set in the command's environment with the secret as its value." : f.auth === "query" ? "Appended to the URL with the secret as its value." : "Sent with every request with the secret as its value."}</p>
            </div>
          ) : null}
        </div>

        {needsSecret(f.auth) ? (
          <div className="grid gap-1.5">
            <Label htmlFor="mcp-secret">Secret reference</Label>
            <Input id="mcp-secret" value={f.secret_ref} onChange={(e) => set("secret_ref", e.target.value)} placeholder="op://PULSE/github-mcp/token" spellCheck={false} className="font-mono text-[13px]" />
            <p className="text-xs text-muted-foreground anywhere">
              Where the secret lives, never the secret: {options.cred_shapes}. {secretHint(f.auth)} The same providers as the push key - a 1Password token kept with <Mono>pulse cred token</Mono> or in{" "}
              <Link href={hrefs.settings("runner")} className="underline underline-offset-2">
                Settings
              </Link>{" "}
              serves op:// references here too.
            </p>
          </div>
        ) : null}

        {f.transport === "stdio" ? <EnvEditor rows={f.env} onChange={(rows) => set("env", rows)} /> : null}

        <div className="grid gap-1.5">
          <Label htmlFor="mcp-note">Note</Label>
          <Textarea id="mcp-note" value={f.note} onChange={(e) => set("note", e.target.value)} rows={2} placeholder="What it is for: issues and pull requests of every project; read before planning." className="min-h-[3.5rem]" />
          <p className="text-xs text-muted-foreground">Shown to agents in their brief next to the name - say what the server is for and when to use it.</p>
        </div>

        {touched && errors.length ? (
          <Notice tone="warn">
            <ul className="list-disc pl-4">
              {errors.map((e) => (
                <li key={e}>{e}</li>
              ))}
            </ul>
          </Notice>
        ) : null}
      </div>
      <DialogFooter className="items-center gap-2">
        <Button variant="ghost" onClick={close} disabled={busy}>
          Cancel
        </Button>
        <Button onClick={() => void submit()} disabled={busy || (touched && errors.length > 0)}>
          {busy ? <Loader2Icon className="animate-spin" /> : server ? <CheckIcon /> : <PlusIcon />}
          {server ? "Save" : "Add server"}
        </Button>
      </DialogFooter>
    </>
  );
}

// Environment for a stdio command. A value may be a pulse-cred reference,
// resolved when the server starts, so a token never sits in the store.
function EnvEditor({ rows, onChange }: { rows: EnvRow[]; onChange: (rows: EnvRow[]) => void }) {
  const update = (i: number, patch: Partial<EnvRow>) => onChange(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  return (
    <div className="grid gap-1.5">
      <div className="flex items-center justify-between">
        <Label>Environment</Label>
        <Button type="button" size="xs" variant="outline" onClick={() => onChange([...rows, { key: "", value: "" }])}>
          <PlusIcon /> Add variable
        </Button>
      </div>
      {rows.length ? (
        <div className="grid gap-1.5">
          {rows.map((r, i) => (
            <div key={i} className="grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)_auto] items-center gap-1.5">
              <Input aria-label={"Variable " + (i + 1) + " name"} value={r.key} onChange={(e) => update(i, { key: e.target.value })} placeholder="GITHUB_HOST" spellCheck={false} className="font-mono text-[12.5px]" />
              <Input aria-label={"Variable " + (i + 1) + " value"} value={r.value} onChange={(e) => update(i, { value: e.target.value })} placeholder="value or op://… reference" spellCheck={false} className="font-mono text-[12.5px]" />
              <Button type="button" size="icon-sm" variant="ghost" onClick={() => onChange(rows.filter((_, j) => j !== i))} aria-label={"Remove variable " + (r.key || i + 1)}>
                <TrashIcon />
              </Button>
            </div>
          ))}
        </div>
      ) : null}
      <p className="text-xs text-muted-foreground">Plain values, or references (op://, keychain://, file://) read when the command starts. A value that looks like a token is refused.</p>
    </div>
  );
}

// ----- connect ---------------------------------------------------------------

type ConnectState = { kind: "running" } | { kind: "ok"; result: McpConnectResult } | { kind: "failed"; error: string; why: string; fix: string };

// Connect = initialize + tools/list, recorded on the row as its last check.
// The dialog opens running, then shows the server's name and version with
// its tools, or what went wrong and what to do about it.
export function ConnectDialog({ server, onOpenChange }: { server: McpServer | null; onOpenChange: (o: boolean) => void }) {
  return (
    <Dialog open={!!server} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto sm:max-w-xl">{server ? <ConnectBody server={server} /> : null}</DialogContent>
    </Dialog>
  );
}

function ConnectBody({ server }: { server: McpServer }) {
  // One attempt per (server, counter); "Try again" bumps the counter and
  // resets the view in the same click, so the effect only reports results.
  const [run, setRun] = useState<{ attempt: number; state: ConnectState }>({ attempt: 0, state: { kind: "running" } });
  const state = run.state;
  const attempt = run.attempt;
  useEffect(() => {
    let cancelled = false;
    writes
      .connectMcp(server.id)
      .then((result) => {
        if (cancelled) return;
        setRun({ attempt, state: { kind: "ok", result } });
        refreshMcp();
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setRun({ attempt, state: { kind: "failed", ...errorText(e) } });
        refreshMcp();
      });
    return () => {
      cancelled = true;
    };
  }, [server.id, attempt]);
  return (
    <>
      <DialogHeader>
        <DialogTitle className="flex items-center gap-2">
          <PlugZapIcon className="size-4 text-primary" aria-hidden /> Connect {server.name}
        </DialogTitle>
        <DialogDescription>
          The hub resolves the reference, speaks <Mono>initialize</Mono> and asks for the tools. What it learns is written on the row and shown to agents in their brief.
        </DialogDescription>
      </DialogHeader>
      {state.kind === "running" ? (
        <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground" role="status" aria-live="polite">
          <Loader2Icon className="size-4 animate-spin" /> Connecting over {server.transport}…
        </div>
      ) : state.kind === "ok" ? (
        <div className="grid gap-3">
          <Notice tone="ok">
            <span className="font-medium">{state.result.info.name || server.name}</span>
            {state.result.info.version ? <span> {state.result.info.version}</span> : null}
            {state.result.info.protocol_version ? <span className="text-muted-foreground"> · protocol {state.result.info.protocol_version}</span> : null}
            <span className="block text-xs text-muted-foreground">{state.result.note}</span>
          </Notice>
          {state.result.info.instructions ? <p className="text-sm leading-relaxed text-muted-foreground anywhere">{state.result.info.instructions}</p> : null}
          {state.result.tools.length ? (
            <ul className="grid max-h-80 gap-1.5 overflow-y-auto pr-1">
              {state.result.tools.map((t) => (
                <li key={t.name} className="surface-glass rounded-xl border px-3 py-2">
                  <div className="flex items-center gap-2">
                    <WrenchIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
                    <Mono>{t.name}</Mono>
                  </div>
                  {t.description ? <p className="mt-1 text-xs leading-snug text-muted-foreground anywhere">{t.description}</p> : null}
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-sm text-muted-foreground">The server answered but offers no tools.</p>
          )}
          <p className="text-xs text-muted-foreground">
            Agents call one with <Mono>pulse mcp call {server.name} TOOL --args &apos;{"{...}"}&apos;</Mono>.
          </p>
        </div>
      ) : (
        <div className="grid gap-3">
          <Notice tone="bad">
            <span className="font-medium anywhere">{state.error}</span>
            {state.why ? <span className="block text-xs anywhere">why: {state.why}</span> : null}
            {state.fix ? <span className="block text-xs anywhere">fix: {state.fix}</span> : null}
          </Notice>
          <div>
            <Button size="sm" variant="outline" onClick={() => setRun((cur) => ({ attempt: cur.attempt + 1, state: { kind: "running" } }))}>
              Try again
            </Button>
          </div>
        </div>
      )}
    </>
  );
}
