// The Go protocol deliberately differs from the historical scripts:
//
//   - Configuration is in evolution records, settings and public fixture rows,
//     not Markdown frontmatter. Product-first ordering requires exactly one
//     case-insensitive "Product scope: allowed" line in the project's evolution
//     record with slug "evaluators". Equivalent affirmative values are listed
//     beside parseProductScope. Missing/ambiguous scope fails closed. The
//     constitution's Editable scope table still bounds the single target file;
//     product paths require Stage 4, operating-layer paths Stage 2 or higher.
//   - Each enabled public/signal fixture plus every private fixture gets one
//     detached --no-hardlinks clone of a committed ref. There is no transcript
//     grader, fixture_runs ledger, median-of-three or clone-retention option.
//     The caller supplies an owner-owned scorer printing a finite [0,1] score,
//     optionally on a "score: X" line. Total is the weighted mean.
//   - Public --answer-key paths, the legacy evaluator/archive/board answer keys,
//     and .pulse are removed from every clone. The source Git object history is
//     retained so HEAD remains the requested ref; these are throwaway checkouts,
//     not information-flow or OS sandboxes. Do not put credentials in Git.
//   - Worker PULSE_HOME points to a new empty directory per fixture, PULSE_JOB
//     prevents store creation/migration, and inherited PULSE/GIT variables are
//     stripped. HOME/authentication remain the harness's own: this prevents
//     ordinary pulse commands from reading the real board, not hostile same-user
//     filesystem access. Use an external sandbox for untrusted harnesses.
//   - Scorers run after all harness descendants are terminated. Clone write
//     permissions are removed and content/mode fingerprints reject mutation.
//     This is an audited read-only contract, not a replacement for an OS sandbox.
//     Harness/scorer output is never forwarded; only parsed numeric scores are.
//   - Private request/scorer files must be regular, owner-only, outside every
//     registered checkout and outside the managed fleet area. Their filenames
//     never reach a score or diagnostic: H1, H2, ... are new opaque indexes.
//     Setup creates pending templates without overwriting or enabling the loop.
//   - Archive is called only by runner experiment landing. CLI measurement,
//     shadow findings, reflection and fleet verification cannot write it.
//     There is no CLI baseline writer: the owner authorizes the initial
//     experiment job; the first kept runner archive opens the baseline gate.
//   - Cycles take eligible voted suggestions, not invented signal hypotheses.
//     A suggestion needs File:, Variable:, Hypothesis: (summary is the default),
//     Class: and optionally Tier:/Target: lines. Missing scope is a refusal.
//     Public names and the private count are the card's complete fixture list.
//     A voted pick without those lines is agents' work, not the owner's
//     (ADR-044): the planner queues one reflect job - source suggestion:<id>,
//     one open per project - whose agent posts the card as a new suggestion on
//     the same thread; tally shows the pick as carded when it ends, and the
//     card earns its own votes. LoopClaims names the in-flight experiment's
//     suggestion and every candidate with a valid card; the improve planner
//     leaves all of them to the loop and builds the one-liners.
//     InstructionShaped is an explicit conservative tripwire on open threads,
//     not a semantic authority detector: a command names its object right after
//     the verb (ADR-043). The owner resolves a stop by deciding yes on the stop
//     question in evolution/loop-gate (the post is not a command; the loop
//     plans past it) or by closing the unsafe thread.
//     Cycles reconcile job events through the process bus and cross-process
//     Bridge, retain only a renewable SQL lease, and never touch source branches.
//     A stopped observer cancels only a queued job it created. Leased jobs stay
//     with the runner, which owns cancellation, STOP checks and landing guards.
//   - Limits are "none" or "cycles=N,hours=H", measured from the latest off-to-on
//     setting event. Repeating "on" cannot reset a cap. Archive event ordering
//     distinguishes rows written in the same second as the switch. An in-flight
//     job may finalize past a cap only after its runner archive exists; no other
//     closed gate is bypassed. See the cycle budget regression tests.
//   - Fleet manifests and results are store rows, never worker result files.
//     Clean is scoped to one recorded fleet id, refuses pending workers and
//     preserves history. A wave is limited to 256 workers; further waves are
//     explicit rather than an accidental unbounded clone allocation.
//   - Usage is best-effort, read-only local telemetry: the legacy JSONL/message
//     fields, including Copilot final-vs-live and Codex cumulative semantics.
//     --days selects files updated in the window, not token usage inside that
//     period. OpenCode also recognizes ~/.config/opencode/storage/message.
//
// Hook integration: hooks.StopJSON should call StopReminder with its read-only
// store (nil is supported), retaining the existing once-per-session guard.
package evolve
