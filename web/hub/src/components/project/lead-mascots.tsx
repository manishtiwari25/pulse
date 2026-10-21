import type { CSSProperties } from "react";
import { cn } from "@/lib/utils";

// One small cartoon per lead role: an object with a face, drawn inline so it
// takes the theme's ink and needs no image. The bodies are flat pastels that
// read on paper and in the dark; the face is always navy. Motion lives in
// globals.css (`mascot-*`) and stops under prefers-reduced-motion: the eyes
// blink now and then, a hovered card wiggles once, a picked one hops.
export type MascotMood = "idle" | "picked" | "asleep";

const BODY: Record<string, string> = {
  rsi: "oklch(0.74 0.13 290)",
  docs: "oklch(0.78 0.11 230)",
  security: "oklch(0.76 0.12 160)",
  monitor: "oklch(0.72 0.11 245)",
  tests: "oklch(0.81 0.12 180)",
  deps: "oklch(0.82 0.09 65)",
  release: "oklch(0.74 0.14 22)",
  perf: "oklch(0.84 0.13 95)",
  a11y: "oklch(0.8 0.1 335)",
  triage: "oklch(0.78 0.09 270)",
  tidy: "oklch(0.8 0.1 130)",
  dx: "oklch(0.75 0.12 42)",
  tool: "oklch(0.78 0.12 200)",
  skill: "oklch(0.83 0.11 80)",
};
const INK = "oklch(0.32 0.045 262)";
const BLUSH = "oklch(0.72 0.16 15)";
const PAPER = "oklch(0.985 0.004 95)";

export function mascotColor(role: string): string {
  return BODY[role] ?? "oklch(0.82 0.025 250)";
}

// `color` overrides the role's body colour, so a family of one shape (the
// toolkit's plugs) can still tell its members apart.
export function RoleMascot({ role, size = 64, mood = "idle", className, title, color }: { role: string; size?: number; mood?: MascotMood; className?: string; title?: string; color?: string }) {
  const Body = BODIES[role] ?? Sticker;
  const body = color ?? mascotColor(role);
  const style = { "--m": body, "--m-deep": "color-mix(in oklch, " + body + " 72%, " + INK + ")", "--m-light": "color-mix(in oklch, " + body + " 55%, white)" } as CSSProperties;
  return (
    <svg viewBox="0 0 64 64" width={size} height={size} role="img" aria-label={title ?? role + " mascot"} className={cn("mascot shrink-0 overflow-visible", className)} data-mood={mood} style={style}>
      <Body mood={mood} />
    </svg>
  );
}

