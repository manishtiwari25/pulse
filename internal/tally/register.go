// Package tally is the board's rule set in one place (legacy/pulse.py Tally,
// once board.sh): thread, claim, suggestion, proposal and amendment statuses,
// the typed records the dashboard reads (`pulse tally`, alias `records`), the
// digest (`pulse board`), the ballot, the proposals list, `show`, `hash`, and
// - new with D-H - the constitution record and `amend apply`.
package tally

import (
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func init() {
	cli.Register(cli.Command{
		Name: "board", Group: "agents", ReadOnly: true,
		Summary: "the digest an agent reads before work",
		Usage:   "pulse board [--project SLUG] [--area AREA] [--agent NAME] [--now TIME] [--json]",
		Flags:   func(fs *flag.FlagSet) { fs.String("area", "", "one area only") },
		Run:     runBoard,
	})
	cli.Register(cli.Command{
		Name: "show", Group: "agents", ReadOnly: true,
		Summary: "print one post",
		Usage:   "pulse show <post-id> [--json]",
		Run:     runShow,
	})
	cli.Register(cli.Command{
		Name: "queue", Group: "agents", ReadOnly: true,
		Summary: "the experiment queue of one project, or work wanted across every project",
		Usage:   "pulse queue [--project SLUG] [--agent NAME] [--limit N] [--now TIME] [--json]",
		Flags:   func(fs *flag.FlagSet) { fs.Int("limit", 0, "show at most N work items across projects (0 = all)") },
		Run:     runQueue,
	})
	cli.Register(cli.Command{
		Name: "ballot", Group: "agents", ReadOnly: true,
		Summary: "open suggestions and amendments you can still vote on",
		Usage:   "pulse ballot [--project SLUG] [--agent NAME] [--now TIME] [--json]",
		Run:     runBallot,
	})
	cli.Register(cli.Command{
		Name: "proposals", Group: "human", ReadOnly: true,
		Summary: "what waits for the human's yes or no, across every project",
		Usage:   "pulse proposals [--project SLUG] [--json]",
		Run:     runProposals,
	})
	cli.Register(cli.Command{
		Name: "tally", Aliases: []string{"records"}, Group: "inspect", ReadOnly: true,
		Summary: "the tally as typed records (what the dashboard reads)",
		Usage:   "pulse tally [--project SLUG] [--area AREA] [--now TIME] [--quorum N] [--expiry-days N] [--stale-days N] [--amend-quorum N] [--amend-majority N/M] [--json]",
		Flags:   thresholdFlags,
		Run:     runTally,
	})
	cli.Register(cli.Command{
		Name: "hash", Group: "agents", ReadOnly: true,
		Summary: "the text_hash of an amendment's proposed text (a post id, a file, or - for stdin)",
		Usage:   "pulse hash <post-id|file|->",
		Run:     runHash,
	})
	cli.Register(cli.Command{
		Name: "constitution", Group: "agents", ReadOnly: true,
		Summary: "print a project's constitution record (EVO-constitution)",
		Usage:   "pulse constitution [--project SLUG] [--rev N] [--json]",
		Flags:   func(fs *flag.FlagSet) { fs.Int("rev", 0, "a past revision (default: current)") },
		Run:     runConstitution,
	})
	cli.Register(cli.Command{
		Name: "amend", Group: "agents",
		Summary: "amend apply <post-id>: write a passed amendment into the constitution record (idempotent)",
		Usage:   "pulse amend apply <post-id> [--now TIME] [--json]",
		Run:     runAmend,
	})
}

func thresholdFlags(fs *flag.FlagSet) {
	fs.String("area", "", "one area only")
	fs.Int("quorum", 0, "override the quorum")
	fs.Int("expiry-days", 0, "override expiry_days")
	fs.Int("stale-days", 0, "override stale_days")
	fs.Int("amend-quorum", 0, "override amend_quorum")
	fs.String("amend-majority", "", "override amend_majority (a fraction such as 2/3)")
}

// thresholdsFrom reads the override flags that were defined on fs.
func thresholdsFrom(c *cli.Context, fs *flag.FlagSet) (Thresholds, error) {
	th := Thresholds{Now: c.Now}
	geti := func(name string) int {
		if f := fs.Lookup(name); f != nil {
			return atoi(f.Value.String())
		}
		return 0
	}
	th.Quorum, th.ExpiryDays, th.StaleDays, th.AmendQuorum = geti("quorum"), geti("expiry-days"), geti("stale-days"), geti("amend-quorum")
	if f := fs.Lookup("amend-majority"); f != nil && f.Value.String() != "" {
		v := f.Value.String()
		if _, _, err := store.Majority(v); err != nil {
			return th, cli.Errorf(2, fmt.Sprintf("A majority must be a fraction such as 2/3, not %q", v), "amend-majority is a share of the counted votes", "pulse tally --amend-majority 2/3")
		}
		th.AmendMajority = v
	}
	return th, nil
}

func areaFlag(fs *flag.FlagSet) string {
	if f := fs.Lookup("area"); f != nil {
		return f.Value.String()
	}
	return ""
}

// currentProject is legacy/pulse.py current_project: --project / $PULSE_PROJECT,
// else the registered project the working directory sits in.
func currentProject(c *cli.Context) (string, error) {
	if c.Project != "" {
		if c.Project == store.CommonsSlug {
			return c.Project, nil
		}
		p, err := c.Store.Project(c.Project)
		if err != nil {
			return "", err
		}
		if p == nil {
			return "", cli.Errorf(2, fmt.Sprintf("No project %q", c.Project), "it is not registered in this store", "pulse projects")
		}
		return p.Slug, nil
	}
	p, err := c.Store.ProjectForDir(store.Cwd())
	if err != nil {
		return "", err
	}
	if p == nil {
		return "", cli.Errorf(2, "The current directory is not inside a registered project",
			"pulse needs to know which board you mean", "pulse <cmd> --project SLUG (or set PULSE_PROJECT, or run pulse init here)")
	}
	return p.Slug, nil
}

// resolveAgent is legacy/pulse.py resolve_agent(required=False): the agent row
// for --agent / $PULSE_AGENT (a name or a legacy runner/model id), "" when
// unknown or unset; a pruned agent is refused.
func resolveAgent(c *cli.Context) (string, error) {
	ident := c.Agent
	if ident == "" {
		return "", nil
	}
	row, err := c.Store.Agent(ident)
	if err != nil {
		return "", err
	}
	if row == nil && strings.Contains(ident, "/") {
		var name string
		err := c.Store.DB.QueryRow("SELECT name FROM agents WHERE legacy_id = ? ORDER BY last_active_at DESC LIMIT 1", ident).Scan(&name)
		if err == nil {
			row, err = c.Store.Agent(name)
			if err != nil {
				return "", err
			}
		}
	}
	if row == nil {
		return "", nil
	}
	if row.Status == "pruned" {
		return "", cli.Errorf(2, fmt.Sprintf("Agent %q was pruned on %s", row.Name, row.PrunedAt), row.PrunedReason, "pulse agent join --name NEW-NAME --runner R --model M")
	}
	return row.Name, nil
}

func wrap(err error) error {
	var np *NoProjectError
	var nn *NoPostError
	var ae *ApplyError
	switch {
	case err == nil:
		return nil
	case errors.As(err, &np):
		return cli.Errorf(2, fmt.Sprintf("No project %q", np.Slug), "it is not registered in this store", "pulse projects")
	case errors.As(err, &nn):
		return cli.Errorf(2, fmt.Sprintf("No post %q", nn.ID), "no post has that id", "pulse board")
	case errors.As(err, &ae):
		return cli.Errorf(2, ae.What, ae.Why, ae.Fix)
	}
	return err
}

func runBoard(c *cli.Context, fs *flag.FlagSet, _ []string) error {
	slug, err := currentProject(c)
	if err != nil {
		return err
	}
	agent, err := resolveAgent(c)
	if err != nil {
		return err
	}
	th := Thresholds{Now: c.Now}
	if c.JSON {
		res, err := Compute(c.Store, slug, areaFlag(fs), th)
		if err != nil {
			return wrap(err)
		}
		return cli.WriteJSON(c.Out, res)
	}
	session := c.Session
	if agent != "" && session == "" {
		session = agentSession(c.Store, agent)
	}
	text, err := DigestFor(c.Store, slug, areaFlag(fs), th, agent, session)
	if err != nil {
		return wrap(err)
	}
	posts, err := PostsFor(c.Store, slug, areaFlag(fs))
	if err != nil {
		return wrap(err)
	}
	bareSummaries := 0
	for _, post := range posts {
		if post.Kind == "vote" {
			switch strings.ToLower(strings.TrimSpace(post.Summary)) {
			case "up", "down":
				bareSummaries++
			}
		}
	}
	if bareSummaries > 0 {
		if _, err := fmt.Fprintf(c.Err, "warning: %d vote post(s) have a bare up/down summary; include an explanatory phrase in the vote summary\n", bareSummaries); err != nil {
			return err
		}
	}
	if c.Color {
		text = boldHeadings(text, c.Style())
	}
	_, err = io.WriteString(c.Out, text)
	return err
}

// boldHeadings makes the digest's section titles (the unindented ALL-CAPS
// lines) bold on a terminal; the text stays byte-identical otherwise.
func boldHeadings(text string, st cli.Style) string {
	lines := strings.Split(text, "\n")
	for i, l := range lines {
		word := strings.SplitN(l, " ", 2)[0]
		if word != "" && word == strings.ToUpper(word) && word != strings.ToLower(word) {
			lines[i] = st.Bold(l)
		}
	}
	return strings.Join(lines, "\n")
}

func runShow(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if len(args) != 1 {
		return cli.Errorf(2, "show needs one post id", "usage: pulse show <post-id>", "pulse board")
	}
	if c.JSON {
		p, err := loadPost(c.Store, args[0])
		if err != nil {
			return wrap(err)
		}
		return cli.WriteJSON(c.Out, PostJSON(p))
	}
	text, err := Show(c.Store, args[0])
	if err != nil {
		return wrap(err)
	}
	_, err = io.WriteString(c.Out, text)
	return err
}

// runQueue: with --project (or $PULSE_PROJECT) the project's experiment
// queue exactly as the Python printed it; without, the work wanted across
// every active project, priority then age (D-M: nudge folded into queue).
func runQueue(c *cli.Context, fs *flag.FlagSet, _ []string) error {
	if c.Project == "" {
		agent, err := resolveAgent(c)
		if err != nil {
			return err
		}
		q, err := WorkWanted(c.Store, nil, agent, Thresholds{Now: c.Now})
		if err != nil {
			return wrap(err)
		}
		limit := 0
		if f := fs.Lookup("limit"); f != nil {
			limit = atoi(f.Value.String())
		}
		if c.JSON {
			if limit > 0 && limit < len(q.Work) {
				q.Work = q.Work[:limit]
			}
			return cli.WriteJSON(c.Out, q)
		}
		_, err = io.WriteString(c.Out, WorkText(q, limit))
		return err
	}
	slug, err := currentProject(c)
	if err != nil {
		return err
	}
	res, err := Compute(c.Store, slug, "", Thresholds{Now: c.Now})
	if err != nil {
		return wrap(err)
	}
	if c.JSON {
		open := []Suggestion{}
		for _, x := range res.Suggestions {
			if x.Status != "closed" {
				open = append(open, x)
			}
		}
		return cli.WriteJSON(c.Out, open)
	}
	_, err = io.WriteString(c.Out, QueueText(res))
	return err
}

func runBallot(c *cli.Context, _ *flag.FlagSet, _ []string) error {
	slug, err := currentProject(c)
	if err != nil {
		return err
	}
	agent, err := resolveAgent(c)
	if err != nil {
		return err
	}
	session := c.Session
	if agent != "" && session == "" {
		session = agentSession(c.Store, agent)
	}
	items, err := Ballot(c.Store, slug, agent, session, Thresholds{Now: c.Now})
	if err != nil {
		return wrap(err)
	}
	if c.JSON {
		sugs, ams := splitBallot(items)
		if sugs == nil {
			sugs = []Suggestion{}
		}
		if ams == nil {
			ams = []Amendment{}
		}
		return cli.WriteJSON(c.Out, map[string]any{"project": slug, "agent": agent, "suggestions": sugs, "amendments": ams})
	}
	_, err = io.WriteString(c.Out, BallotText(slug, agent, items))
	return err
}

func runProposals(c *cli.Context, _ *flag.FlagSet, _ []string) error {
	if c.Project != "" && c.Project != store.CommonsSlug {
		if p, err := c.Store.Project(c.Project); err != nil {
			return err
		} else if p == nil {
			return cli.Errorf(2, fmt.Sprintf("No project %q", c.Project), "it is not registered in this store", "pulse projects")
		}
	}
	items, err := Proposals(c.Store, c.Project)
	if err != nil {
		return wrap(err)
	}
	if c.JSON {
		if items == nil {
			items = []Proposal{}
		}
		return cli.WriteJSON(c.Out, items)
	}
	_, err = io.WriteString(c.Out, ProposalsText(items))
	return err
}

func runTally(c *cli.Context, fs *flag.FlagSet, _ []string) error {
	slug, err := currentProject(c)
	if err != nil {
		return err
	}
	th, err := thresholdsFrom(c, fs)
	if err != nil {
		return err
	}
	res, err := Compute(c.Store, slug, areaFlag(fs), th)
	if err != nil {
		return wrap(err)
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, res)
	}
	_, err = io.WriteString(c.Out, res.Records())
	return err
}

