package board

import (
	"bufio"
	"flag"
	"fmt"
	"io"
	"os"
	"strconv"
	"strings"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// UninstallService removes the daemon's user service (`pulse remove service`).
// The runner package, which owns the service files, assigns it at init; board
// cannot import runner (runner imports board), so the hook is a variable.
var UninstallService func(out io.Writer) string

// Stdin is where `post` reads a body from when neither --body nor --body-file
// is given and stdin is not a terminal; tests replace it.
var Stdin io.Reader = os.Stdin

// StdinIsTTY reports whether Stdin is a terminal; tests replace it.
var StdinIsTTY = func() bool { return cli.IsTTY(os.Stdin) }

func init() {
	cli.Register(cli.Command{
		Name: "agent", Group: "agents",
		Summary: "join with a unique name, spawn a worker, list, or prune",
		Usage: "pulse agent join --name NAME [--runner R] [--model M] [--session S] [--display D] [--kind session|worker]\n" +
			"       pulse agent spawn [--runner R] [--model M]\n" +
			"       pulse agent list [--all] [--json]\n" +
			"       pulse agent prune [--days N] [--dry-run] [--now T]",
		Flags: func(fs *flag.FlagSet) {
			fs.String("name", "", "the agent's unique name (join)")
			fs.String("runner", "", "the runner the agent runs in (join, spawn)")
			fs.String("model", "", "the model behind the agent (join, spawn)")
			fs.String("display", "", "a display name (join)")
			fs.String("kind", "session", "session or worker (join)")
			fs.Bool("all", false, "include pruned agents (list)")
			fs.Int("days", -1, "prune after this many days without useful output (default: setting prune_after_days)")
			fs.Bool("dry-run", false, "prune: report only")
		},
		Run: runAgent,
	})
	cli.Register(cli.Command{
		Name: "post", Group: "agents",
		Summary: "write one immutable post",
		Usage: "pulse post KIND --area A --thread T --summary S [--body TEXT | --body-file F | < stdin] [--on POST] [--vote up|down]\n" +
			"       [--expires T] [--resolution R] [--links L] [--files F] [--text-hash H] [--origin-project P] [--claim-hours H] [--project SLUG]\n" +
			"       kinds: " + strings.Join(Kinds, ", "),
		Flags: postFlags,
		Run:   runPost,
	})
	cli.Register(cli.Command{
		Name: "claim", Group: "agents",
		Summary: "shortcut: post a claim that expires after claim_hours",
		Usage:   "pulse claim --area A --thread T --summary S [--hours H] [--project SLUG]",
		Flags: func(fs *flag.FlagSet) {
			fs.String("area", "", "the board area")
			fs.String("thread", "", "the thread")
			fs.String("summary", "", "one line")
			fs.Float64("hours", 0, "expiry in hours from now (default: setting claim_hours)")
		},
		Run: runClaim,
	})
	cli.Register(cli.Command{
		Name: "vote", Group: "agents",
		Summary: "shortcut: vote up or down on a suggestion or amendment",
		Usage:   "pulse vote POST_ID up|down --summary REASON [--body TEXT] [--text-hash H]",
		Flags: func(fs *flag.FlagSet) {
			fs.String("summary", "", "the reason, one line")
			fs.String("body", "", "the body (an amendment vote carries a `Read:` line)")
			fs.String("text-hash", "", "the amendment's text hash (see pulse hash)")
		},
		Run: runVote,
	})
	cli.Register(cli.Command{
		Name: "decide", Group: "human",
		Summary: "the human records yes or no on a proposal (a yes queues one job, WU-022)",
		Usage:   "pulse decide POST_ID yes|no [--words '...']",
		Flags: func(fs *flag.FlagSet) {
			fs.String("words", "", "the words that go with the answer")
		},
		Run: runDecide,
	})
	cli.Register(cli.Command{
		Name: "job", Group: "runner",
		Summary: "add or finish a job by hand",
		Usage: "pulse job add --title T [--kind K] [--brief B] [--tier 0|1] [--priority N] [--source S] [--project SLUG]\n" +
			"       pulse job finish JOB_ID done|failed|cancelled|blocked|queued [--result TEXT] [--branch B]",
		Flags: func(fs *flag.FlagSet) {
			fs.String("kind", "task", "job kind: "+strings.Join(JobKinds, ", "))
			fs.String("title", "", "one line (add)")
			fs.String("brief", "", "what the worker reads (add)")
			fs.Int("tier", 1, "0 or 1 (add)")
			fs.Int("priority", 0, "higher runs first (add)")
			fs.String("source", "", "what queued it; an open job with the same source is reused (add)")
			fs.String("result", "", "the outcome text (finish)")
			fs.String("branch", "", "the branch the work is on (finish)")
		},
		Run: runJob,
	})
	cli.Register(cli.Command{
		Name: "init", Group: "human",
		Summary: "register a checkout as a project, queue its onboarding job, import its environment recipe",
		Usage:   "pulse init [PATH] [--name N] [--remote URL] [--verify CMD] [--brain]",
		Flags: func(fs *flag.FlagSet) {
			fs.String("path", "", "the checkout (default: the current directory)")
			fs.String("name", "", "the project's name (default: the folder name)")
			fs.String("remote", "", "the remote URL (default: remote.origin.url)")
			fs.String("verify", "", "the project's own check command, e.g. 'npm test'")
			fs.Bool("brain", false, "this checkout is the PULSE framework itself")
		},
		Run: runInit,
	})
	cli.Register(cli.Command{
		Name: "export", Group: "records",
		Summary: "copy the store to a .sql or .sqlite file outside every project",
		Usage:   "pulse export --sql PATH | --db PATH [--force]",
		Flags: func(fs *flag.FlagSet) {
			fs.String("sql", "", "write a SQL dump here")
			fs.String("db", "", "write a SQLite copy here")
			fs.Bool("force", false, "overwrite an existing file")
		},
		Run: runExport,
	})
	cli.Register(cli.Command{
		Name: "import", Group: "records",
		Summary: "register the projects of a JSON registry (the pre-store ~/.pulse/projects.json)",
		Usage:   "pulse import --registry PATH",
		Flags: func(fs *flag.FlagSet) {
			fs.String("registry", "", "the JSON registry (default: PULSE_HOME/projects.json)")
		},
		Run: runImport,
	})
	cli.Register(cli.Command{
		Name: "remove", Group: "human",
		Summary: "deactivate a project or retire an executor: preview, then apply; nothing is deleted",
		Usage: "pulse remove project SLUG [--dry-run] [--yes] [--json]\n" +
			"       pulse remove executor NAME [--dry-run] [--yes] [--json]   (the old spelling was: pulse executor remove NAME)\n" +
			"       pulse remove service                                     (the daemon's user service; today: pulse service uninstall)",
		Flags: func(fs *flag.FlagSet) {
			fs.Bool("dry-run", false, "show the preview and change nothing")
			fs.Bool("yes", false, "apply without the confirmation prompt (only asked on a terminal)")
		},
		Run: runRemove,
	})
}

func flagString(fs *flag.FlagSet, name string) string {
	if f := fs.Lookup(name); f != nil {
		return f.Value.String()
	}
	return ""
}

func flagBool(fs *flag.FlagSet, name string) bool {
	return flagString(fs, name) == "true"
}

func flagInt(fs *flag.FlagSet, name string) int {
	n, _ := strconv.Atoi(flagString(fs, name))
	return n
}

func flagFloat(fs *flag.FlagSet, name string) float64 {
	f, _ := strconv.ParseFloat(flagString(fs, name), 64)
	return f
}

func flagGiven(fs *flag.FlagSet, name string) bool {
	given := false
	fs.Visit(func(f *flag.Flag) {
		if f.Name == name {
			given = true
		}
	})
	return given
}

func usage(name, what string) error {
	return refuseFix(2, "pulse "+name+": "+what, "", "pulse help "+name)
}

// actor is the agent this process acts as, else the human.
func actor(c *cli.Context) string {
	if c.Agent != "" {
		return c.Agent
	}
	return store.HumanAgent
}

// CurrentProject is --project / $PULSE_PROJECT, else the registered project
// the current directory sits in; the commons is a pseudo project.
func CurrentProject(s *store.Store, explicit string) (*store.Project, error) {
	if explicit == "" {
		explicit = os.Getenv("PULSE_PROJECT")
	}
	if explicit != "" {
		if explicit == store.CommonsSlug {
			return &store.Project{Slug: store.CommonsSlug, Name: "the commons"}, nil
		}
		p, err := s.Project(explicit)
		if err != nil {
			return nil, err
		}
		if p == nil {
			return nil, refuse(2, "No project %s (pulse projects lists them)", pyRepr(explicit))
		}
		return p, nil
	}
	p, err := s.ProjectForDir(store.Cwd())
	if err != nil {
		return nil, err
	}
	if p == nil {
		return nil, refuse(2, "The current directory is not inside a registered project; pass --project SLUG, set PULSE_PROJECT, "+
			"or run `pulse init` here")
	}
	return p, nil
}

func runAgent(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		return usage("agent", "join, spawn, list or prune")
	}
	switch args[0] {
	case "join":
		kind := flagString(fs, "kind")
		if kind != "session" && kind != "worker" {
			return usage("agent", "--kind is session or worker")
		}
		if flagString(fs, "name") == "" {
			return usage("agent", "join needs --name NAME")
		}
		row, created, err := JoinAgent(c.Store, JoinInput{Name: flagString(fs, "name"), Runner: flagString(fs, "runner"),
			Model: flagString(fs, "model"), Session: c.Session, Display: flagString(fs, "display"), Kind: kind})
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"agent": row, "created": created})
		}
		word := "welcome back"
		if created {
			word = "joined"
		}
		c.Printf("%s %s (%s/%s); export PULSE_AGENT=%s\n", word, row.Name, row.Runner, row.Model, row.Name)
	case "spawn":
		row, err := SpawnAgent(c.Store, flagString(fs, "runner"), flagString(fs, "model"), "worker")
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, row)
		}
		fmt.Fprintln(c.Out, row.Name)
	case "list":
		cards, err := Scorecards(c.Store, flagBool(fs, "all"))
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, cards)
		}
		for _, r := range cards {
			useful := r.LastUsefulAt
			if useful == "" {
				useful = "-"
			}
			beat := r.LastHeartbeatAt
			if beat == "" {
				beat = "-"
			}
			fmt.Fprintf(c.Out, "%-16s %-7s %-8s presence %-7s posts %-4d jobs %d/%d  last heartbeat %s  last useful %s  %s/%s\n",
				r.Name, r.Status, r.Kind, r.Presence, r.Posts, r.JobsDone, r.JobsDone+r.JobsFailed, beat, useful, r.Runner, r.Model)
		}
	case "prune":
		dry := flagBool(fs, "dry-run")
		pruned, err := PruneAgents(c.Store, c.Now, flagInt(fs, "days"), actorOr(c, "daemon"), dry)
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, pruned)
		}
		for _, p := range pruned {
			if dry {
				fmt.Fprintf(c.Out, "would prune %s: %s\n", p.Name, p.Reason)
			} else {
				fmt.Fprintf(c.Out, "pruned %s: %s\n", p.Name, p.Reason)
			}
		}
	default:
		return usage("agent", "unknown action "+pyRepr(args[0])+"; join, spawn, list or prune")
	}
	return nil
}

