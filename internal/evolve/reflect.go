package evolve

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

type WorkingState struct {
	Branch  string   `json:"branch"`
	Head    string   `json:"head"`
	Changes []string `json:"changes"`
	Problem string   `json:"problem,omitempty"`
}

type Hygiene struct {
	Branches  []string `json:"branches"`
	Worktrees []string `json:"worktrees"`
	Clones    []string `json:"clones"`
	Problems  []string `json:"problems"`
}

type Reflection struct {
	Project       string             `json:"project"`
	Working       WorkingState       `json:"working"`
	Board         string             `json:"board"`
	Ballot        []tally.BallotItem `json:"ballot"`
	Proposals     []tally.Proposal   `json:"proposals"`
	Gates         *LoopStatus        `json:"gates,omitempty"`
	Hygiene       Hygiene            `json:"hygiene"`
	VerifyCmd     string             `json:"verify_cmd"`
	Level         string             `json:"level"`
	NextHumanStep string             `json:"next_human_step"`
	NextCommands  []string           `json:"next_commands"`
	Problems      []string           `json:"problems"`
}

func lines(text string) []string {
	out := []string{}
	for _, line := range strings.Split(text, "\n") {
		if strings.TrimSpace(line) != "" {
			out = append(out, line)
		}
	}
	return out
}

func workingState(ctx context.Context, repo string) WorkingState {
	state := WorkingState{Changes: []string{}}
	head, err := gitOutput(ctx, repo, "rev-parse", "--short", "HEAD")
	if err != nil {
		state.Problem = "checkout is unavailable or has no committed HEAD"
		return state
	}
	state.Head = head
	state.Branch, err = gitOutput(ctx, repo, "symbolic-ref", "--short", "--quiet", "HEAD")
	if err != nil {
		state.Branch = "detached"
	}
	status, err := gitOutput(ctx, repo, "status", "--short", "--untracked-files=normal")
	if err != nil {
		state.Problem = "git status could not inspect the working tree"
	} else {
		state.Changes = lines(status)
	}
	return state
}

func hygiene(ctx context.Context, s *store.Store, repo string) Hygiene {
	h := Hygiene{Branches: []string{}, Worktrees: []string{}, Clones: []string{}, Problems: []string{}}
	branches, err := gitOutput(ctx, repo, "for-each-ref", "--format=%(refname:short)", "refs/heads/evolve/", "refs/heads/pulse/")
	if err != nil {
		h.Problems = append(h.Problems, "cannot inspect stray experiment branches")
	} else {
		h.Branches = lines(branches)
	}
	worktrees, err := gitOutput(ctx, repo, "worktree", "list", "--porcelain")
	if err != nil {
		h.Problems = append(h.Problems, "cannot inspect worktrees")
	} else {
		for _, line := range strings.Split(worktrees, "\n") {
			if value, ok := strings.CutPrefix(line, "worktree "); ok {
				h.Worktrees = append(h.Worktrees, value)
			}
		}
	}
	if s != nil {
		root, err := fleetRoot(s, false)
		if err != nil {
			h.Problems = append(h.Problems, "fleet location does not pass boundary checks")
			return h
		}
		entries, err := os.ReadDir(root)
		if err != nil && !errors.Is(err, os.ErrNotExist) {
			h.Problems = append(h.Problems, "cannot inspect managed clone directories")
		}
		for _, entry := range entries {
			if entry.IsDir() && managedID.MatchString(entry.Name()) {
				h.Clones = append(h.Clones, filepath.Join(root, entry.Name()))
			}
		}
	}
	return h
}