// Eyes, blush and a mouth, centred on (cx, cy). `w` spreads the eyes.
function Face({ cx, cy, w = 7, mood, scale = 1, mouth = "smile" }: { cx: number; cy: number; w?: number; mood: MascotMood; scale?: number; mouth?: "smile" | "pulse" | "o" }) {
  const s = scale;
  const eyeY = cy - 1.5 * s;
  const happy = mood === "picked";
  const asleep = mood === "asleep";
  const mouthPath = () => {
    const y = cy + 4.5 * s;
    if (mouth === "pulse") return <path d={`M${cx - 6 * s} ${y} h${2 * s} l${1.3 * s} -${3 * s} l${1.7 * s} ${6 * s} l${1.7 * s} -${4.5 * s} l${1.3 * s} ${1.5 * s} h${2 * s}`} fill="none" stroke={INK} strokeWidth={1.5 * s} strokeLinecap="round" strokeLinejoin="round" />;
    if (mouth === "o" || (happy && mouth !== "smile")) return <ellipse cx={cx} cy={y} rx={2.2 * s} ry={happy ? 3 * s : 2 * s} fill={INK} />;
    if (happy) return <path d={`M${cx - 4.5 * s} ${y - 1.5 * s} q${4.5 * s} ${6.5 * s} ${9 * s} 0 z`} fill={INK} />;
    if (asleep) return <path d={`M${cx - 2.5 * s} ${y} q${2.5 * s} ${1.5 * s} ${5 * s} 0`} fill="none" stroke={INK} strokeWidth={1.4 * s} strokeLinecap="round" />;
    return <path d={`M${cx - 3.5 * s} ${y - 0.5 * s} q${3.5 * s} ${4 * s} ${7 * s} 0`} fill="none" stroke={INK} strokeWidth={1.6 * s} strokeLinecap="round" />;
  };
  return (
    <g className="mascot-face">
      <circle cx={cx - w - 2.5 * s} cy={cy + 2.5 * s} r={2.2 * s} fill={BLUSH} opacity={0.4} />
      <circle cx={cx + w + 2.5 * s} cy={cy + 2.5 * s} r={2.2 * s} fill={BLUSH} opacity={0.4} />
      {asleep ? (
        <g stroke={INK} strokeWidth={1.6 * s} strokeLinecap="round" fill="none">
          <path d={`M${cx - w - 2.2 * s} ${eyeY} q${2.2 * s} ${2 * s} ${4.4 * s} 0`} />
          <path d={`M${cx + w - 2.2 * s} ${eyeY} q${2.2 * s} ${2 * s} ${4.4 * s} 0`} />
        </g>
      ) : happy ? (
        <g stroke={INK} strokeWidth={1.8 * s} strokeLinecap="round" fill="none">
          <path d={`M${cx - w - 2.4 * s} ${eyeY + 1.2 * s} q${2.4 * s} -${3.2 * s} ${4.8 * s} 0`} />
          <path d={`M${cx + w - 2.4 * s} ${eyeY + 1.2 * s} q${2.4 * s} -${3.2 * s} ${4.8 * s} 0`} />
        </g>
      ) : (
        <g className="mascot-eyes">
          <circle cx={cx - w} cy={eyeY} r={2 * s} fill={INK} />
          <circle cx={cx + w} cy={eyeY} r={2 * s} fill={INK} />
          <circle cx={cx - w + 0.7 * s} cy={eyeY - 0.7 * s} r={0.6 * s} fill={PAPER} />
          <circle cx={cx + w + 0.7 * s} cy={eyeY - 0.7 * s} r={0.6 * s} fill={PAPER} />
        </g>
      )}
      {mouthPath()}
      {asleep ? (
        <text x={cx + w + 8 * s} y={cy - 7 * s} fontSize={7 * s} fontWeight={700} fill={INK} opacity={0.7} fontFamily="ui-sans-serif, system-ui">
          z
        </text>
      ) : null}
    </g>
  );
}

type BodyProps = { mood: MascotMood };

// rsi: a loop of two arrows chasing each other, the face in the middle.
function Loop({ mood }: BodyProps) {
  return (
    <>
      <circle cx={32} cy={32} r={26} fill="var(--m-light)" />
      <g className="mascot-spin-slow" style={{ transformOrigin: "32px 32px" }}>
        <g fill="none" stroke="var(--m-deep)" strokeWidth={5} strokeLinecap="round">
          <path d="M14 27 A19 19 0 0 1 48 20" />
          <path d="M50 37 A19 19 0 0 1 16 44" />
        </g>
        <g fill="var(--m-deep)">
          <path d="M44 13 l9 5 l-8 6 z" />
          <path d="M20 51 l-9 -5 l8 -6 z" />
        </g>
      </g>
      <circle cx={32} cy={32} r={13} fill="var(--m)" />
      <Face cx={32} cy={32} w={5} scale={0.85} mood={mood} />
    </>
  );
}

