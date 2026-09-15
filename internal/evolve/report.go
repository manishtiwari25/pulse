package evolve

import (
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

// Report is the read-only self-improvement report ADR-012 asked for, from the store: what the
// operating layer and the measured loop did for one project in a window, as counts anyone can
// check against the same tables. It answers the owner's questions - is anything improving, does the
// voting work, does the loop run - without a hand-written analysis.
type Report struct {
	Project     string       `json:"project"`
	Days        int          `json:"days"`
	Since       string       `json:"since"`
	Now         string       `json:"now"`
	Status      *LoopStatus  `json:"status"`
	Posts       []KindCount  `json:"posts"`
	Suggestions Funnel       `json:"suggestions"`
	CardJobs    CardJobCount `json:"card_jobs"`
	Jobs        []JobCount   `json:"jobs"`
	Research    Research     `json:"research"`
	Landings    Landings     `json:"landings"`
	Experiments Experiments  `json:"experiments"`
	Owner       OwnerInbox   `json:"owner"`
}

// Research is what the planner's research jobs (ADR-045; kind reflect, source research:*) left behind in the
// window: the posts their runs made, by what the rest of the planner runs on. Cards are the suggestions that
// name one File: and one Variable:. The jobs themselves are the "research" row of Jobs.
type Research struct {
	Jobs        int `json:"jobs"`
	Suggestions int `json:"suggestions"`
	Cards       int `json:"cards"`
	Votes       int `json:"votes"`
	Findings    int `json:"findings"`
	Answers     int `json:"answers"`
}

type KindCount struct {
	Kind  string `json:"kind"`
	Count int    `json:"count"`
}

// Funnel counts the suggestions posted in the window by where the vote and the planner left them.
type Funnel struct {
	Posted     int `json:"posted"`
	Voted      int `json:"voted"`       // at least one independent vote
	NeedsVotes int `json:"needs_votes"` // below quorum, not expired
	Eligible   int `json:"eligible"`    // at quorum, nothing built yet
	Carded     int `json:"carded"`      // a card job turned it into an experiment card (ADR-044)
	Built      int `json:"built"`       // an improve or experiment job ended done
	Tried      int `json:"tried"`       // a job ended failed or cancelled
	Expired    int `json:"expired"`
	Closed     int `json:"closed"`
	WithCard   int `json:"with_card"` // the post itself names one File: and one Variable:
	InScope    int `json:"in_scope"`  // and the constraint check accepts it as an experiment card now
}

// CardJobCount counts the loop's card jobs (ADR-044) created in the window.
type CardJobCount struct {
	Queued int `json:"queued"` // queued, leased or blocked
	Carded int `json:"carded"` // done, a card was posted
	NoCard int `json:"no_card"`
	Failed int `json:"failed"`
}

// JobCount splits one kind's jobs created in the window by what they left behind: a done job whose
// runs committed nothing is counted apart from one that changed the repository.
type JobCount struct {
	Kind          string `json:"kind"`
	DoneChanged   int    `json:"done_changed"`
	DoneUnchanged int    `json:"done_unchanged"`
	Failed        int    `json:"failed"`
	Cancelled     int    `json:"cancelled"`
	Open          int    `json:"open"`
}

type Landings struct {
	Landed       int `json:"landed"`
	CodeFiles    int `json:"code_files"` // landings whose evidence names at least one code file
	DocsOnly     int `json:"docs_only"`
	Pushed       int `json:"pushed"`
	Held         int `json:"held"`
	Conflict     int `json:"conflict"`
	FailedVerify int `json:"failed_verify"`
}

type Experiments struct {
	Planned   int `json:"planned"` // experiment jobs created
	Kept      int `json:"kept"`    // archives in the window, baseline excluded
	Discarded int `json:"discarded"`
	Cycles    int `json:"cycles_total"` // since the loop was enabled
}

type OwnerInbox struct {
	StopQuestions int `json:"stop_questions"` // undecided evolution-stop questions, all time
	Questions     int `json:"questions"`      // other unanswered questions for the owner, all time
}

// BuildReport computes the report for project over the last days days.
func BuildReport(s *store.Store, project string, days int) (*Report, error) {
	if days <= 0 {
		days = 7
	}
	now := s.Now()
	at, err := time.Parse("2006-01-02T15:04:05Z", now)
	if err != nil {
		return nil, err
	}
	since := at.Add(-time.Duration(days) * 24 * time.Hour).Format("2006-01-02T15:04:05Z")
	status, err := Status(s, project)
	if err != nil {
		return nil, err
	}
	r := &Report{Project: project, Days: days, Since: since, Now: now, Status: status}
	if r.Posts, err = postCounts(s, project, since); err != nil {
		return nil, err
	}
	if r.Suggestions, err = suggestionFunnel(s, project, since, status); err != nil {
		return nil, err
	}
	if r.CardJobs, err = cardJobCounts(s, project, since); err != nil {
		return nil, err
	}
	if r.Jobs, err = jobCounts(s, project, since); err != nil {
		return nil, err
	}
	if r.Research, err = researchCounts(s, project, since); err != nil {
		return nil, err
	}
	if r.Landings, err = landingCounts(s, project, since); err != nil {
		return nil, err
	}
	if r.Experiments, err = experimentCounts(s, project, since, status); err != nil {
		return nil, err
	}
	if r.Owner, err = ownerInbox(s, project); err != nil {
		return nil, err
	}
	return r, nil
}

func postCounts(s *store.Store, project, since string) ([]KindCount, error) {
	rows, err := s.DB.Query("SELECT kind, COUNT(*) FROM posts WHERE project = ? AND date >= ? GROUP BY kind ORDER BY COUNT(*) DESC, kind", project, since)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []KindCount{}
	for rows.Next() {
		var k KindCount
		if err := rows.Scan(&k.Kind, &k.Count); err != nil {
			return nil, err
		}
		out = append(out, k)
	}
	return out, rows.Err()
}

func suggestionFunnel(s *store.Store, project, since string, status *LoopStatus) (Funnel, error) {
	var f Funnel
	result, err := tally.Compute(s, project, "", tally.Thresholds{})
	if err != nil {
		var applyErr *tally.ApplyError
		if !errors.As(err, &applyErr) {
			return f, err
		}
		return f, nil
	}
	posts := map[string]*tally.Post{}
	for i := range result.Posts {
		posts[result.Posts[i].ID] = &result.Posts[i]
	}
	for _, sg := range result.Suggestions {
		if sg.Date < since {
			continue
		}
		f.Posted++
		if sg.Voters > 0 {
			f.Voted++
		}
		switch sg.Status {
		case "needs votes":
			f.NeedsVotes++
		case "eligible":
			f.Eligible++
		case "carded":
			f.Carded++
		case "built":
			f.Built++
		case "tried":
			f.Tried++
		case "expired":
			f.Expired++
		case "closed":
			f.Closed++
		}
		if p := posts[sg.ID]; p != nil {
			fields := cardFields(p.Body)
			if fields["file"] != "" && fields["variable"] != "" {
				f.WithCard++
			}
			if _, err := makeCard(s, p, status); err == nil {
				f.InScope++
			}
		}
	}
	return f, nil
}

func cardJobCounts(s *store.Store, project, since string) (CardJobCount, error) {
	var c CardJobCount
	rows, err := s.DB.Query(`SELECT status, result FROM jobs WHERE project = ? AND kind = 'reflect' AND source LIKE 'suggestion:%'
		AND title LIKE ? AND created_at >= ?`, project, CardJobTitle+"%", since)
	if err != nil {
		return c, err
	}
	defer rows.Close()
	for rows.Next() {
		var status, result string
		if err := rows.Scan(&status, &result); err != nil {
			return c, err
		}
		switch {
		case status == "queued" || status == "leased" || status == "blocked":
			c.Queued++
		case status == "done" && strings.HasPrefix(strings.ToLower(strings.TrimSpace(result)), "no card"):
			c.NoCard++
		case status == "done":
			c.Carded++
		default:
			c.Failed++
		}
	}
	return c, rows.Err()
}

// researchJobSQL tells a research job from the other reflect jobs (a daily reflect, a card job) for a jobs row j.
const researchJobSQL = "(j.kind = 'reflect' AND j.source LIKE 'research:%')"

func jobCounts(s *store.Store, project, since string) ([]JobCount, error) {
	rows, err := s.DB.Query(`SELECT CASE WHEN `+researchJobSQL+` THEN 'research' ELSE j.kind END AS kind, j.status,
		(j.branch != '' OR EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.commits > 0)) AS changed
		FROM jobs j WHERE j.project = ? AND j.created_at >= ?`, project, since)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	byKind := map[string]*JobCount{}
	for rows.Next() {
		var kind, status string
		var changed bool
		if err := rows.Scan(&kind, &status, &changed); err != nil {
			return nil, err
		}
		c := byKind[kind]
		if c == nil {
			c = &JobCount{Kind: kind}
			byKind[kind] = c
		}
		switch status {
		case "done":
			if changed {
				c.DoneChanged++
			} else {
				c.DoneUnchanged++
			}
		case "failed":
			c.Failed++
		case "cancelled":
			c.Cancelled++
		default:
			c.Open++
		}
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	out := make([]JobCount, 0, len(byKind))
	for _, c := range byKind {
		out = append(out, *c)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Kind < out[j].Kind })
	return out, nil
}

// researchCounts counts the research jobs created in the window and the posts their runs made: a post's session
// is its run's id, the run names its job.
func researchCounts(s *store.Store, project, since string) (Research, error) {
	var out Research
	if err := s.DB.QueryRow(`SELECT COUNT(*) FROM jobs j WHERE j.project = ? AND `+researchJobSQL+` AND j.created_at >= ?`,
		project, since).Scan(&out.Jobs); err != nil {
		return out, err
	}
	rows, err := s.DB.Query(`SELECT p.kind, p.body FROM posts p JOIN runs r ON r.id = p.session JOIN jobs j ON j.id = r.job_id
		WHERE j.project = ? AND `+researchJobSQL+` AND p.created_at >= ?`, project, since)
	if err != nil {
		return out, err
	}
	defer rows.Close()
	for rows.Next() {
		var kind, body string
		if err := rows.Scan(&kind, &body); err != nil {
			return out, err
		}
		switch kind {
		case "suggestion":
			out.Suggestions++
			if f := cardFields(body); f["file"] != "" && f["variable"] != "" {
				out.Cards++
			}
		case "vote":
			out.Votes++
		case "finding":
			out.Findings++
		case "answer":
			out.Answers++
		}
	}
	return out, rows.Err()
}

func landingCounts(s *store.Store, project, since string) (Landings, error) {
	var l Landings
	rows, err := s.DB.Query(`SELECT action, detail FROM events WHERE action IN ('land.landed', 'land.pushed', 'land.held', 'land.conflict', 'land.failed-verify')
		AND at >= ? AND detail LIKE '%"project"%'`, since)
	if err != nil {
		return l, err
	}
	defer rows.Close()
	for rows.Next() {
		var action, detail string
		if err := rows.Scan(&action, &detail); err != nil {
			return l, err
		}
		var d struct {
			Project      string `json:"project"`
			CodeDelivery *struct {
				CodeFiles    int `json:"code_files"`
				DocsFiles    int `json:"docs_files"`
				FilesChanged int `json:"files_changed"`
			} `json:"code_delivery"`
		}
		if json.Unmarshal([]byte(detail), &d) != nil || d.Project != project {
			continue
		}
		switch action {
		case "land.landed":
			l.Landed++
			if d.CodeDelivery != nil {
				if d.CodeDelivery.CodeFiles > 0 {
					l.CodeFiles++
				} else if d.CodeDelivery.DocsFiles > 0 && d.CodeDelivery.DocsFiles == d.CodeDelivery.FilesChanged {
					l.DocsOnly++
				}
			}
		case "land.pushed":
			l.Pushed++
		case "land.held":
			l.Held++
		case "land.conflict":
			l.Conflict++
		case "land.failed-verify":
			l.FailedVerify++
		}
	}
	return l, rows.Err()
}

func experimentCounts(s *store.Store, project, since string, status *LoopStatus) (Experiments, error) {
	e := Experiments{Cycles: status.Cycles}
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM jobs WHERE project = ? AND kind = 'experiment' AND created_at >= ?", project, since).Scan(&e.Planned); err != nil {
		return e, err
	}
	archive, err := ListArchive(s, project)
	if err != nil {
		return e, err
	}
	for _, a := range archive {
		if a.At < since || a.Hypothesis == BaselineHypothesis {
			continue
		}
		if a.Kept {
			e.Kept++
		} else {
			e.Discarded++
		}
	}
	return e, nil
}

