"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { CheckIcon, ClipboardIcon, ExternalLinkIcon, LogOutIcon, RefreshCwIcon, RotateCcwIcon, TerminalIcon } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { ErrorNotice, Kv, Notice } from "@/components/pulse/panel";
import { Mono, ToneBadge } from "@/components/pulse/who";
import { errorText, writes, type HarnessDeviceFlow, type HarnessLogin, type HarnessModel, type HarnessModelsResponse, type HarnessPrompt, type HarnessStatus, type Runner, type Setting } from "@/lib/api";
import { fmtDate, num, rel } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { invalidate, useApi, useNow } from "@/lib/use-api";
import { cn } from "@/lib/utils";

const SETTING_KEYS = ["harness", "provider", "harness.max_steps", "harness.step_timeout", "harness.output_limit", "harness.max_cost", "harness.keep_observations", "harness.system_prompt", "worker_model", "light_model"];
const SAME_MODEL = "__same__";

function setting(rows: Setting[] | undefined, name: string): Setting | undefined {
  return rows?.find((s) => s.name === name);
}

function modelLabel(m: HarnessModel) {
  return m.name || m.id;
}

function saveSetting(name: string, value: string) {
  return writes.setSetting(name, value).then(() => {
    invalidate("harness");
    invalidate("settings");
  });
}

function CopyButton({ text, label = "Copy" }: { text: string; label?: string }) {
  return (
    <Button
      type="button"
      size="sm"
      variant="outline"
      onClick={() =>
        navigator.clipboard
          .writeText(text)
          .then(() => toast.success("Copied."))
          .catch(() => toast.error("Could not copy. Select the text and copy it yourself."))
      }
    >
      <ClipboardIcon /> {label}
    </Button>
  );
}

function sourceText(source: string): { label: string; borrowed: boolean } {
  const s = source.trim().toLowerCase();
  if (s === "device login") return { label: "signed in here", borrowed: false };
  if (s === "token login") return { label: "pasted token", borrowed: false };
  if (s === "environment" || s === "editor login") return { label: "borrowed", borrowed: true };
  return { label: source || "unknown", borrowed: false };
}

function LoginHealth({ login, runner }: { login?: HarnessLogin; runner?: Runner }) {
  const source = sourceText(login?.source || "");
  if (!login?.logged_in) {
    return (
      <div className="grid gap-2">
        <Notice tone="warn">{login?.detail || "Not signed in."}</Notice>
        {runner?.hold ? (
          <Kv rows={[["why workers are held", <Notice key="hold" tone="warn" className="py-2">{runner.hold}</Notice>]]} />
        ) : null}
      </div>
    );
  }
  return (
    <div className="grid gap-2">
      <div className="flex flex-wrap items-center gap-1.5">
        <ToneBadge tone="ok" dot>
          Signed in
        </ToneBadge>
        <span>
          Signed in as <b>{login.user}</b>
        </span>
      </div>
      <Kv
        rows={[
          ["source", source.label],
          ...(login.plan ? [["plan", login.plan] as [React.ReactNode, React.ReactNode]] : []),
          ...(login.expires_at ? [["expiry", rel(login.expires_at) + " (" + fmtDate(login.expires_at) + ")"] as [React.ReactNode, React.ReactNode]] : []),
          ["why workers are held", runner?.hold ? <Notice tone="warn" className="py-2">{runner.hold}</Notice> : <span className="text-muted-foreground">Workers run on this machine</span>],
        ]}
      />
      {source.borrowed ? <p className="text-xs text-muted-foreground">Borrowed logins are not shared with executors.</p> : null}
      {login.detail ? <Notice>{login.detail}</Notice> : null}
    </div>
  );
}

