import { useId, type CSSProperties } from "react";
import { cn } from "@/lib/utils";
import { hashHue } from "@/lib/format";

// The console's cartoon cast, drawn inline so it takes the theme and needs
// no images: Pip (PULSE itself) with a mood for each state the hub can be
// in, a face for every agent made from its name, and a folder with a face
// for every project. Motion lives in globals.css (`pip-*`, `mascot-*`) and
// stops under prefers-reduced-motion.
const INK = "oklch(0.3 0.045 262)";
const PAPER = "oklch(0.985 0.004 95)";
const BLUSH = "oklch(0.72 0.16 15)";
// Limbs and props sit outside the body, on the page itself, so they take a
// mid slate that shows on paper and in the dark alike; words float in the
// theme's muted ink.
const LIMB = "oklch(0.55 0.09 262)";
const WORD = "var(--muted-foreground)";

function hash(seed: string): number {
  let h = 2166136261;
  for (const ch of seed) h = (Math.imul(h ^ ch.charCodeAt(0), 16777619)) >>> 0;
  return h;
}

// ----- Pip ---------------------------------------------------------------

export type PipMood = "happy" | "waiting" | "busy" | "asleep" | "worried" | "lost" | "curious" | "working";

const PIP_LABEL: Record<PipMood, string> = {
  happy: "Pip, PULSE's mascot, waving",
  waiting: "Pip holding a note for you",
  busy: "Pip at work",
  asleep: "Pip asleep",
  worried: "Pip looking worried",
  lost: "Pip looking for the page",
  curious: "Pip looking curious",
  working: "Pip typing on a laptop",
};

function pipLabel(mood: PipMood, coffee: boolean, charging: boolean): string {
  if (mood !== "working") return PIP_LABEL[mood];
  return PIP_LABEL.working + (coffee ? " with a coffee" : "") + (charging ? ", the laptop on its charger" : "");
}

export interface PipSceneProps {
  // `working` only: Pip sips a coffee while it types (the runner holds the
  // machine awake) and the laptop sits on its charger (that hold has AC power).
  coffee?: boolean;
  charging?: boolean;
}

// A rounded tile in the brand's mint-sky-violet with a face whose smile is
// the pulse line. `decorative` hides it from assistive tech when the words
// next to it already say everything.
export function PulseMascot({ mood = "happy", coffee = false, charging = false, size = 72, className, title, decorative, style }: PipSceneProps & { mood?: PipMood; size?: number; className?: string; title?: string; decorative?: boolean; style?: CSSProperties }) {
  const gid = "pip-" + useId().replace(/[^a-zA-Z0-9]/g, "");
  const dim = mood === "asleep" || mood === "worried";
  const desk = mood === "working";
  return (
    <svg viewBox="0 0 80 80" width={size} height={size} role={decorative ? undefined : "img"} aria-hidden={decorative || undefined} aria-label={decorative ? undefined : (title ?? pipLabel(mood, coffee, charging))} className={cn("mascot pip shrink-0 overflow-visible", className)} data-mood={mood} style={style}>
      <defs>
        <linearGradient id={gid} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" style={{ stopColor: "var(--brand-mint)" }} />
          <stop offset="52%" style={{ stopColor: "var(--brand-sky)" }} />
          <stop offset="100%" style={{ stopColor: "var(--brand-violet)" }} />
        </linearGradient>
      </defs>
      <path d="M40 16 V9" stroke={LIMB} strokeWidth={2.5} strokeLinecap="round" />
      <circle cx={40} cy={6.5} r={3.2} fill={dim ? "oklch(0.78 0.02 250)" : "var(--you)"} className={dim ? undefined : "mascot-beat"} />
      <PipArms mood={mood} />
      <rect x={12} y={16} width={56} height={52} rx={17} fill={"url(#" + gid + ")"} />
      <rect x={13} y={17} width={54} height={50} rx={16} fill="none" stroke="oklch(1 0 0 / 0.35)" strokeWidth={1.5} />
      <ellipse cx={30} cy={69} rx={7} ry={3.2} fill={LIMB} />
      <ellipse cx={50} cy={69} rx={7} ry={3.2} fill={LIMB} />
      <PipFace mood={mood} sipping={desk && coffee} />
      <PipProps mood={mood} />
      {desk ? <PipDesk coffee={coffee} charging={charging} /> : null}
    </svg>
  );
}

