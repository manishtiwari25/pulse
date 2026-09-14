package runner

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/codedelivery"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestLandingRecordsCodeDeliveryEvidenceAndWorkSnapshot(t *testing.T) {
	s := newStore(t)
	project, root := newRepo(t, s)
	join(t, s, "aurora")
	writeFile(t, root, "internal/old.go", "package internal\n")
	writeFile(t, root, "cmd/deleted/main.go", "package main\n")
	run(t, root, "git", "add", "-A")
	run(t, root, "git", "commit", "-q", "-m", "seed source files")
	before := headOf(t, root)

	branch := "pulse/aurora/code-delivery"
	run(t, root, "git", "switch", "-q", "-c", branch)
	run(t, root, "git", "mv", "internal/old.go", "internal/new.go")
	if err := os.Remove(filepath.Join(root, "cmd/deleted/main.go")); err != nil {
		t.Fatal(err)
	}
	writeFile(t, root, "web/hub/src/card.css", ".card { color: green; }\n")
	writeFile(t, root, "web/hub/out/index.js", "generated();\n")
	writeFile(t, root, "README.md", "# proj\n\nDocs.\n")
	run(t, root, "git", "add", "-A")
	run(t, root, "git", "commit", "-q", "-m", "deliver source and docs")
	run(t, root, "git", "switch", "-q", "main")

	job, _, err := addJob(s, "proj", "task", "Deliver source", "do it", 1, 0, "task:TASK-001", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', branch = ?, lease_agent = ? WHERE id = ?", branch, "aurora", job.str("id"))
	exec1(s.DB, `INSERT INTO runs(id, job_id, agent, project, started_at, finished_at, status, executor)
		VALUES('run-code-delivery', ?, 'aurora', 'proj', ?, ?, 'done', 'local')`, job.str("id"), testNow, testNow)
	state, note, err := landBranch(s, project, branch, "daemon", testNow, landOpts{Job: jobRow(s.DB, job.str("id")), Verify: false})
	if err != nil || state != "landed" {
		t.Fatalf("landBranch = %s %s %v", state, note, err)
	}

	var detail map[string]any
	if err := json.Unmarshal([]byte(mustRows(s.DB, "SELECT detail FROM events WHERE action = 'land.landed'")[0].str("detail")), &detail); err != nil {
		t.Fatal(err)
	}
	evidence, ok := codedelivery.DecodeEvidence(detail["code_delivery"])
	if !ok || evidence.Evidence != codedelivery.EvidenceRecorded || evidence.Before != before || evidence.After != headOf(t, root) {
		t.Fatalf("evidence missing or wrong: ok=%t %+v detail=%v", ok, evidence, detail)
	}
	if evidence.FilesChanged != 5 || evidence.CodeFiles != 3 || evidence.DocsFiles != 1 || evidence.NonCodeFiles != 1 {
		t.Fatalf("evidence counts: %+v", evidence)
	}
	if got := strings.Join(evidence.CodePaths, ","); got != "cmd/deleted/main.go,internal/new.go,web/hub/src/card.css" {
		t.Fatalf("code paths = %s", got)
	}
	work := codedelivery.DecodeWorkSnapshot(detail["work"])
	if work.Job == nil || work.Job.ID != job.str("id") || work.Job.Source != "task:TASK-001" || work.Worker == nil ||
		work.Worker.Name != "aurora" || work.Worker.Runner != "r" || work.Worker.Model != "m" || work.Worker.RunID != "run-code-delivery" ||
		work.Worker.Executor != "local" {
		t.Fatalf("work snapshot: %+v", work)
	}
}

