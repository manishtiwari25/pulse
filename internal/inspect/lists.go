package inspect

import (
	"encoding/json"
	"flag"
	"fmt"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func runProjects(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if err := noArgs(args); err != nil {
		return err
	}
	projects, err := c.Store.Projects(false)
	if err != nil {
		return err
	}
	if projects == nil {
		projects = []store.Project{}
	}
	if c.JSON {
		return jsonOutput(c, projects)
	}
	for _, p := range projects {
		brain := ""
		if p.Brain {
			brain = "  (brain)"
		}
		c.Printf("%-18s %-8s %s%s  %s\n", p.Slug, p.Status, p.Path, brain, p.Remote)
	}
	return nil
}

// Job is the schema-3 job row, without abbreviating its title, brief or result.
type Job struct {
	ID         string `json:"id"`
	Project    string `json:"project"`
	Kind       string `json:"kind"`
	Tier       int    `json:"tier"`
	Title      string `json:"title"`
	Brief      string `json:"brief"`
	Source     string `json:"source"`
	Priority   int    `json:"priority"`
	Status     string `json:"status"`
	CreatedAt  string `json:"created_at"`
	CreatedBy  string `json:"created_by"`
	LeaseAgent string `json:"lease_agent"`
	LeaseUntil string `json:"lease_until"`
	Generation int    `json:"generation"`
	Attempts   int    `json:"attempts"`
	FinishedAt string `json:"finished_at"`
	Result     string `json:"result"`
	Branch     string `json:"branch"`
	Recovery   string `json:"recovery"`
}

// Jobs lists the queue in the same order as Python's print_jobs.
func Jobs(s *store.Store, project string, includeFinished bool) ([]Job, error) {
	q := `SELECT j.id, j.project, j.kind, j.tier, j.title, j.brief, j.source, j.priority, j.status,
		j.created_at, j.created_by, j.lease_agent, j.lease_until, j.generation, j.attempts, j.finished_at, j.result, j.branch,
		COALESCE(e.at, ''), COALESCE(e.detail, '')
		FROM jobs j
		LEFT JOIN events e ON e.seq = (
			SELECT seq FROM events WHERE action = 'job.recover' AND subject = j.id ORDER BY seq DESC LIMIT 1
		)
		WHERE 1=1`
	var params []any
	if project != "" {
		q += " AND j.project = ?"
		params = append(params, project)
	}
	if !includeFinished {
		q += " AND j.status IN ('queued', 'leased', 'blocked')"
	}
	rows, err := s.DB.Query(q+" ORDER BY j.status, j.priority DESC, j.created_at", params...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Job{}
	for rows.Next() {
		var j Job
		var recoverAt, recoverDetail string
		if err := rows.Scan(&j.ID, &j.Project, &j.Kind, &j.Tier, &j.Title, &j.Brief, &j.Source, &j.Priority,
			&j.Status, &j.CreatedAt, &j.CreatedBy, &j.LeaseAgent, &j.LeaseUntil, &j.Generation, &j.Attempts,
			&j.FinishedAt, &j.Result, &j.Branch, &recoverAt, &recoverDetail); err != nil {
			return nil, err
		}
		j.Recovery = recoveryText(recoverAt, recoverDetail)
		out = append(out, j)
	}
	return out, rows.Err()
}

func recoveryText(at, detail string) string {
	if detail == "" {
		return ""
	}
	var doc map[string]any
	if err := json.Unmarshal([]byte(detail), &doc); err != nil {
		return ""
	}
	outcome, _ := doc["outcome"].(string)
	switch outcome {
	case "requeued":
		return "ran again " + at
	case "replanned":
		if job, _ := doc["job"].(string); job != "" {
			return "re-planned as " + job
		}
		return "re-planned"
	case "held":
		why, _ := doc["why"].(string)
		return "held: " + why
	case "retried":
		return "retried by hand " + at
	default:
		return ""
	}
}

func runJobs(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if err := noArgs(args); err != nil {
		return err
	}
	jobs, err := Jobs(c.Store, c.Project, boolFlag(fs, "all"))
	if err != nil {
		return err
	}
	if c.JSON {
		return jsonOutput(c, jobs)
	}
	if len(jobs) == 0 {
		c.Println("no jobs")
	}
	for _, j := range jobs {
		lease := ""
		if j.Status == "leased" {
			lease = fmt.Sprintf(" by %s until %s", j.LeaseAgent, j.LeaseUntil)
		}
		c.Printf("%-9s %-14s %-8s tier %d  %-34s %s%s\n", j.Status, j.Project, j.Kind, j.Tier, j.ID, trunc(j.Title, 60), lease)
		if j.Result != "" || j.Recovery != "" {
			detail := trunc(j.Result, 100)
			if j.Recovery != "" {
				if detail != "" {
					detail += "  "
				}
				detail += "recovery: " + trunc(j.Recovery, 100)
			}
			c.Printf("          %s\n", detail)
		}
	}
	return nil
}

type AgentScorecard struct {
	store.Agent
	board.AgentPresence
	Findings    int    `json:"findings"`
	Suggestions int    `json:"suggestions"`
	Votes       int    `json:"votes"`
	Answers     int    `json:"answers"`
	Results     int    `json:"results"`
	N           int    `json:"n"`
	LastPost    string `json:"last_post"`
}

// AgentScorecards counts posts independently of the agent's cached posts field.
func AgentScorecards(s *store.Store, includePruned bool) ([]AgentScorecard, error) {
	presence, err := board.AgentPresences(s, "")
	if err != nil {
		return nil, err
	}
	agents, err := s.Agents("")
	if err != nil {
		return nil, err
	}
	out := []AgentScorecard{}
	indices := make(map[string]int, len(agents))
	for _, a := range agents {
		if !includePruned && a.Status == "pruned" {
			continue
		}
		indices[a.Name] = len(out)
		out = append(out, AgentScorecard{Agent: a, AgentPresence: presence[a.Name]})
	}
	rows, err := s.DB.Query(`SELECT agent, SUM(kind = 'finding'), SUM(kind = 'suggestion'), SUM(kind = 'vote'),
		SUM(kind = 'answer'), SUM(kind = 'result'), COUNT(*), MAX(date) FROM posts GROUP BY agent`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	for rows.Next() {
		var name string
		var stats AgentScorecard
		if err := rows.Scan(&name, &stats.Findings, &stats.Suggestions, &stats.Votes, &stats.Answers,
			&stats.Results, &stats.N, &stats.LastPost); err != nil {
			return nil, err
		}
		if index, ok := indices[name]; ok {
			stats.Agent = out[index].Agent
			stats.AgentPresence = out[index].AgentPresence
			out[index] = stats
		}
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	sort.Slice(out, func(i, j int) bool {
		a, b := out[i], out[j]
		if a.Status != b.Status {
			return a.Status < b.Status
		}
		if a.LastActiveAt != b.LastActiveAt {
			return a.LastActiveAt > b.LastActiveAt
		}
		return a.Name < b.Name
	})
	return out, nil
}

func runAgents(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if err := noArgs(args); err != nil {
		return err
	}
	agents, err := AgentScorecards(c.Store, boolFlag(fs, "all"))
	if err != nil {
		return err
	}
	if c.JSON {
		return jsonOutput(c, agents)
	}
	for _, a := range agents {
		c.Printf("%-16s %-7s %-8s presence %-7s posts %-4d findings %-3d suggestions %-3d votes %-3d jobs %d/%d  last heartbeat %s  last useful %s\n",
			a.Name, a.Status, a.Kind, a.Presence, a.Posts, a.Findings, a.Suggestions, a.Votes, a.JobsDone,
			a.JobsDone+a.JobsFailed, orDash(a.LastHeartbeatAt), orDash(a.LastUsefulAt))
	}
	return nil
}

// Event keeps detail as the stored string, including the Python JSON spacing.
type Event struct {
	Seq     int64  `json:"seq"`
	At      string `json:"at"`
	Actor   string `json:"actor"`
	Action  string `json:"action"`
	Subject string `json:"subject"`
	Detail  string `json:"detail"`
}

// Events returns the newest events after a sequence number or UTC timestamp.
// Project membership includes direct subjects, referenced board/runner rows,
// and events that identify their project in JSON detail.
func Events(s *store.Store, project, since string, limit int) ([]Event, error) {
	q := "SELECT e.seq, e.at, e.actor, e.action, e.subject, e.detail FROM events e WHERE 1=1"
	var params []any
	if project != "" {
		q += ` AND (e.subject = ? OR e.subject IN (SELECT id FROM posts WHERE project = ?)
			OR e.subject IN (SELECT id FROM jobs WHERE project = ?)
			OR e.subject IN (SELECT id FROM runs WHERE project = ?)
			OR CASE WHEN json_valid(e.detail) THEN json_extract(e.detail, '$.project') END = ?)`
		params = append(params, project, project, project, project, project)
	}
	if since != "" {
		if seq, err := strconv.ParseInt(since, 10, 64); err == nil && seq >= 0 {
			q += " AND e.seq > ?"
			params = append(params, seq)
		} else {
			at, err := time.Parse("2006-01-02T15:04:05Z", since)
			if err != nil {
				return nil, cli.Errorf(2, "pulse: --since takes a non-negative event sequence or a UTC time like 2026-09-05T08:00:00Z", "", "")
			}
			q += " AND e.at > ?"
			params = append(params, at.Format("2006-01-02T15:04:05Z"))
		}
	}
	q += " ORDER BY e.seq DESC LIMIT ?"
	params = append(params, limit)
	rows, err := s.DB.Query(q, params...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Event{}
	for rows.Next() {
		var e Event
		if err := rows.Scan(&e.Seq, &e.At, &e.Actor, &e.Action, &e.Subject, &e.Detail); err != nil {
			return nil, err
		}
		out = append(out, e)
	}
	return out, rows.Err()
}

func runEvents(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if err := noArgs(args); err != nil {
		return err
	}
	limit := fs.Lookup("limit").Value.(flag.Getter).Get().(int)
	events, err := Events(c.Store, c.Project, stringFlag(fs, "since"), limit)
	if err != nil {
		return err
	}
	redact := bus.Redactor(c.Store)
	for i, e := range events {
		masked := redact(bus.Event{ID: e.Seq, At: e.At, Actor: e.Actor, Action: e.Action, Subject: e.Subject, Detail: e.Detail})
		events[i] = Event{Seq: masked.ID, At: masked.At, Actor: masked.Actor, Action: masked.Action, Subject: masked.Subject, Detail: masked.Detail}
	}
	if c.JSON {
		return jsonOutput(c, events)
	}
	for _, e := range events {
		c.Printf("%s %-8d %-18s %-16s %-40s %s\n", e.At, e.Seq, e.Action, trunc(e.Actor, 16), trunc(e.Subject, 40), trunc(e.Detail, 80))
	}
	return nil
}

func joinCounts(counts []StatusCount) string {
	parts := make([]string, len(counts))
	for i, count := range counts {
		parts[i] = fmt.Sprintf("%d %s", count.N, count.Status)
	}
	return strings.Join(parts, ", ")
}