const ARM = { stroke: LIMB, strokeWidth: 5, strokeLinecap: "round" as const, fill: "none" };

function PipArms({ mood }: { mood: PipMood }) {
  const arm = ARM;
  // At the desk the arms reach over the body to the keyboard (PipDesk).
  if (mood === "working") return null;
  if (mood === "happy") {
    return (
      <>
        <path d="M13 46 q-6 6 -4 14" {...arm} />
        {/* The whole arm - upper arm, forearm and open hand - is one group that
            pivots at the shoulder (67,42), the point the body hides; the
            keyframes live in globals.css (wave-hand). */}
        <g className="pip-wave">
          <path d="M67 42 q9 -2 8 -14" {...arm} />
          <circle cx={75} cy={26} r={4.2} fill={LIMB} />
          <path d="M72 23.5 l-1.5 -3.2 M75 22 l0 -3.4 M78 23.5 l1.5 -3.2" stroke={LIMB} strokeWidth={2.2} strokeLinecap="round" fill="none" />
        </g>
      </>
    );
  }
  if (mood === "waiting") {
    return (
      <>
        <path d="M13 46 q-6 6 -4 14" {...arm} />
        <path d="M67 44 q9 -2 10 -10" {...arm} />
      </>
    );
  }
  if (mood === "lost") {
    return (
      <>
        <path d="M13 46 q-8 2 -9 10" {...arm} />
        <path d="M67 40 q6 -8 4 -14" {...arm} />
      </>
    );
  }
  return (
    <>
      <path d="M13 46 q-6 6 -4 14" {...arm} />
      <path d="M67 46 q6 6 4 14" {...arm} />
    </>
  );
}

