"use client";

import Link from "next/link";
import { useEffect, useRef, useState } from "react";
import { CheckIcon, CopyIcon, ExternalLinkIcon, KeyRoundIcon, Loader2Icon, PlugZapIcon, PlusIcon, TrashIcon, WrenchIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { showApiError } from "@/components/pulse/confirm";
import { copyText } from "@/components/pulse/evolve";
import { PulseMascot } from "@/components/pulse/mascots";
import { Disclosure, Notice } from "@/components/pulse/panel";
import { Mono } from "@/components/pulse/who";
import { api, enc, errorText, writes, type McpAuth, type McpConnectResult, type McpLoginFlow, type McpLoginInput, type McpLoginState, type McpOptions, type McpServer, type McpTransport, type ProjectsPayload } from "@/lib/api";
import { EMPTY_MCP_FORM, TRANSPORT_BLURB, TRANSPORT_LABEL, authExplain, authLabel, authFieldLabel, authFieldPlaceholder, authsFor, formFromServer, mcpInput, mcpPatch, mcpFormReady, needsSecret, secretHint, validateMcpForm, type EnvRow, type McpForm } from "@/lib/mcp";
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

// `initial` pre-fills a new server's form (the toolkit's suggestions use
// it); the owner still reads every field and adds the secret reference.
export function McpServerDialog({ open, onOpenChange, options, server, project, initial }: { open: boolean; onOpenChange: (o: boolean) => void; options: McpOptions; server?: McpServer | null; project?: string; initial?: Partial<McpForm> }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-2xl">{open ? <McpFormBody options={options} server={server ?? null} project={project ?? ""} initial={initial} close={() => onOpenChange(false)} /> : null}</DialogContent>
    </Dialog>
  );
}

