import * as Switch from '@radix-ui/react-switch';
import { cn } from '../../lib/cn';

export function Seg({ options, value, onChange, label }: { options: [string, string][]; value: string; onChange: (v: string) => void; label: string }) {
  return (
    <div role="group" aria-label={label} className="inline-flex rounded-chip border border-line bg-panel-2 p-0.5">
      {options.map(([v, lab]) => (
        <button key={v} type="button" aria-pressed={v === value} onClick={() => onChange(v)} className={cn('rounded-[6px] px-2.5 py-0.5 text-[12.5px] text-muted hover:text-fg', v === value && 'bg-panel text-fg shadow-panel')}>
          {lab}
        </button>
      ))}
    </div>
  );
}

export function Toggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="inline-flex cursor-pointer items-center gap-2 text-[13px] text-muted">
      <Switch.Root checked={checked} onCheckedChange={onChange} className={cn('relative h-[18px] w-[32px] rounded-full border border-line-2 bg-chip transition-colors data-[state=checked]:border-accent data-[state=checked]:bg-accent')}>
        <Switch.Thumb className="block h-[14px] w-[14px] translate-x-[1px] rounded-full bg-white shadow transition-transform data-[state=checked]:translate-x-[15px]" />
      </Switch.Root>
      {label}
    </label>
  );
}