function PipFace({ mood, sipping }: { mood: PipMood; sipping?: boolean }) {
  const lx = 31;
  const rx = 49;
  const ey = 41;
  const my = 55;
  // At the desk the eyes rest a little lower, on the screen.
  const py = ey + (mood === "lost" ? -0.6 : mood === "working" ? 1.4 : 0.3);
  const openEyes = (children?: React.ReactNode) => (
    <g className="mascot-eyes">
      <circle cx={lx} cy={ey} r={5} fill={PAPER} />
      <circle cx={rx} cy={ey} r={5} fill={PAPER} />
      <g className={mood === "lost" ? "pip-look" : undefined}>
        <circle cx={lx + (mood === "lost" ? -1.6 : mood === "waiting" ? 1.2 : 0)} cy={py} r={mood === "worried" ? 2 : 2.6} fill={INK} />
        <circle cx={rx + (mood === "lost" ? 1.6 : mood === "waiting" ? 1.2 : 0)} cy={py} r={mood === "worried" ? 2 : 2.6} fill={INK} />
        <circle cx={lx + 1} cy={ey - 0.8} r={0.9} fill={PAPER} />
        <circle cx={rx + 1} cy={ey - 0.8} r={0.9} fill={PAPER} />
      </g>
      {children}
    </g>
  );
  const eyes =
    mood === "asleep" ? (
      <g stroke={INK} strokeWidth={2.4} strokeLinecap="round" fill="none">
        <path d={`M${lx - 4.5} ${ey} q4.5 3.5 9 0`} />
        <path d={`M${rx - 4.5} ${ey} q4.5 3.5 9 0`} />
      </g>
    ) : sipping ? (
      // The eyes close, content, while the coffee is at the mouth; the sip
      // itself is drawn in PipDesk on the same clock. Without motion the
      // plain eyes stand in (pip-still / pip-anim, globals.css).
      <>
        <g className="pip-still">{openEyes()}</g>
        <g className="pip-anim">
          {openEyes(<animate attributeName="opacity" values="1;1;0;0;1;1" keyTimes={SIP_EYES} dur={SIP} repeatCount="indefinite" />)}
          <g stroke={INK} strokeWidth={2.4} strokeLinecap="round" fill="none" opacity={0}>
            <path d={`M${lx - 4.5} ${ey + 1} q4.5 -3.5 9 0`} />
            <path d={`M${rx - 4.5} ${ey + 1} q4.5 -3.5 9 0`} />
            <animate attributeName="opacity" values="0;0;1;1;0;0" keyTimes={SIP_EYES} dur={SIP} repeatCount="indefinite" />
          </g>
        </g>
      </>
    ) : (
      openEyes()
    );
  const brows =
    mood === "busy" ? (
      <g stroke={INK} strokeWidth={2.2} strokeLinecap="round" fill="none">
        <path d={`M${lx - 5} ${ey - 9} l9 2.5`} />
        <path d={`M${rx + 5} ${ey - 9} l-9 2.5`} />
      </g>
    ) : mood === "worried" ? (
      <g stroke={INK} strokeWidth={2.2} strokeLinecap="round" fill="none">
        <path d={`M${lx - 5} ${ey - 7} l9 -2.5`} />
        <path d={`M${rx + 5} ${ey - 7} l-9 -2.5`} />
      </g>
    ) : mood === "curious" ? (
      <path d={`M${rx - 5} ${ey - 9.5} q5 -2.5 10 0`} stroke={INK} strokeWidth={2.2} strokeLinecap="round" fill="none" />
    ) : null;
  const mouth = (() => {
    if (mood === "happy" || mood === "curious" || mood === "working") return <path d={`M${lx - 1} ${my} h5 l3 -5 l4 9 l4 -7 l2 3 h6`} fill="none" stroke={PAPER} strokeWidth={2.6} strokeLinecap="round" strokeLinejoin="round" />;
    if (mood === "waiting" || mood === "lost") return <ellipse cx={40} cy={my} rx={3} ry={3.6} fill={INK} />;
    if (mood === "busy") return <path d={`M34 ${my} h12`} fill="none" stroke={INK} strokeWidth={2.4} strokeLinecap="round" />;
    if (mood === "asleep") return <path d={`M37 ${my} q3 2 6 0`} fill="none" stroke={INK} strokeWidth={2.2} strokeLinecap="round" />;
    return <path d={`M32 ${my + 1} q4 -3 8 0 q4 3 8 0`} fill="none" stroke={INK} strokeWidth={2.4} strokeLinecap="round" />;
  })();
  return (
    <g className="mascot-face">
      <circle cx={lx - 6} cy={ey + 8} r={3.2} fill={BLUSH} opacity={mood === "worried" || mood === "asleep" ? 0.25 : 0.45} />
      <circle cx={rx + 6} cy={ey + 8} r={3.2} fill={BLUSH} opacity={mood === "worried" || mood === "asleep" ? 0.25 : 0.45} />
      {eyes}
      {brows}
      {mouth}
    </g>
  );
}

// What Pip holds or what floats around it: a marigold note, sweat, z's, a
// magnifying glass and a question.
function PipProps({ mood }: { mood: PipMood }) {
  if (mood === "waiting") {
    return (
      <g transform="rotate(10 78 28)">
        <rect x={68} y={20} width={20} height={15} rx={2.5} fill="var(--you)" stroke={LIMB} strokeWidth={1.6} />
        <path d="M68 20 l10 8 l10 -8" fill="none" stroke={LIMB} strokeWidth={1.6} strokeLinejoin="round" />
        <g className="pip-sparkle">
          <text x={58} y={16} fontSize={11} fontWeight={800} fill="var(--you-ink)" fontFamily="ui-sans-serif, system-ui">!</text>
        </g>
      </g>
    );
  }
  if (mood === "busy" || mood === "worried") {
    return (
      <>
        <path d="M63 24 q3 5 0 7 q-3 -2 0 -7 z" fill="var(--brand-sky)" stroke={LIMB} strokeWidth={1.2} className="pip-drop" />
        {mood === "busy" ? (
          <g stroke={LIMB} strokeWidth={2} strokeLinecap="round" opacity={0.7}>
            <path d="M4 36 h6" />
            <path d="M2 44 h6" />
            <path d="M5 52 h5" />
          </g>
        ) : null}
      </>
    );
  }
  if (mood === "asleep") {
    return (
      <g fill={WORD} fontWeight={800} fontFamily="ui-sans-serif, system-ui">
        <text x={60} y={20} fontSize={9} className="pip-zz">z</text>
        <text x={68} y={12} fontSize={12} className="pip-zz" style={{ animationDelay: "0.8s" }}>z</text>
      </g>
    );
  }
  if (mood === "lost") {
    return (
      <>
        <g className="pip-look" style={{ transformOrigin: "70px 24px" }}>
          <circle cx={70} cy={24} r={7.5} fill="oklch(1 0 0 / 0.55)" stroke={LIMB} strokeWidth={2.6} />
          <path d="M75.5 29.5 l6.5 6.5" stroke={LIMB} strokeWidth={3.5} strokeLinecap="round" />
        </g>
        <text x={6} y={18} fontSize={14} fontWeight={800} fill={WORD} fontFamily="ui-sans-serif, system-ui" className="pip-sparkle">?</text>
      </>
    );
  }
  return null;
}