// The form's state lives inside the open dialog so it starts over each time.
function McpFormBody({ options, server, project, initial, close }: { options: McpOptions; server: McpServer | null; project: string; initial?: Partial<McpForm>; close: () => void }) {
  const { data: projects } = useApi<ProjectsPayload>("projects", { refresh: false });
  const [f, setF] = useState<McpForm>(() => (server ? formFromServer(server) : { ...EMPTY_MCP_FORM, project, ...initial }));
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
        toast.success(r.server.name + " saved.", { id, description: "Check it to make sure it still answers." });
      } else {
        const r = await writes.addMcp(mcpInput(f));
        toast.success(r.server.name + " added.", { id, description: "Connect it to see what it offers. Agents see it in their brief while it is on." });
      }
      refreshMcp();
      close();
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, server ? "The tool wasn't saved" : "The tool wasn't added");
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <DialogHeader>
        <DialogTitle>{server ? "Edit " + server.name : "Add a tool"}</DialogTitle>
        <DialogDescription>Connect an MCP server so agents can use what it offers. If it needs a secret, the secret stays where you keep it; PULSE only stores where to find it.</DialogDescription>
      </DialogHeader>
      <div className="grid gap-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="grid content-start gap-1.5">
            <Label htmlFor="mcp-name">Name</Label>
            <Input id="mcp-name" value={f.name} onChange={(e) => set("name", e.target.value)} placeholder="github" maxLength={64} spellCheck={false} autoFocus={!server} className="font-mono text-[13px]" />
            <p className="text-xs text-muted-foreground">Short and lowercase, like github. Agents call the tool by this name.</p>
          </div>
          <div className="grid content-start gap-1.5">
            <Label htmlFor="mcp-project">Who can use it</Label>
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
            <p className="text-xs text-muted-foreground">Agents of every project, or of one. A project&apos;s own tool replaces a shared one with the same name.</p>
          </div>
        </div>

        <fieldset className="grid gap-2">
          <legend className="mb-1.5 text-sm font-medium">How PULSE reaches it</legend>
          <div className="grid gap-2 sm:grid-cols-3">
            {options.transports.map((t) => (
              <label key={t} className={cn("flex cursor-pointer items-start gap-2.5 rounded-xl border bg-card px-3 py-2.5 text-left transition-colors has-[:checked]:border-primary/60 has-[:checked]:bg-primary/5", "hover:bg-muted/50")}>
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
            <div className="grid content-start gap-1.5">
              <Label htmlFor="mcp-command">Command</Label>
              <Input id="mcp-command" value={f.command} onChange={(e) => set("command", e.target.value)} placeholder="npx" spellCheck={false} className="font-mono text-[13px]" />
              <p className="text-xs text-muted-foreground">The program to start on this machine, by name (on the PATH) or full path. It runs as the runner&apos;s user. A whole line such as <code>npx -y @azure/mcp@latest server start</code> works too: the first word is the program, the rest its arguments.</p>
            </div>
            <div className="grid content-start gap-1.5">
              <Label htmlFor="mcp-args">Arguments</Label>
              <Textarea id="mcp-args" value={f.args} onChange={(e) => set("args", e.target.value)} rows={3} placeholder={"-y\n@modelcontextprotocol/server-github"} spellCheck={false} className="min-h-[4.5rem] font-mono text-[13px]" />
              <p className="text-xs text-muted-foreground">What follows the command: one per line, or several on a line separated by spaces as in a shell. Quote an argument that holds spaces.</p>
            </div>
          </div>
        ) : (
          <div className="grid content-start gap-1.5">
            <Label htmlFor="mcp-url">Web address (URL)</Label>
            <Input id="mcp-url" value={f.url} onChange={(e) => set("url", e.target.value)} placeholder="https://mcp.example.com/mcp" spellCheck={false} className="font-mono text-[13px]" />
            <p className="text-xs text-muted-foreground">{f.transport === "http" ? "The address the server answers on, often ending in /mcp." : "The address of the event stream. The server tells PULSE where to send requests."}</p>
          </div>
        )}

        <div className="grid gap-3 sm:grid-cols-2">
          <div className="grid content-start gap-1.5">
            <Label htmlFor="mcp-auth">Sign-in</Label>
            <Select value={f.auth} onValueChange={(v) => set("auth", v as McpAuth)}>
              <SelectTrigger id="mcp-auth" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {auths.map((a) => (
                  <SelectItem key={a.name} value={a.name}>
                    {authLabel(a.name)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground anywhere">{authExplain(options, f.auth)}</p>
          </div>
          {fieldLabel ? (
            <div className="grid content-start gap-1.5">
              <Label htmlFor="mcp-auth-header">{fieldLabel}</Label>
              <Input id="mcp-auth-header" value={f.auth_header} onChange={(e) => set("auth_header", e.target.value)} placeholder={authFieldPlaceholder(f.auth)} spellCheck={false} className="font-mono text-[13px]" />
              <p className="text-xs text-muted-foreground">
                {f.auth === "env" ? "PULSE sets this variable for the program, with the secret as its value." : f.auth === "query" ? "PULSE adds this parameter to the address, with the secret as its value." : "PULSE sends this header with every request, with the secret as its value."}
              </p>
            </div>
          ) : null}
        </div>

        {f.auth === "oauth" ? (
          <Notice tone="info">
            After saving, sign in once: the tool&apos;s card gets a <span className="font-medium">Sign in</span> button that walks you through it here, and a terminal on this machine can run <Mono>pulse mcp login {f.name.trim() || "NAME"}</Mono> instead. PULSE keeps the tokens on this machine and renews them; <span className="font-medium">Sign out</span> forgets them.
          </Notice>
        ) : null}

        {needsSecret(f.auth) ? (
          <div className="grid content-start gap-1.5">
            <Label htmlFor="mcp-secret">Where the secret is kept</Label>
            <Input id="mcp-secret" value={f.secret_ref} onChange={(e) => set("secret_ref", e.target.value)} placeholder="op://PULSE/github-mcp/token" spellCheck={false} className="font-mono text-[13px]" />
            <p className="text-xs text-muted-foreground anywhere">
              A reference, never the secret itself: <span className="font-mono text-[12px]">{options.cred_shapes}</span>. {secretHint(f.auth)} The 1Password token saved in{" "}
              <Link href={hrefs.settings("runner")} className="underline underline-offset-2">
                Settings
              </Link>{" "}
              (or with <Mono>pulse cred token</Mono>) also unlocks op:// references here.
            </p>
          </div>
        ) : null}

        {f.transport === "stdio" ? <EnvEditor rows={f.env} onChange={(rows) => set("env", rows)} /> : null}

        <div className="grid content-start gap-1.5">
          <Label htmlFor="mcp-note">What it&apos;s for</Label>
          <Textarea id="mcp-note" value={f.note} onChange={(e) => set("note", e.target.value)} rows={2} placeholder="What it is for: issues and pull requests of every project; read before planning." className="min-h-[3.5rem]" />
          <p className="text-xs text-muted-foreground">Agents read this next to the name. Say what the tool is for and when to use it.</p>
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
        <Button onClick={() => void submit()} disabled={busy || !mcpFormReady(f) || (touched && errors.length > 0)}>
          {busy ? <Loader2Icon className="animate-spin" /> : server ? <CheckIcon /> : <PlusIcon />}
          {server ? "Save" : "Add tool"}
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
    <div className="grid content-start gap-1.5">
      <div className="flex items-center justify-between">
        <Label>Environment variables</Label>
        <Button type="button" size="xs" variant="outline" onClick={() => onChange([...rows, { key: "", value: "" }])}>
          <PlusIcon /> Add a variable
        </Button>
      </div>
      {rows.length ? (
        <div className="grid content-start gap-1.5">
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
      <p className="text-xs text-muted-foreground">Plain values, or references (op://, keychain://, file://) PULSE reads when the program starts. A value that looks like a token is refused: use a reference instead.</p>
    </div>
  );
}

// ----- connect ---------------------------------------------------------------

type ConnectState = { kind: "running" } | { kind: "ok"; result: McpConnectResult } | { kind: "failed"; error: string; why: string; fix: string };

// Connect = initialize + tools/list, recorded on the row as its last check.
// The dialog opens running, then shows the server's name and version with
// its tools, or what went wrong and what to do about it.
export function ConnectDialog({ server, onOpenChange, onSignIn }: { server: McpServer | null; onOpenChange: (o: boolean) => void; onSignIn?: (s: McpServer) => void }) {
  return (
    <Dialog open={!!server} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">{server ? <ConnectBody server={server} onSignIn={onSignIn} /> : null}</DialogContent>
    </Dialog>
  );
}

// A failed check of an `auth: oauth` server that points at `pulse mcp login`
// is a missing or expired sign-in; the dialog offers to run it here.
const wantsSignIn = (server: McpServer, fix: string): boolean => server.auth === "oauth" && /pulse mcp login/.test(fix);

function ConnectBody({ server, onSignIn }: { server: McpServer; onSignIn?: (s: McpServer) => void }) {
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
          <PlugZapIcon className="size-4 text-primary" aria-hidden /> Checking {server.name}
        </DialogTitle>
        <DialogDescription>PULSE connects to the tool and asks what it offers. The answer is saved with the tool and shown to agents in their brief.</DialogDescription>
      </DialogHeader>
      {state.kind === "running" ? (
        <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground" role="status" aria-live="polite">
          <Loader2Icon className="size-4 motion-safe:animate-spin" aria-hidden /> Connecting to {server.name}…
        </div>
      ) : state.kind === "ok" ? (
        <div className="grid gap-3">
          <Notice tone="ok">
            <span className="font-medium">{"Connected to " + (state.result.info.name || server.name)}</span>
            {state.result.info.version ? <span title={state.result.info.protocol_version ? "MCP protocol " + state.result.info.protocol_version : undefined}>{" (version " + state.result.info.version + ")"}</span> : null}
            <span className="block text-xs text-muted-foreground anywhere">{state.result.note}</span>
          </Notice>
          {state.result.info.instructions ? <p className="text-sm leading-relaxed text-muted-foreground anywhere">{state.result.info.instructions}</p> : null}
          {state.result.tools.length ? (
            <>
              <p className="text-sm font-medium">{state.result.tools.length === 1 ? "1 tool it offers" : state.result.tools.length + " tools it offers"}</p>
              <ul className="grid max-h-80 gap-1.5 overflow-y-auto pr-1">
                {state.result.tools.map((t) => (
                  <li key={t.name} className="rounded-xl border bg-card px-3 py-2">
                    <div className="flex items-center gap-2">
                      <WrenchIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
                      <Mono>{t.name}</Mono>
                    </div>
                    {t.description ? <p className="mt-1 text-xs leading-snug text-muted-foreground anywhere">{t.description}</p> : null}
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <p className="text-sm text-muted-foreground">The server answered, but it offers no tools yet.</p>
          )}
          <Disclosure summary="How agents use it" contentClassName="mt-1.5 text-xs leading-relaxed text-muted-foreground">
            <p>
              Agents call a tool with{" "}
              <Mono>
                pulse mcp call {server.name} TOOL --args &apos;{"{...}"}&apos;
              </Mono>
              .
            </p>
          </Disclosure>
        </div>
      ) : (
        <div className="grid gap-3">
          <Notice tone="bad">
            <span className="font-medium anywhere">{"Couldn't connect: " + state.error}</span>
            {state.why ? <span className="mt-1 block text-[13px] anywhere">{"Why: " + state.why}</span> : null}
            {state.fix ? <span className="mt-1 block text-[13px] anywhere">{"What to do: " + state.fix}</span> : null}
          </Notice>
          <div className="flex flex-wrap gap-2">
            {onSignIn && wantsSignIn(server, state.fix) ? (
              <Button size="sm" onClick={() => onSignIn(server)}>
                <KeyRoundIcon /> Sign in
              </Button>
            ) : null}
            <Button size="sm" variant="outline" onClick={() => setRun((cur) => ({ attempt: cur.attempt + 1, state: { kind: "running" } }))}>
              Check again
            </Button>
          </div>
        </div>
      )}
    </>
  );
}

// ----- OAuth sign-in -----------------------------------------------------------

// The sign-in `pulse mcp login` runs, from the browser. The hub does the
// discovery and registration, then waits on a thread while the person takes
// the one step: approve in a new tab (code flow) or type a short code on
// the provider's page (device flow). GitHub registers no clients, so for it
// the hub first asks for the id of an OAuth App the person creates.
type SignInState =
  | { kind: "starting" }
  | { kind: "pending"; login: McpLoginState }
  | { kind: "ok"; login: McpLoginState }
  | { kind: "failed"; error: string; why: string; fix: string; login?: McpLoginState }
  | { kind: "needs_client"; login: McpLoginState };

const POLL_MS = 1500;

export function SignInDialog({ server, onOpenChange, onSignedIn }: { server: McpServer | null; onOpenChange: (o: boolean) => void; onSignedIn?: (s: McpServer) => void }) {
  return (
    <Dialog open={!!server} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">{server ? <SignInBody server={server} close={() => onOpenChange(false)} onSignedIn={onSignedIn} /> : null}</DialogContent>
    </Dialog>
  );
}

// Opens `url` in a new tab. Right after a click the browser still counts the
// activation, so this usually works without a blocker; the button stays as
// the fallback either way.
function openTab(url: string): boolean {
  if (typeof window === "undefined" || !url) return false;
  const w = window.open(url, "_blank", "noopener,noreferrer");
  return !!w;
}

const loginPath = (id: string) => "mcp/" + enc(id) + "/login";

function SignInBody({ server, close, onSignedIn }: { server: McpServer; close: () => void; onSignedIn?: (s: McpServer) => void }) {
  const [state, setState] = useState<SignInState>({ kind: "starting" });
  const [attempt, setAttempt] = useState(0);
  const [input, setInput] = useState<McpLoginInput>({});
  const opened = useRef(false);
  // The parent passes a fresh arrow each render; the poll must not restart for it.
  const signedIn = useRef(onSignedIn);
  useEffect(() => {
    signedIn.current = onSignedIn;
  }, [onSignedIn]);
  const terminal = "pulse mcp login " + server.name;

  // One start per attempt: POST, then settle on what the hub said.
  useEffect(() => {
    let cancelled = false;
    opened.current = false;
    writes
      .loginMcp(server.id, input)
      .then((login) => {
        if (cancelled) return;
        if (login.state === "needs_client") setState({ kind: "needs_client", login });
        else if (login.state === "pending") setState({ kind: "pending", login });
        else if (login.state === "ok") setState({ kind: "ok", login });
        else setState({ kind: "failed", error: "The sign-in did not start", why: login.why ?? "", fix: "Try again.", login });
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setState({ kind: "failed", ...errorText(e) });
      });
    return () => {
      cancelled = true;
    };
    // `input` is set together with `attempt` by the form's submit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [server.id, attempt]);

  // While pending: open the tab once, then poll until the hub says ok or failed.
  useEffect(() => {
    if (state.kind !== "pending") return;
    const login = state.login;
    if (!opened.current) {
      opened.current = true;
      const at = login.flow === "device" ? login.verification_uri_complete || login.verification_uri || "" : login.url ?? "";
      if (at) openTab(at);
    }
    let cancelled = false;
    const tick = () => {
      api<McpLoginState>(loginPath(server.id))
        .then((next) => {
          if (cancelled) return;
          if (next.state === "ok") {
            setState({ kind: "ok", login: next });
            toast.success("Signed in to " + server.name + ".", { description: "PULSE keeps the tokens on this machine and renews them." });
            refreshMcp();
            signedIn.current?.(next.sign_in ? { ...server, sign_in: next.sign_in } : server);
          } else if (next.state === "failed") {
            setState({ kind: "failed", error: next.cancelled ? "The sign-in was cancelled" : "The sign-in did not finish", why: next.why ?? "", fix: next.cancelled ? "Start it again when you are ready." : "Fix what the why line names and try again.", login: next });
            refreshMcp();
          } else if (next.state === "none") {
            setState({ kind: "failed", error: "The sign-in was dropped", why: "The hub forgot this attempt - it may have restarted.", fix: "Try again." });
          }
        })
        .catch(() => {
          // A missed poll is not a failure; the next one tells.
        });
    };
    const timer = setInterval(tick, POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [state, server]);

  const restart = (next: McpLoginInput) => {
    setInput(next);
    setState({ kind: "starting" });
    setAttempt((n) => n + 1);
  };
  const mood = state.kind === "ok" ? "happy" : state.kind === "failed" ? "worried" : state.kind === "needs_client" ? "curious" : state.kind === "pending" ? "waiting" : "busy";

  return (
    <>
      <DialogHeader>
        <DialogTitle className="flex items-center gap-2">
          <KeyRoundIcon className="size-4 text-primary" aria-hidden /> Sign in to {server.name}
        </DialogTitle>
        <DialogDescription>Once. PULSE keeps the tokens on this machine, renews them itself, and agents call the tool from then on.</DialogDescription>
      </DialogHeader>
      <div className="flex items-start gap-4">
        <PulseMascot mood={mood} size={64} decorative className="mt-1 hidden shrink-0 sm:block" />
        <div className="min-w-0 flex-1">
          {state.kind === "starting" ? (
            <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground" role="status" aria-live="polite">
              <Loader2Icon className="size-4 motion-safe:animate-spin" aria-hidden /> Asking {server.name} how to sign in…
            </div>
          ) : state.kind === "pending" ? (
            <PendingStep login={state.login} server={server} />
          ) : state.kind === "ok" ? (
            <div className="grid gap-3">
              <Notice tone="ok">
                <span className="font-medium">Signed in.</span>
                {state.login.note ? <span className="block text-xs text-muted-foreground anywhere">{state.login.note}</span> : null}
              </Notice>
              <p className="text-sm text-muted-foreground">Next, check the connection: PULSE asks the tool what it offers and shows it to agents.</p>
            </div>
          ) : state.kind === "needs_client" ? (
            <ClientForm login={state.login} server={server} initial={input} onStart={restart} />
          ) : (
            <div className="grid gap-3">
              <Notice tone="bad">
                <span className="font-medium anywhere">{state.error}</span>
                {state.why ? <span className="mt-1 block text-[13px] anywhere">{"Why: " + state.why}</span> : null}
                {state.fix ? <span className="mt-1 block text-[13px] anywhere">{"What to do: " + state.fix}</span> : null}
              </Notice>
            </div>
          )}
          {state.kind !== "needs_client" ? (
            <Disclosure summary="Prefer the terminal?" className="mt-3" contentClassName="mt-1.5 text-xs leading-relaxed text-muted-foreground">
              <p>
                The same sign-in runs with <Mono>{terminal}</Mono> on this machine; <Mono>pulse mcp logout {server.name}</Mono> forgets the tokens.
              </p>
            </Disclosure>
          ) : null}
        </div>
      </div>
      <DialogFooter className="gap-2 sm:gap-2">
        {state.kind === "failed" ? (
          <Button size="sm" variant="outline" onClick={() => restart(input)}>
            Try again
          </Button>
        ) : null}
        {state.kind === "ok" ? (
          <Button size="sm" onClick={close}>
            <CheckIcon /> Done
          </Button>
        ) : (
          <Button size="sm" variant="ghost" onClick={close}>
            {state.kind === "pending" ? "Keep waiting in the background" : "Close"}
          </Button>
        )}
      </DialogFooter>
    </>
  );
}

// What the person does now, by flow. The hub keeps waiting even when this
// dialog closes; the card's state line catches up on the next refresh.
function PendingStep({ login, server }: { login: McpLoginState; server: McpServer }) {
  const device = login.flow === "device";
  const at = device ? login.verification_uri ?? "" : login.url ?? "";
  const minutes = Math.max(1, Math.round((login.expires_in ?? 300) / 60));
  return (
    <div className="grid gap-3">
      {device ? (
        <>
          <p className="text-sm leading-relaxed">
            Open the provider&apos;s page and type this code. The page is <span className="font-mono text-[13px]">{hostOf(at)}</span>, where you are already signed in as yourself.
          </p>
          <UserCode code={login.user_code ?? ""} />
          {login.built_in && login.client_label ? (
            <p className="text-xs leading-relaxed text-muted-foreground">
              The page asks you to authorize <strong className="font-medium text-foreground">{login.client_label}</strong>, not PULSE: that is the public app PULSE signs in through, so there is nothing for you to register. The token stays on this machine, in PULSE&apos;s own grant file.
            </p>
          ) : null}
        </>
      ) : (
        <p className="text-sm leading-relaxed">
          A new tab asks you to approve PULSE at <span className="font-mono text-[13px]">{hostOf(at)}</span>. Approve it there and come back; this window notices by itself.
        </p>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" asChild>
          <a href={device ? login.verification_uri_complete || at : at} target="_blank" rel="noopener noreferrer">
            <ExternalLinkIcon /> {device ? "Open the code page" : "Open the sign-in page"}
          </a>
        </Button>
        <span className="flex items-center gap-1.5 text-xs text-muted-foreground" role="status" aria-live="polite">
          <Loader2Icon className="size-3.5 motion-safe:animate-spin" aria-hidden /> Waiting for {server.name}… up to {minutes} min.
        </span>
      </div>
      {!device && login.redirect_uri ? <p className="text-xs leading-relaxed text-muted-foreground anywhere">The provider sends the approval back to {login.redirect_uri}, a door PULSE opened on this machine just for this sign-in.</p> : null}
    </div>
  );
}

function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

function UserCode({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    void copyText(code)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1800);
      })
      .catch(() => toast.error("Couldn't copy. Select the code and copy it yourself."));
  };
  return (
    <div className="flex flex-wrap items-center gap-3 rounded-2xl border-2 border-dashed border-primary/40 bg-primary/5 px-4 py-3">
      <span className="select-all font-mono text-2xl font-semibold tracking-[0.2em] text-foreground" aria-label={"Code " + code.split("").join(" ")}>
        {code}
      </span>
      <Button size="sm" variant="outline" onClick={copy} aria-label="Copy the code">
        {copied ? <CheckIcon /> : <CopyIcon />} {copied ? "Copied" : "Copy"}
      </Button>
    </div>
  );
}

// The provider registers no clients (GitHub): the person creates one and
// hands over its id. With the device flow that is all; the code flow also
// needs the redirect port the client was registered with and, usually, the
// secret as a reference.
function ClientForm({ login, server, initial, onStart }: { login: McpLoginState; server: McpServer; initial: McpLoginInput; onStart: (input: McpLoginInput) => void }) {
  const deviceOffered = !!login.device_flow;
  const [clientId, setClientId] = useState(initial.client_id ?? "");
  const [flow, setFlow] = useState<McpLoginFlow>(initial.flow === "code" || !deviceOffered ? "code" : "device");
  const [port, setPort] = useState(initial.redirect_port ? String(initial.redirect_port) : login.redirect_port ? String(login.redirect_port) : "");
  const [secretRef, setSecretRef] = useState(initial.client_secret_ref ?? "");
  const [scope, setScope] = useState(initial.scope ?? "");
  const [touched, setTouched] = useState(false);
  const portN = Number(port);
  const portOk = flow === "device" || (port.trim() !== "" && Number.isInteger(portN) && portN >= 1 && portN <= 65535);
  const ready = clientId.trim() !== "" && portOk;
  const docs = login.service_documentation || "";
  const github = /github\.com/.test(login.issuer ?? "") || /github\.com/.test(docs);
  const redirect = "http://127.0.0.1:" + (port.trim() || "PORT") + "/callback";
  const submit = () => {
    setTouched(true);
    if (!ready) return;
    const next: McpLoginInput = { client_id: clientId.trim(), flow };
    if (flow === "code") {
      next.redirect_port = portN;
      if (secretRef.trim()) next.client_secret_ref = secretRef.trim();
    }
    if (scope.trim()) next.scope = scope.trim();
    onStart(next);
  };
  return (
    <div className="grid gap-3">
      <Notice tone="info">
        <span className="font-medium anywhere">{hostOf(login.issuer ?? "") || server.name} does not register apps by itself.</span>
        <span className="mt-1 block text-[13px] anywhere">
          Create one there{docs ? " (its developer settings)" : ""}, then paste its client id here. {deviceOffered ? "With the device flow it needs no secret and no redirect address: you type a short code on the provider's page." : "Register the redirect address shown below with it."}
        </span>
        {docs ? (
          <a href={docs} target="_blank" rel="noopener noreferrer" className="mt-1.5 inline-flex items-center gap-1 text-[13px] font-medium underline underline-offset-2">
            <ExternalLinkIcon className="size-3.5" aria-hidden /> Open {hostOf(docs)}
          </a>
        ) : null}
      </Notice>
      {github ? (
        <Disclosure summary="On GitHub, step by step" contentClassName="mt-1.5 text-xs leading-relaxed text-muted-foreground">
          <ol className="list-decimal space-y-1 pl-4">
            <li>Settings → Developer settings → OAuth Apps → New OAuth App.</li>
            <li>Any name and homepage; for the callback use {deviceOffered && flow === "device" ? "any address (the device flow does not use it)" : <Mono>{redirect}</Mono>}.</li>
            {deviceOffered && flow === "device" ? <li>Tick &quot;Enable Device Flow&quot; and save.</li> : <li>Generate a client secret and keep it as a reference (keychain://, op://, file://), not here.</li>}
            <li>Copy the Client ID (it starts with Iv1. or Ov23) into the field below.</li>
          </ol>
        </Disclosure>
      ) : null}
      <div className="grid gap-1.5">
        <Label htmlFor="mcp-client-id">Client id</Label>
        <Input id="mcp-client-id" value={clientId} onChange={(e) => setClientId(e.target.value)} placeholder={github ? "Iv1.0123456789abcdef" : "the id the provider shows"} spellCheck={false} autoFocus className="font-mono text-[13px]" aria-invalid={touched && !clientId.trim() ? true : undefined} />
        {touched && !clientId.trim() ? <p className="text-xs text-bad">The client id is what the provider gave you when you registered the app.</p> : null}
      </div>
      {deviceOffered ? (
        <div className="grid gap-1.5">
          <Label htmlFor="mcp-flow">How to approve</Label>
          <Select value={flow} onValueChange={(v) => setFlow(v as McpLoginFlow)}>
            <SelectTrigger id="mcp-flow" className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="device">Type a code on the provider&apos;s page (no secret, no port)</SelectItem>
              <SelectItem value="code">Approve in a browser tab (needs the redirect port and usually a secret)</SelectItem>
            </SelectContent>
          </Select>
        </div>
      ) : null}
      {flow === "code" ? (
        <>
          <div className="grid gap-1.5">
            <Label htmlFor="mcp-port">Redirect port</Label>
            <Input id="mcp-port" value={port} onChange={(e) => setPort(e.target.value)} inputMode="numeric" placeholder="e.g. 8976" className="font-mono text-[13px]" aria-invalid={touched && !portOk ? true : undefined} />
            <p className="text-xs leading-relaxed text-muted-foreground anywhere">
              The app must be registered with the callback <span className="font-mono text-[12px]">{redirect}</span>; a fixed port keeps it the same every time.
            </p>
            {touched && !portOk ? <p className="text-xs text-bad">A port between 1 and 65535.</p> : null}
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="mcp-client-secret">Client secret, as a reference (optional)</Label>
            <Input id="mcp-client-secret" value={secretRef} onChange={(e) => setSecretRef(e.target.value)} placeholder="keychain://pulse/github-oauth-secret" spellCheck={false} className="font-mono text-[13px]" />
            <p className="text-xs leading-relaxed text-muted-foreground">Never the secret itself: op://, keychain:// or file:// pointing at it on this machine.</p>
          </div>
        </>
      ) : null}
      <div className="grid gap-1.5">
        <Label htmlFor="mcp-scope">Scopes (optional)</Label>
        <Input id="mcp-scope" value={scope} onChange={(e) => setScope(e.target.value)} placeholder={(login.scopes_supported ?? []).join(" ") || "what the server suggests"} spellCheck={false} className="font-mono text-[13px]" />
        {login.scopes_supported?.length ? <p className="text-xs leading-relaxed text-muted-foreground anywhere">The server lists: {login.scopes_supported.join(", ")}. Empty takes what it suggests.</p> : null}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={submit} disabled={touched && !ready}>
          <KeyRoundIcon /> Start the sign-in
        </Button>
      </div>
      <Disclosure summary="Prefer the terminal?" contentClassName="mt-1.5 text-xs leading-relaxed text-muted-foreground">
        <p className="anywhere">{login.command || "pulse mcp login " + server.name + " --client-id ID"}</p>
      </Disclosure>
    </div>
  );
}