// Reflect is read-only, including health checks: it names verify_cmd but never
// runs project-defined code. Missing setup lowers the ladder instead of hiding it.
func Reflect(ctx context.Context, s *store.Store, project, area, agent, session string) (*Reflection, error) {
	r := &Reflection{
		Project: project, Level: "A", NextHumanStep: "pulse init PATH",
		Board:  "NOTICES\n  no PULSE store or registered project; observation is still available",
		Ballot: []tally.BallotItem{}, Proposals: []tally.Proposal{}, Problems: []string{}, NextCommands: []string{},
	}
	repo := store.Cwd()
	if s == nil {
		r.Working, r.Hygiene = workingState(ctx, repo), hygiene(ctx, nil, repo)
		r.NextCommands = []string{"pulse init PATH", "pulse reflect --project P"}
		return r, nil
	}
	p, err := board.CurrentProject(s, project)
	if err != nil {
		r.Problems = append(r.Problems, "select a registered project with --project")
		r.NextHumanStep = "pulse projects"
		r.Working, r.Hygiene = workingState(ctx, repo), hygiene(ctx, s, repo)
		r.NextCommands = []string{"pulse projects", "pulse reflect --project P"}
		return r, nil
	}
	if p == nil || p.Slug == store.CommonsSlug {
		return nil, inputError("Reflection needs a project checkout", "use the commons board separately", "pulse projects")
	}
	r.Project, repo, r.VerifyCmd = p.Slug, p.Path, p.VerifyCmd
	r.Working, r.Hygiene = workingState(ctx, repo), hygiene(ctx, s, repo)
	a, err := board.ResolveAgent(s, agent, false)
	if err != nil {
		return nil, err
	}
	if a != nil {
		agent = a.Name
		session = board.AgentSession(a, session)
	} else {
		agent = ""
	}
	r.Board, err = tally.DigestFor(s, p.Slug, area, tally.Thresholds{}, agent, session)
	if err != nil {
		return nil, err
	}
	r.Ballot, err = tally.Ballot(s, p.Slug, agent, session, tally.Thresholds{})
	if err != nil {
		return nil, err
	}
	r.Proposals, err = tally.Proposals(s, p.Slug)
	if err != nil {
		return nil, err
	}
	r.Gates, err = Status(s, p.Slug)
	if err != nil {
		return nil, err
	}
	r.Level, r.NextHumanStep = r.Gates.Level, r.Gates.NextHumanStep
	if area == "" {
		area = "evolution"
	}
	r.NextCommands = append(r.NextCommands, "pulse post finding --project "+p.Slug+" --area "+area+
		" --thread reflection --summary 'FACT' --body 'EVIDENCE'")
	for _, item := range r.Ballot {
		id := ""
		if item.Suggestion != nil {
			id = item.Suggestion.ID
		} else if item.Amendment != nil {
			id = item.Amendment.ID
		}
		if id != "" {
			r.NextCommands = append(r.NextCommands, "pulse show "+id,
				"pulse vote "+id+" up|down --summary 'EVIDENCE'")
			if item.Amendment != nil {
				r.NextCommands[len(r.NextCommands)-1] += " --text-hash HASH --body 'Read: WHAT CHANGES'"
			}
			break
		}
	}
	if len(r.Ballot) == 0 {
		r.NextCommands = append(r.NextCommands, "pulse ballot --project "+p.Slug)
	}
	if r.Level == "D" && (r.Gates.Planning.State == "ready" || r.Gates.Planning.State == "queued" ||
		r.Gates.Planning.State == "leased" || r.Gates.Planning.State == "finalizing") {
		r.NextCommands = append(r.NextCommands, "pulse evolve cycle --project "+p.Slug)
	} else {
		r.NextCommands = append(r.NextCommands, "pulse evolve status --project "+p.Slug)
	}
	r.NextCommands = append(r.NextCommands, "pulse evolve report --project "+p.Slug)
	return r, nil
}

func printReflection(w io.Writer, color bool, r *Reflection) {
	fmt.Fprintln(w, "REFLECT -", r.Project)
	fmt.Fprintf(w, "\nWORKING STATE\n  branch %s at %s; %d changed path(s)\n", r.Working.Branch, r.Working.Head, len(r.Working.Changes))
	for _, change := range r.Working.Changes {
		fmt.Fprintln(w, " ", change)
	}
	if r.Working.Problem != "" {
		fmt.Fprintln(w, " ", r.Working.Problem)
	}
	fmt.Fprintln(w, "\nBOARD\n"+r.Board)
	fmt.Fprintln(w, tally.BallotText(r.Project, "", r.Ballot))
	fmt.Fprintln(w, "FOR THE HUMAN")
	fmt.Fprintln(w, tally.ProposalsText(r.Proposals))
	if r.Gates != nil {
		fmt.Fprintln(w, "LOOP GATES")
		cli.Table(w, color, nil, statusRows(r.Gates))
	}
	fmt.Fprintln(w, "\nHYGIENE (report other sessions' strays; never delete them)")
	cli.Table(w, color, []string{"KIND", "COUNT", "PATHS"}, [][]string{
		{"evolve/* and pulse/* branches", fmt.Sprint(len(r.Hygiene.Branches)), strings.Join(r.Hygiene.Branches, ", ")},
		{"worktrees", fmt.Sprint(len(r.Hygiene.Worktrees)), strings.Join(r.Hygiene.Worktrees, ", ")},
		{"managed clone roots", fmt.Sprint(len(r.Hygiene.Clones)), strings.Join(r.Hygiene.Clones, ", ")},
	})
	verify := r.VerifyCmd
	if verify == "" {
		verify = "not configured"
	}
	fmt.Fprintln(w, "\nHEALTH CHECKS\n  project verify_cmd (not run):", verify)
	for _, problem := range append(append([]string{}, r.Problems...), r.Hygiene.Problems...) {
		fmt.Fprintln(w, " ", problem)
	}
	fmt.Fprintln(w, "\nLEVEL ALLOWED:", r.Level)
	fmt.Fprintln(w, "NEXT HUMAN STEP:", r.NextHumanStep)
	fmt.Fprintln(w, "\nNEXT COMMANDS (fill uppercase placeholders; choose your vote from evidence)")
	for _, command := range r.NextCommands {
		fmt.Fprintln(w, command)
	}
}
