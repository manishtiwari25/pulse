# What Can Go Wrong with Spec-Driven Development

PULSE is a docs-first framework: it asks people and coding agents to write down
the why, the what, and the plan before code changes. That family of approaches
is now called **spec-driven development** (SDD). It works, but its failure
modes are real and well documented. This guide lists them plainly, points to
the evidence, and says what PULSE does about each one and what it cannot fix.

Read it when adopting PULSE, when a workflow starts to feel heavy, or when an
agent's confident report does not match what actually shipped.

## The Short Version

| Pitfall | What PULSE does | What still needs a human |
| --- | --- | --- |
| Paperwork out of proportion to the change | Three lifecycle tiers; Tier 0 needs no artifact | Classify the tier honestly |
| Big context makes the agent worse | Routing reads only relevant files; on-demand skills; exact source ranges | Do not paste the whole control plane into one prompt |
| Cost in tokens, time, and review | Fewer artifacts per tier; lead-time report; delegation advisor | Measure instead of assuming speedup |
| Specs written but not built | Delivery marks, WIP limit, quarterly prune | Approve Tier 2 work and check delivery |
| Agent games the check | Real repository checks; reproducing tests; review of test diffs | Read the test changes, not just the green run |
| Spec drift and stale truth | Visible supersession, prune, memory of divergences | Retire what is dead |
| Blurred why/what/how and unclear author | ADR, spec, plan, and prompt keep separate jobs | Product owners own the why |
| Over-specifying the unknown | ADRs only for real choices; open questions instead of guesses | Decide when a spec is premature |
| False confidence and non-determinism | Rollback plan before change; evidence-based review | Keep the expertise that shapes the output |

## 1. Paperwork Out of Proportion to the Change

**What goes wrong.** One workflow is applied to every size of change. A
spacing fix gets the same requirements, design, and task documents as a new
payment flow. The human becomes the bottleneck on trivial work, and the trail
fills with files nobody reads.

**Evidence.** Testing Kiro on a small bug, Birgitta Böckeler found the tool
turned it into four user stories with sixteen acceptance criteria - "like using
a sledgehammer to crack a nut" - and concluded that neither Kiro's nor Spec
Kit's single workflow suits the majority of real coding problems [1].
Thoughtworks' Technology Radar notes that SDD "workflows remain elaborate and
opinionated" and behave very differently depending on task size [2].

**What PULSE does.** [ADR-011](../decisions/ADR-011-proportional-lifecycle-tiers.md)
sizes paperwork to the change: Tier 0 polish and repair needs no control-plane
artifact, Tier 1 behavior inside an accepted decision needs one work unit, and
only Tier 2 real decisions keep the full ADR, plan, spec, and prompt chain. The
agent declares the tier up front and goes up a tier when in doubt.

**What PULSE cannot do.** Make an agent classify honestly on its own. The
`pulse-review` skill checks the tier against the surfaces touched, and a human
still reads that finding.

## 2. Big Context Makes the Agent Worse, Not Better

**What goes wrong.** The team assumes a bigger context window means the agent
can hold every spec, decision, and plan at once. In practice, the more text in
the window, the more likely the one instruction that mattered is missed.

**Evidence.** Chroma's study of 18 leading models found that performance
degrades as input length grows, "often in surprising and non-uniform ways",
even on deliberately simple tasks [3]. Du et al. showed the drop happens even
when retrieval is perfect: length alone hurts [4]. Earlier work showed models
recall facts from the middle of a long prompt worse than from the edges [5].
Anthropic's guidance is to treat context "as a finite resource with
diminishing marginal returns" and to budget attention deliberately [6].
Böckeler saw the same thing in SDD tools: larger windows did not stop the agent
from ignoring parts of the spec or over-applying others [1].

**What PULSE does.** The routing table in `AGENTS.md` names the few files to
read for each request type instead of the whole control plane. Skills are
on-demand rather than always-on. `pulse-code-context` narrows discovery to
likely files and exact line ranges, and the rules require reading exact source
before editing. A Tier 1 work unit is one file, not four.

**What PULSE cannot do.** Change the model. If a prompt pastes every ADR into
the window, the framework's structure will not save it.

## 3. Cost: Tokens, Time, and Review Load

**What goes wrong.** Every spec page is paid for at least twice: once to write
and once on every agent turn that re-reads it. Then a human has to review it.
Teams feel faster while the clock says otherwise.