func runHash(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if len(args) != 1 {
		return cli.Errorf(2, "hash needs a post id, a file, or -", "usage: pulse hash <post-id|file|->", "pulse ballot")
	}
	arg := args[0]
	var text string
	if arg == "-" {
		data, err := io.ReadAll(os.Stdin)
		if err != nil {
			return err
		}
		text = string(data)
	} else {
		p, err := loadPost(c.Store, arg)
		if err == nil {
			block := fencedBlock(p.Body, "Proposed text")
			if block == nil {
				return cli.Errorf(1, fmt.Sprintf("No fenced block under '## Proposed text' in %s", arg),
					"an amendment quotes its proposed text in a fenced block under that heading", "pulse show "+arg)
			}
			fmt.Fprintln(c.Out, blockHash(block))
			return nil
		}
		var nn *NoPostError
		if !errors.As(err, &nn) {
			return err
		}
		data, err := os.ReadFile(arg)
		if err != nil {
			return cli.Errorf(2, fmt.Sprintf("No post or file %q", arg), "hash takes a post id, a file path, or - for stdin", "pulse ballot")
		}
		text = string(data)
	}
	fmt.Fprintln(c.Out, TextHash(text))
	return nil
}

func runConstitution(c *cli.Context, fs *flag.FlagSet, _ []string) error {
	slug, err := currentProject(c)
	if err != nil {
		return err
	}
	rev := 0
	if f := fs.Lookup("rev"); f != nil {
		rev = atoi(f.Value.String())
	}
	rec, revision, err := ConstitutionRecord(c.Store, slug, rev)
	if errors.Is(err, store.ErrNoRecord) {
		return cli.Errorf(2, fmt.Sprintf("%s has no constitution record", slug), "no record of kind evolution with slug constitution exists for it",
			"pulse migrate --records --repo PATH --project "+slug+"  (or pulse record new evolution --title 'Constitution')")
	}
	if err != nil {
		return err
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, map[string]any{"record": rec, "revision": revision})
	}
	_, err = io.WriteString(c.Out, revision.Body)
	if err == nil && !strings.HasSuffix(revision.Body, "\n") {
		_, err = io.WriteString(c.Out, "\n")
	}
	return err
}

