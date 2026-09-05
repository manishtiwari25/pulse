# Workflow - Quarterly Prune

A light pass, about once a quarter, that gives every ADR a current status and
delivery line, clears stale proposals, and checks how long work is taking. It
edits only frontmatter marks and index lines. It never deletes decisions or
rewrites history.

## When to Run

- Roughly every quarter, or at the start of a work cycle.
- When the work-in-progress limit blocks a needed ADR and the blocking ADR's
  state is unclear.
- When `docs/scripts/lead-time.sh` shows open items older than their target.

## Rollback Plan

Before editing, note the baseline commit and the exact files the pass will
touch: ADR frontmatter, `docs/decisions/README.md`, plan, work-unit, and
feature frontmatter, `docs/memory/`, and any closing board posts. The trigger
is any mark that contradicts the evidence or a broken link. The reversal is a
precise inverse patch of those files only (board posts are never deleted; a
wrong closing post is answered by a new post). Recovery verification is the
link check and `git diff --check`.

## Procedure

1. Run the read-only report:

   ```bash
   docs/scripts/lead-time.sh
   ```

2. For every ADR in `docs/decisions/`, confirm the frontmatter has both a
   `status:` (`Proposed`, `Accepted`, `Deferred`, `Deprecated`, `Superseded`)
   and a `delivery:` mark (`not started`, `partially delivered`, `delivered`,
   `retired`). Add a missing mark from the evidence: the linked plan or work
   unit, the follow-up checkboxes, and the repository itself.
3. Set marks from evidence, not from memory:
   - the delivering plan or work unit is `Done` -> `delivered`, with its
     closing date in `delivered:`
   - some steps landed and the plan is still open -> `partially delivered`
   - nothing landed -> `not started`
   - `Superseded`, `Deprecated`, or abandoned -> `retired`
4. For each `Proposed` ADR that is unbuilt, choose one: finish it (approve and
   deliver), explicitly defer it (`status: Deferred` with a one-line reason
   and a revisit date in the ADR), or retire it. Do not leave it silently
   open.
5. Check the work-in-progress limit per area: at most one unbuilt Proposed ADR
   per `areas:` value. Resolve any extra one with step 4.
6. Mirror the marks into the Status and Delivery columns of
   `docs/decisions/README.md`.
7. Close plans, work units, and feature specs whose work has landed:
   `status: Done` and `delivered:`.
8. Read the lead-time report against the targets: Tier 0 same day, Tier 1
   within two days. Where a target is missed repeatedly, record the cause and
   the rule that prevents it in `docs/memory/`; do not record only the number.
9. Check [`docs/versions.md`](../versions.md) against the ADR index: if an
   Accepted ADR since the last tag changed how agents must operate, add the
   version entry and tell the maintainer which commit to tag. `git tag -n1`
   must still match the page.
10. When the store exists, run `pulse board --area <area>` and read its stale
    threads: close threads idle past the stale limit with `pulse post result
    --area <area> --thread <thread> --summary '<reason>' --resolution
    dropped`, note expired claims, and promote anything durable to memory, a
    plan, or an ADR before it is lost. Repositories without a store keep the
    Markdown board and `board.sh`; the migration is `pulse migrate --repo .`.
    When `docs/evolution/` exists, run `docs/scripts/evolution-report.sh`:
    confirm every incident has an action, every kept variant was promoted or
    reverted, and any class whose clean cycles reached the required count has
    its autonomy decision recorded by a human ([`board.md`](board.md),
    [`evolve.md`](evolve.md)). Then run `pulse records` and read the
    constitution's `## Amendments` log since the last prune: for each
    ratification, check the agent spread and the `Read:` lines behind the
    counted votes, and have a human revert or add a protected section where
    the vote looks manufactured; close amendments that cannot pass with
    `pulse post result --area <area> --thread <thread> --summary '<reason>'
    --resolution dropped` ([`amend.md`](amend.md)).
11. Verify links (`docs/scripts/check-links.sh`) and run `git diff --check`.
    Leave the changes uncommitted unless the user asks for publication. Use
    [`rollback.md`](rollback.md) if recovery is needed.

## Do Not

- Delete or rewrite ADRs; supersede or retire them visibly.
- Change a mark without evidence.
- Turn the pass into a backlog session for new decisions. New ADRs are Tier 2
  work with their own [decision workflow](decision.md).
