import { useState } from 'preact/hooks';
import { api, postJson, type Setting, type SettingsPayload } from '../api';
import { Chip } from '../components/ui/chip';
import { EmptyState, Lead, Notice, PageTitle, Panel, Row, SkeletonPage, Stack } from '../components/ui/panel';
import { Mono, When } from '../components/ui/who';
import { cn } from '../lib/cn';
import { useApi } from '../lib/use-api';
import type { ViewProps } from './types';

// The human's caps, grouped the way the human thinks about them. Every name
// here must exist in pulse.py's DEFAULT_SETTINGS; anything the server sends
// that no group lists lands under "Other" so nothing is ever hidden.
const GROUPS: [string, string, string[]][] = [
  ['The machine', 'how much the runner may do at once and per day', ['workers', 'parallel_per_project', 'jobs_per_day', 'minutes_per_job', 'max_ai_credits', 'stop']],
  ['The worker', 'what program runs in each clone and how its work lands', ['runner_cmd', 'worker_model', 'landing']],
  ['The board', 'when a suggestion is ready, when a claim or thread goes stale, when an agent is pruned', ['quorum', 'expiry_days', 'claim_hours', 'stale_days', 'prune_after_days']],
  ['Amendments', 'what it takes to change the constitution by vote (ADR-014)', ['amend_quorum', 'amend_majority']],
];

function hint(s: Setting): string {
  switch (s.kind) {
    case 'count':
      return 'a whole number' + (s.min ? ', at least ' + s.min : '');
    case 'count-or-none':
      return 'a whole number, or none';
    case 'credits':
      return 'a whole number, at least ' + (s.min ?? 30);
    case 'fraction':
      return 'a fraction such as 2/3';
    case 'command':
      return 'a program name or path';
    default:
      return '';
  }
}