// docs: a closed book, the face on its cover.
function Book({ mood }: BodyProps) {
  return (
    <>
      <rect x={13} y={9} width={40} height={46} rx={5} fill="var(--m-deep)" />
      <rect x={17} y={7} width={38} height={46} rx={5} fill="var(--m)" />
      <rect x={17} y={7} width={8} height={46} rx={3} fill="var(--m-deep)" opacity={0.55} />
      <rect x={22} y={50} width={31} height={5} rx={2} fill={PAPER} opacity={0.9} />
      <path d="M44 7 v12 l3.5 -3 l3.5 3 v-12 z" fill={BLUSH} opacity={0.85} />
      <Face cx={38} cy={30} w={6} mood={mood} />
    </>
  );
}

// security: a shield with a face.
function Shield({ mood }: BodyProps) {
  return (
    <>
      <path d="M32 5 L54 13 V31 C54 44 45 53 32 59 C19 53 10 44 10 31 V13 Z" fill="var(--m-deep)" />
      <path d="M32 9 L50 15.5 V31 C50 42 42 50 32 55 C22 50 14 42 14 31 V15.5 Z" fill="var(--m)" />
      <path d="M32 9 L50 15.5 V31 C50 42 42 50 32 55 Z" fill="var(--m-light)" opacity={0.35} />
      <Face cx={32} cy={31} w={6.5} mood={mood} />
    </>
  );
}

// monitor: a screen on a stand; its mouth is a pulse line.
function Screen({ mood }: BodyProps) {
  return (
    <>
      <rect x={24} y={49} width={16} height={5} rx={1.5} fill="var(--m-deep)" />
      <rect x={18} y={54} width={28} height={4} rx={2} fill="var(--m-deep)" />
      <rect x={6} y={9} width={52} height={40} rx={6} fill="var(--m-deep)" />
      <rect x={10} y={13} width={44} height={32} rx={4} fill="var(--m)" />
      <circle cx={49} cy={18} r={2} fill={PAPER} opacity={0.8} className="mascot-beat" />
      <Face cx={32} cy={29} w={7} mood={mood} mouth={mood === "picked" ? "smile" : "pulse"} />
    </>
  );
}

// tests: a flask with something bubbling in it.
function Flask({ mood }: BodyProps) {
  return (
    <>
      <path d="M26 6 h12 v3 h-3 v14 l15 26 a5 5 0 0 1 -4.3 7.5 h-27.4 a5 5 0 0 1 -4.3 -7.5 l15 -26 v-14 h-3 z" fill="var(--m-light)" stroke="var(--m-deep)" strokeWidth={2.5} strokeLinejoin="round" />
      <path d="M22.5 38 h19 l8.5 14.8 a1.5 1.5 0 0 1 -1.3 2.2 h-33.4 a1.5 1.5 0 0 1 -1.3 -2.2 z" fill="var(--m)" />
      <g fill={PAPER} opacity={0.85}>
        <circle cx={28} cy={46} r={1.6} className="mascot-bubble" />
        <circle cx={36} cy={49} r={1.2} className="mascot-bubble" style={{ animationDelay: "0.6s" }} />
        <circle cx={33} cy={43} r={1} className="mascot-bubble" style={{ animationDelay: "1.1s" }} />
      </g>
      <Face cx={32} cy={30} w={5.5} scale={0.9} mood={mood} />
    </>
  );
}

// deps: three boxes stacked, the top one with the face.
function Boxes({ mood }: BodyProps) {
  return (
    <>
      <rect x={8} y={41} width={48} height={17} rx={3} fill="var(--m-deep)" />
      <rect x={8} y={41} width={48} height={4} fill="var(--m)" opacity={0.5} />
      <rect x={13} y={24} width={38} height={17} rx={3} fill="var(--m)" />
      <rect x={13} y={24} width={38} height={4} fill="var(--m-light)" opacity={0.6} />
      <rect x={19} y={7} width={26} height={17} rx={3} fill="var(--m-light)" stroke="var(--m-deep)" strokeWidth={1.5} />
      <rect x={30} y={7} width={4} height={6} fill="var(--m-deep)" opacity={0.6} />
      <Face cx={32} cy={33} w={5.5} scale={0.85} mood={mood} />
    </>
  );
}