// The desk scene (`working`): a laptop in front of Pip, its lid towards us,
// both hands on the keys. With `coffee` the right hand holds a steaming mug
// and lifts it for a sip every few seconds - the arm, the mug and the eyes
// (PipFace) share one SMIL clock, so they stay together; under
// prefers-reduced-motion the still copy shows instead (globals.css). With
// `charging` a cable runs from the laptop and a small bolt glows on the lid.
const SIP = "7s";
// Rest, lift, drink, put down, rest.
const SIP_TIMES = "0;0.55;0.62;0.8;0.87;1";
const SIP_EYES = "0;0.6;0.63;0.8;0.83;1";
const SIP_EASE = "0.4 0 0.2 1;0.4 0 0.2 1;0.4 0 0.2 1;0.4 0 0.2 1;0.4 0 0.2 1";
const LID = "oklch(0.3 0.035 262)";

function PipDesk({ coffee, charging }: { coffee: boolean; charging: boolean }) {
  return (
    <g className="pip-desk">
      <rect x={21} y={59} width={38} height={8} rx={2} fill="oklch(0.64 0.04 262)" />
      <path d="M12 47 Q7 63 29 62" {...ARM} />
      <g className="pip-type" data-hand="l">
        <circle cx={29} cy={62} r={3.8} fill={LIMB} />
      </g>
      {coffee ? (
        <>
          <g className="pip-still">
            <line x1={68} y1={47} x2={60} y2={62} {...ARM} />
            <PipMug />
          </g>
          <g className="pip-anim">
            <line x1={68} y1={47} x2={60} y2={62} {...ARM}>
              <animate attributeName="x2" values="60;60;44;44;60;60" keyTimes={SIP_TIMES} dur={SIP} repeatCount="indefinite" calcMode="spline" keySplines={SIP_EASE} />
              <animate attributeName="y2" values="62;62;55;55;62;62" keyTimes={SIP_TIMES} dur={SIP} repeatCount="indefinite" calcMode="spline" keySplines={SIP_EASE} />
            </line>
            <g>
              <animateTransform attributeName="transform" type="translate" values="0 0;0 0;-16 -7;-16 -7;0 0;0 0" keyTimes={SIP_TIMES} dur={SIP} repeatCount="indefinite" calcMode="spline" keySplines={SIP_EASE} />
              <g>
                <animateTransform attributeName="transform" type="rotate" values="0 64.5 60.5;0 64.5 60.5;-18 64.5 60.5;-18 64.5 60.5;0 64.5 60.5;0 64.5 60.5" keyTimes="0;0.63;0.67;0.76;0.8;1" dur={SIP} repeatCount="indefinite" calcMode="spline" keySplines={SIP_EASE} />
                <PipMug />
              </g>
            </g>
          </g>
        </>
      ) : (
        <>
          <path d="M68 47 Q73 63 51 62" {...ARM} />
          <g className="pip-type" data-hand="r">
            <circle cx={51} cy={62} r={3.8} fill={LIMB} />
          </g>
        </>
      )}
      <rect x={24} y={64} width={32} height={14} rx={2.5} fill={LID} stroke="oklch(1 0 0 / 0.28)" strokeWidth={1.2} />
      <path d="M29 71 h3 l2 -3 l3 6 l2 -4 l1 2 h4" fill="none" stroke="var(--brand-mint)" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" />
      {charging ? (
        <>
          <path d="M24 74.5 C 17 74.5, 12 76, 8 79.5" fill="none" stroke={LIMB} strokeWidth={2.2} strokeLinecap="round" />
          {/* Drawn from the plug end so the dashes run into the laptop. */}
          <path d="M8 79.5 C 12 76, 17 74.5, 24 74.5" fill="none" stroke="var(--ok)" strokeWidth={1.3} strokeLinecap="round" className="pip-cable-flow" />
          <path d="M51 65.5 l-3 4.3 h2.4 l-1.4 3.7 l3.6 -4.8 h-2.4 z" fill="var(--ok)" className="pip-bolt" />
        </>
      ) : null}
    </g>
  );
}