function SettingRow({ s, canEdit, onSaved }: { s: Setting; canEdit: boolean; onSaved: () => void }) {
  const [draft, setDraft] = useState(s.value);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState(false);
  const dirty = draft !== s.value;
  const save = async (value: string = draft) => {
    setBusy(true);
    setError('');
    setSaved(false);
    try {
      await postJson('settings', { name: s.name, value });
      setSaved(true);
      onSaved();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const inputClass = 'h-8 rounded-chip border border-line bg-panel px-2.5 text-[13px] text-fg tnum placeholder:text-dim focus-visible:border-accent disabled:opacity-60';
  const isSwitch = s.kind === 'switch';
  const isChoice = s.kind === 'choice';
  const isOn = s.value === '1';
  return (
    <div className="grid gap-1.5 border-t border-line py-3 first:border-t-0 md:grid-cols-[minmax(180px,1fr)_minmax(260px,2fr)_minmax(220px,1fr)] md:items-start md:gap-4">
      <div className="min-w-0">
        <Mono className="text-[13px] font-semibold">{s.name}</Mono>
        <p className="mt-0.5 text-[12.5px] text-muted [overflow-wrap:anywhere]">{s.description}</p>
      </div>
      <div className="min-w-0">
        {isSwitch ? (
          <Row>
            <Chip tone={isOn ? 'bad' : 'ok'} dotted>
              {isOn ? 'runner stopped' : 'runner allowed'}
            </Chip>
            <button type="button" disabled={!canEdit || busy} onClick={() => save(isOn ? '0' : '1')} className={cn('h-8 rounded-chip px-3 text-[13px] font-semibold text-white', isOn ? 'bg-ok' : 'bg-warn', (!canEdit || busy) && 'opacity-60')}>
              {busy ? 'Saving\u2026' : isOn ? 'Let it run' : 'Stop after the runs in flight'}
            </button>
          </Row>
        ) : (
          <Row>
            {isChoice ? (
              <select aria-label={s.name} value={draft} disabled={!canEdit || busy} onChange={(e) => setDraft((e.currentTarget as HTMLSelectElement).value)} className={inputClass}>
                {(s.choices || []).map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
            ) : (
              <input
                type="text"
                inputMode={s.kind === 'count' || s.kind === 'credits' ? 'numeric' : undefined}
                aria-label={s.name}
                value={draft}
                disabled={!canEdit || busy}
                placeholder={s.default || 'empty'}
                onInput={(e) => setDraft((e.currentTarget as HTMLInputElement).value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && dirty && canEdit) save();
                }}
                className={cn(inputClass, s.kind === 'command' || s.kind === 'text' ? 'min-w-[240px] flex-1' : 'w-[120px]')}
              />
            )}
            <button type="button" disabled={!canEdit || busy || !dirty} onClick={() => save()} className={cn('h-8 rounded-chip bg-accent px-3 text-[13px] font-semibold text-white', (!canEdit || busy || !dirty) && 'opacity-50')}>
              {busy ? 'Saving\u2026' : 'Save'}
            </button>
            {dirty && !busy ? (
              <button type="button" onClick={() => setDraft(s.value)} className="h-8 rounded-chip border border-line px-3 text-[13px] hover:border-line-2">
                Undo
              </button>
            ) : null}
          </Row>
        )}
        {hint(s) ? <p className="mt-1 text-[12px] text-dim">{hint(s)}</p> : null}
        {error ? (
          <p role="alert" className="mt-1 text-[12.5px] text-bad [overflow-wrap:anywhere]">
            {error}
          </p>
        ) : saved ? (
          <p role="status" className="mt-1 text-[12.5px] text-ok">
            Saved.
          </p>
        ) : null}
      </div>
      <div className="text-[12.5px] text-muted [overflow-wrap:anywhere]">
        <div>
          {'default '}
          <Mono>{s.default || '(empty)'}</Mono>
        </div>
        {s.updated_at ? (
          <div>
            {'changed '}
            <When d={s.updated_at} />
            {s.updated_by ? ' by ' : ''}
            {s.updated_by ? <Mono>{s.updated_by}</Mono> : null}
          </div>
        ) : (
          <div>never changed</div>
        )}
      </div>
    </div>
  );
}

export function SettingsView({ revision }: ViewProps) {
  const { data, error, reload } = useApi<SettingsPayload>(() => api<SettingsPayload>('settings'), 'settings', revision);
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  if (!data.available) {
    return (
      <>
        <PageTitle sub="the caps the runner works under">Settings</PageTitle>
        <EmptyState hint="python3 docs/scripts/pulse.py init">No store on this machine yet. The settings appear once a project is initialised in the store.</EmptyState>
      </>
    );
  }
  const byName = new Map(data.settings.map((s) => [s.name, s]));
  const listed = new Set(GROUPS.flatMap(([, , names]) => names));
  const other = data.settings.filter((s) => !listed.has(s.name));
  const groups: [string, string, Setting[]][] = GROUPS.map(([title, sub, names]) => [title, sub, names.map((n) => byName.get(n)).filter((s): s is Setting => Boolean(s))]);
  if (other.length) groups.push(['Other', 'settings this page has no group for yet', other]);
  return (
    <>
      <PageTitle sub="the caps the runner works under - yours to tune, never the agents'">Settings</PageTitle>
      <Lead>
        These numbers are the human's (ADR-019, ADR-025). Agents never change them and no vote can; when the machine wants more it posts a finding and waits for you. A change is saved to the store at once, recorded with who made it, and the daemon reads it on its next loop - no restart.
        {data.can_edit ? null : ' This page is being read from another address or without the store, so it shows the settings but cannot change them: open the hub on the machine that runs it.'}
      </Lead>
      <Stack>
        {groups.map(([title, sub, rows]) => (
          <Panel key={title} title={title} actions={<span className="text-[12.5px] text-muted">{sub}</span>}>
            {rows.length ? (
              rows.map((s) => <SettingRow key={s.name + ':' + s.value + ':' + s.updated_at} s={s} canEdit={data.can_edit} onSaved={reload} />)
            ) : (
              <EmptyState>Nothing here.</EmptyState>
            )}
          </Panel>
        ))}
      </Stack>
    </>
  );
}