func ownerInbox(s *store.Store, project string) (OwnerInbox, error) {
	var o OwnerInbox
	rows, err := s.DB.Query(`SELECT q.summary, q.extra FROM posts q WHERE q.project = ? AND q.kind = 'question'
		AND NOT EXISTS (SELECT 1 FROM decisions d WHERE d.post_id = q.id)
		AND NOT EXISTS (SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date)`, project)
	if err != nil {
		return o, err
	}
	defer rows.Close()
	for rows.Next() {
		var summary, extra string
		if err := rows.Scan(&summary, &extra); err != nil {
			return o, err
		}
		if forOwner, err := store.PostForOwner(extra); err != nil || !forOwner {
			continue
		}
		if summary == StopSummary {
			o.StopQuestions++
		} else {
			o.Questions++
		}
	}
	return o, rows.Err()
}

func reportRows(r *Report) [][]string {
	n := strconv.Itoa
	var posts []string
	for _, k := range r.Posts {
		posts = append(posts, fmt.Sprintf("%s %d", k.Kind, k.Count))
	}
	if len(posts) == 0 {
		posts = []string{"none"}
	}
	f := r.Suggestions
	rows := [][]string{
		{"Window", fmt.Sprintf("last %d day(s), since %s", r.Days, r.Since)},
		{"Posts", strings.Join(posts, ", ")},
		{"Suggestions posted", fmt.Sprintf("%d; %d voted; %d name one File: and one Variable:, %d of those inside the editable scope", f.Posted, f.Voted, f.WithCard, f.InScope)},
		{"Suggestions now", fmt.Sprintf("%d need votes, %d eligible, %d carded, %d built, %d tried, %d expired, %d closed",
			f.NeedsVotes, f.Eligible, f.Carded, f.Built, f.Tried, f.Expired, f.Closed)},
		{"Card jobs", fmt.Sprintf("%d open, %d carded, %d no card, %d failed", r.CardJobs.Queued, r.CardJobs.Carded, r.CardJobs.NoCard, r.CardJobs.Failed)},
	}
	if len(r.Jobs) == 0 {
		rows = append(rows, []string{"Jobs", "none"})
	}
	for _, j := range r.Jobs {
		rows = append(rows, []string{"Jobs " + j.Kind, fmt.Sprintf("%d done with commits, %d done without, %d failed, %d cancelled, %d open",
			j.DoneChanged, j.DoneUnchanged, j.Failed, j.Cancelled, j.Open)})
	}
	rows = append(rows, []string{"Research", fmt.Sprintf("%d job(s); left %d suggestion(s) (%d cards), %d vote(s), %d finding(s), %d answer(s)",
		r.Research.Jobs, r.Research.Suggestions, r.Research.Cards, r.Research.Votes, r.Research.Findings, r.Research.Answers)})
	l := r.Landings
	rows = append(rows,
		[]string{"Landings", fmt.Sprintf("%d landed (%d with code files, %d docs only); %d pushed; %d held, %d conflicts, %d failed verify",
			l.Landed, l.CodeFiles, l.DocsOnly, l.Pushed, l.Held, l.Conflict, l.FailedVerify)},
		[]string{"Experiments", fmt.Sprintf("%d planned; %d kept, %d discarded; %d cycles since the loop was enabled",
			r.Experiments.Planned, r.Experiments.Kept, r.Experiments.Discarded, r.Experiments.Cycles)},
		[]string{"Owner inbox", n(r.Owner.StopQuestions) + " undecided stop question(s), " + n(r.Owner.Questions) + " other open question(s)"},
	)
	return rows
}