func runAmend(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if len(args) != 2 || args[0] != "apply" {
		return cli.Errorf(2, "amend takes: apply <post-id>", "the only verb today is apply", "pulse amend apply <post-id>")
	}
	applied, err := ApplyAmendment(c.Store, args[1], Thresholds{Now: c.Now})
	if err != nil {
		return wrap(err)
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, applied)
	}
	if applied.AlreadyAt != "" {
		c.Printf("already applied at %s: %s is at revision %d\n", applied.AlreadyAt, applied.RecordID, applied.Rev)
		return nil
	}
	notice, err := Ratify(c.Store, applied, c.Agent)
	if err != nil {
		return err
	}
	c.Printf("applied %s: %s revision %d by %s\n", applied.PostID, applied.RecordID, applied.Rev, applied.Author)
	c.Printf("ratified: %s closes the amendment\n", notice.ID)
	return nil
}

// Ratify posts the ratification result (resolution ratified) on the
// amendment's thread as agent (the runner's SystemAgent when empty), which
// closes the amendment in the tally. It is what the runner calls after
// ApplyAmendment and what `pulse amend apply` prints.
func Ratify(s *store.Store, applied *Applied, agent string) (*board.Post, error) {
	am, err := loadPost(s, applied.PostID)
	if err != nil {
		return nil, err
	}
	summary := "ratified: " + am.Summary
	if n := len([]rune(summary)); n > 240 {
		summary = string([]rune(summary)[:237]) + "..."
	}
	in := board.PostInput{Kind: "result", Project: am.Project, Area: am.Area, Thread: am.Thread, Summary: summary,
		Body: applied.Notice + "\n\nRecord: " + applied.RecordID + " revision " + fmt.Sprint(applied.Rev) + "\nHash: " + applied.Hash + "\n",
		On:   applied.PostID, Resolution: "ratified", Agent: agent}
	if agent == "" {
		in.Agent, in.NoAgentCheck, in.Runner = SystemAgent, true, "pulse"
	}
	return board.CreatePost(s, in)
}
