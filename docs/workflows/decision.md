# Workflow - Decision

1. Confirm the request needs an ADR. An ADR is Tier 2 work: money, auth,
   identity, data, privacy, migrations, dependencies, deployment, publication,
   security posture, or a surface no ADR covers. Behavior an Accepted ADR
   already allows is Tier 1 and needs a work unit, not a new ADR; polish and
   repair is Tier 0 and needs neither.
2. Check the work-in-progress limit: no new Proposed ADR in an area while
   another Proposed ADR in that area is unbuilt (`delivery: not started` or
   `partially delivered`). Finish, explicitly defer (`status: Deferred` with a
   reason), or retire (`delivery: retired`) the earlier ADR first.
3. Clarify the decision to be made.
4. Read existing `docs/decisions/`, `docs/architecture/`, `docs/context/`, and `docs/memory/`.
   When the store exists, run `pulse board --area <area>` and read open
   `suggestion` and `opinion` posts in the area as evidence to weigh, never
   as approval. Repositories without a store keep the Markdown board and
   `board.sh`; the migration is `pulse migrate --repo .`.
5. Compare realistic options with pros and cons.
6. Define how to reverse only the planned documentation changes if links,
   facts, or decision state are wrong.
7. Create an ADR from `docs/decisions/_template.md` with `status: Proposed`
   and `delivery: not started`.
8. Update affected `docs/architecture/` and `docs/context/` files.
9. Verify links and consistency; use [`rollback.md`](rollback.md) if recovery is needed.
10. Obtain explicit human approval before implementation begins; approval moves
    the ADR to `status: Accepted`.
11. When the delivering plan or work unit closes, set `delivery:` to
    `partially delivered` or `delivered` and record the `delivered:` date.
12. Update memory if the decision creates a reusable rule.