// release: a rocket, the face in the porthole, a flame that flickers.
function Rocket({ mood }: BodyProps) {
  return (
    <>
      <g className="mascot-flame" style={{ transformOrigin: "32px 50px" }}>
        <path d="M26 50 q6 12 12 0 q-3 4 -6 7 q-3 -3 -6 -7 z" fill="oklch(0.8 0.16 70)" />
        <path d="M29 50 q3 7 6 0 q-1.5 2.5 -3 4 q-1.5 -1.5 -3 -4 z" fill="oklch(0.9 0.15 95)" />
      </g>
      <path d="M20 44 l-9 8 v-16 l9 -2 z" fill="var(--m-deep)" />
      <path d="M44 44 l9 8 v-16 l-9 -2 z" fill="var(--m-deep)" />
      <path d="M32 4 C44 12 46 30 44 50 H20 C18 30 20 12 32 4 Z" fill="var(--m)" />
      <path d="M32 4 C44 12 46 30 44 50 H36 C38 30 38 12 32 4 Z" fill="var(--m-deep)" opacity={0.25} />
      <circle cx={32} cy={27} r={10.5} fill="var(--m-deep)" />
      <circle cx={32} cy={27} r={8.5} fill="var(--m-light)" />
      <Face cx={32} cy={27} w={4} scale={0.7} mood={mood} />
    </>
  );
}

// perf: a stopwatch; its hand runs when the card is picked.
function Stopwatch({ mood }: BodyProps) {
  return (
    <>
      <rect x={28} y={4} width={8} height={6} rx={1.5} fill="var(--m-deep)" />
      <rect x={30} y={8} width={4} height={5} fill="var(--m-deep)" />
      <rect x={45} y={10} width={7} height={4} rx={1.5} fill="var(--m-deep)" transform="rotate(40 48.5 12)" />
      <circle cx={32} cy={35} r={24} fill="var(--m-deep)" />
      <circle cx={32} cy={35} r={20} fill="var(--m)" />
      <g stroke="var(--m-deep)" strokeWidth={2} strokeLinecap="round" opacity={0.7}>
        <path d="M32 17 v3" />
        <path d="M50 35 h-3" />
        <path d="M32 53 v-3" />
        <path d="M14 35 h3" />
      </g>
      <g className={mood === "picked" ? "mascot-hand" : undefined} style={{ transformOrigin: "32px 35px" }}>
        <path d="M32 35 L40 23" stroke={INK} strokeWidth={2.2} strokeLinecap="round" />
      </g>
      <Face cx={32} cy={37} w={5.5} scale={0.85} mood={mood} />
    </>
  );
}

// a11y: a round face in round glasses, saying "Aa".
function Glasses({ mood }: BodyProps) {
  return (
    <>
      <circle cx={30} cy={36} r={23} fill="var(--m)" />
      <circle cx={30} cy={36} r={23} fill="var(--m-light)" opacity={0.3} />
      <Face cx={30} cy={37} w={7.5} mood={mood} />
      <g fill="none" stroke={INK} strokeWidth={2}>
        <circle cx={22.5} cy={35.5} r={6} />
        <circle cx={37.5} cy={35.5} r={6} />
        <path d="M28.5 35.5 h3" />
        <path d="M16.5 34.5 l-4 -1.5" />
        <path d="M43.5 34.5 l4 -1.5" />
      </g>
      <g className="mascot-bubble-pop">
        <path d="M43 6 h17 a3 3 0 0 1 3 3 v10 a3 3 0 0 1 -3 3 h-10 l-5 4 v-4 h-2 a3 3 0 0 1 -3 -3 v-10 a3 3 0 0 1 3 -3 z" fill={PAPER} stroke={INK} strokeWidth={1.4} />
        <text x={46} y={18.5} fontSize={10} fontWeight={700} fill={INK} fontFamily="ui-sans-serif, system-ui">
          Aa
        </text>
      </g>
    </>
  );
}

