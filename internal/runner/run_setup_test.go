package runner

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

func TestRunJobRecipeSetupRunsOnceBeforeWorkerAndVerify(t *testing.T) {
	if _, err := exec.LookPath("ssh-keygen"); err != nil {
		t.Skip("no ssh-keygen")
	}
	s := newStore(t)
	proj, _ := newRepo(t, s)
	join(t, s, "aurora")

	// Keep the trace outside the clone so runJob's cleanup cannot erase it.
	markerPath := filepath.Join(s.Home, "setup-order")
	marker := shellQuote(markerPath)
	recipe := recipeWith(nil, []string{`printf 'setup\n' >> ` + marker}, nil, nil, "")
	if id, _, _, err := envSet(s, proj, recipe, humanAgent, "store", "setup ordering regression"); err != nil || id != "CTX-environment" {
		t.Fatalf("set recipe: %s %v", id, err)
	}
	verifyCmd := `test "$(cat ` + marker + `)" = "$(printf 'setup\nworker')" && printf 'verify\n' >> ` + marker
	exec1(s.DB, "UPDATE projects SET verify_cmd = ? WHERE slug = ?", verifyCmd, proj.str("slug"))

	job, _, err := addJob(s, "proj", "task", "setup ordering", "add a worker marker", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	job, err = leaseJob(s, "aurora", testNow, leaseOpts{JobID: job.str("id")})
	if err != nil || job == nil {
		t.Fatalf("lease: %v %v", job, err)
	}
	worker := `test "$(cat ` + marker + `)" = setup &&
printf 'worker\n' >> ` + marker + ` &&
printf 'worker\n' > worker.txt &&
git add worker.txt &&
git commit -q -m 'add worker marker' &&
echo 'SUMMARY: setup ran once before worker'`
	r, err := runJob(s, job, "aurora", testNow, runOpts{WorkerCmd: "sh -c " + shellQuote(worker)})
	if err != nil || r == nil {
		t.Fatalf("run: %v %v", r, err)
	}
	if r.str("status") != "done" || r.int("commits") != 1 || r.str("verify") != "verify exit 0" {
		t.Fatalf("run: %v", r)
	}
	got, err := os.ReadFile(markerPath)
	if err != nil {
		t.Fatal(err)
	}
	if want := "setup\nworker\nverify\n"; string(got) != want {
		t.Fatalf("stage trace = %q, want %q", got, want)
	}
}
