package runner

import (
	"strings"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Jobs and leases: the queue the runner works from (legacy/pulse.py 1582-1710).
// Adding and finishing a job are board.AddJob / board.FinishJob (the same rules
// `pulse job add|finish` apply); the lease itself stays here, the runner being
// the only process that takes one.

// queueOnboardingJob queues a project's first job (board.QueueOnboardingJob);
// the brief tells the agent to write the environment recipe (D-K) unless a
// CTX-environment record already exists.
func queueOnboardingJob(s *store.Store, slug, createdBy string) (row, bool, error) {
	rec, _ := recipeRecord(s, slug)
	j, created, err := board.QueueOnboardingJob(s, slug, createdBy, rec != nil)
	if err != nil {
		return nil, false, err
	}
	return jobRow(s.DB, j.ID), created, nil
}

// addJob queues a job through board.AddJob; dedupe returns the open job with
// the same source instead of a second one.
func addJob(s *store.Store, project, kind, title, brief string, tier, priority int, source, createdBy string, dedupe bool) (row, bool, error) {
	j, created, err := board.AddJob(s, board.JobInput{Project: project, Kind: kind, Title: title, Brief: brief, Tier: tier,
		Priority: priority, Source: source, CreatedBy: createdBy, NoDedupe: !dedupe})
	if err != nil {
		return nil, false, err
	}
	return jobRow(s.DB, j.ID), created, nil
}

func jobRow(q querier, id string) row {
	return mustOne(q, "SELECT * FROM jobs WHERE id = ?", id)
}

func parallelPerProject(s *store.Store) int {
	n := settingInt(s, "parallel_per_project")
	if n < 1 {
		return 1
	}
	return n
}

// Worker selection and leasing share this predicate for a job aliased as k.
// A finish is provisional while its controller runs; queued takeovers still
// use the lease generation fence.
const occupiedJobSQL = "(k.status = 'leased' OR (k.status != 'queued' AND EXISTS " +
	"(SELECT 1 FROM runs r WHERE r.job_id = k.id AND r.project = k.project AND r.agent = k.lease_agent AND r.status = 'running')))"

// leaseOpts narrows lease_job(): one project, one job, or the slugs an executor may serve.
type leaseOpts struct {
	Project  string
	JobID    string
	Minutes  int // 0 means minutes_per_job + 5
	Projects []string
	AnyProj  bool // Projects == nil means "any project" in Python; AnyProj=false with empty Projects means none
}

// leaseJob is lease_job(): atomically take the next queued job under the
// per-project limit, requeueing expired leases first. Jobs of kind lead are
// the project's lead's alone (D-C): a lead agent leases only its own
// project's lead jobs and every other agent skips them.
func leaseJob(s *store.Store, agent, now string, o leaseOpts) (job row, err error) {
	minutes := o.Minutes
	if minutes == 0 {
		minutes = settingInt(s, "minutes_per_job") + 5
	}
	limit := parallelPerProject(s)
	isLead := false
	if a, aerr := s.Agent(agent); aerr == nil && a != nil && a.Kind == leadKind {
		isLead = true
	}
	err = immediate(s, func(q querier) error {
		for _, r := range mustRows(q, "SELECT id FROM jobs WHERE status = 'leased' AND lease_until != '' AND lease_until < ?", now) {
			exec1(q, "UPDATE jobs SET status = 'queued', lease_agent = '', lease_until = '' WHERE id = ?", r.str("id"))
			eventAt(q, now, "lease", "job.requeue", r.str("id"), "lease expired")
		}
		sql := "SELECT j.* FROM jobs j JOIN projects p ON p.slug = j.project WHERE j.status = 'queued' AND p.status = 'active' " +
			"AND (SELECT COUNT(*) FROM jobs k WHERE k.project = j.project AND " + occupiedJobSQL + ") < ?"
		args := []any{limit}
		if o.Project != "" {
			sql += " AND j.project = ?"
			args = append(args, o.Project)
		}
		if o.Projects != nil {
			marks := make([]string, len(o.Projects))
			for i, p := range o.Projects {
				marks[i] = "?"
				args = append(args, p)
			}
			sql += " AND j.project IN (" + strings.Join(marks, ",") + ")"
		}
		if o.JobID != "" {
			sql += " AND j.id = ?"
			args = append(args, o.JobID)
		}
		if isLead {
			sql += " AND j.kind = ? AND (j.project || '-lead') = ?"
			args = append(args, leadKind, agent)
		} else {
			sql += " AND j.kind != ?"
			args = append(args, leadKind)
		}
		// A lease is a dispatch reservation even before its goroutine records run.start.
		sql += " ORDER BY MAX(COALESCE((SELECT MAX(r.started_at) FROM runs r WHERE r.project = j.project), '')," +
			" COALESCE((SELECT MAX(e.at) FROM jobs k JOIN events e ON e.subject = k.id AND e.action = 'job.lease'" +
			" WHERE k.project = j.project AND k.status = 'leased'), '')) ASC," +
			" j.priority DESC, j.created_at LIMIT 1"
		r := mustOne(q, sql, args...)
		if r == nil {
			return nil
		}
		if held := mustOne(q, "SELECT k.id FROM jobs k WHERE k.lease_agent = ? AND "+occupiedJobSQL+" LIMIT 1", agent); held != nil {
			return fail(2, "Agent %s is already working on job %s; wait for its run or lease to finish, or choose another agent", agent, held.str("id"))
		}
		until := plusMinutes(now, float64(minutes))
		exec1(q, "UPDATE jobs SET status = 'leased', lease_agent = ?, lease_until = ?, generation = generation + 1, attempts = attempts + 1 "+
			"WHERE id = ? AND status = 'queued'", agent, until, r.str("id"))
		eventAt(q, now, agent, "job.lease", r.str("id"), map[string]any{"until": until})
		job = mustOne(q, "SELECT * FROM jobs WHERE id = ?", r.str("id"))
		return nil
	})
	return job, err
}

// finishJob ends (or requeues) a job through board.FinishJob; generation < 0
// means "not checked".
func finishJob(s *store.Store, jobID, agent, status, result, branch string, generation int) (row, error) {
	var gen *int
	if generation >= 0 {
		gen = &generation
	}
	j, err := board.FinishJob(s, jobID, agent, status, result, branch, gen)
	if err != nil {
		return nil, err
	}
	return jobRow(s.DB, j.ID), nil
}

func renewLease(q querier, now, jobID, agent string, generation, minutes int) bool {
	until := plusMinutes(now, float64(minutes))
	res, err := q.Exec("UPDATE jobs SET lease_until = ? WHERE id = ? AND status = 'leased' AND lease_agent = ? AND generation = ?",
		until, jobID, agent, generation)
	if err != nil {
		return false
	}
	if n, _ := res.RowsAffected(); n != 1 {
		return false
	}
	eventAt(q, now, agent, "job.renew", jobID, map[string]any{"until": until})
	return true
}
