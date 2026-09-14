package runner

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/codedelivery"
	"github.com/manishtiwari25/pulse/internal/store"
)

const landingEvidenceMaxBytes = 2 << 20

// An operation-specific reflog entry proves the old tip; sampling HEAD around
// merge does not, because another writer can advance it between the samples.
func landingRefTransition(ctx context.Context, root, ref, target, action string) (string, string) {
	raw, reason := limitedGitOutput(ctx, root, 10*time.Second, 64<<10,
		"reflog", "show", "--max-count=32", "--format=%H%x00%gs%x00", ref)
	if reason != "" {
		return "", ""
	}
	fields := strings.Split(string(raw), "\x00")
	match := -1
	for i := 0; i+1 < len(fields); i += 2 {
		if strings.HasPrefix(fields[i+1], action+": ") {
			if match >= 0 {
				return "", ""
			}
			match = i
		}
	}
	if match < 0 || match+3 >= len(fields) {
		return "", ""
	}
	after := strings.TrimSpace(fields[match])
	before := strings.TrimSpace(fields[match+2])
	if after != target || !landingObjectID(before) {
		return "", ""
	}
	return before, after
}

func landingObjectID(value string) bool {
	if len(value) != 40 && len(value) != 64 {
		return false
	}
	for _, c := range value {
		if !strings.ContainsRune("0123456789abcdef", c) {
			return false
		}
	}
	return true
}

func experimentArchive(s *store.Store, project, branch, run string) (row, error) {
	if run == "" {
		return nil, nil
	}
	return queryOne(s.DB, `SELECT id, note, kept FROM evolution_archive
		WHERE project = ? AND branch = ? AND run_id = ?
		ORDER BY at DESC, CAST(SUBSTR(id, 3) AS INTEGER) DESC LIMIT 1`, project, branch, run)
}

func landingCodeDeliveryEvidence(ctx context.Context, root, before, after string) codedelivery.Evidence {
	if before == "" || after == "" {
		return codedelivery.UnavailableEvidence(before, after, codedelivery.ReasonMissingTransition)
	}
	if before == after {
		evidence, err := codedelivery.SummarizeNameStatus(nil, before, after, codedelivery.DefaultCodePathLimit, codedelivery.DefaultFileLimit)
		if err != nil {
			return codedelivery.UnavailableEvidence(before, after, codedelivery.ReasonEvidenceMalformed)
		}
		return evidence
	}
	nameStatus, reason := limitedGitOutput(ctx, root, 60*time.Second, landingEvidenceMaxBytes,
		"diff", "--name-status", "-z", "--find-renames", before+".."+after)
	if reason != "" {
		return codedelivery.UnavailableEvidence(before, after, reason)
	}
	numstat, reason := limitedGitOutput(ctx, root, 60*time.Second, landingEvidenceMaxBytes,
		"diff", "--numstat", "-z", "--no-renames", before+".."+after)
	if reason != "" {
		return codedelivery.UnavailableEvidence(before, after, reason)
	}
	binaries, truncated, err := codedelivery.ParseNumstatBinaryPaths(numstat, codedelivery.DefaultFileLimit)
	if err != nil {
		return codedelivery.UnavailableEvidence(before, after, codedelivery.ReasonEvidenceMalformed)
	}
	if truncated {
		return codedelivery.UnavailableEvidence(before, after, codedelivery.ReasonEvidenceTooLarge)
	}
	evidence, err := codedelivery.SummarizeNameStatus(nameStatus, before, after, codedelivery.DefaultCodePathLimit, codedelivery.DefaultFileLimit, binaries)
	if err != nil {
		return codedelivery.UnavailableEvidence(before, after, codedelivery.ReasonEvidenceMalformed)
	}
	if evidence.ChangesTruncated {
		return codedelivery.UnavailableEvidence(before, after, codedelivery.ReasonEvidenceTooLarge)
	}
	return evidence
}

type cappedBuffer struct {
	buffer   bytes.Buffer
	limit    int
	exceeded bool
	cancel   context.CancelFunc
}

func (b *cappedBuffer) Write(p []byte) (int, error) {
	space := b.limit - b.buffer.Len()
	if space > 0 {
		if len(p) <= space {
			_, _ = b.buffer.Write(p)
			return len(p), nil
		}
		_, _ = b.buffer.Write(p[:space])
	}
	b.exceeded = true
	if b.cancel != nil {
		b.cancel()
	}
	return max(space, 0), errLandingEvidenceTooLarge
}

var errLandingEvidenceTooLarge = errors.New("landing evidence too large")

