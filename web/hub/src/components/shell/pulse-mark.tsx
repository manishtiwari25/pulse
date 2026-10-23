import { cn } from "@/lib/utils";

// The brand mark: the logo's pulse waveform on the mint-sky-violet tile.
// The only place the gradient appears (Pip, the mascot, wears the same one).
// Inside a `.brand-link` the line redraws itself on hover.
export function PulseMark({ className }: { className?: string }) {
  return (
    <span aria-hidden className={cn("inline-flex size-8 shrink-0 items-center justify-center rounded-[9px] bg-brand shadow-[inset_0_0_0_1px_oklch(1_0_0/.25)]", className)}>
      <svg viewBox="0 0 256 256" className="size-[78%]" fill="none">
        <path d="M30 135h41l19-47 34 101 31-77 18 23h53" stroke="white" strokeWidth="22" strokeLinecap="round" strokeLinejoin="round" className="pulse-mark-path" />
      </svg>
    </span>
  );
}

// The same waveform as a line, drawn once when the page loads (the home
// greeting). Reduced motion shows it drawn.
export function PulseLine({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 132 28" fill="none" aria-hidden className={cn("h-7 w-[132px] text-primary", className)}>
      <path
        d="M2 15h38l8-11 12 21 11-17 7 7h52"
        stroke="currentColor"
        strokeWidth="2.25"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="pulse-draw [--pulse-length:170]"
      />
      <circle cx="130" cy="15" r="2.6" fill="currentColor" />
    </svg>
  );
}