func actorOr(c *cli.Context, def string) string {
	if c.Agent != "" {
		return c.Agent
	}
	return def
}

func postFlags(fs *flag.FlagSet) {
	fs.String("area", "", "the board area (lowercase words with dashes)")
	fs.String("thread", "", "the thread")
	fs.String("summary", "", "one line, at most 240 characters")
	fs.String("body", "", "the body text")
	fs.String("body-file", "", "read the body from a file")
	fs.String("on", "", "the post this answers")
	fs.String("vote", "", "up or down (votes only)")
	fs.String("expires", "", "UTC time like 2026-09-05T08:00:00Z")
	fs.String("resolution", "", "one of "+strings.Join(Resolutions, ", "))
	fs.String("links", "", "related links")
	fs.String("files", "", "files touched")
	fs.String("text-hash", "", "an amendment's text hash")
	fs.String("origin-project", "", "the project a commons post came from")
	fs.Float64("claim-hours", 0, "for a claim: expiry in hours from now")
}

// ReadBody is pulse.py read_body: --body-file, else --body, else stdin when it is not a terminal.
func ReadBody(fs *flag.FlagSet) (string, error) {
	if f := flagString(fs, "body-file"); f != "" {
		data, err := os.ReadFile(f)
		if err != nil {
			return "", refuseFix(2, "Cannot read --body-file "+f, err.Error(), "pass a readable file, or --body TEXT")
		}
		return string(data), nil
	}
	if flagGiven(fs, "body") {
		return flagString(fs, "body"), nil
	}
	if !StdinIsTTY() {
		data, err := io.ReadAll(Stdin)
		if err != nil {
			return "", err
		}
		return string(data), nil
	}
	return "", nil
}