func limitedGitOutput(ctx context.Context, repo string, timeout time.Duration, maxBytes int, args ...string) ([]byte, string) {
	if ctx == nil {
		ctx = context.Background()
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	if repo != "" {
		args = append([]string{"-C", repo}, args...)
	}
	cmd := exec.CommandContext(ctx, "git", args...)
	cmd.Env = append(os.Environ(), "GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0")
	out := cappedBuffer{limit: maxBytes, cancel: cancel}
	errOut := cappedBuffer{limit: 4096, cancel: cancel}
	cmd.Stdout = &out
	cmd.Stderr = &errOut
	err := cmd.Run()
	if out.exceeded || errOut.exceeded || errors.Is(err, errLandingEvidenceTooLarge) {
		return nil, codedelivery.ReasonEvidenceTooLarge
	}
	if ctx.Err() != nil {
		return nil, codedelivery.ReasonGitUnavailable
	}
	if err != nil {
		return nil, codedelivery.ReasonGitUnavailable
	}
	return out.buffer.Bytes(), ""
}

func landingWorkSnapshot(s *store.Store, project, job row, o landOpts) codedelivery.WorkSnapshot {
	if job == nil {
		return codedelivery.WorkSnapshot{}
	}
	work := codedelivery.WorkSnapshot{
		Job: &codedelivery.JobSnapshot{
			ID: job.str("id"), Title: job.str("title"), Kind: job.str("kind"),
			Status: job.str("status"), Source: job.str("source"), Generation: job.int("generation"),
		},
	}
	worker := codedelivery.WorkerSnapshot{Name: job.str("lease_agent"), RunID: o.RunID}
	if worker.RunID == "" {
		if row, err := queryOne(s.DB, `SELECT id, agent, executor FROM runs
			WHERE job_id = ? AND project = ? AND status = 'done'
			ORDER BY finished_at DESC, started_at DESC, id DESC LIMIT 1`, job.str("id"), project.str("slug")); err == nil && row != nil {
			worker.RunID = row.str("id")
			if worker.Name == "" {
				worker.Name = row.str("agent")
			}
			worker.Executor = row.str("executor")
		}
	}
	if worker.Name != "" {
		if agent, err := s.Agent(worker.Name); err == nil && agent != nil {
			worker.Runner, worker.Model = agent.Runner, agent.Model
		}
		work.Worker = &worker
	}
	return work
}

// Only the short final state change is serialized, never git or verification.
// A reader cannot observe branch clearing without its matching landing receipt.
func recordLanding(s *store.Store, project row, branch, actor, action, result string, o landOpts, detail map[string]any) error {
	tx, err := s.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	job := o.Job
	detail["project"], detail["job"] = project.str("slug"), jobIDOf(job)
	if o.ArchiveID != "" {
		if job == nil || o.RunID == "" {
			return fmt.Errorf("cannot finalize archive %s without its experiment run", o.ArchiveID)
		}
		var current bool
		kept := action == "land.landed" || action == "land.merged"
		err := tx.QueryRow(`SELECT EXISTS (
			SELECT 1 FROM evolution_archive a JOIN runs r ON r.id = a.run_id AND r.project = a.project
			JOIN jobs j ON j.id = r.job_id AND j.project = a.project
			WHERE a.project = ? AND a.id = ? AND a.branch = ? AND a.run_id = ? AND a.kept = ?
			AND j.id = ? AND j.kind = 'experiment' AND j.status = 'done' AND r.status = 'done'
			AND r.id = (SELECT latest.id FROM runs latest WHERE latest.job_id = j.id
				ORDER BY latest.started_at DESC, latest.id DESC LIMIT 1)
			AND NOT EXISTS (SELECT 1 FROM runs active WHERE active.job_id = j.id AND active.status = 'running'))`,
			project.str("slug"), o.ArchiveID, branch, o.RunID, kept, job.str("id")).Scan(&current)
		if err != nil {
			return err
		}
		if !current {
			return fmt.Errorf("experiment %s changed before archive %s could be finalized; owner review needed", job.str("id"), o.ArchiveID)
		}
		detail["archive"], detail["run_id"] = o.ArchiveID, o.RunID
	}
	if job != nil {
		changed, err := tx.Exec(`UPDATE jobs SET branch = '', result = result || ?
			WHERE id = ? AND branch = ? AND generation = ?`, result, job.str("id"), branch, job.int("generation"))
		if err != nil {
			return err
		}
		n, err := changed.RowsAffected()
		if err != nil {
			return err
		}
		if n != 1 {
			return fmt.Errorf("job %s changed before landing finalization; owner review needed", job.str("id"))
		}
	}
	if err := tx.Event(actor, action, branch, detail); err != nil {
		return err
	}
	return tx.Commit()
}

func failMissingExperiment(s *store.Store, project, job row, actor, run, archive, reason string) error {
	tx, err := s.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	changed, err := tx.Exec(`UPDATE jobs SET status = 'failed', branch = '', finished_at = ?, lease_until = '',
		result = result || ? WHERE id = ? AND status = 'done' AND branch = ? AND generation = ?
		AND COALESCE((SELECT r.id FROM runs r WHERE r.job_id = jobs.id
			ORDER BY r.started_at DESC, r.id DESC LIMIT 1), '') = ?
		AND NOT EXISTS (SELECT 1 FROM runs active WHERE active.job_id = jobs.id AND active.status = 'running')`,
		s.Now(), " | "+reason, job.str("id"), job.str("branch"), job.int("generation"), run)
	if err != nil {
		return err
	}
	n, err := changed.RowsAffected()
	if err != nil {
		return err
	}
	if n != 1 {
		return fmt.Errorf("experiment %s changed before its missing artifact could be recorded", job.str("id"))
	}
	detail := map[string]any{"project": project.str("slug"), "job": job.str("id"),
		"run_id": run, "archive": archive, "result": reason}
	if err := tx.Event(actor, "job.failed", job.str("id"), detail); err != nil {
		return err
	}
	if err := tx.Event(actor, "land.missing", job.str("branch"), detail); err != nil {
		return err
	}
	return tx.Commit()
}