function DeviceLogin({ onDone }: { onDone: () => void }) {
  const [flow, setFlow] = useState<HarnessDeviceFlow | null>(null);
  const [status, setStatus] = useState<"idle" | "starting" | "pending" | "expired" | "denied">("idle");
  const [pollDelay, setPollDelay] = useState(5);
  const [polls, setPolls] = useState(0);
  const [error, setError] = useState("");
  const [startedAt, setStartedAt] = useState(0);
  const now = useNow(1000);
  const remaining = flow ? Math.max(0, Math.ceil((startedAt + flow.expires_in * 1000 - now) / 1000)) : 0;

  const start = () => {
    setStatus("starting");
    setError("");
    writes
      .harnessLogin()
      .then((r) => {
        setFlow(r);
        setStartedAt(Date.now());
        setPollDelay(Math.max(1, r.interval || 5));
        setPolls(0);
        setStatus("pending");
      })
      .catch((e: unknown) => {
        const t = errorText(e);
        setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
        setStatus("idle");
      });
  };

  useEffect(() => {
    if (!flow || status !== "pending") return;
    const id = setTimeout(() => {
      writes
        .harnessLoginPoll(flow.device_code)
        .then((r) => {
          setPolls((n) => n + 1);
          if (r.status === "slow_down") setPollDelay((n) => n + 5);
          if (r.status === "expired") setStatus("expired");
          if (r.status === "denied") setStatus("denied");
          if (r.status === "complete" && r.login) {
            toast.success("Signed in as " + r.login.user + ".");
            invalidate("harness");
            invalidate("harness/models");
            setFlow(null);
            setStatus("idle");
            onDone();
          }
        })
        .catch((e: unknown) => {
          const t = errorText(e);
          setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
          setStatus("idle");
        });
    }, pollDelay * 1000);
    return () => clearTimeout(id);
  }, [flow, status, pollDelay, polls, onDone]);

  if (!flow) {
    return (
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" onClick={start} disabled={status === "starting"}>
          {status === "starting" ? <RefreshCwIcon className="animate-spin" /> : null} Sign in with GitHub
        </Button>
        {error ? <span className="text-xs text-bad">{error}</span> : null}
      </div>
    );
  }

  return (
    <div className="grid gap-3 rounded-lg border bg-muted/30 p-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <p className="text-xs text-muted-foreground">Enter this code at GitHub:</p>
          <p className="mt-1 font-mono text-2xl font-semibold tracking-wider">{flow.user_code}</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <CopyButton text={flow.user_code} label="Copy code" />
          <Button asChild size="sm" variant="outline">
            <a href={flow.verification_uri} target="_blank" rel="noreferrer">
              Open GitHub <ExternalLinkIcon />
            </a>
          </Button>
        </div>
      </div>
      <p className="text-xs text-muted-foreground">
        {status === "pending" ? "Waiting for GitHub. " : status === "expired" ? "The code expired; try again. " : "The sign-in was denied. "}
        {remaining > 0 ? num(remaining) + " seconds left." : ""}
      </p>
      {status === "expired" || status === "denied" ? (
        <div className="flex gap-2">
          <Button size="sm" onClick={start}>
            Try again
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setFlow(null)}>
            Cancel
          </Button>
        </div>
      ) : (
        <Button size="sm" variant="ghost" className="w-fit" onClick={() => setFlow(null)}>
          Cancel
        </Button>
      )}
    </div>
  );
}

function TokenLogin({ onDone }: { onDone: () => void }) {
  const [open, setOpen] = useState(false);
  const [token, setToken] = useState("");
  const [error, setError] = useState("");
  const submit = () => {
    setError("");
    const id = toast.loading("Signing in…");
    writes
      .harnessLoginToken(token)
      .then((r) => {
        toast.success("Signed in as " + r.login.user + ".", { id });
        setToken("");
        setOpen(false);
        invalidate("harness");
        invalidate("harness/models");
        onDone();
      })
      .catch((e: unknown) => {
        toast.dismiss(id);
        const t = errorText(e);
        setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
      });
  };
  return (
    <div className="grid gap-2">
      <button type="button" className="w-fit text-xs text-muted-foreground underline-offset-4 hover:underline" onClick={() => setOpen((v) => !v)}>
        Advanced: paste a token
      </button>
      {open ? (
        <div className="flex flex-wrap items-end gap-2">
          <div className="grid min-w-52 gap-1">
            <Label htmlFor="harness-token">GitHub token</Label>
            <Input id="harness-token" type="password" value={token} onChange={(e) => setToken(e.target.value)} onKeyDown={(e) => e.key === "Enter" && token.trim() && submit()} placeholder="gho_…" />
          </div>
          <Button type="button" onClick={submit} disabled={!token.trim()}>
            Sign in
          </Button>
          {error ? <p className="basis-full text-xs text-bad">{error}</p> : null}
        </div>
      ) : null}
    </div>
  );
}

