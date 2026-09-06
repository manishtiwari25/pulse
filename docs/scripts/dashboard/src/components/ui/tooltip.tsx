import * as T from '@radix-ui/react-tooltip';
import type { ComponentChildren } from 'preact';

export const TooltipProvider = T.Provider;

// A Radix tooltip whose content is always text. Wraps any focusable trigger;
// non-focusable children get a span with tabIndex so keyboard users reach it.
export function Tip({ text, children, side = 'top' }: { text: string; children: ComponentChildren; side?: 'top' | 'bottom' | 'left' | 'right' }) {
  if (!text) return <>{children}</>;
  return (
    <T.Root delayDuration={250}>
      <T.Trigger asChild>{children}</T.Trigger>
      <T.Portal>
        <T.Content side={side} sideOffset={6} collisionPadding={8} className="z-50 max-w-[360px] rounded-chip bg-tip-bg px-2.5 py-1.5 text-[12px] leading-snug text-tip-fg shadow-panel [overflow-wrap:anywhere] select-none">
          {text}
          <T.Arrow className="fill-tip-bg" width={10} height={5} />
        </T.Content>
      </T.Portal>
    </T.Root>
  );
}