func runPost(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) != 1 {
		return usage("post", "one positional KIND: "+strings.Join(Kinds, ", "))
	}
	kind := args[0]
	if !contains(Kinds, kind) {
		return refuse(2, "Kind must be one of: %s", strings.Join(Kinds, ", "))
	}
	if v := flagString(fs, "vote"); v != "" && v != "up" && v != "down" {
		return usage("post", "--vote is up or down")
	}
	if r := flagString(fs, "resolution"); r != "" && !contains(Resolutions, r) {
		return refuse(2, "Resolution must be one of: %s", strings.Join(Resolutions, ", "))
	}
	for _, need := range []string{"area", "thread", "summary"} {
		if flagString(fs, need) == "" {
			return usage("post", "--area, --thread and --summary are required")
		}
	}
	proj, err := CurrentProject(c.Store, c.Project)
	if err != nil {
		return err
	}
	body, err := ReadBody(fs)
	if err != nil {
		return err
	}
	expires := flagString(fs, "expires")
	if kind == "claim" && expires == "" {
		if expires, err = ClaimExpiry(c.Store, flagFloat(fs, "claim-hours")); err != nil {
			return err
		}
	}
	row, err := CreatePost(c.Store, PostInput{Kind: kind, Project: proj.Slug, Area: flagString(fs, "area"), Thread: flagString(fs, "thread"),
		Summary: flagString(fs, "summary"), Body: body, Agent: c.Agent, Session: c.Session, On: flagString(fs, "on"),
		Vote: flagString(fs, "vote"), Expires: expires, Resolution: flagString(fs, "resolution"), Links: flagString(fs, "links"),
		Files: flagString(fs, "files"), TextHash: flagString(fs, "text-hash"), OriginProject: flagString(fs, "origin-project")})
	if err != nil {
		return err
	}
	return printPost(c, row)
}

