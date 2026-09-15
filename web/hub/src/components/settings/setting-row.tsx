"use client";

import { useState } from "react";
import { EraserIcon, EyeOffIcon, RotateCcwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { Mono, ToneBadge, When } from "@/components/pulse/who";
import { cn } from "@/lib/utils";
import { writes, type Setting } from "@/lib/api";

export function hint(s: Setting): string {
  if (s.secret) return "owner-private: stored, but never shown here, to agents or in events";
  switch (s.kind) {
    case "count":
      return "a whole number" + (s.min ? ", at least " + s.min : "");
    case "count-or-none":
      return "a whole number, or none";
    case "credits":
      return "a whole number, at least " + (s.min ?? 30);
    case "number":
      return "a number, decimals allowed" + (s.min !== undefined && s.min !== null ? ", at least " + s.min : "");
    case "fraction":
      return "a fraction such as 2/3";
    case "command":
      return "a program name or path";
    case "identifier":
      return "a name, id or path - never a secret";
    case "reference":
      return "where the key lives - op://VAULT/ITEM/FIELD, keychain://SERVICE/ACCOUNT or file:///PATH - never the key itself";
    default:
      return "";
  }
}

// One setting row, two columns from md up: what it is (name, description,
// default and last change on one quiet line) and the editor for its kind
// with Save right next to it and the hint after it. On phones the editor
// stacks under the text. Saving is confirmed, then POST /api/v1/settings.
// `title` puts a human name in front of the key for a row shown on its own
// (the kill switch).
type SettingLabels = { on?: string; off?: string; choices?: Record<string, string> };

export function SettingRow({ s, canEdit, labels, title }: { s: Setting; canEdit: boolean; labels?: SettingLabels; title?: React.ReactNode }) {
  const [draftState, setDraftState] = useState({ name: s.name, value: s.value, draft: s.value });
  const [pending, setPending] = useState<string | null>(null);
  let draft = draftState.draft;
  if (draftState.name !== s.name || draftState.value !== s.value) {
    draft = s.value;
    setDraftState({ name: s.name, value: s.value, draft: s.value });
  }
  const setDraft = (value: string) => setDraftState({ name: s.name, value: s.value, draft: value });
  const dirty = draft !== s.value;
  const isSwitch = s.kind === "switch";
  const isChoice = s.kind === "choice";
  const isOn = s.value === "1";
  const isStop = s.name === "stop";
  const secret = !!s.secret;
  const unset = s.updated_by === "default" || !s.updated_at;
  const wide = s.kind === "command" || s.kind === "text" || s.kind === "identifier" || s.kind === "reference";
  const h = hint(s);
  return (
    <div className="grid gap-3 px-4 py-3 md:grid-cols-[minmax(0,1fr)_auto] md:items-center">
      <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1">
        {title ? <span className="text-sm font-semibold">{title}</span> : null}
        <Mono className={cn("font-medium text-foreground", title && "text-muted-foreground")}>{s.name}</Mono>
        {secret ? (
          <ToneBadge tone="warn" className="gap-1 self-center">
            <EyeOffIcon className="size-3" /> secret
          </ToneBadge>
        ) : null}
        <p className="basis-full text-xs leading-snug text-muted-foreground anywhere">
          {"default "}
          <Mono className="text-[11.5px]">{s.default || "(empty)"}</Mono>
          {" · "}
          {unset ? (
            "not set yet - the default is in effect"
          ) : (
            <>
              {"changed "}
              <When d={s.updated_at} />
              {s.updated_by ? " by " : ""}
              {s.updated_by ? <Mono className="text-[11.5px]">{s.updated_by}</Mono> : null}
            </>
          )}
        </p>
        <p className="basis-full text-[13px] leading-snug text-muted-foreground anywhere">{s.description}</p>
      </div>
      <div className="flex min-w-0 flex-col gap-1.5 md:items-end">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
        {isSwitch ? (
          <div className="flex min-h-8 items-center gap-2">
            <Switch id={"sw-" + s.name} checked={isOn} disabled={!canEdit} onCheckedChange={(v) => setPending(v ? "1" : "0")} aria-label={s.name} />
            <label htmlFor={"sw-" + s.name} className={cn("text-[13px]", isOn ? (isStop ? "font-semibold text-bad" : "text-ok") : isStop ? "text-ok" : "text-muted-foreground")}>
              {isOn ? labels?.on ?? "on" : labels?.off ?? "off"}
            </label>
          </div>
        ) : (
          <>
            {isChoice ? (
              <Select value={draft} onValueChange={setDraft} disabled={!canEdit}>
                <SelectTrigger aria-label={s.name} className="h-8 w-56">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {(s.choices || []).map((c) => (
                    <SelectItem key={c} value={c}>
                      {labels?.choices?.[c] ?? c}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            ) : (
              <Input
                type={secret ? "password" : "text"}
                inputMode={s.kind === "count" || s.kind === "credits" ? "numeric" : s.kind === "number" ? "decimal" : undefined}
                aria-label={s.name}
                value={secret && !dirty ? "" : draft}
                disabled={!canEdit}
                placeholder={secret ? (s.value ? "(set - value never shown)" : "(not set)") : s.default || "empty"}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  // preventDefault: the dialog focuses its button at once and the same keypress would activate it.
                  if (e.key === "Enter" && dirty && canEdit) {
                    e.preventDefault();
                    setPending(draft);
                  }
                  if (e.key === "Escape") setDraft(s.value);
                }}
                className={cn("h-8", s.kind === "count" || s.kind === "count-or-none" || s.kind === "credits" || s.kind === "number" ? "w-32 shrink-0 tnum" : wide ? "w-64 max-w-full" : "w-32 shrink-0")}
              />
            )}
            <Button size="sm" variant={dirty ? "default" : "outline"} disabled={!canEdit || !dirty} onClick={() => setPending(draft)}>
              Save
            </Button>
            {dirty ? (
              <Button size="sm" variant="ghost" onClick={() => setDraft(s.value)} aria-label="Undo">
                <RotateCcwIcon /> Undo
              </Button>
            ) : null}
            {secret && s.value && !dirty && canEdit ? (
              <Button size="sm" variant="ghost" onClick={() => setPending("")} aria-label={"Clear " + s.name}>
                <EraserIcon /> Clear
              </Button>
            ) : null}
          </>
        )}
        </div>
        {h ? <p className="text-xs leading-snug text-muted-foreground anywhere">{h}</p> : null}
      </div>
      <ConfirmDialog
        open={pending !== null}
        onOpenChange={(o) => !o && setPending(null)}
        title={"Change " + s.name + "?"}
        description={
          <span>
            {isSwitch ? (pending === "1" ? "Turn it on" : "Turn it off") : secret ? (
              pending ? (s.value ? "Replace the private value (never shown here)" : "Set the private value (never shown here)") : "Clear the private value"
            ) : <>
              from <Mono>{s.value || "(empty)"}</Mono> to <Mono>{pending || "(empty)"}</Mono>
            </>}
            . Saved to the store at once, recorded with who made it; the daemon reads it on its next loop.
          </span>
        }
        action="Save"
        success={s.name + " saved."}
        onConfirm={() => writes.setSetting(s.name, pending ?? "")}
        invalidates={(p) => p.startsWith("settings") || p.startsWith("runner") || p.startsWith("compute") || p.startsWith("events") || p.startsWith("org")}
      />
    </div>
  );
}
