package tasks

import (
	"sort"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Worker is a currently leased job in a task's work set.
type Worker struct {
	Job      string `json:"job"`
	Kind     string `json:"kind"`
	Title    string `json:"title"`
	Agent    string `json:"agent"`
	Since    string `json:"since"`
	Executor string `json:"executor"`
}

// Work summarizes the jobs attached to a task.
type Work struct {
	Total   int      `json:"jobs_total"`
	Queued  int      `json:"jobs_queued"`
	Blocked int      `json:"jobs_blocked"`
	Running int      `json:"jobs_running"`
	Done    int      `json:"jobs_done"`
	Failed  int      `json:"jobs_failed"`
	Working []Worker `json:"working"`
}

type workJob struct {
	id         string
	project    string
	kind       string
	title      string
	status     string
	source     string
	agent      string
	leaseUntil string
	since      string
}

type runningRun struct {
	startedAt string
	executor  string
}

// WorkFor summarizes task job sets in bulk. The job set for each task is all
// jobs in the same project with source "task:<id>", plus the lead planning job
// named by task.job_id when present.
func WorkFor(s *store.Store, ts []Task) (map[string]Work, error) {
	out := make(map[string]Work, len(ts))
	if len(ts) == 0 {
		return out, nil
	}

	sourceTasks := map[string][]int{}
	jobTasks := map[string][]int{}
	var sourceArgs, jobArgs []any
	seenSources, seenJobs := map[string]bool{}, map[string]bool{}
	for i, t := range ts {
		out[t.ID] = Work{Working: []Worker{}}
		if t.ID != "" {
			source := "task:" + t.ID
			sourceTasks[workKey(t.Project, source)] = append(sourceTasks[workKey(t.Project, source)], i)
			if !seenSources[source] {
				seenSources[source] = true
				sourceArgs = append(sourceArgs, source)
			}
		}
		if t.JobID != "" {
			jobTasks[workKey(t.Project, t.JobID)] = append(jobTasks[workKey(t.Project, t.JobID)], i)
			if !seenJobs[t.JobID] {
				seenJobs[t.JobID] = true
				jobArgs = append(jobArgs, t.JobID)
			}
		}
	}
	if len(sourceArgs) == 0 && len(jobArgs) == 0 {
		return out, nil
	}

	query, args := workJobsQuery(sourceArgs, jobArgs)
	rows, err := s.DB.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var jobs []workJob
	activeJobIDs := map[string]bool{}
	now := s.Now()
	for rows.Next() {
		var j workJob
		if err := rows.Scan(&j.id, &j.project, &j.kind, &j.title, &j.status, &j.source, &j.agent, &j.leaseUntil, &j.since); err != nil {
			return nil, err
		}
		if len(workTaskIndexes(ts, sourceTasks, jobTasks, j)) == 0 {
			continue
		}
		jobs = append(jobs, j)
		if workIsActive(j, now) {
			activeJobIDs[j.id] = true
		}
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}

	runs, err := workRunningRuns(s, activeJobIDs)
	if err != nil {
		return nil, err
	}
	for _, j := range jobs {
		for _, idx := range workTaskIndexes(ts, sourceTasks, jobTasks, j) {
			t := ts[idx]
			w := out[t.ID]
			w.Total++
			switch j.status {
			case "queued":
				w.Queued++
			case "blocked":
				w.Blocked++
			case "leased":
				w.Running++
			case "done":
				w.Done++
			case "failed":
				w.Failed++
			}
			if workIsActive(j, now) {
				worker := Worker{Job: j.id, Kind: j.kind, Title: j.title, Agent: j.agent, Since: j.since}
				if run, ok := runs[j.id]; ok {
					worker.Since = run.startedAt
					worker.Executor = run.executor
				}
				w.Working = append(w.Working, worker)
			}
			out[t.ID] = w
		}
	}
	for id, w := range out {
		sort.Slice(w.Working, func(i, j int) bool {
			if w.Working[i].Since == w.Working[j].Since {
				return w.Working[i].Job < w.Working[j].Job
			}
			return w.Working[i].Since < w.Working[j].Since
		})
		out[id] = w
	}
	return out, nil
}

func workJobsQuery(sourceArgs, jobArgs []any) (string, []any) {
	clauses := []string{}
	var args []any
	if len(sourceArgs) > 0 {
		clauses = append(clauses, "source IN ("+workPlaceholders(len(sourceArgs))+")")
		args = append(args, sourceArgs...)
	}
	if len(jobArgs) > 0 {
		clauses = append(clauses, "id IN ("+workPlaceholders(len(jobArgs))+")")
		args = append(args, jobArgs...)
	}
	return `SELECT id, project, kind, title, status, source, lease_agent, lease_until,
		COALESCE((SELECT MAX(at) FROM events WHERE action = 'job.lease' AND subject = jobs.id AND actor = jobs.lease_agent), created_at)
		FROM jobs WHERE ` + strings.Join(clauses, " OR ") + ` ORDER BY created_at, id`, args
}

func workRunningRuns(s *store.Store, jobIDs map[string]bool) (map[string]runningRun, error) {
	out := map[string]runningRun{}
	if len(jobIDs) == 0 {
		return out, nil
	}
	args := make([]any, 0, len(jobIDs))
	for id := range jobIDs {
		args = append(args, id)
	}
	rows, err := s.DB.Query(`SELECT job_id, started_at, executor FROM runs
		WHERE status = 'running' AND mode != 'dry-run' AND job_id IN (`+workPlaceholders(len(args))+`)
		ORDER BY started_at, rowid`, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	for rows.Next() {
		var jobID string
		var run runningRun
		if err := rows.Scan(&jobID, &run.startedAt, &run.executor); err != nil {
			return nil, err
		}
		if _, exists := out[jobID]; !exists {
			out[jobID] = run
		}
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	return out, nil
}

func workTaskIndexes(ts []Task, sourceTasks, jobTasks map[string][]int, j workJob) []int {
	seen := map[int]bool{}
	var out []int
	for _, idx := range sourceTasks[workKey(j.project, j.source)] {
		seen[idx] = true
		out = append(out, idx)
	}
	for _, idx := range jobTasks[workKey(j.project, j.id)] {
		if !seen[idx] {
			out = append(out, idx)
		}
	}
	return out
}

func workIsActive(j workJob, now string) bool {
	return j.status == "leased" && j.agent != "" && (j.leaseUntil == "" || j.leaseUntil > now)
}

func workKey(project, value string) string {
	return project + "\x00" + value
}

func workPlaceholders(n int) string {
	if n <= 0 {
		return ""
	}
	return strings.TrimRight(strings.Repeat("?,", n), ",")
}