func printPost(c *cli.Context, row *Post) error {
	if c.JSON {
		return cli.WriteJSON(c.Out, row)
	}
	fmt.Fprintln(c.Out, row.ID)
	return nil
}

func runClaim(c *cli.Context, fs *flag.FlagSet, _ []string) error {
	for _, need := range []string{"area", "thread", "summary"} {
		if flagString(fs, need) == "" {
			return usage("claim", "--area, --thread and --summary are required")
		}
	}
	proj, err := CurrentProject(c.Store, c.Project)
	if err != nil {
		return err
	}
	expires, err := ClaimExpiry(c.Store, flagFloat(fs, "hours"))
	if err != nil {
		return err
	}
	row, err := CreatePost(c.Store, PostInput{Kind: "claim", Project: proj.Slug, Area: flagString(fs, "area"), Thread: flagString(fs, "thread"),
		Summary: flagString(fs, "summary"), Agent: c.Agent, Session: c.Session, Expires: expires})
	if err != nil {
		return err
	}
	return printPost(c, row)
}

func runVote(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) != 2 || (args[1] != "up" && args[1] != "down") {
		return usage("vote", "pulse vote POST_ID up|down --summary REASON")
	}
	if flagString(fs, "summary") == "" {
		return usage("vote", "--summary REASON is required")
	}
	target, err := PostByID(c.Store, args[0])
	if err != nil {
		return err
	}
	if target == nil {
		return refuse(2, "No post %s", pyRepr(args[0]))
	}
	row, err := CreatePost(c.Store, PostInput{Kind: "vote", Project: target.Project, Area: target.Area, Thread: target.Thread,
		Summary: flagString(fs, "summary"), Body: flagString(fs, "body"), Agent: c.Agent, Session: c.Session, On: args[0],
		Vote: args[1], TextHash: flagString(fs, "text-hash")})
	if err != nil {
		return err
	}
	return printPost(c, row)
}