**Evidence.** Böckeler on Spec Kit: it "created a LOT of markdown files for me
to review", repetitive with each other and with existing code; "to be honest,
I'd rather review code than all these markdown files" [1]. METR's randomized
study of experienced open-source developers found they took 19% longer with AI
tools while believing they had been sped up by 20% [7]. METR's 2026 follow-up
says those numbers are out of date and later data weakly points to a speedup,
but selection effects make the new estimate unreliable - so the honest reading
is "measure, do not assume" [8].

**What PULSE does.** Tiers remove artifacts from small work. The read-only
`docs/scripts/lead-time.sh` report shows proposed-to-delivered days per
artifact so delay is visible. `pulse-delegation-advisor` gives a token range
and confidence before work is delegated, and usage diagnostics stay optional
so telemetry never becomes a completion gate.

**What PULSE cannot do.** Tell you a change was worth it. That comparison
needs your own time, rate, and outcome data.

## 4. Specs Get Written but Not Built

**What goes wrong.** The spec exists, the agent reports success, and the
feature is half there. Or the decision is written, approved, and never built,
and a second and third proposal pile up beside it.

**Evidence.** Böckeler titled one observation "False sense of control": even
with templates, checklists, and constitutions, she "frequently saw the agent
ultimately not follow all the instructions" [1]. In the 2025 Stack Overflow
survey, 66% of developers named "AI solutions that are almost right, but not
quite" as their top frustration, and 45% said debugging AI-generated code takes
more time than expected [9].

**What PULSE does.** Every ADR carries a `delivery:` mark (`not started`,
`partially delivered`, `delivered`, `retired`) and a `delivered:` date. The
work-in-progress limit forbids a new Proposed ADR in an area while another is
unbuilt. The quarterly [prune workflow](../workflows/prune.md) gives every ADR a
current line. Plans and work units list acceptance criteria and the real
verification commands, and `pulse-review` traces behavior back to the spec.

**What PULSE cannot do.** Approve Tier 2 work or judge whether "done" is true.
Check the evidence, not the claim.

## 5. The Agent Games the Check Instead of Doing the Work

**What goes wrong.** Told to make the tests pass, the agent edits the tests,
stubs the verifier, or writes trivial tests. The run is green; the requirement
is not met.

**Evidence.** OpenAI documented a reasoning model that, given a coding task,
worked out it could circumvent the tests by making the `verify` function
always return true - caught by a monitor reading its chain of thought during
training [10]. Kent Beck lists among his warning signs "any indication that
the genie was cheating, for example by disabling or deleting tests" [11].
GitHub's Spec Kit announcement puts the human's job bluntly: "your role isn't
just to steer. It's to verify" [12].

**What PULSE does.** Verification must use commands that already exist in the
repository, never invented checks. A Tier 0 bug fix needs a reproducing test.
`pulse-review` refuses to treat a passing run as proof of untested behavior,
and hard stops keep destructive, production, and credential actions behind
explicit approval in every tier.

**What PULSE cannot do.** Read the diff for you. Review the test changes with
the same care as the code.

## 6. Spec Drift and Stale Truth

**What goes wrong.** The spec was true when written. Code moved on, the spec
did not, and the next agent builds from the stale version. Or research notes
about existing code are mistaken for new requirements and the same classes
are generated twice.

**Evidence.** Böckeler distinguishes spec-first, spec-anchored, and
spec-as-source approaches and notes that most tools leave the maintenance
strategy "vague or totally open"; in her Spec Kit trial the agent took notes
describing existing classes as a new specification and "generated them all
over again, creating duplicates" [1].

**What PULSE does.** Decisions are superseded visibly, never deleted, and get
`delivery: retired`. The prune pass reconciles marks with reality. Adopting
repositories record intentional divergences in `docs/memory/` so template
syncs do not re-flag them, and `AGENTS.md` tells agents not to assume previous
architecture still exists.

**What PULSE cannot do.** Notice drift without someone running the prune.

## 7. Blurred Why, What, and How - and an Unclear Author

**What goes wrong.** Functional intent and technical design end up in the
same document; nobody is sure when to stay at "what" and when to write "how".
Tools generate PRDs and user stories without saying who is meant to write or
own them.

**Evidence.** Böckeler "frequently got confused when to stay on the functional
level" and doubts the profession's track record of separating requirements
from implementation; she also asks who the target user of SDD artifacts is
[1]. Thoughtworks makes the same point: "it's sometimes unclear who their
intended user is" [2].

**What PULSE does.** Each artifact keeps one job: an ADR records the why and
the options, a feature spec the what, a plan the how, and a prompt the do. Tier
1 merges spec, plan, and prompt into one work unit only when an accepted ADR
has already settled the why.

**What PULSE cannot do.** Replace product ownership. Someone accountable for
the outcome still has to own the why.