function ModelSelect({ name, label, current, models, disabled }: { name: "worker_model" | "light_model"; label: string; current: string; models: HarnessModel[]; disabled?: boolean }) {
  const [custom, setCustom] = useState(current);
  const ids = new Set(models.map((m) => m.id));
  const selectValue = name === "light_model" && !current ? SAME_MODEL : ids.has(current) ? current : "__custom__";
  const save = (value: string) => {
    const v = value === SAME_MODEL ? "" : value;
    const id = toast.loading("Saving " + label.toLowerCase() + "…");
    saveSetting(name, v)
      .then(() => toast.success(label + " saved.", { id }))
      .catch((e: unknown) => {
        toast.dismiss(id);
        const t = errorText(e);
        toast.error(t.error, { description: [t.why, t.fix].filter(Boolean).join(" ") });
      });
  };
  return (
    <div className="grid gap-1.5">
      <Label>{label}</Label>
      <div className="flex flex-wrap items-center gap-2">
        <Select value={selectValue} onValueChange={save} disabled={disabled}>
          <SelectTrigger className="w-72 max-w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {name === "light_model" ? <SelectItem value={SAME_MODEL}>Same as implementation</SelectItem> : null}
            {models.map((m) => (
              <SelectItem key={m.id} value={m.id}>
                <span className="flex min-w-0 items-center gap-1.5">
                  <span>{modelLabel(m)}</span>
                  <span className="text-xs text-muted-foreground">{m.id}</span>
                  {m.premium > 1 ? <Badge variant="outline">{"×" + m.premium}</Badge> : null}
                  {m.preview ? <Badge variant="outline">preview</Badge> : null}
                  {m.default ? <Badge variant="secondary">default</Badge> : null}
                </span>
              </SelectItem>
            ))}
            {!ids.has(current) && current ? <SelectItem value="__custom__">{current}</SelectItem> : null}
          </SelectContent>
        </Select>
        <Input
          aria-label={label + " custom id"}
          value={custom}
          onChange={(e) => setCustom(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") save(custom.trim());
            if (e.key === "Escape") setCustom(current);
          }}
          onBlur={() => {
            if (custom.trim() !== current) save(custom.trim());
          }}
          placeholder="Other model id…"
          className="w-56"
          disabled={disabled}
        />
      </div>
    </div>
  );
}

function LimitInput({ s }: { s: Setting }) {
  const [draftState, setDraftState] = useState({ name: s.name, value: s.value, draft: s.value });
  const [error, setError] = useState("");
  const draft = draftState.name === s.name && draftState.value === s.value ? draftState.draft : s.value;
  const setDraft = (value: string) => setDraftState({ name: s.name, value: s.value, draft: value });
  const save = () => {
    if (draft === s.value) return;
    if (s.kind === "number" ? !/^\d+(\.\d+)?$/.test(draft) : !/^\d+$/.test(draft)) {
      setError(s.kind === "number" ? "Enter a number." : "Enter digits only.");
      return;
    }
    if (s.min !== null && Number(draft) < s.min) {
      setError("Enter " + s.min + " or more.");
      return;
    }
    setError("");
    saveSetting(s.name, draft).catch((e: unknown) => {
      const t = errorText(e);
      setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
    });
  };
  return (
    <div className="grid gap-1.5">
      <Label htmlFor={"harness-" + s.name}>{s.name.replace(/^harness\./, "").replace(/_/g, " ")}</Label>
      <Input
        id={"harness-" + s.name}
        value={draft}
        inputMode={s.kind === "number" ? "decimal" : "numeric"}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => {
          if (e.key === "Enter") save();
          if (e.key === "Escape") setDraft(s.value);
        }}
        aria-invalid={!!error}
        className="w-32 tnum"
      />
      <p className={cn("max-w-52 text-xs leading-snug", error ? "text-bad" : "text-muted-foreground")}>{error || s.description}</p>
    </div>
  );
}