func runDecide(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) != 2 || (args[1] != "yes" && args[1] != "no") {
		return usage("decide", "pulse decide POST_ID yes|no [--words '...']")
	}
	if c.Agent != "" && c.Agent != store.HumanAgent {
		return refuseFix(2, "pulse decide is the human's call", "an agent proposes and votes; the owner answers in the hub or here without PULSE_AGENT",
			"pulse vote "+args[0]+" up|down --summary REASON")
	}
	d, err := Decide(c.Store, args[0], args[1], flagString(fs, "words"), store.HumanAgent)
	if err != nil {
		return err
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, d)
	}
	c.Printf("recorded %s as %s\n", d.Answer, d.Result.ID)
	if d.Job != nil {
		c.Printf("queued %s: %s\n", d.Job.ID, d.Job.Title)
	}
	return nil
}

func runJob(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		return usage("job", "add or finish")
	}
	switch args[0] {
	case "add":
		if flagString(fs, "title") == "" {
			return usage("job", "add needs --title")
		}
		proj, err := CurrentProject(c.Store, c.Project)
		if err != nil {
			return err
		}
		job, created, err := AddJob(c.Store, JobInput{Project: proj.Slug, Kind: flagString(fs, "kind"), Title: flagString(fs, "title"),
			Brief: flagString(fs, "brief"), Tier: flagInt(fs, "tier"), Priority: flagInt(fs, "priority"), Source: flagString(fs, "source"),
			CreatedBy: c.Agent})
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"job": job, "created": created})
		}
		if created {
			fmt.Fprintln(c.Out, job.ID)
		} else {
			fmt.Fprintf(c.Out, "already queued: %s\n", job.ID)
		}
	case "finish":
		if len(args) != 3 {
			return usage("job", "finish JOB_ID done|failed|cancelled|blocked|queued")
		}
		if !contains(JobEndStates, args[2]) {
			return refuse(2, "A job ends done, failed, cancelled, blocked, or back to queued")
		}
		agent, err := ResolveAgent(c.Store, c.Agent, false)
		if err != nil {
			return err
		}
		name := store.HumanAgent
		if agent != nil {
			name = agent.Name
		}
		job, err := FinishJob(c.Store, args[1], name, args[2], flagString(fs, "result"), flagString(fs, "branch"), nil)
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, job)
		}
		fmt.Fprintf(c.Out, "%s %s\n", job.ID, job.Status)
	default:
		return usage("job", "unknown action "+pyRepr(args[0])+"; add or finish")
	}
	return nil
}

// InitResult is what `pulse init` did.
type InitResult struct {
	Project     *store.Project `json:"project"`
	Created     bool           `json:"created"`
	Job         *Job           `json:"job"`
	JobQueued   bool           `json:"job_queued"`
	Environment string         `json:"environment_record"`
	Store       string         `json:"store"`
}

// Init registers the checkout, queues the onboarding job and imports the
// environment recipe when the checkout has one (D-K).
func Init(s *store.Store, path, name, remote string, brain bool, verifyCmd, who string) (*InitResult, error) {
	if path == "" {
		path = "."
	}
	proj, created, err := RegisterProject(s, path, name, remote, brain, verifyCmd, who, "")
	if err != nil {
		return nil, err
	}
	envID, err := ImportEnvironment(s, proj, who)
	if err != nil {
		return nil, err
	}
	hasRecipe := envID != ""
	if !hasRecipe {
		if _, _, err := s.GetRecord(EnvironmentRecordID(proj), 0); err == nil {
			hasRecipe = true
		}
	}
	job, queued, err := QueueOnboardingJob(s, proj.Slug, "init", hasRecipe)
	if err != nil {
		return nil, err
	}
	return &InitResult{Project: proj, Created: created, Job: job, JobQueued: queued, Environment: envID, Store: s.Path}, nil
}

