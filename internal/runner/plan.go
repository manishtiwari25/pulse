package runner

import (
	"fmt"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

const ballotHours = 6 // a card gets its own session's votes first; idle hands vote on what is still short after this long

var signalRe = regexp.MustCompile(`(?im)^\s*(?:-\s*)?` + "`?signal`?" + `\s*:\s*(.+?)\s*$|\bsignal:\s*(\S+)`)

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
	return mustRows(q, "SELECT * FROM posts WHERE project = ? ORDER BY date, id", slug)
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

func signalTokens(text string) []string {
	var out []string
	for _, m := range signalRe.FindAllStringSubmatch(text, -1) {
		raw := m[1]
		if raw == "" {
			raw = m[2]
		}
		raw = strings.Trim(strings.TrimSpace(raw), "`'\"()[],;")
		for _, tok := range regexp.MustCompile(`[\s,;]+`).Split(raw, -1) {
			tok = strings.Trim(tok, "`'\"()[],;.")
			if tok != "" {
				out = append(out, tok)
			}
		}
	}
	return out
}

func relOfToken(tok string) string {
	rel, _, _ := strings.Cut(tok, ":")
	rel, _, _ = strings.Cut(rel, "#")
	return rel
}

// signalResolves: a worker's card carries evidence when a `Signal:` names something that exists.
func signalResolves(q querier, project, post row) bool {
	text := post.str("summary") + "\n" + post.str("body")
	root := ""
	if project != nil {
		root = project.str("path")
	}
	for _, tok := range signalTokens(text) {
		if strings.HasPrefix(strings.ToLower(tok), "check:") && len(tok) > 6 {
			return true
		}
		if postRow(q, tok) != nil {
			return true
		}
		rel := relOfToken(tok)
		if root != "" && rel != "" && !strings.HasPrefix(rel, "/") && !contains(strings.Split(filepath.Clean(rel), "/"), "..") && fileExists(filepath.Join(root, rel)) {
			return true
		}
	}
	return false
}

func isWorkerPost(q querier, post row) bool {
	r := agentRow(q, post.str("agent"))
	return r != nil && r.str("kind") == "worker"
}

func isOwnerAddressedPost(post row) bool {
	if post == nil {
		return false
	}
	thread := strings.ToLower(post.str("thread"))
	if thread == "owner-questions" || thread == "owner-question" || strings.HasPrefix(thread, "owner-") {
		return true
	}
	low := strings.ToLower(strings.TrimSpace(post.str("summary")))
	for _, p := range []string{"evaluator proposal for the owner", "release candidate", "owner question", "vote requested"} {
		if strings.HasPrefix(low, p) {
			return true
		}
	}
	return strings.Contains(low, "for the owner") || strings.Contains(low, "vote requested")
}

func ownerTargeted(post row) bool {
	for _, tok := range signalTokens(post.str("summary") + "\n" + post.str("body")) {
		rel := strings.TrimLeft(relOfToken(tok), "/")
		for _, p := range ownerPaths {
			if rel == p || strings.HasPrefix(rel, p) {
				return true
			}
		}
	}
	return false
}

// planJobs is plan_jobs(): the next job for a project with fewer than parallel_per_project waiting.
func planJobs(s *store.Store, slug, now, createdBy string) ([]row, error) {
	waiting := mustOne(s.DB, "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND status IN ('queued', 'leased')", slug).int("n")
	if waiting >= parallelPerProject(s) {
		return nil, nil
	}
	project := projectBySlug(s.DB, slug)
	// ADR-031: a project at the baseline gate gets the baseline job before anything else.
	if job, isNew, err := planBaseline(s, project, slug, now, createdBy); err != nil {
		return nil, err
	} else if isNew {
		return []row{job}, nil
	}
	suggestions := suggestionRecords(s, slug, now)
	var created []row
	for _, sg := range suggestions {
		if sg.Status != "eligible" || sg.Score <= 0 {
			continue
		}
		post := postRow(s.DB, sg.ID)
		if isOwnerAddressedPost(post) {
			continue
		}
		if post != nil && isWorkerPost(s.DB, post) {
			if !signalResolves(s.DB, project, post) || ownerTargeted(post) {
				continue
			}
		}
		brief := fmt.Sprintf("The agents voted this the top eligible suggestion (score %+d, %d voters). Thread %s.\n\n%s\n\n"+
			"One variable, measured by the project's own checks. When the change is kept, say what moved in SUMMARY:.",
			sg.Score, sg.Voters, sg.Key, post.str("body"))
		job, isNew, err := addJob(s, slug, "improve", fmt.Sprintf("Implement suggestion %s: %s", sg.ID, sg.Summary), brief, 1,
			10+sg.Score, "suggestion:"+sg.ID, createdBy, true)
		if err != nil {
			return created, err
		}
		if isNew {
			created = append(created, job)
		}
		break
	}
	if len(created) == 0 {
		tried := map[string]bool{}
		for _, r := range mustRows(s.DB, "SELECT source FROM jobs WHERE source LIKE 'question:%'") {
			tried[r.str("source")] = true
		}
		for _, q := range openQuestions(s, slug, now) {
			if tried["question:"+q.str("id")] {
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