func TestLandingReceiptFailureAfterEvidenceKeepsCarrier(t *testing.T) {
	s := newStore(t)
	project, root := newRepo(t, s)
	join(t, s, "aurora")
	branch := "pulse/aurora/receipt-failure"
	run(t, root, "git", "switch", "-q", "-c", branch)
	writeFile(t, root, "internal/api.go", "package internal\n")
	run(t, root, "git", "add", "-A")
	run(t, root, "git", "commit", "-q", "-m", "source change")
	run(t, root, "git", "switch", "-q", "main")
	job, _, err := addJob(s, "proj", "task", "Deliver source", "do it", 1, 0, "manual", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', branch = ?, lease_agent = ? WHERE id = ?", branch, "aurora", job.str("id"))
	exec1(s.DB, `CREATE TRIGGER reject_code_delivery_receipt BEFORE INSERT ON events
		WHEN NEW.action = 'land.landed' BEGIN SELECT RAISE(ABORT, 'receipt refused'); END`)

	state, note, err := landBranch(s, project, branch, "daemon", testNow, landOpts{Job: jobRow(s.DB, job.str("id")), Verify: false})
	if err == nil || state != "" || !strings.Contains(err.Error(), "receipt refused") {
		t.Fatalf("receipt failure hidden: state=%q note=%q err=%v", state, note, err)
	}
	current := jobRow(s.DB, job.str("id"))
	if current.str("branch") != branch || current.str("status") != "done" {
		t.Fatalf("failed receipt cleared carrier: %v", current)
	}
	if exists, why := branchExists(root, branch); !exists || why != "" {
		t.Fatalf("failed receipt removed branch: exists=%t why=%q", exists, why)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed'"); n != 0 {
		t.Fatalf("failed receipt left success event: %d", n)
	}
}

func TestLandingEvidenceUsesActualAdvancedDefaultTransition(t *testing.T) {
	s := newStore(t)
	project, root := newRepo(t, s)
	join(t, s, "aurora")
	branch := "pulse/aurora/advanced-default"
	run(t, root, "git", "switch", "-q", "-c", branch)
	writeFile(t, root, "a.go", "package main\n")
	run(t, root, "git", "add", "-A")
	run(t, root, "git", "commit", "-q", "-m", "add a")
	first := headOf(t, root)
	writeFile(t, root, "b.go", "package main\n")
	run(t, root, "git", "add", "-A")
	run(t, root, "git", "commit", "-q", "-m", "add b")
	run(t, root, "git", "switch", "-q", "main")
	job, _, err := addJob(s, "proj", "task", "Deliver source", "do it", 1, 0, "manual", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', branch = ?, lease_agent = ? WHERE id = ?", branch, "aurora", job.str("id"))
	project["verify_cmd"] = "git -C " + shellQuote(root) + " merge --ff-only " + shellQuote(first)

	state, note, err := landBranch(s, project, branch, "daemon", testNow, landOpts{Job: jobRow(s.DB, job.str("id")), Verify: true})
	if err != nil || state != "landed" {
		t.Fatalf("landBranch = %s %s %v", state, note, err)
	}
	evidence := landedEvidence(t, s)
	if evidence.Before != first || evidence.After != headOf(t, root) || evidence.CodeFiles != 1 || strings.Join(evidence.CodePaths, ",") != "b.go" {
		t.Fatalf("stale pre-verification evidence was recorded: %+v", evidence)
	}
}

func TestLandingEvidenceRecordsNoCodeWhenDefaultAlreadyAtTarget(t *testing.T) {
	s := newStore(t)
	project, root := newRepo(t, s)
	join(t, s, "aurora")
	branch := "pulse/aurora/no-op-default"
	run(t, root, "git", "switch", "-q", "-c", branch)
	writeFile(t, root, "a.go", "package main\n")
	writeFile(t, root, "b.go", "package main\n")
	run(t, root, "git", "add", "-A")
	run(t, root, "git", "commit", "-q", "-m", "add files")
	target := headOf(t, root)
	run(t, root, "git", "switch", "-q", "main")
	job, _, err := addJob(s, "proj", "task", "Deliver source", "do it", 1, 0, "manual", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', branch = ?, lease_agent = ? WHERE id = ?", branch, "aurora", job.str("id"))
	project["verify_cmd"] = "git -C " + shellQuote(root) + " merge --ff-only " + shellQuote(target)

	state, note, err := landBranch(s, project, branch, "daemon", testNow, landOpts{Job: jobRow(s.DB, job.str("id")), Verify: true})
	if err != nil || state != "landed" {
		t.Fatalf("landBranch = %s %s %v", state, note, err)
	}
	evidence := landedEvidence(t, s)
	if evidence.Evidence != codedelivery.EvidenceUnavailable || evidence.Reason != codedelivery.ReasonMissingTransition ||
		evidence.FilesChanged != 0 || evidence.CodeFiles != 0 || len(evidence.CodePaths) != 0 {
		t.Fatalf("no-op transition counted stale code: %+v", evidence)
	}
}

func landedEvidence(t *testing.T, s *store.Store) codedelivery.Evidence {
	t.Helper()
	var detail map[string]any
	if err := json.Unmarshal([]byte(mustRows(s.DB, "SELECT detail FROM events WHERE action = 'land.landed' ORDER BY seq DESC LIMIT 1")[0].str("detail")), &detail); err != nil {
		t.Fatal(err)
	}
	evidence, ok := codedelivery.DecodeEvidence(detail["code_delivery"])
	if !ok {
		t.Fatalf("landing detail has no code_delivery: %#v", detail)
	}
	return evidence
}

func writeFile(t *testing.T, root, rel, text string) {
	t.Helper()
	path := filepath.Join(root, rel)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
}
