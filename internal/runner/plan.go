package runner

import (
	"fmt"
	"strings"

	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

// ballotHours is lead.ballot_hours: a card gets its own session's votes first; idle hands vote on what is
// still short after this long, at most one ballot per project per that many hours (ADR-045).
func ballotHours(s *store.Store) int {
	if h := settingInt(s, "lead.ballot_hours"); h >= 1 {
		return h
	}
	return 1
}

func recordPlanDeferred(s *store.Store, slug, now, actor, suggestionID string) {
	if planDeferredEventExists(s, slug, suggestionID) {
		return
	}
	eventAt(s.DB, now, actor, "plan.deferred", suggestionID, map[string]any{"project": slug, "reason": "left to the evolution loop"})
}

func planDeferredEventExists(s *store.Store, slug, suggestionID string) bool {
	return mustOne(s.DB, `SELECT COUNT(*) AS n FROM events WHERE action = 'plan.deferred' AND subject = ?
		AND json_valid(detail) AND json_extract(detail, '$.project') = ?`, suggestionID, slug).int("n") != 0
}

func lastEventSeq(s *store.Store) int {
	return mustOne(s.DB, "SELECT COALESCE(MAX(seq), 0) AS n FROM events").int("n")
}

func planDeferredLinesSince(s *store.Store, seq int) []string {
	rows := mustRows(s.DB, `SELECT subject, json_extract(detail, '$.project') AS project FROM events
		WHERE seq > ? AND action = 'plan.deferred' AND json_valid(detail) ORDER BY seq`, seq)
	lines := make([]string, 0, len(rows))
	for _, r := range rows {
		lines = append(lines, fmt.Sprintf("plan %s: left suggestion %s to the evolution loop", r.str("project"), r.str("subject")))
	}
	return lines
}

// threadStatuses is Tally.thread_status for every thread of one project: closed, stale, or open.
func threadStatuses(s *store.Store, rows []row, now string) map[string]string {
	staleDays := settingInt(s, "stale_days")
	today := dayNumber(now)
	last := map[string]string{}
	closed := map[string]bool{}
	for _, r := range rows {
		key := r.str("area") + "/" + r.str("thread")
		if _, ok := last[key]; !ok || r.str("date") >= last[key] {
			last[key] = r.str("date")
		}
		if r.str("resolution") != "" {
			closed[key] = true
		}
	}
	out := map[string]string{}
	for key, d := range last {
		switch {
		case closed[key]:
			out[key] = "closed"
		case today-dayNumber(d) > staleDays:
			out[key] = "stale"
		default:
			out[key] = "open"
		}
	}
	return out
}

func projectPosts(q querier, slug string) []row {
	posts := mustRows(q, "SELECT rowid AS post_order, * FROM posts WHERE project = ? ORDER BY date, rowid", slug)
	for _, p := range posts {
		parsePostRow(p)
	}
	return posts
}

// openQuestions is open_questions(): questions outside jobs/ with no answer after them in an open thread.
func openQuestions(s *store.Store, slug, now string) []row {
	rows := projectPosts(s.DB, slug)
	status := threadStatuses(s, rows, now)
	answered := map[string]bool{}
	answeredAt := map[string]string{}
	threadAnswers := map[string]row{}
	for _, r := range rows {
		if r.str("kind") != "answer" {
			continue
		}
		key := r.str("area") + "/" + r.str("thread")
		answeredAt[key] = r.str("date")
		if r.str("on_post") != "" {
			answered[r.str("on_post")] = true
		} else {
			threadAnswers[key] = r
		}
	}
	var out []row
	for _, r := range rows {
		if r.str("kind") != "question" || r.str("area") == "jobs" {
			continue
		}
		key := r.str("area") + "/" + r.str("thread")
		if status[key] == "closed" || answered[r.str("id")] || answeredAt[key] > r.str("date") {
			continue
		}
		// A later dated answer resets the thread's turn. Within one timestamp,
		// an explicit link answers that question; an unlinked reply uses order.
		if a := threadAnswers[key]; a != nil && a.str("date") == r.str("date") && a.int("post_order") > r.int("post_order") {
			continue
		}
		out = append(out, r)
	}
	return out
}

// ballotDue is ballot_due(): below-quorum cards older than lead.ballot_hours, unless a ballot job was planned since.
func ballotDue(s *store.Store, slug, now string, suggestions []tally.Suggestion) []tally.Suggestion {
	since := plusHours(now, -float64(ballotHours(s)))
	prefix := "ballot:" + slug + ":"
	last := mustOne(s.DB, "SELECT MAX(substr(source, ?)) AS m FROM jobs WHERE project = ? AND kind = 'ballot' AND source LIKE ?",
		len(prefix)+1, slug, prefix+"%").str("m")
	if last != "" && last > stampOf(since) {
		return nil
	}
	var out []tally.Suggestion
	for _, sg := range suggestions {
		if sg.Status == "needs votes" && sg.Date <= since {
			out = append(out, sg)
		}
	}
	return out
}

func isOwnerAddressedPost(post row) bool {
	return post["for_owner"] == true
}

// answerable says whether a worker may be sent to answer a question. Questions addressed to the owner, or
// aimed at owner-only paths, wait for the human; questions the planner's own actor or the loop posted (the
// gate stop, an environment hold) are machine-authored asks that a worker cannot settle: a clone run
// always ends "owner disposition required", so planning them only burns worker hours.
func answerable(q row, plannerActor string) bool {
	if isOwnerAddressedPost(q) || ownerTargeted(q) {
		return false
	}
	if agent := q.str("agent"); agent == plannerActor || agent == "daemon" {
		return false
	}
	return q.str("summary") != evolve.StopSummary
}

func tallyPost(s *store.Store, id string) *tally.Post {
	r := postRow(s.DB, id)
	if r == nil {
		return nil
	}
	return &tally.Post{ID: r.str("id"), Thread: r.str("thread"), Agent: r.str("agent"), Summary: r.str("summary"), Body: r.str("body"),
		ForOwner: isOwnerAddressedPost(r)}
}

func ownerTargeted(post row) bool {
	return tally.OwnerTargeted(post.str("summary"), post.str("body"))
}

// planJobs is plan_jobs(): the next job for a project with fewer than parallel_per_project waiting.
func planJobs(s *store.Store, slug, now, createdBy string) ([]row, error) {
	waiting := mustOne(s.DB, "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND kind != 'experiment' AND status IN ('queued', 'leased')", slug).int("n")
	if waiting >= parallelPerProject(s) {
		return nil, nil
	}
	// ADR-031: a project at the baseline gate gets the baseline job before anything else.
	project := projectBySlug(s.DB, slug)
	if job, isNew, err := planBaseline(s, project, slug, now, createdBy); err != nil {
		return nil, err
	} else if isNew {
		return []row{job}, nil
	}
	var created []row
	// lead.stuck_thread_jobs: a thread whose last N worker jobs all committed nothing - read-only triage, claims,
	// questions and handoffs that pass the same open point to the next run - has stopped moving. The planner seeds
	// no more jobs from that thread's handoffs, suggestions or questions and asks the lead once to decide it.
	stuck := stuckThreads(s, slug, now, createdBy)
	// A handoff is work a worker left for the next one; the queue ranks it first, and so does the planner: the
	// oldest open handoff nobody claimed becomes one improve job, tried once (like a question), never when it
	// is addressed to the owner or points at owner-only paths.
	if res, err := tally.Compute(s, slug, "", tally.Thresholds{Now: now}); err == nil && res != nil {
		tried := map[string]bool{}
		for _, r := range mustRows(s.DB, "SELECT source FROM jobs WHERE source LIKE 'handoff:%'") {
			tried[r.str("source")] = true
		}
		for _, p := range tally.OpenHandoffs(res) {
			if tried["handoff:"+p.ID] || stuck[p.Area+"/"+p.Thread] {
				continue
			}
			post := postRow(s.DB, p.ID)
			if post == nil || isOwnerAddressedPost(post) || ownerTargeted(post) {
				continue
			}
			brief := fmt.Sprintf("A worker handed off thread %s/%s and nobody claimed it since (handoff %s by %s, %s).\n\n%s\n\n"+
				"Read the handoff (`pulse show %s`) and the thread it names (`pulse board --project %s`), post a claim on the thread, "+
				"then continue exactly the work the handoff states - one variable at a time, proven by the project's own checks. "+
				"When the handoff says the next step needs the owner, post a question on the thread (a `QUESTION:` line) and stop. "+
				"End with `SUMMARY:` saying what moved.",
				p.Area, p.Thread, p.ID, p.Agent, p.Date, p.Body, p.ID, slug)
			job, isNew, err := addJob(s, slug, "improve", fmt.Sprintf("Continue handoff %s: %s", p.ID, trunc(p.Summary, 120)), brief, 1, 8,
				"handoff:"+p.ID, createdBy, true)
			if err != nil {
				return created, err
			}
			if isNew {
				created = append(created, job)
			}
			break
		}
	}
	suggestions := suggestionRecords(s, slug, now)
	// The loop claims every eligible suggestion whose experiment card is valid, and the in-flight
	// experiment's (ADR-044 D4); the planner leaves those to it and builds the rest.
	claims, _, err := evolve.LoopClaims(s, slug, createdBy)
	if err != nil {
		return created, err
	}
	if len(created) == 0 {
		for _, sg := range suggestions {
			if sg.Status != "eligible" {
				continue
			}
			if claims[sg.ID] {
				recordPlanDeferred(s, slug, now, createdBy, sg.ID)
				continue
			}
			post := tallyPost(s, sg.ID)
			projectPath := ""
			if project != nil {
				projectPath = project.str("path")
			}
			if tally.PlanGate(s, projectPath, sg, post) != "" || stuck[sg.Key] {
				continue
			}
			pRow := postRow(s.DB, sg.ID)
			brief := fmt.Sprintf("The agents voted this the top eligible suggestion (score %+d, %d voters). Thread %s.\n\n%s\n\n"+
				"One variable, measured by the project's own checks. When the change is kept, say what moved in SUMMARY:.",
				sg.Score, sg.Voters, sg.Key, pRow.str("body"))
			job, isNew, err := addJob(s, slug, "improve", fmt.Sprintf("Implement suggestion %s: %s", sg.ID, sg.Summary), brief, 1,
				10+sg.Score, "suggestion:"+sg.ID, createdBy, true)
			if err != nil {
				return created, err
			}
			if isNew {
				created = append(created, job)
				break
			}
		}
	}
	if len(created) == 0 {
		tried := map[string]bool{}
		for _, r := range mustRows(s.DB, "SELECT source FROM jobs WHERE source LIKE 'question:%'") {
			tried[r.str("source")] = true
		}
		// A thread gets one worker attempt per human turn: while an open question in it was already tried (the
		// job runs, or failed), the next question there waits too, until an answer or a closing post resets
		// the thread. Otherwise every failed attempt that ends in a new QUESTION: line seeds the next run.
		open := openQuestions(s, slug, now)
		waiting := map[string]bool{}
		for _, q := range open {
			if tried["question:"+q.str("id")] {
				waiting[q.str("area")+"/"+q.str("thread")] = true
			}
		}
		for _, q := range open {
			if tried["question:"+q.str("id")] || waiting[q.str("area")+"/"+q.str("thread")] || !answerable(q, createdBy) || mentionsProjectLead(q) ||
				stuck[q.str("area")+"/"+q.str("thread")] {
				continue
			}
			brief := fmt.Sprintf("Question %s in thread %s/%s:\n\n%s\n\nInvestigate in the clone and write the answer as SUMMARY:; the runner posts it. "+
				"If only a human can answer it, write QUESTION: with what you need and stop.", q.str("id"), q.str("area"), q.str("thread"), q.str("body"))
			job, isNew, err := addJob(s, slug, "answer", fmt.Sprintf("Answer question %s: %s", q.str("id"), q.str("summary")), brief, 0, 5,
				"question:"+q.str("id"), createdBy, true)
			if err != nil {
				return created, err
			}
			if isNew {
				created = append(created, job)
			}
			break
		}
	}
	if len(created) == 0 {
		if cards := ballotDue(s, slug, now, suggestions); len(cards) > 0 {
			var listed []string
			for _, c := range cards {
				listed = append(listed, fmt.Sprintf("- `%s` %s (%d of %d voters): %s", c.ID, c.Key, c.Voters, settingInt(s, "quorum"), trunc(c.Summary, 160)))
			}
			brief := fmt.Sprintf("%d suggestion(s) on this project's board have waited more than %d hours below the vote quorum, and "+
				"nothing else is queued. Read each card on the ballot below in full (its body is in the Ballot section; "+
				"check the path or post it names in the clone when it names one) and cast one `VOTE: <id> up|down <reason>` "+
				"line per card in your report: up when the change is one variable a worker could make and measure with the "+
				"project's own checks, down when it is a guess, a duplicate, owner-only, or already done. Never vote on "+
				"your own cards. Change no files and make no commits; a ballot job lands nothing. Write `SUMMARY:` with "+
				"how many votes you cast and the one pattern you saw across the cards.\n\nWaiting below quorum:\n%s",
				len(cards), ballotHours(s), strings.Join(listed, "\n"))
			job, isNew, err := addJob(s, slug, "ballot", fmt.Sprintf("Vote the ballot: %d card(s) below quorum for %s", len(cards), slug), brief,
				0, 3, fmt.Sprintf("ballot:%s:%s", slug, stampOf(now)), createdBy, true)
			if err != nil {
				return created, err
			}
			if isNew {
				created = append(created, job)
			}
		}
	}
	if len(created) == 0 {
		source := fmt.Sprintf("reflect:%s:%s", slug, now[:10])
		ever := mustOne(s.DB, "SELECT 1 AS x FROM jobs WHERE project = ? AND kind = 'reflect'", slug)
		doneToday := mustOne(s.DB, "SELECT 1 AS x FROM jobs WHERE project = ? AND kind = 'reflect' AND (source = ? OR (source = ? AND created_at LIKE ?))",
			slug, source, "onboard-reflect:"+slug, now[:10]+"%")
		if ever == nil || doneToday == nil {
			brief := "Run the project's own checks (tests, lint, links, `git diff --check`), read the memory records " +
				"(`pulse record list memory`) and the open ADRs' acceptance criteria (`pulse record list adr --status Proposed`), " +
				"and fix one Tier 0 thing you can prove with the checks (a flaky test, a broken link, a stale doc, a lint " +
				"finding). Read `pulse evolve report --project " + slug + "` for what the loop did this week - jobs done " +
				"without a change, card jobs that found nothing, experiments planned, questions waiting for the owner - " +
				"and when one gap is measurable, post it as one SUGGESTION: line naming File:, Variable: and Hypothesis:. " +
				"Record every other signal as FINDING: lines so the runner posts them."
			job, isNew, err := addJob(s, slug, "reflect", "Daily reflect: find and fix one provable Tier 0 issue", brief, 0, 1, source, createdBy, true)
			if err != nil {
				return created, err
			}
			if isNew {
				created = append(created, job)
			}
		}
	}
	// ADR-045: a project that is quiet - nothing plannable, nothing queued, nothing created for lead.research_minutes -
	// gets a research job, so idle time produces the cards and votes the rest of the planner runs on.
	if len(created) == 0 {
		job, isNew, err := planResearch(s, slug, now, createdBy)
		if err != nil {
			return created, err
		}
		if isNew {
			created = append(created, job)
		}
	}
	return created, nil
}

// stuckThreadSummary starts the one question the planner posts on a thread that hit lead.stuck_thread_jobs.
const stuckThreadSummary = "Stuck thread:"

// stuckThreads is the set of area/thread keys of one project whose worker jobs stopped moving it: the newest
// lead.stuck_thread_jobs finished jobs seeded by the thread's suggestions, handoffs and questions all committed
// nothing (each run read, claimed, asked or handed off, and the next repeated it). A reflect job - the loop's
// card job (ADR-044) - commits nothing by design and does not count. Such a thread costs a job per
// tick while only the lead or the owner can decide it, so the planner seeds nothing from it and posts one question
// for the owner on the thread - the lead's brief carries it. The count restarts when a job from the thread commits
// (a run with commits, or a branch waiting to land) and when the lead or the owner answers on the thread: each
// decision buys the next lead.stuck_thread_jobs attempts. 0 turns the cap off.
func stuckThreads(s *store.Store, slug, now, actor string) map[string]bool {
	limit := settingInt(s, "lead.stuck_thread_jobs")
	stuck := map[string]bool{}
	if limit <= 0 {
		return stuck
	}
	resetAt := map[string]string{}
	for _, r := range mustRows(s.DB, "SELECT area || '/' || thread AS key, MAX(date) AS d FROM posts "+
		"WHERE project = ? AND kind = 'answer' AND agent IN (?, ?) GROUP BY area, thread", slug, leadName(slug), store.HumanAgent) {
		resetAt[r.str("key")] = r.str("d")
	}
	type episode struct {
		ids, notes []string
		closed     bool
	}
	episodes := map[string]*episode{}
	for _, r := range mustRows(s.DB, "SELECT j.id, j.status, j.result, j.finished_at, "+
		"(j.branch != '' OR EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.commits > 0)) AS committed, "+
		"p.area || '/' || p.thread AS key FROM jobs j JOIN posts p ON p.id = substr(j.source, instr(j.source, ':') + 1) "+
		"WHERE j.project = ? AND j.status IN ('done', 'failed') AND j.kind != 'reflect' "+
		"AND (j.source LIKE 'suggestion:%' OR j.source LIKE 'handoff:%' OR j.source LIKE 'question:%') "+
		"ORDER BY j.finished_at DESC, j.created_at DESC", slug) {
		key := r.str("key")
		ep := episodes[key]
		if ep == nil {
			ep = &episode{}
			episodes[key] = ep
		}
		if ep.closed {
			continue
		}
		if r.int("committed") != 0 || r.str("finished_at") <= resetAt[key] {
			ep.closed = true
			if r.str("finished_at") > resetAt[key] {
				resetAt[key] = r.str("finished_at")
			}
			continue
		}
		ep.ids = append(ep.ids, r.str("id"))
		ep.notes = append(ep.notes, fmt.Sprintf("- %s (%s): %s", r.str("id"), r.str("status"), trunc(r.str("result"), 120)))
	}
	for key, ep := range episodes {
		if len(ep.ids) < limit {
			continue
		}
		stuck[key] = true
		area, thread, _ := strings.Cut(key, "/")
		if mustOne(s.DB, "SELECT 1 AS x FROM posts WHERE project = ? AND area = ? AND thread = ? AND kind = 'question' AND summary LIKE ? AND date > ?",
			slug, area, thread, stuckThreadSummary+"%", resetAt[key]) != nil {
			continue
		}
		body := fmt.Sprintf("The planner seeded %d finished worker jobs from thread %s (its suggestions, handoffs and questions) and none "+
			"committed: each run read, claimed, asked or handed off, and the next one repeated it. The planner seeds no more jobs "+
			"from this thread (lead.stuck_thread_jobs = %d) until a job from it commits or the lead or the owner answers on it.\n\n"+
			"@%s: decide it. Queue the one job with the decision written into its brief "+
			"(`pulse job add --project %s --kind improve --source <suggestion:|handoff:|question:><post-id> --title \"...\" --brief \"...\" [--tier 0|1]`), "+
			"answer on the thread with what the next worker must do differently, close the thread with an answer saying why it stops, "+
			"or hand this question to the owner when only the owner can decide.\n\n"+
			"Jobs that changed nothing:\n%s\n", len(ep.ids), key, limit, leadName(slug), slug, strings.Join(ep.notes, "\n"))
		if _, err := createPost(s, now, postOpts{Kind: "question", Project: slug, Area: area, Thread: thread, ForOwner: true,
			Agent: actorAgent(s, actor), Summary: fmt.Sprintf("%s %d worker jobs in a row changed no code; lead decision needed", stuckThreadSummary, len(ep.ids)),
			Body: body}); err != nil {
			eventAt(s.DB, now, actor, "plan.stuck.error", key, err.Error())
		}
	}
	return stuck
}