function PromptEditor() {
  const prompt = useApi<HarnessPrompt>("harness/prompt", { refresh: 30000 });
  const [draftState, setDraftState] = useState({ marker: "", draft: "" });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const d = prompt.data;
  const marker = d ? (d.custom ? "custom:" + d.template : "default:" + d.default) : "";
  const draft = draftState.marker === marker ? draftState.draft : d?.custom ? d.template : d?.default || "";
  const dirty = !!d && draft !== (d.custom ? d.template : d.default);
  const save = (value: string, label: string) => {
    setSaving(true);
    setError("");
    writes
      .setSetting("harness.system_prompt", value)
      .then(() => {
        toast.success(label);
        invalidate("harness");
        invalidate("harness/prompt");
        invalidate("settings");
        return prompt.reload();
      })
      .catch((e: unknown) => {
        const t = errorText(e);
        setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
      })
      .finally(() => setSaving(false));
  };
  return (
    <section className="grid gap-3 rounded-lg border bg-muted/20 p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h3 className="text-sm font-semibold">System prompt</h3>
          <p className="text-xs text-muted-foreground">The harness renders this server-side for each run; leave it empty to use the built-in default.</p>
        </div>
        {d ? <ToneBadge tone={d.custom ? "accent" : "neutral"}>{d.custom ? "custom" : "built-in"}</ToneBadge> : null}
      </div>
      {prompt.error && !d ? <ErrorNotice error={prompt.error} onRetry={() => void prompt.reload()} /> : null}
      {d ? (
        <>
          <Textarea
            aria-label="Harness system prompt template"
            value={draft}
            onChange={(e) => {
              setDraftState({ marker, draft: e.target.value });
              setError("");
            }}
            rows={10}
            spellCheck={false}
            className={cn("min-h-48 font-mono text-[12.5px] leading-relaxed", !d.custom && !dirty && "bg-muted/40 text-muted-foreground")}
          />
          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" size="sm" onClick={() => save(draft, "Harness prompt saved.")} disabled={saving || !dirty}>
              {saving ? <RefreshCwIcon className="animate-spin" /> : <CheckIcon />} Save
            </Button>
            <Button type="button" size="sm" variant="outline" onClick={() => save("", "Harness prompt reset to built-in.")} disabled={saving || (!d.custom && !dirty)}>
              <RotateCcwIcon /> Reset to built-in
            </Button>
            {dirty ? (
              <Button type="button" size="sm" variant="ghost" onClick={() => setDraftState({ marker, draft: d.custom ? d.template : d.default })} disabled={saving}>
                Undo changes
              </Button>
            ) : null}
          </div>
          {d.error || error ? <Notice tone="bad">{error || d.error}</Notice> : null}
          <details className="rounded-lg border bg-card px-3 py-2 text-sm">
            <summary className="cursor-pointer font-medium">Preview</summary>
            <pre className="mt-2 max-h-80 overflow-auto whitespace-pre-wrap break-words rounded-md bg-muted/40 p-3 font-mono text-xs leading-relaxed">{d.rendered || "(empty)"}</pre>
          </details>
        </>
      ) : null}
    </section>
  );
}