// triage: an inbox tray with letters in it, the face on the front.
function Inbox({ mood }: BodyProps) {
  return (
    <>
      <g className="mascot-letters">
        <rect x={16} y={14} width={22} height={16} rx={2} fill={PAPER} stroke="var(--m-deep)" strokeWidth={1.5} transform="rotate(-8 27 22)" />
        <path d="M16 14 l11 9 l11 -9" fill="none" stroke="var(--m-deep)" strokeWidth={1.5} transform="rotate(-8 27 22)" />
        <rect x={28} y={10} width={22} height={16} rx={2} fill={PAPER} stroke="var(--m-deep)" strokeWidth={1.5} transform="rotate(7 39 18)" />
        <path d="M28 10 l11 9 l11 -9" fill="none" stroke="var(--m-deep)" strokeWidth={1.5} transform="rotate(7 39 18)" />
      </g>
      <path d="M8 30 h48 l-4 24 a3 3 0 0 1 -3 2.5 h-34 a3 3 0 0 1 -3 -2.5 z" fill="var(--m)" />
      <path d="M8 30 h48 l-1.5 9 h-45 z" fill="var(--m-deep)" opacity={0.3} />
      <Face cx={32} cy={46} w={6} scale={0.85} mood={mood} />
    </>
  );
}

// tidy: a broom, the face on the bristles.
function Broom({ mood }: BodyProps) {
  return (
    <>
      <path d="M46 4 l5 3 l-17 26 l-5 -3 z" fill="var(--m-deep)" />
      <path d="M22 30 h18 l5 7 h-28 z" fill="var(--m-deep)" opacity={0.6} />
      <path d="M15 36 h32 l5 22 h-42 z" fill="var(--m)" />
      <g stroke="var(--m-deep)" strokeWidth={1.4} opacity={0.5} strokeLinecap="round">
        <path d="M18 50 l-1 7" />
        <path d="M46 50 l1 7" />
      </g>
      <g className="mascot-sparkle" fill="var(--m-deep)">
        <path d="M12 16 l1.5 3.5 l3.5 1.5 l-3.5 1.5 l-1.5 3.5 l-1.5 -3.5 l-3.5 -1.5 l3.5 -1.5 z" />
        <path d="M24 10 l1 2.2 l2.2 1 l-2.2 1 l-1 2.2 l-1 -2.2 l-2.2 -1 l2.2 -1 z" opacity={0.7} />
      </g>
      <Face cx={31} cy={47} w={6} scale={0.85} mood={mood} />
    </>
  );
}

// dx: a toolbox with a handle and a latch.
function Toolbox({ mood }: BodyProps) {
  return (
    <>
      <rect x={22} y={6} width={20} height={14} rx={5} fill="none" stroke="var(--m-deep)" strokeWidth={4} />
      <rect x={6} y={17} width={52} height={14} rx={4} fill="var(--m-deep)" />
      <rect x={8} y={28} width={48} height={28} rx={4} fill="var(--m)" />
      <rect x={27} y={26} width={10} height={7} rx={2} fill="var(--m-light)" stroke="var(--m-deep)" strokeWidth={1.5} />
      <Face cx={32} cy={44} w={6} scale={0.9} mood={mood} />
    </>
  );
}

