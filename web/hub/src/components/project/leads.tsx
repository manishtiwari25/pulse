"use client";

import { useState } from "react";
import { CheckIcon, CrownIcon, Loader2Icon, PauseIcon, PenLineIcon, PlayIcon, PlusIcon, SparklesIcon, SquareIcon, XIcon, ZapIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { showApiError } from "@/components/pulse/confirm";
import { Panel } from "@/components/pulse/panel";
import { Mono, ToneBadge, When } from "@/components/pulse/who";
import { RoleMascot } from "@/components/project/lead-mascots";
import { writes, type Lead, type LeadGap, type LeadInput, type LeadRole, type LeadsPayload } from "@/lib/api";
import { num, plural } from "@/lib/format";
import { invalidate, useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// The seats on a project: one lead per concern (the twelve built-in roles,
// or a word of the owner's), each sitting at its cadence under its brief.
// The panel lists who sits, offers the seats PULSE suggests from what the
// project shows, and opens a gallery of the rest - one illustrated card per
// role - where a click picks a seat and one more seats it. The last card
// takes a role of the owner's own: a word, a title, a brief, a cadence.
export function LeadsPanel({ slug, canDecide = true }: { slug: string; canDecide?: boolean }) {
  const q = useApi<LeadsPayload>(slug === "commons" ? null : "leads?project=" + encodeURIComponent(slug), { refresh: 60000 });
  const [busy, setBusy] = useState("");
  const [open, setOpen] = useState(false);
  if (slug === "commons") return null;
  const d = q.data;
  const leads = (d?.leads ?? []).filter((l) => l.status !== "ended");
  const suggestions = d?.suggestions ?? [];
  const seated = new Set(leads.map((l) => l.role));
  const spare = (d?.roles ?? []).filter((r) => !seated.has(r.role));
  const refresh = () => invalidate((p) => p.startsWith("leads") || p.startsWith("roster") || p.startsWith("projects/"));

  const act = async (key: string, what: string, run: () => Promise<unknown>, done: string) => {
    setBusy(key);
    const id = toast.loading(what + "…");
    try {
      await run();
      toast.success(done, { id });
      refresh();
      return true;
    } catch (e) {
      toast.dismiss(id);
      showApiError(e);
      return false;
    } finally {
      setBusy("");
    }
  };
  const seat = (input: LeadInput, title: string) => act("seat:" + input.role, "Seating the " + title.toLowerCase(), () => writes.addLead(input), title + " seated. Its first sitting is queued.");

  const summary = leads.length ? plural(leads.length, "lead") + " seated" + (suggestions.length ? ", " + plural(suggestions.length, "suggestion") : "") : suggestions.length ? plural(suggestions.length, "suggested seat") : undefined;
  return (
    <Panel
      title="Leads"
      icon={CrownIcon}
      actions={
        <div className="flex items-center gap-3">
          {summary ? <span>{summary}</span> : null}
          {canDecide && d && !open ? (
            <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
              <PlusIcon /> Seat a lead
            </Button>
          ) : null}
        </div>
      }
    >
      {!d ? (
        <p className="text-[13px] text-muted-foreground">{q.error ? "The leads didn't load (" + q.error + "). They refresh on their own every minute." : "Loading…"}</p>
      ) : (
        <div className="grid gap-4">
          {leads.length ? (
            <ul className="grid gap-3">
              {leads.map((l) => (
                <LeadRow key={l.id} lead={l} busy={busy} canDecide={canDecide} act={act} />
              ))}
            </ul>
          ) : (
            <p className="text-sm leading-relaxed text-muted-foreground">
              No role lead sits on {slug} yet. The project lead tends the board; a role lead owns one concern and sits at its own cadence - the self-improvement loop, the guides, the security read, the health watch, the tests, the releases, and more.
            </p>
          )}
          {suggestions.length ? (
            <div className="grid gap-2">
              <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">PULSE suggests</p>
              {suggestions.map((g) => (
                <GapRow key={g.role} gap={g} busy={busy === "seat:" + g.role} canDecide={canDecide} onSeat={() => seat({ project: slug, role: g.role }, g.title)} />
              ))}
            </div>
          ) : null}
          {canDecide && open ? (
            <SeatGallery
              slug={slug}
              roles={spare}
              suggested={new Map(suggestions.map((g) => [g.role, g.why]))}
              busy={busy.startsWith("seat:")}
              onClose={() => setOpen(false)}
              onSeat={async (input, title) => {
                if (await seat(input, title)) setOpen(false);
              }}
            />
          ) : (
            <p className="text-[12.5px] leading-relaxed text-muted-foreground">
              From a shell: <Mono>{"pulse lead add --project " + slug + " --role <role>"}</Mono>
              {" - a built-in role by its name, or a word of your own with "}
              <Mono>{"--title '…' --brief '…'"}</Mono>.
            </p>
          )}
        </div>
      )}
    </Panel>
  );
}

function LeadRow({ lead: l, busy, canDecide, act }: { lead: Lead; busy: string; canDecide: boolean; act: (key: string, what: string, run: () => Promise<unknown>, done: string) => Promise<boolean> }) {
  const paused = l.status === "paused";
  const mine = busy.startsWith(l.id + ":");
  const every = everyText(l.every_minutes);
  return (
    <li className="flex min-w-0 flex-wrap items-start justify-between gap-x-4 gap-y-2 rounded-xl border bg-card/60 px-4 py-3">
      <div className="flex min-w-0 flex-1 basis-64 items-start gap-3">
        <RoleMascot role={l.role} size={44} mood={paused ? "asleep" : "idle"} title={l.title} className={cn("mt-0.5", paused && "opacity-70 saturate-50")} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-medium text-foreground">{l.title}</span>
            <ToneBadge tone={paused ? "neutral" : "ok"} dot>
              {paused ? "Paused" : "Active"}
            </ToneBadge>
            <Mono>{l.agent}</Mono>
          </div>
          <p className="mt-1 text-[13px] text-muted-foreground">
            {"Sits " + every + ". "}
            {paused ? (
              "Paused" + (l.note ? ": " + l.note.replace(/[.!?]+$/, "") : "") + ". "
            ) : (
              <>
                {"Next sitting "}
                <When d={l.next_at} />
                {". "}
              </>
            )}
            {l.runs ? (
              <>
                {plural(l.runs, "sitting") + " so far, last "}
                <When d={l.last_at} />
                {"."}
              </>
            ) : (
              "No sitting yet."
            )}
          </p>
        </div>
      </div>
      {canDecide ? (
        <div className="flex shrink-0 flex-wrap items-center gap-1.5">
          <Button size="sm" variant="outline" disabled={!!busy || paused} title="Queue a sitting now" onClick={() => act(l.id + ":run", "Queueing a sitting", () => writes.leadAction(l.id, "run"), l.title + " sits next.")}>
            {busy === l.id + ":run" ? <Loader2Icon className="animate-spin" /> : <ZapIcon />} Sit now
          </Button>
          {paused ? (
            <Button size="sm" variant="outline" disabled={!!busy} onClick={() => act(l.id + ":resume", "Resuming", () => writes.leadAction(l.id, "resume"), l.title + " sits again.")}>
              {busy === l.id + ":resume" ? <Loader2Icon className="animate-spin" /> : <PlayIcon />} Resume
            </Button>
          ) : (
            <Button size="sm" variant="outline" disabled={!!busy} onClick={() => act(l.id + ":pause", "Pausing", () => writes.leadAction(l.id, "pause"), l.title + " is paused; resume it any time.")}>
              {busy === l.id + ":pause" ? <Loader2Icon className="animate-spin" /> : <PauseIcon />} Pause
            </Button>
          )}
          <Button size="sm" variant="ghost" disabled={!!busy} title="End this seat; its queued sittings are cancelled" onClick={() => act(l.id + ":end", "Ending the seat", () => writes.leadAction(l.id, "end"), l.title + " ended.")}>
            {mine && busy.endsWith(":end") ? <Loader2Icon className="animate-spin" /> : <SquareIcon />} End
          </Button>
        </div>
      ) : null}
    </li>
  );
}

function GapRow({ gap: g, busy, canDecide, onSeat }: { gap: LeadGap; busy: boolean; canDecide: boolean; onSeat: () => void }) {
  return (
    <div className="flex min-w-0 flex-wrap items-start justify-between gap-x-4 gap-y-2 rounded-xl border border-dashed bg-card/40 px-4 py-3">
      <div className="flex min-w-0 flex-1 basis-64 items-start gap-3">
        <RoleMascot role={g.role} size={44} title={g.title} className="mt-0.5" />
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium text-foreground">{g.title}</p>
          <p className="mt-0.5 text-[13px] leading-relaxed text-muted-foreground">{g.why}</p>
          <p className="mt-1 text-[12.5px] text-muted-foreground">
            Or from a shell: <Mono>{g.command}</Mono>
          </p>
        </div>
      </div>
      {canDecide ? (
        <Button size="sm" disabled={busy} onClick={onSeat}>
          {busy ? <Loader2Icon className="animate-spin" /> : <PlusIcon />} Seat this lead
        </Button>
      ) : null}
    </div>
  );
}

const CUSTOM = "__custom";
const EVERY_CHOICES: [string, string][] = [
  ["12h", "twice a day"],
  ["1d", "every day"],
  ["3d", "every three days"],
  ["1w", "every week"],
  ["2w", "every two weeks"],
];
const ROLE_WORD = /^[a-z][a-z0-9-]{1,23}$/;

// The gallery: a card per role nobody sits in, and one for a role of the
// owner's own. A click picks (the card lifts, its mascot hops, a check
// appears); the footer says what the seat does and seats it.
function SeatGallery({ slug, roles, suggested, busy, onClose, onSeat }: { slug: string; roles: LeadRole[]; suggested: Map<string, string>; busy: boolean; onClose: () => void; onSeat: (input: LeadInput, title: string) => Promise<void> }) {
  const [pick, setPick] = useState("");
  const [word, setWord] = useState("");
  const [title, setTitle] = useState("");
  const [brief, setBrief] = useState("");
  const [every, setEvery] = useState("1d");
  const chosen = roles.find((r) => r.role === pick) ?? null;
  const custom = pick === CUSTOM;
  const wordOk = ROLE_WORD.test(word.trim());
  const taken = roles.some((r) => r.role === word.trim());
  const customOk = wordOk && !taken && title.trim().length > 0 && brief.trim().length > 0;
  const customTitle = title.trim() || (word.trim() ? word.trim()[0].toUpperCase() + word.trim().slice(1) + " lead" : "Your lead");
  const choose = (role: string) => setPick((p) => (p === role ? "" : role));
  const submit = () => {
    if (chosen) return onSeat({ project: slug, role: chosen.role }, chosen.title);
    if (custom && customOk) return onSeat({ project: slug, role: word.trim(), title: title.trim(), brief: brief.trim(), every }, title.trim());
  };
  return (
    <div className="grid gap-3 rounded-2xl border bg-card/40 p-3 sm:p-4" role="group" aria-label="Seat a lead">
      <div className="flex items-start justify-between gap-3">
        <div>
          <p className="text-sm font-medium text-foreground">Who should sit next?</p>
          <p className="text-[13px] text-muted-foreground">Each lead is an agent of its own that owns one concern and sits at its own cadence. Pick a card.</p>
        </div>
        <Button size="icon" variant="ghost" aria-label="Close" onClick={onClose}>
          <XIcon />
        </Button>
      </div>
      <ul className="grid grid-cols-2 gap-2.5 sm:grid-cols-3 md:grid-cols-4 xl:grid-cols-6">
        {roles.map((r, i) => (
          <RoleCard key={r.role} index={i} role={r.role} title={r.title} every={r.every} picked={pick === r.role} suggested={suggested.has(r.role)} onPick={() => choose(r.role)} />
        ))}
        <RoleCard index={roles.length} role={word.trim() && custom ? word.trim() : "custom"} title="A word of your own" every={custom ? every : ""} picked={custom} custom onPick={() => choose(CUSTOM)} />
      </ul>
      {chosen ? (
        <div key={chosen.role} className="flex flex-wrap items-center justify-between gap-x-4 gap-y-3 rounded-xl border bg-card px-4 py-3 animate-in fade-in-0 slide-in-from-bottom-1 duration-300">
          <div className="min-w-0 flex-1 basis-72">
            <p className="text-sm text-foreground">
              <span className="font-medium">{chosen.title}</span> {chosen.about}.
            </p>
            <p className="mt-1 text-[12.5px] text-muted-foreground">
              {"Sits " + everyWord(chosen.every) + " as "}
              <Mono>{slug + "-lead-" + chosen.role}</Mono>
              {suggested.has(chosen.role) ? <span className="ml-1.5 text-you-ink">{"· suggested: " + suggested.get(chosen.role)}</span> : null}
            </p>
          </div>
          <Button size="sm" disabled={busy} onClick={submit}>
            {busy ? <Loader2Icon className="animate-spin" /> : <CheckIcon />} {"Seat the " + chosen.title.toLowerCase()}
          </Button>
        </div>
      ) : custom ? (
        <form
          className="grid gap-3 rounded-xl border bg-card px-4 py-3 animate-in fade-in-0 slide-in-from-bottom-1 duration-300"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <p className="text-sm text-foreground">
            <span className="font-medium">{customTitle}</span> - a concern the built-in roles do not cover. Its brief is what the lead reads at every sitting, so say what it owns and what counts as done.
          </p>
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="grid content-start gap-1.5">
              <Label htmlFor="lead-word">Role word</Label>
              <Input id="lead-word" value={word} placeholder="content" autoFocus spellCheck={false} autoComplete="off" onChange={(e) => setWord(e.target.value.toLowerCase())} aria-invalid={word.trim() !== "" && (!wordOk || taken)} />
              <p className="text-[12px] text-muted-foreground">{taken ? "That is a built-in role; pick its card." : "Lowercase letters, digits, hyphens."}</p>
            </div>
            <div className="grid content-start gap-1.5">
              <Label htmlFor="lead-title">Title</Label>
              <Input id="lead-title" value={title} placeholder="Content lead" onChange={(e) => setTitle(e.target.value)} />
              <p className="text-[12px] text-muted-foreground">
                {"Its agent will be "}
                <Mono>{slug + "-lead-" + (wordOk ? word.trim() : "<word>")}</Mono>
              </p>
            </div>
          </div>
          <div className="grid gap-1.5 sm:max-w-56">
            <Label htmlFor="lead-every">Sits</Label>
            <Select value={every} onValueChange={setEvery}>
              <SelectTrigger id="lead-every" aria-label="Cadence" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {EVERY_CHOICES.map(([v, label]) => (
                  <SelectItem key={v} value={v}>
                    {label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="lead-brief">Standing brief</Label>
            <Textarea id="lead-brief" value={brief} rows={3} placeholder="Write one post a week in docs/blog, in the project's voice; keep the archive page true to what is there; a post a reader could not follow is a bug." onChange={(e) => setBrief(e.target.value)} />
          </div>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-[12.5px] text-muted-foreground">
              The same as <Mono>{"pulse lead add --project " + slug + " --role " + (wordOk ? word.trim() : "<word>") + " --title '…' --brief '…' --every " + every}</Mono>
            </p>
            <Button size="sm" type="submit" disabled={busy || !customOk}>
              {busy ? <Loader2Icon className="animate-spin" /> : <CheckIcon />} {title.trim() || word.trim() ? "Seat the " + customTitle.toLowerCase() : "Seat this lead"}
            </Button>
          </div>
        </form>
      ) : null}
    </div>
  );
}

function RoleCard({ index, role, title, every, picked, suggested, custom, onPick }: { index: number; role: string; title: string; every: string; picked: boolean; suggested?: boolean; custom?: boolean; onPick: () => void }) {
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
          custom && !picked && "border-dashed",
        )}
        style={{ "--mascot-delay": (index % 5) * 0.9 + "s" } as React.CSSProperties}
      >
        {picked ? (
          <span className="absolute top-2 right-2 flex size-5 items-center justify-center rounded-full bg-primary text-primary-foreground animate-in zoom-in-50 duration-200" aria-hidden>
            <CheckIcon className="size-3" />
          </span>
        ) : suggested ? (
          <span className="absolute top-2 right-2 flex size-5 items-center justify-center rounded-full bg-you text-you-ink" title="PULSE suggests this seat" aria-label="Suggested">
            <SparklesIcon className="size-3" />
          </span>
        ) : custom ? (
          <span className="absolute top-2 right-2 flex size-5 items-center justify-center rounded-full bg-muted text-muted-foreground" aria-hidden>
            <PenLineIcon className="size-3" />
          </span>
        ) : null}
        <RoleMascot role={role} size={64} mood={picked ? "picked" : "idle"} title={title} />
        <span className="mt-0.5 text-[13px] leading-tight font-medium text-foreground">{title}</span>
        <span className="text-[11.5px] text-muted-foreground">{every ? everyWord(every) : "your words"}</span>
      </button>
    </li>
  );
}

function everyWord(every: string): string {
  const m = /^(\d+)([hdw])$/.exec(every);
  if (!m) return "every " + every;
  const n = Number(m[1]);
  const unit = m[2] === "h" ? "hour" : m[2] === "d" ? "day" : "week";
  if (n === 1) return unit === "day" ? "daily" : unit === "week" ? "weekly" : "hourly";
  return "every " + plural(n, unit);
}

function everyText(minutes: number): string {
  if (minutes % (7 * 24 * 60) === 0) return minutes === 7 * 24 * 60 ? "weekly" : "every " + plural(minutes / (7 * 24 * 60), "week");
  if (minutes % (24 * 60) === 0) return minutes === 24 * 60 ? "daily" : "every " + plural(minutes / (24 * 60), "day");
  if (minutes % 60 === 0) return minutes === 60 ? "hourly" : "every " + plural(minutes / 60, "hour");
  return "every " + num(minutes) + " min";
}