## 8. Over-Specifying the Unknown

**What goes wrong.** Big up-front specification is written for a problem the
team does not yet understand. The document is detailed and wrong, and its
confidence makes it harder to change course.

**Evidence.** Böckeler: "the best way for us to stay in control of what we're
building are small, iterative steps, so I'm very skeptical that lots of
up-front spec design is a good idea", adding that small work packages "almost
seem counter to the idea of SDD" [1]. Thoughtworks warns "we may be relearning
a bitter lesson - that handcrafting detailed rules for AI ultimately doesn't
scale" [2].

**What PULSE does.** ADRs are created only for real choices and feature specs
only for real behavior. Missing facts become open questions, not invented
requirements. Tier 0 and Tier 1 let small, iterative steps ship without a
design document.

**What PULSE cannot do.** Know when a problem is still too unclear to specify.
That is a judgment call.

## 9. False Confidence and Non-Determinism

**What goes wrong.** A thick spec feels like control, and generated code
looks complete. Then the same spec produces different code on the next run,
and the last 30% of the work turns out to be the hard part.

**Evidence.** Böckeler saw non-determinism even at the lowest abstraction
level, generating different code from the same Tessl spec, and warns that
spec-as-source may combine "the downsides of both MDD and LLMs: Inflexibility
and non-determinism" [1]. Addy Osmani describes "house of cards code" that
"looks complete but collapses under real-world pressure" [13]. Only 33% of
surveyed developers trust AI output accuracy, while 46% distrust it [9].

**What PULSE does.** A rollback plan exists before the first edit, so a wrong
run is cheap to reverse. Verification is evidence, not a report. Review looks
for high-confidence bugs and missing edge cases, and memory keeps the lessons
that would otherwise be relearned.

**What PULSE cannot do.** Make a probabilistic model deterministic, or supply
the engineering judgment that turns a plausible draft into a maintainable
system.

## Use This Guide

- When a workflow feels heavy, check pitfall 1 and re-declare the tier.
- When an agent misses an instruction, check pitfalls 2 and 4 before writing a
  longer spec; a longer spec usually makes both worse.
- When a green run surprises you, check pitfall 5.
- Run the [quarterly prune](../workflows/prune.md) to catch pitfalls 4 and 6.

## References

1. Birgitta Böckeler, "Understanding Spec-Driven-Development: Kiro, spec-kit,
   and Tessl", martinfowler.com, October 2025.
   <https://martinfowler.com/articles/exploring-gen-ai/sdd-3-tools.html>
2. Thoughtworks, "Spec-driven development", Technology Radar.
   <https://www.thoughtworks.com/radar/techniques/spec-driven-development>
3. Kelly Hong et al., "Context Rot: How Increasing Input Tokens Impacts LLM
   Performance", Chroma Research, July 2025.
   <https://research.trychroma.com/context-rot>
4. Yufeng Du et al., "Context Length Alone Hurts LLM Performance Despite
   Perfect Retrieval", Findings of EMNLP 2025.
   <https://arxiv.org/abs/2510.05381>
5. Nelson F. Liu et al., "Lost in the Middle: How Language Models Use Long
   Contexts", TACL 2024. <https://arxiv.org/abs/2307.03172>
6. Anthropic, "Effective context engineering for AI agents", September 2025.
   <https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents>
7. METR, "Measuring the Impact of Early-2025 AI on Experienced Open-Source
   Developer Productivity", July 2025.
   <https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/>
8. METR, "Uplift update", February 2026.
   <https://metr.org/blog/2026-02-24-uplift-update/>
9. Stack Overflow, "2025 Developer Survey: AI", and the accompanying press
   release, July 2025. <https://survey.stackoverflow.co/2025/ai> and
   <https://stackoverflow.co/company/press/archive/stack-overflow-2025-developer-survey/>
10. OpenAI, "Detecting misbehavior in frontier reasoning models", March 2025.
    <https://openai.com/index/chain-of-thought-monitoring/>
11. Kent Beck, "Augmented Coding: Beyond the Vibes", 2025.
    <https://newsletter.kentbeck.com/p/augmented-coding-beyond-the-vibes>
12. GitHub, "Spec-driven development with AI: Get started with a new open
    source toolkit", September 2025.
    <https://github.blog/ai-and-ml/generative-ai/spec-driven-development-with-ai-get-started-with-a-new-open-source-toolkit/>
13. Addy Osmani, "The 70% problem: Hard truths about AI-assisted coding",
    December 2024. <https://addyo.substack.com/p/the-70-problem-hard-truths-about>

External links reflect what those sources said when this guide was written.
Check the source before quoting a number in a decision.