export function HarnessSettingsCard({ className }: { className?: string }) {
  const h = useApi<HarnessStatus>("harness", { refresh: 15000 });
  const runner = useApi<Runner>("runner", { refresh: 15000 });
  const models = useApi<HarnessModelsResponse>("harness/models", { refresh: 60000 });
  const d = h.data;
  const rows = d?.settings;
  const mode = setting(rows, "harness");
  const provider = setting(rows, "provider");
  const budgets = ["harness.max_steps", "minutes_per_job", "harness.max_cost"].map((k) => setting(rows, k)).filter((s): s is Setting => !!s);
  const limits = ["harness.step_timeout", "harness.output_limit", "harness.keep_observations"].map((k) => setting(rows, k)).filter((s): s is Setting => !!s);
  const signedIn = d?.provider.login?.logged_in === true;
  const currentModels = models.data?.models ?? [];
  const modelError = models.error && !models.data ? models.error : "";

  const saveChoice = (name: string, value: string) => {
    const id = toast.loading("Saving " + name + "…");
    saveSetting(name, value)
      .then(() => toast.success(name + " saved.", { id }))
      .catch((e: unknown) => {
        toast.dismiss(id);
        const t = errorText(e);
        toast.error(t.error, { description: [t.why, t.fix].filter(Boolean).join(" ") });
      });
  };

  return (
    <Card id="harness" className={cn("scroll-mt-32", className)}>
      <CardHeader className="border-b">
        <div className="flex items-start gap-3">
          <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary" aria-hidden>
            <TerminalIcon className="size-4" />
          </span>
          <div className="min-w-0">
            <CardTitle>Harness</CardTitle>
            <p className="mt-0.5 text-[13px] leading-snug text-muted-foreground">The built-in harness runs every job through the model provider below. Nothing else to install.</p>
          </div>
        </div>
      </CardHeader>
      <CardContent className="grid gap-5 py-4">
        {h.error && !d ? <ErrorNotice error={h.error} onRetry={() => void h.reload()} /> : null}
        {!d ? <p className="text-sm text-muted-foreground">Loading harness settings…</p> : null}
        {d ? (
          <>
            <section className="grid gap-2">
              <h3 className="text-sm font-semibold">Mode</h3>
              <div className="flex flex-wrap items-center gap-2">
                <Select value={mode?.value || d.harness} onValueChange={(v) => saveChoice("harness", v)}>
                  <SelectTrigger className="w-56">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="native">Native</SelectItem>
                    <SelectItem value="copilot-cli">Copilot CLI (legacy)</SelectItem>
                  </SelectContent>
                </Select>
                <ToneBadge tone={d.harness === "native" ? "ok" : "warn"}>{d.harness === "native" ? "built in" : "legacy"}</ToneBadge>
              </div>
              {d.harness === "copilot-cli" ? <Notice tone="warn">The legacy mode needs the copilot npm CLI installed and logged in on each executor. The runner_cmd and max_ai_credits settings apply there.</Notice> : null}
            </section>

            <section className="grid gap-2">
              <h3 className="text-sm font-semibold">Provider</h3>
              <div className="flex flex-wrap items-center gap-2">
                <span>{d.provider.label}</span>
                {signedIn ? (
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() =>
                      writes.harnessLogout().then((r) => {
                        toast.success(r.login.logged_in ? "Still signed in." : "Signed out.");
                        invalidate("harness");
                        invalidate("harness/models");
                      })
                    }
                  >
                    <LogOutIcon /> Sign out
                  </Button>
                ) : null}
              </div>
              <LoginHealth login={d.provider.login} runner={runner.data} />
              {provider ? (
                <Select value={provider.value || d.provider.name} onValueChange={(v) => saveChoice("provider", v)}>
                  <SelectTrigger className="w-56">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {d.providers.map((p) => (
                      <SelectItem key={p.name} value={p.name}>
                        {p.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : null}
              {!signedIn ? (
                <>
                  <DeviceLogin onDone={() => void h.reload()} />
                  <TokenLogin onDone={() => void h.reload()} />
                </>
              ) : null}
            </section>

            <section className="grid gap-3">
              <h3 className="text-sm font-semibold">Models</h3>
              {modelError ? (
                <Notice tone={modelError.includes("not signed in") ? "warn" : "bad"}>
                  <span className="grid gap-1">
                    <span>{modelError}</span>
                    <span>
                      Current implementation model: <Mono>{d.models.worker || "(default)"}</Mono>. Current light model: <Mono>{d.models.light || "same as implementation"}</Mono>.
                    </span>
                    <span>Sign in with GitHub to load available Copilot models, or enter an exact model id below.</span>
                  </span>
                </Notice>
              ) : null}
              <div className="grid gap-3 lg:grid-cols-2">
                <ModelSelect name="worker_model" label="Model for implementation runs" current={d.models.worker} models={currentModels} disabled={false} />
                <ModelSelect name="light_model" label="Model for light runs" current={d.models.light} models={currentModels} disabled={false} />
              </div>
            </section>

            <section className="grid gap-3">
              <div>
                <h3 className="text-sm font-semibold">Budgets</h3>
                <p className="text-xs text-muted-foreground">Steps cap model/tool turns, minutes cap wall-clock time per job, and cost caps provider units with 0 meaning no cap.</p>
              </div>
              <div className="grid gap-3 sm:grid-cols-3">{budgets.map((s) => <LimitInput key={s.name} s={s} />)}</div>
            </section>

            <section className="grid gap-3">
              <h3 className="text-sm font-semibold">Transcript limits</h3>
              <div className="grid gap-3 sm:grid-cols-3">{limits.map((s) => <LimitInput key={s.name} s={s} />)}</div>
            </section>

            <PromptEditor />

            <section className="flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-muted/30 px-3 py-2">
              <div>
                <h3 className="text-sm font-semibold">Activity</h3>
                <p className="text-xs text-muted-foreground">
                  {num(d.sessions.running)} running · {num(d.sessions.today)} today · {num(d.sessions.failed_today)} failed today
                  {" · "}
                  {num(d.sessions.submitted_today)} submitted today · {num(d.sessions.cost_today)} provider units
                </p>
              </div>
              <Button asChild size="sm" variant="outline">
                <Link href={hrefs.harness()}>
                  Open Harness <CheckIcon />
                </Link>
              </Button>
            </section>
          </>
        ) : null}
      </CardContent>
    </Card>
  );
}

export function isHarnessSetting(name: string): boolean {
  return SETTING_KEYS.includes(name);
}