// A mug in the right hand, steaming.
function PipMug() {
  return (
    <>
      <rect x={60} y={56} width={9} height={9.5} rx={1.6} fill="oklch(0.95 0.01 95)" stroke={LIMB} strokeWidth={1.4} />
      <path d="M69 58.6 q3.8 0 3.8 2.6 q0 2.6 -3.8 2.6" fill="none" stroke={LIMB} strokeWidth={1.6} />
      <circle cx={60} cy={62} r={3.8} fill={LIMB} />
      <g className="pip-steam" stroke="var(--awake)" strokeWidth={1.3} strokeLinecap="round" fill="none">
        <path d="M62.4 54 q-1.2 -2 0 -4" />
        <path d="M64.6 54 q-1.2 -2 0 -4" />
        <path d="M66.8 54 q-1.2 -2 0 -4" />
      </g>
    </>
  );
}

// ----- agents ------------------------------------------------------------

// Every agent gets a face from its name: the body colour, its shape, eyes,
// mouth and hat all come from the same hash, so one agent looks the same on
// every page and no two of a team look quite alike. The owner is always the
// teal face with the marigold star.
export function AgentFace({ seed, size = 24, human, className, style }: { seed: string; size?: number; human?: boolean; className?: string; style?: CSSProperties }) {
  const h = hash(seed || "?");
  const hue = hashHue(seed || "?");
  const shape = human ? 0 : h % 3;
  const eyes = human ? 2 : (h >>> 2) % 3;
  const mouth = human ? 0 : (h >>> 4) % 3;
  const hat = human ? 0 : (h >>> 6) % 4;
  const body = human ? "var(--primary)" : `oklch(0.8 0.11 ${hue})`;
  const deep = human ? "var(--primary-strong)" : `oklch(0.6 0.12 ${hue})`;
  const ink = human ? "var(--primary-foreground)" : INK;
  const delay = ((h >>> 8) % 40) / 10;
  const lx = 11.5;
  const rx = 20.5;
  const ey = 15;
  return (
    <svg viewBox="0 0 32 32" width={size} height={size} aria-hidden className={cn("mascot shrink-0 overflow-visible", className)} style={{ "--mascot-delay": delay + "s", ...style } as CSSProperties}>
      {hat === 3 ? <path d="M5 16 a11 11 0 0 1 22 0" fill="none" stroke={deep} strokeWidth={2.4} /> : null}
      {shape === 0 ? <circle cx={16} cy={17} r={14} fill={body} /> : shape === 1 ? <rect x={3} y={4} width={26} height={26} rx={8} fill={body} /> : <path d="M16 3 C25 3 29 12 29 19 C29 26 23 31 16 31 C9 31 3 26 3 19 C3 12 7 3 16 3 Z" fill={body} />}
      {hat === 1 ? (
        <>
          <path d="M16 4 V0.5" stroke={ink} strokeWidth={1.6} strokeLinecap="round" />
          <circle cx={16} cy={0} r={1.7} fill={deep} />
        </>
      ) : hat === 2 ? (
        <g stroke={deep} strokeWidth={2} strokeLinecap="round" fill="none">
          <path d="M12 4 l-2 -3" />
          <path d="M16 3 v-3" />
          <path d="M20 4 l2 -3" />
        </g>
      ) : hat === 3 ? (
        <g fill={deep}>
          <rect x={2.5} y={13} width={4.5} height={7} rx={1.5} />
          <rect x={25} y={13} width={4.5} height={7} rx={1.5} />
        </g>
      ) : null}
      <circle cx={lx - 3} cy={ey + 5} r={1.8} fill={BLUSH} opacity={0.4} />
      <circle cx={rx + 3} cy={ey + 5} r={1.8} fill={BLUSH} opacity={0.4} />
      {eyes === 2 ? (
        <g stroke={ink} strokeWidth={1.8} strokeLinecap="round" fill="none">
          <path d={`M${lx - 2.4} ${ey + 1} q2.4 -3 4.8 0`} />
          <path d={`M${rx - 2.4} ${ey + 1} q2.4 -3 4.8 0`} />
        </g>
      ) : (
        <g className="mascot-eyes">
          {eyes === 1 ? (
            <>
              <circle cx={lx} cy={ey} r={3} fill={PAPER} />
              <circle cx={rx} cy={ey} r={3} fill={PAPER} />
            </>
          ) : null}
          <circle cx={lx} cy={ey} r={eyes === 1 ? 1.6 : 1.9} fill={INK} />
          <circle cx={rx} cy={ey} r={eyes === 1 ? 1.6 : 1.9} fill={INK} />
          <circle cx={lx + 0.6} cy={ey - 0.6} r={0.55} fill={PAPER} />
          <circle cx={rx + 0.6} cy={ey - 0.6} r={0.55} fill={PAPER} />
        </g>
      )}
      {mouth === 0 ? <path d="M12 21 q4 4 8 0" fill="none" stroke={ink} strokeWidth={1.7} strokeLinecap="round" /> : mouth === 1 ? <path d="M12 20.5 q4 6.5 8 0 z" fill={ink} /> : <path d="M12.5 21 q1.75 2.6 3.5 0 q1.75 2.6 3.5 0" fill="none" stroke={ink} strokeWidth={1.6} strokeLinecap="round" />}
      {human ? <path d="M25 2 l1.6 3.3 l3.6 0.5 l-2.6 2.5 l0.6 3.6 l-3.2 -1.7 l-3.2 1.7 l0.6 -3.6 l-2.6 -2.5 l3.6 -0.5 z" fill="var(--you)" stroke={INK} strokeWidth={0.8} /> : null}
    </svg>
  );
}

