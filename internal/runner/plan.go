package runner

import (
	"fmt"
	"strings"

	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

const ballotHours = 6 // a card gets its own session's votes first; idle hands vote on what is still short after this long

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
	posts := mustRows(q, "SELECT * FROM posts WHERE project = ? ORDER BY date, id", slug)
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
	for _, r := range rows {
		if r.str("kind") == "answer" && r.str("on_post") != "" {
			answered[r.str("on_post")] = true
		}
	}
	var out []row
	for _, r := range rows {
		if r.str("kind") != "question" || r.str("area") == "jobs" {
			continue
		}
		key := r.str("area") + "/" + r.str("thread")
		if status[key] == "closed" || answered[r.str("id")] {
			continue
		}
		later := false
		for _, x := range rows {
			if x.str("kind") == "answer" && x.str("area") == r.str("area") && x.str("thread") == r.str("thread") && x.str("date") > r.str("date") {
				later = true
				break
			}
		}
		if later {
			continue
		}
		out = append(out, r)
	}
	return out
}

// ballotDue is ballot_due(): below-quorum cards older than ballotHours, unless a ballot job was planned since.
func ballotDue(s *store.Store, slug, now string, suggestions []tally.Suggestion) []tally.Suggestion {
	since := plusHours(now, -ballotHours)
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
	waiting := mustOne(s.DB, "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND status IN ('queued', 'leased')", slug).int("n")
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
	// A handoff is work a worker left for the next one; the queue ranks it first, and so does the planner: the
	// oldest open handoff nobody claimed becomes one improve job, tried once (like a question), never when it
	// is addressed to the owner or points at owner-only paths.
	if res, err := tally.Compute(s, slug, "", tally.Thresholds{Now: now}); err == nil && res != nil {
		tried := map[string]bool{}
		for _, r := range mustRows(s.DB, "SELECT source FROM jobs WHERE source LIKE 'handoff:%'") {
			tried[r.str("source")] = true
		}
		for _, p := range tally.OpenHandoffs(res) {
			if tried["handoff:"+p.ID] {
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
	if len(created) == 0 {
		for _, sg := range suggestions {
			if sg.Status != "eligible" {
				continue
			}
			post := tallyPost(s, sg.ID)
			projectPath := ""
			if project != nil {
				projectPath = project.str("path")
			}
			if tally.PlanGate(s, projectPath, sg, post) != "" {
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
		for _, q := range openQuestions(s, slug, now) {
			if tried["question:"+q.str("id")] || !answerable(q, createdBy) {
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
				len(cards), ballotHours, strings.Join(listed, "\n"))
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
				"finding). Record every other signal as FINDING: lines so the runner posts them."
			job, isNew, err := addJob(s, slug, "reflect", "Daily reflect: find and fix one provable Tier 0 issue", brief, 0, 1, source, createdBy, true)
			if err != nil {
				return created, err
			}
			if isNew {
				created = append(created, job)
			}
		}
	}
	return created, nil
}
