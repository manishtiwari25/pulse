package inspect

import (
	"bytes"
	"context"
	"errors"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/testutil"
)

type Signer struct {
	Agent         string `json:"agent"`
	SigningPubkey string `json:"signing_pubkey"`
	ID            string `json:"id"`
	JobID         string `json:"job_id"`
	Project       string `json:"project"`
}

func (s Signer) Line() string {
	return fmt.Sprintf("%s@pulse.local namespaces=\"git\" %s %s/%s", s.Agent, s.SigningPubkey, s.Project, s.ID)
}

func Signers(s *store.Store, project string) ([]Signer, error) {
	q := "SELECT agent, signing_pubkey, id, job_id, project FROM runs WHERE signing_pubkey != ''"
	var params []any
	if project != "" {
		q += " AND project = ?"
		params = append(params, project)
	}
	rows, err := s.DB.Query(q+" ORDER BY started_at", params...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Signer{}
	for rows.Next() {
		var signer Signer
		if err := rows.Scan(&signer.Agent, &signer.SigningPubkey, &signer.ID, &signer.JobID, &signer.Project); err != nil {
			return nil, err
		}
		out = append(out, signer)
	}
	return out, rows.Err()
}

func runSigners(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if err := noArgs(args); err != nil {
		return err
	}
	signers, err := Signers(c.Store, c.Project)
	if err != nil {
		return err
	}
	if c.JSON {
		return jsonOutput(c, signers)
	}
	if len(signers) == 0 {
		fmt.Fprintln(c.Err, "# no run has a signing key yet (ADR-022); runs made before it carry none")
	}
	for _, signer := range signers {
		c.Println(signer.Line())
	}
	return nil
}

type VerifiedRun struct {
	ID        string `json:"id"`
	Agent     string `json:"agent"`
	Executor  string `json:"executor"`
	Status    string `json:"status"`
	StartedAt string `json:"started_at"`
	Signature string `json:"signature"`
}

type Verification struct {
	Commit        string        `json:"commit"`
	Author        string        `json:"author"`
	AuthorEmail   string        `json:"author_email"`
	PulseAgent    string        `json:"pulse_agent"`
	PulseJob      string        `json:"pulse_job"`
	PulseExecutor string        `json:"pulse_executor"`
	Runs          []VerifiedRun `json:"runs"`
	Verified      bool          `json:"verified"`
	Verdict       string        `json:"verdict"`
}

func (v *Verification) Text() string {
	var out strings.Builder
	fmt.Fprintf(&out, "commit    %s\nauthor    %s <%s>\ntrailers  Pulse-Agent: %s  Pulse-Job: %s  Pulse-Executor: %s\n",
		v.Commit, v.Author, v.AuthorEmail, orDash(v.PulseAgent), orDash(v.PulseJob), orDash(v.PulseExecutor))
	for _, r := range v.Runs {
		fmt.Fprintf(&out, "run       %s  agent %s  executor %s  status %s  started %s  signature %s\n",
			r.ID, r.Agent, r.Executor, r.Status, r.StartedAt, r.Signature)
	}
	fmt.Fprintf(&out, "verdict   %s\n", v.Verdict)
	return out.String()
}

func gitOutput(root string, args ...string) (string, string, int, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", append([]string{"--no-pager", "-C", root}, args...)...)
	// `pulse verify` runs inside git hooks (post-commit, pre-push); scrub the
	// hook's GIT_DIR/GIT_INDEX_FILE so git inspects root, not the hook's repo.
	cmd.Env = testutil.StripHookGitEnv(os.Environ())
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	err := cmd.Run()
	if ctx.Err() != nil {
		return "", "", 0, fmt.Errorf("git in %s: %w", root, ctx.Err())
	}
	var exit *exec.ExitError
	if errors.As(err, &exit) {
		return stdout.String(), stderr.String(), exit.ExitCode(), nil
	}
	if err != nil {
		return "", "", 0, err
	}
	return stdout.String(), stderr.String(), 0, nil
}

// VerifyCommit verifies a commit with one run key at a time. The temporary
// allowed-signers file lives outside the store and is unique to this call.
func VerifyCommit(s *store.Store, project *store.Project, ref string) (result *Verification, err error) {
	sha, _, code, err := gitOutput(project.Path, "rev-parse", "--verify", "--quiet", "--end-of-options", ref+"^{commit}")
	if err != nil {
		return nil, err
	}
	if code != 0 {
		return nil, cli.Errorf(2, fmt.Sprintf("pulse: no commit %s in %s", pyRepr(ref), project.Path), "", "")
	}
	sha = strings.TrimSpace(sha)
	format := "%an%x1f%ae%x1f%(trailers:key=Pulse-Agent,valueonly,separator=%x1d)%x1f%(trailers:key=Pulse-Job,valueonly,separator=%x1d)%x1f%(trailers:key=Pulse-Executor,valueonly,separator=%x1d)"
	out, stderr, code, err := gitOutput(project.Path, "log", "-1", "--format="+format, sha)
	if err != nil {
		return nil, err
	}
	if code != 0 {
		return nil, fmt.Errorf("git log failed in %s: %s", project.Path, strings.TrimSpace(stderr))
	}
	fields := append(strings.Split(out, "\x1f"), "", "", "", "", "")
	for i := range 5 {
		fields[i] = strings.ReplaceAll(strings.TrimSpace(fields[i]), "\x1d", ", ")
	}
	v := &Verification{Commit: sha, Author: fields[0], AuthorEmail: fields[1], PulseAgent: fields[2],
		PulseJob: fields[3], PulseExecutor: fields[4], Runs: []VerifiedRun{}}
	if v.PulseJob == "" {
		v.Verdict = "NOT verified: no Pulse-Job trailer - not a PULSE worker commit (a human's, or made before ADR-022)"
		return v, nil
	}
	rows, err := s.DB.Query(`SELECT id, agent, executor, status, started_at, signing_pubkey FROM runs
		WHERE job_id = ? AND signing_pubkey != '' ORDER BY started_at DESC`, v.PulseJob)
	if err != nil {
		return nil, err
	}
	type candidate struct {
		run VerifiedRun
		key string
	}
	candidates := []candidate{}
	for rows.Next() {
		var c candidate
		if err := rows.Scan(&c.run.ID, &c.run.Agent, &c.run.Executor, &c.run.Status, &c.run.StartedAt, &c.key); err != nil {
			rows.Close()
			return nil, err
		}
		candidates = append(candidates, c)
	}
	err = rows.Err()
	rows.Close()
	if err != nil {
		return nil, err
	}
	if len(candidates) == 0 {
		v.Verdict = "NOT verified: the store has no run with a key for job " + v.PulseJob
		return v, nil
	}
	file, err := os.CreateTemp("", "pulse-verify-*.signers")
	if err != nil {
		return nil, err
	}
	path := file.Name()
	defer func() {
		if removeErr := os.Remove(path); removeErr != nil {
			err = errors.Join(err, fmt.Errorf("remove temporary signers file: %w", removeErr))
		}
	}()
	if err := file.Close(); err != nil {
		return nil, err
	}
	for _, c := range candidates {
		line := fmt.Sprintf("%s@pulse.local namespaces=\"git\" %s\n", c.run.Agent, c.key)
		if err := os.WriteFile(path, []byte(line), 0600); err != nil {
			return nil, err
		}
		signature, _, code, err := gitOutput(project.Path, "-c", "gpg.ssh.program=ssh-keygen",
			"-c", "gpg.ssh.allowedSignersFile="+path, "log", "-1", "--format=%G?", sha)
		if err != nil {
			return nil, err
		}
		if code != 0 {
			return nil, fmt.Errorf("git signature inspection failed in %s (exit %d)", project.Path, code)
		}
		c.run.Signature = strings.TrimSpace(signature)
		if c.run.Signature == "" {
			c.run.Signature = "?"
		}
		v.Runs = append(v.Runs, c.run)
		if c.run.Signature == "G" && v.Author == c.run.Agent && v.AuthorEmail == c.run.Agent+"@pulse.local" && v.PulseAgent == c.run.Agent {
			v.Verified = true
			v.Verdict = fmt.Sprintf("verified: signed by the key of run %s, authored by %s, job %s on %s", c.run.ID, c.run.Agent, v.PulseJob, c.run.Executor)
			return v, nil
		}
	}
	v.Verdict = "NOT verified: no run of job " + v.PulseJob + " signed this commit with its key as its agent"
	return v, nil
}

func runVerify(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if len(args) != 1 {
		return cli.Errorf(2, "pulse: verify needs a ref", "", "")
	}
	project, err := currentProject(c)
	if err != nil {
		return err
	}
	report, err := VerifyCommit(c.Store, project, args[0])
	if err != nil {
		return err
	}
	if c.JSON {
		if err := jsonOutput(c, report); err != nil {
			return err
		}
	} else {
		c.Printf("%s", report.Text())
	}
	if !report.Verified {
		return &cli.Error{Code: 1}
	}
	return nil
}