// tool: a plug with a face; its cable sways, and a spark shows when picked.
function Plug({ mood }: BodyProps) {
  return (
    <>
      <path d="M32 50 q0 8 -9 9 q-9 1 -9 -5" fill="none" stroke="var(--m-deep)" strokeWidth={3.5} strokeLinecap="round" className="mascot-cable" style={{ transformOrigin: "32px 50px" }} />
      <rect x={22} y={4} width={5} height={14} rx={2.5} fill="var(--m-deep)" />
      <rect x={37} y={4} width={5} height={14} rx={2.5} fill="var(--m-deep)" />
      <path d="M14 20 h36 a4 4 0 0 1 4 4 v6 a18 18 0 0 1 -18 18 h-8 a18 18 0 0 1 -18 -18 v-6 a4 4 0 0 1 4 -4 z" fill="var(--m)" />
      <path d="M14 20 h36 a4 4 0 0 1 4 4 v3 h-44 v-3 a4 4 0 0 1 4 -4 z" fill="var(--m-light)" opacity={0.55} />
      {mood === "picked" ? (
        <g className="mascot-sparkle" fill="oklch(0.85 0.17 90)">
          <path d="M50 8 l1.5 3.5 l3.5 1.5 l-3.5 1.5 l-1.5 3.5 l-1.5 -3.5 l-3.5 -1.5 l3.5 -1.5 z" />
          <path d="M12 10 l1 2.2 l2.2 1 l-2.2 1 l-1 2.2 l-1 -2.2 l-2.2 -1 l2.2 -1 z" opacity={0.8} />
        </g>
      ) : null}
      <Face cx={32} cy={34} w={6.5} mood={mood} />
    </>
  );
}

// skill: a recipe card - a curled page with lines on it and the face below.
function Recipe({ mood }: BodyProps) {
  return (
    <>
      <path d="M14 10 h32 l8 8 v38 a3 3 0 0 1 -3 3 h-37 a3 3 0 0 1 -3 -3 v-43 a3 3 0 0 1 3 -3 z" fill="var(--m-deep)" />
      <path d="M12 7 h32 l8 8 v38 a3 3 0 0 1 -3 3 h-37 a3 3 0 0 1 -3 -3 v-43 a3 3 0 0 1 3 -3 z" fill="var(--m)" />
      <path d="M44 7 v8 h8 z" fill="var(--m-light)" />
      <g stroke={PAPER} strokeWidth={2.4} strokeLinecap="round" opacity={0.9}>
        <path d="M17 17 h16" />
        <path d="M17 23 h22" />
        <path d="M17 29 h12" />
      </g>
      <circle cx={46} cy={27} r={4} fill="var(--m-light)" />
      <path d="M43.5 27 l2 2 l3.5 -4" fill="none" stroke="var(--m-deep)" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" />
      <Face cx={32} cy={43} w={6} scale={0.9} mood={mood} />
    </>
  );
}

// A word of the owner's: a sticker with a star, so a custom role has a face too.
function Sticker({ mood }: BodyProps) {
  return (
    <>
      <path d="M32 4 l6.5 8.5 l10.5 -2 l1 10.7 l9.7 4.6 l-6.4 8.6 l4.2 9.9 l-10.5 2.4 l-2.8 10.4 l-9.9 -4.2 l-8.8 6.1 l-4.6 -9.7 l-10.7 -0.9 l2.1 -10.5 l-8.1 -7.1 l9.4 -5.2 l-0.5 -10.7 l10.7 1.2 z" fill="var(--m)" />
      <path d="M32 10 l5 6.5 l8 -1.5 l0.8 8.2 l7.4 3.5 l-4.9 6.6 l3.2 7.6 l-8 1.8 l-2.2 8 l-7.6 -3.2 l-6.7 4.7 l-3.5 -7.4 l-8.2 -0.7 l1.6 -8.1 l-6.2 -5.4 l7.2 -4 l-0.4 -8.2 l8.2 0.9 z" fill="var(--m-light)" opacity={0.5} />
      <Face cx={32} cy={33} w={6.5} mood={mood} />
    </>
  );
}

const BODIES: Record<string, (p: BodyProps) => React.JSX.Element> = {
  rsi: Loop,
  docs: Book,
  security: Shield,
  monitor: Screen,
  tests: Flask,
  deps: Boxes,
  release: Rocket,
  perf: Stopwatch,
  a11y: Glasses,
  triage: Inbox,
  tidy: Broom,
  dx: Toolbox,
  tool: Plug,
  skill: Recipe,
};
