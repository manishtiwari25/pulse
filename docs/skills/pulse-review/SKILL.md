---
name: pulse-review
description: Review a change against requested behavior, PULSE decisions and plans, verification evidence, rollback readiness, and honest lifecycle-tier classification. Use for code, documentation, pull request, or implementation reviews.
---

# PULSE Review

Review for meaningful correctness and lifecycle gaps, not cosmetic preference.

## Procedure

1. Read the request, diff, relevant feature spec, plan or work unit, ADRs,
   architecture, and memory.
2. Check that the tier was classified honestly:
   - a tier and one-line reason were declared before the change
   - the diff touches only surfaces the declared tier allows: Tier 0 never
     changes governed behavior, Tier 1 stays inside its named controlling
     Accepted ADR, and anything touching money, auth, identity, data, privacy,
     migrations, dependencies, deployment, publication, security posture, or
     an uncovered surface is Tier 2
   - no hard stop was crossed without explicit approval, in any tier
   - the artifacts match the tier: none for Tier 0, one work unit for Tier 1,
     the full ADR/plan/spec/prompt chain plus approval for Tier 2
   - a Tier 2 ADR did not break the work-in-progress limit
   Report under-classification as a finding in its own right, even when the
   code is correct.
3. Check whether the change delivers the intended behavior.
4. Trace important implementation choices back to accepted decisions.
5. Look for high-confidence bugs, missing edge cases, unsafe state changes,
   weak error handling, or broken integration boundaries.
6. Check verification evidence and whether the selected checks cover the changed behavior.
7. Check rollback readiness:
   - plan or checklist existed before change
   - trigger is specific
   - reversal is scoped
   - user work, history, deployments, and data are protected
   - recovery verification exists
8. Check closure: a delivered work unit or plan carries `delivered:` and the
   controlling ADR's `delivery:` mark was updated.
9. Report findings in severity order with file/line evidence and impact.
10. If no meaningful finding exists, say so directly and note any real residual risk.

Canonical checklist:
<https://github.com/manishtiwari25/pulse/blob/main/docs/prompts/shared/review.prompt.md>

## Do Not

- Report speculative issues without evidence.
- Rewrite the implementation merely to match personal style.
- Treat a successful test run as proof that untested behavior is correct.
- Accept a low tier because the diff is small; judge the surface touched, not
  the line count.