// ----- projects ----------------------------------------------------------

// A folder with a face, in the project's colour (the same hue its initials
// used to carry, so nothing changes meaning).
export function ProjectMascot({ slug, size = 40, className, style }: { slug: string; size?: number; className?: string; style?: CSSProperties }) {
  const hue = hashHue(slug || "?");
  const delay = (hash(slug || "?") % 40) / 10;
  return (
    <svg viewBox="0 0 40 40" width={size} height={size} aria-hidden className={cn("mascot shrink-0 overflow-visible", className)} style={{ "--mascot-delay": delay + "s", ...style } as CSSProperties}>
      <path d="M4 8 h11 l4 4 h15 a3 3 0 0 1 3 3 v18 a3 3 0 0 1 -3 3 h-30 a3 3 0 0 1 -3 -3 v-22 a3 3 0 0 1 3 -3 z" fill={`oklch(0.6 0.12 ${hue})`} />
      <rect x={1} y={15} width={38} height={22} rx={3} fill={`oklch(0.81 0.11 ${hue})`} />
      <circle cx={10} cy={29} r={2} fill={BLUSH} opacity={0.4} />
      <circle cx={30} cy={29} r={2} fill={BLUSH} opacity={0.4} />
      <g className="mascot-eyes">
        <circle cx={14.5} cy={24.5} r={2.3} fill={INK} />
        <circle cx={25.5} cy={24.5} r={2.3} fill={INK} />
        <circle cx={15.3} cy={23.7} r={0.7} fill={PAPER} />
        <circle cx={26.3} cy={23.7} r={0.7} fill={PAPER} />
      </g>
      <path d="M15.5 30 q4.5 4 9 0" fill="none" stroke={INK} strokeWidth={1.9} strokeLinecap="round" />
    </svg>
  );
}