func runInit(c *cli.Context, fs *flag.FlagSet, args []string) error {
	path := flagString(fs, "path")
	if len(args) > 1 {
		return usage("init", "at most one PATH")
	}
	if len(args) == 1 {
		if path != "" && path != args[0] {
			return usage("init", "PATH and --path disagree")
		}
		path = args[0]
	}
	res, err := Init(c.Store, path, flagString(fs, "name"), flagString(fs, "remote"), flagBool(fs, "brain"), flagString(fs, "verify"), actor(c))
	if err != nil {
		return err
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, res)
	}
	word := "refreshed"
	if res.Created {
		word = "registered"
	}
	brain := ""
	if res.Project.Brain {
		brain = " (brain)"
	}
	c.Printf("%s project %s at %s%s\n", word, res.Project.Slug, res.Project.Path, brain)
	q := "already queued"
	if res.JobQueued {
		q = "queued"
	}
	c.Printf("%s onboarding job %s\n", q, res.Job.ID)
	if res.Environment != "" {
		c.Printf("imported docs/context/environment.md as %s\n", res.Environment)
	}
	c.Printf("store: %s\n", res.Store)
	return nil
}

func runExport(c *cli.Context, fs *flag.FlagSet, _ []string) error {
	target, err := ExportStore(c.Store, flagString(fs, "sql"), flagString(fs, "db"), flagBool(fs, "force"))
	if err != nil {
		return err
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, map[string]string{"path": target})
	}
	fmt.Fprintln(c.Out, target)
	return nil
}

func runImport(c *cli.Context, fs *flag.FlagSet, _ []string) error {
	imported, err := ImportRegistry(c.Store, flagString(fs, "registry"), actorOr(c, "migrate"))
	if err != nil {
		return err
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, imported)
	}
	for _, i := range imported {
		word := "kept"
		if i.Created {
			word = "added"
		}
		fmt.Fprintf(c.Out, "registry: %s %s\n", word, i.Slug)
	}
	if len(imported) == 0 {
		c.Printf("registry: nothing to import\n")
	}
	return nil
}

func runRemove(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		return usage("remove", "project SLUG, executor NAME, or service")
	}
	dry, yes := flagBool(fs, "dry-run"), flagBool(fs, "yes")
	var run func(dryRun bool) (*RemovePreview, error)
	switch args[0] {
	case "project":
		if len(args) != 2 {
			return usage("remove", "project needs one SLUG")
		}
		run = func(d bool) (*RemovePreview, error) { return RemoveProject(c.Store, args[1], actor(c), d) }
	case "executor":
		if len(args) != 2 {
			return usage("remove", "executor needs one NAME")
		}
		run = func(d bool) (*RemovePreview, error) { return RemoveExecutor(c.Store, args[1], actor(c), d) }
	case "service":
		if dry {
			c.Printf("would run: pulse service uninstall\n")
			return nil
		}
		if UninstallService == nil {
			return refuseFix(2, "The service uninstaller is not linked into this binary", "internal/runner registers it", "pulse service uninstall")
		}
		UninstallService(c.Out)
		return nil
	default:
		return usage("remove", "unknown target "+pyRepr(args[0])+"; project, executor or service")
	}
	st := c.Style()
	if !dry && !yes && StdinIsTTY() {
		pre, err := run(true)
		if err != nil {
			return err
		}
		pre.Render(c.Out, st)
		fmt.Fprintf(c.Out, "%s [y/N] ", st.Bold("Apply?"))
		line, _ := bufio.NewReader(Stdin).ReadString('\n')
		if a := strings.ToLower(strings.TrimSpace(line)); a != "y" && a != "yes" {
			fmt.Fprintln(c.Out, "not applied")
			return &cli.Error{Code: 1}
		}
		pre, err = run(false)
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, pre)
		}
		fmt.Fprintln(c.Out, st.Green("applied")+"  "+st.Dim(pre.Kind+" "+pre.Name+"; the Yours lines above are still yours"))
		return nil
	}
	pre, err := run(dry)
	if err != nil {
		return err
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, pre)
	}
	pre.Render(c.Out, st)
	return nil
}
