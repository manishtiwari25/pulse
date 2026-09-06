package evolve

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

var Stdin io.Reader = os.Stdin

type repeatedStrings []string

func (s *repeatedStrings) String() string { return strings.Join(*s, ",") }
func (s *repeatedStrings) Set(value string) error {
	*s = append(*s, value)
	return nil
}

func commandFlags(c *cli.Context, name string) (*cli.Context, *flag.FlagSet) {
	copy := *c
	if copy.Out == nil {
		copy.Out = io.Discard
	}
	if copy.Err == nil {
		copy.Err = io.Discard
	}
	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	fs.SetOutput(copy.Err)
	fs.StringVar(&copy.Project, "project", copy.Project, "project slug")
	fs.StringVar(&copy.Agent, "agent", copy.Agent, "calling agent")
	fs.StringVar(&copy.Session, "session", copy.Session, "calling session")
	fs.StringVar(&copy.Home, "home", copy.Home, "PULSE home")
	fs.StringVar(&copy.Now, "now", copy.Now, "fixed UTC clock")
	fs.BoolVar(&copy.JSON, "json", copy.JSON, "machine-readable output")
	fs.BoolVar(&copy.Quiet, "quiet", copy.Quiet, "suppress ordinary text")
	return &copy, fs
}

func parseCommandFlags(fs *flag.FlagSet, args []string) error {
	options, positional := []string{}, []string{}
	for i := 0; i < len(args); i++ {
		arg := args[i]
		if arg == "--" {
			positional = append(positional, args[i+1:]...)
			break
		}
		if arg == "-" || !strings.HasPrefix(arg, "-") {
			positional = append(positional, arg)
			continue
		}
		name, _, equal := strings.Cut(strings.TrimLeft(arg, "-"), "=")
		f := fs.Lookup(name)
		if f == nil {
			if name == "h" || name == "help" {
				return fs.Parse([]string{"-h"})
			}
			return inputError("Unknown command flag", "that flag is not supported", "pulse help "+strings.Fields(fs.Name())[0])
		}
		options = append(options, arg)
		boolean, isBool := f.Value.(interface{ IsBoolFlag() bool })
		if !equal && !(isBool && boolean.IsBoolFlag()) {
			if i+1 == len(args) {
				return inputError("Missing flag value", "the flag requires a value", "pulse help "+strings.Fields(fs.Name())[0])
			}
			i++
			options = append(options, args[i])
		}
	}
	if err := fs.Parse(append(append(options, "--"), positional...)); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return err
		}
		return inputError("Invalid command flags", "a flag value is not valid", "pulse help "+strings.Fields(fs.Name())[0])
	}
	return nil
}

func withStore(c *cli.Context, readOnly bool, fn func(*store.Store) error) (retErr error) {
	s := c.Store
	if s == nil {
		var err error
		s, err = store.Open(store.Options{Home: c.Home, ReadOnly: readOnly, Now: c.Now})
		if err != nil {
			return err
		}
		defer func() { retErr = errors.Join(retErr, s.Close()) }()
	}
	return fn(s)
}

func commandError(err error, command string) error {
	var ce *cli.Error
	if err == nil || errors.Is(err, flag.ErrHelp) || errors.As(err, &ce) {
		return err
	}
	if errors.Is(err, context.Canceled) {
		return cli.Errorf(130, command+" interrupted", "the context was cancelled; owned temporary clones were cleaned", "pulse evolve status")
	}
	var violation *ViolationError
	if errors.As(err, &violation) {
		return cli.Errorf(2, command+" stopped", violation.Error(), "pulse reflect")
	}
	var missing *store.NotFoundError
	if errors.As(err, &missing) {
		return cli.Errorf(2, command+" needs a store", "no store exists in the selected home", "pulse init PATH")
	}
	return cli.Errorf(1, command+" failed", err.Error(), "pulse help "+strings.Fields(command)[0])
}

func currentProject(s *store.Store, c *cli.Context) (*store.Project, error) {
	p, err := board.CurrentProject(s, c.Project)
	if err != nil {
		return nil, err
	}
	if p == nil || p.Slug == store.CommonsSlug {
		return nil, inputError("Select a registered project", "evolution needs a project checkout", "pulse projects")
	}
	return p, nil
}

func callingActor(s *store.Store, c *cli.Context) (string, error) {
	if c.Agent == "" {
		return store.HumanAgent, nil
	}
	agent, err := board.ResolveAgent(s, c.Agent, true)
	if err != nil {
		return "", err
	}
	return agent.Name, nil
}

func readRequest(s *store.Store, source string) (string, error) {
	var reader io.Reader
	if source == "-" {
		reader = Stdin
	} else {
		if source == "" {
			return "", inputError("A fixture request is required", "use a file or - for stdin", "pulse evolve fixture add --help")
		}
		file, err := canonicalPath(source)
		if err != nil {
			return "", inputError("Cannot read fixture request", "the request file cannot be resolved", "pulse evolve fixture add --request FILE --help")
		}
		if value := s.Setting("evolve.heldout_dir", ""); value != "" {
			private, err := canonicalPath(value)
			if err != nil || within(file, private) {
				return "", privateError("a private request cannot be imported as a public fixture")
			}
		}
		f, err := os.Open(file)
		if err != nil {
			return "", inputError("Cannot read fixture request", "the request file cannot be opened", "pulse evolve fixture add --request FILE --help")
		}
		defer f.Close()
		info, err := f.Stat()
		if err != nil || !info.Mode().IsRegular() {
			return "", inputError("Cannot read fixture request", "use a regular text file or stdin", "pulse evolve fixture add --request - --help")
		}
		reader = f
	}
	data, err := io.ReadAll(io.LimitReader(reader, maxFixtureBytes+1))
	if err != nil || len(data) > maxFixtureBytes {
		return "", inputError("Cannot read fixture request", "request must be readable and at most 1 MiB", "pulse evolve fixture add --request FILE --help")
	}
	return string(data), nil
}

func runEvolve(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		args = []string{"status"}
	}
	sub, args := args[0], args[1:]
	if sub == "-h" || sub == "--help" {
		return flag.ErrHelp
	}
	if sub == "fixture" {
		return commandError(runFixture(c, args), "evolve fixture")
	}
	c, fs := commandFlags(c, "evolve "+sub)
	var tree, ref string
	var selected repeatedStrings
	var shadow, once bool
	switch sub {
	case "measure":
		fs.StringVar(&tree, "tree", "", "local source checkout (committed tree only)")
		fs.StringVar(&ref, "ref", "HEAD", "local commit, branch or tag")
		fs.Var(&selected, "fixture", "enabled public fixture name (repeatable; private set is always included)")
		fs.BoolVar(&shadow, "shadow", false, "post scores as a finding; never archive")
	case "cycle":
		fs.BoolVar(&once, "once", false, "finish at most one experiment")
	case "status", "report", "gates", "archive", "setup":
	default:
		return inputError("Unknown evolve subcommand", "choose status, fixture, setup, measure, archive or cycle", "pulse help evolve")
	}
	if err := parseCommandFlags(fs, args); err != nil {
		return err
	}
	args = fs.Args()
	if (sub == "setup" && len(args) != 1) || (sub != "setup" && len(args) != 0) {
		return inputError("Wrong evolve arguments", "setup takes a directory; other commands take flags", "pulse help evolve")
	}
	readOnly := sub == "status" || sub == "report" || sub == "gates" || sub == "archive" || (sub == "measure" && !shadow)
	err := withStore(c, readOnly, func(s *store.Store) error {
		if sub == "archive" {
			if c.Project != "" {
				if _, err := validProject(s, c.Project); err != nil {
					return err
				}
			}
			rows, err := ListArchive(s, c.Project)
			if err != nil {
				return err
			}
			if c.JSON {
				return cli.WriteJSON(c.Out, map[string]any{"archive": rows})
			}
			if !c.Quiet {
				printArchive(c.Out, c.Color, rows)
			}
			return nil
		}
		if sub == "setup" {
			result, err := Setup(s, args[0])
			if err != nil {
				return err
			}
			if c.JSON {
				return cli.WriteJSON(c.Out, result)
			}
			c.Printf("Private fixture setup: %d ready, %d pending; %d template file(s) created. Values withheld.\n", result.Count, result.Pending, result.Created)
			if result.Pending > 0 {
				c.Println("Owner: fill the private templates, then run pulse evolve setup DIR again. Loop remains unchanged.")
			}
			return nil
		}
		p, err := currentProject(s, c)
		if err != nil {
			return err
		}
		switch sub {
		case "status", "report", "gates":
			status, err := Status(s, p.Slug)
			if err != nil {
				return err
			}
			if c.JSON {
				return cli.WriteJSON(c.Out, status)
			}
			if !c.Quiet {
				fmt.Fprintln(c.Out, "SELF-IMPROVEMENT LOOP -", p.Slug)
				cli.Table(c.Out, c.Color, nil, statusRows(status))
			}
			return nil
		case "measure":
			if shadow {
				if _, err := board.ResolveAgent(s, c.Agent, true); err != nil {
					return err
				}
			}
			if tree == "" {
				tree = p.Path
			}
			progress := c.Out
			if c.JSON {
				progress = c.Err
			}
			if c.Quiet {
				progress = io.Discard
			}
			ctx, cancel := signal.NotifyContext(context.Background(), stopSignals()...)
			defer cancel()
			scores, err := Measure(ctx, s, p.Slug, tree, ref, MeasureOptions{Fixtures: selected, Shadow: shadow, Out: progress})
			if err != nil {
				return err
			}
			finding := ""
			if shadow {
				body, err := json.Marshal(scores)
				if err != nil {
					return err
				}
				post, err := board.CreatePost(s, board.PostInput{
					Kind: "finding", Project: p.Slug, Area: "evolution", Thread: "shadow",
					Agent: c.Agent, Session: c.Session,
					Summary: fmt.Sprintf("Shadow measurement %.4f; %d held-out fixtures; not archived", scores.Total, scores.Heldout),
					Body:    string(body),
				})
				if err != nil {
					return err
				}
				finding = post.ID
			}
			if c.JSON {
				return cli.WriteJSON(c.Out, map[string]any{"scores": scores, "finding": finding})
			}
			if !c.Quiet {
				printScores(c.Out, c.Color, scores)
				if finding != "" {
					fmt.Fprintln(c.Out, "Finding:", finding, "(shadow; no archive row)")
				}
			}
			return nil
		case "cycle":
			ctx, cancel := signal.NotifyContext(context.Background(), stopSignals()...)
			defer cancel()
			out := c.Out
			if c.Quiet {
				out = io.Discard
			}
			return Cycle(ctx, s, p.Slug, CycleOptions{Once: once, Agent: c.Agent, Session: c.Session, Out: out, JSON: c.JSON})
		}
		return nil
	})
	return commandError(err, "evolve "+sub)
}

func runFixture(c *cli.Context, args []string) error {
	if len(args) == 0 {
		return inputError("Fixture command required", "choose add, list, disable or enable", "pulse help evolve")
	}
	sub, args := args[0], args[1:]
	c, fs := commandFlags(c, "evolve fixture "+sub)
	var name, request, scorer, kind string
	var weight float64
	var keys repeatedStrings
	switch sub {
	case "add":
		fs.StringVar(&name, "name", "", "public fixture name")
		fs.StringVar(&request, "request", "", "request file or - for stdin")
		fs.StringVar(&scorer, "scorer", "", "owner-owned read-only command; prints a score in [0,1]")
		fs.Float64Var(&weight, "weight", 1, "positive score weight")
		fs.StringVar(&kind, "kind", "public", "public or signal")
		fs.Var(&keys, "answer-key", "repository-relative answer-key path to strip (repeatable)")
	case "list", "disable", "enable":
	default:
		return inputError("Unknown fixture command", "choose add, list, disable or enable", "pulse help evolve")
	}
	if err := parseCommandFlags(fs, args); err != nil {
		return err
	}
	args = fs.Args()
	if ((sub == "enable" || sub == "disable") && len(args) != 1) || ((sub == "add" || sub == "list") && len(args) != 0) {
		return inputError("Wrong fixture arguments", "enable/disable take one name; add/list take flags", "pulse help evolve")
	}
	return withStore(c, sub == "list", func(s *store.Store) error {
		if sub == "list" {
			fixtures, err := ListFixtures(s, c.Project, false)
			if err != nil {
				return err
			}
			if c.JSON {
				return cli.WriteJSON(c.Out, map[string]any{"fixtures": fixtures})
			}
			rows := [][]string{}
			for _, f := range fixtures {
				rows = append(rows, []string{f.Project, f.Name, f.Kind, strconv.FormatFloat(f.Weight, 'g', -1, 64), strconv.FormatBool(f.Enabled)})
			}
			if !c.Quiet {
				cli.Table(c.Out, c.Color, []string{"PROJECT", "NAME", "KIND", "WEIGHT", "ENABLED"}, rows)
			}
			return nil
		}
		p, err := currentProject(s, c)
		if err != nil {
			return err
		}
		actor, err := callingActor(s, c)
		if err != nil {
			return err
		}
		if sub == "add" {
			body, err := readRequest(s, request)
			if err != nil {
				return err
			}
			f, err := AddFixture(s, FixtureInput{Project: p.Slug, Name: name, Request: body, Scorer: scorer, Kind: kind, Weight: weight, Actor: actor, AnswerKeys: keys})
			if err != nil {
				return err
			}
			if c.JSON {
				return cli.WriteJSON(c.Out, f)
			}
			c.Println("Fixture added:", f.Name)
			return nil
		}
		if err := SetFixtureEnabled(s, p.Slug, args[0], sub == "enable", actor); err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"name": args[0], "project": p.Slug, "enabled": sub == "enable"})
		}
		c.Println("Fixture", args[0]+":", sub+"d")
		return nil
	})
}

func runFleet(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		args = []string{"list"}
	}
	sub, args := args[0], args[1:]
	if sub == "-h" || sub == "--help" {
		return flag.ErrHelp
	}
	c, fs := commandFlags(c, "fleet "+sub)
	n, ref := 1, "HEAD"
	switch sub {
	case "prepare":
		fs.IntVar(&n, "n", 1, "workers in this wave (1-256)")
		fs.StringVar(&ref, "ref", "HEAD", "local commit, branch or tag")
	case "verify", "clean", "list":
	default:
		return inputError("Unknown fleet command", "choose prepare, verify, clean or list", "pulse help fleet")
	}
	if err := parseCommandFlags(fs, args); err != nil {
		return err
	}
	args = fs.Args()
	if ((sub == "verify" || sub == "clean") && len(args) != 1) || ((sub == "prepare" || sub == "list") && len(args) != 0) {
		return inputError("Wrong fleet arguments", "verify and clean take a fleet id", "pulse help fleet")
	}
	err := withStore(c, sub == "verify" || sub == "list", func(s *store.Store) error {
		switch sub {
		case "prepare":
			p, err := currentProject(s, c)
			if err != nil {
				return err
			}
			actor, err := callingActor(s, c)
			if err != nil {
				return err
			}
			ctx, cancel := signal.NotifyContext(context.Background(), stopSignals()...)
			defer cancel()
			fleet, err := PrepareFleet(ctx, s, p.Slug, ref, n, actor)
			if err != nil {
				return err
			}
			if c.JSON {
				return cli.WriteJSON(c.Out, fleet)
			}
			c.Println(fleet.ID)
			for _, path := range fleet.Paths {
				c.Println(path)
			}
		case "verify":
			report, err := VerifyFleet(context.Background(), s, args[0])
			if err != nil {
				return err
			}
			if c.JSON {
				if err := cli.WriteJSON(c.Out, report); err != nil {
					return err
				}
			} else if !c.Quiet {
				rows := [][]string{}
				for _, w := range report.Workers {
					result := "detached; no remote; no tags; answer keys absent"
					if w.Problem != "" {
						result = w.Problem
					}
					rows = append(rows, []string{strconv.Itoa(w.Worker), w.Path, result})
				}
				cli.Table(c.Out, c.Color, []string{"WORKER", "CLONE", "STATE"}, rows)
				rows = [][]string{}
				for _, j := range report.Jobs {
					rows = append(rows, []string{j.ID, j.Status, j.Result, j.Branch})
				}
				fmt.Fprintln(c.Out, "\nWORKER RESULTS (store jobs; no result files are read)")
				cli.Table(c.Out, c.Color, []string{"JOB", "STATUS", "RESULT", "BRANCH"}, rows)
			}
			if !report.OK {
				return inputError("Fleet verification failed", "one or more clones violated isolation constraints", "pulse fleet clean "+args[0])
			}
		case "clean":
			actor, err := callingActor(s, c)
			if err != nil {
				return err
			}
			if err := CleanFleet(s, args[0], actor); err != nil {
				return err
			}
			if c.JSON {
				return cli.WriteJSON(c.Out, map[string]any{"id": args[0], "cleaned": true})
			}
			c.Println("Cleaned fleet:", args[0])
		case "list":
			fleets, err := ListFleets(s, c.Project)
			if err != nil {
				return err
			}
			if c.JSON {
				return cli.WriteJSON(c.Out, map[string]any{"fleets": fleets})
			}
			rows := [][]string{}
			for _, f := range fleets {
				state := "prepared"
				if f.CleanedAt != "" {
					state = "cleaned"
				}
				rows = append(rows, []string{f.ID, f.Project, strconv.Itoa(f.Workers), f.Ref, state})
			}
			if !c.Quiet {
				cli.Table(c.Out, c.Color, []string{"ID", "PROJECT", "WORKERS", "REF", "STATE"}, rows)
			}
		}
		return nil
	})
	return commandError(err, "fleet "+sub)
}

func runReflect(c *cli.Context, _ *flag.FlagSet, args []string) error {
	c, fs := commandFlags(c, "reflect")
	area := fs.String("area", "", "board area")
	if err := parseCommandFlags(fs, args); err != nil {
		return err
	}
	if fs.NArg() != 0 {
		return inputError("Wrong reflection arguments", "reflect takes flags, not file paths", "pulse help reflect")
	}
	s := c.Store
	if s == nil {
		var err error
		s, err = store.Open(store.Options{Home: c.Home, ReadOnly: true, Now: c.Now})
		if err != nil {
			var missing *store.NotFoundError
			if !errors.As(err, &missing) {
				return commandError(err, "reflect")
			}
			s = nil
		} else {
			defer s.Close()
		}
	}
	result, err := Reflect(context.Background(), s, c.Project, *area, c.Agent, c.Session)
	if err != nil {
		return commandError(err, "reflect")
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, result)
	}
	if !c.Quiet {
		printReflection(c.Out, c.Color, result)
	}
	return nil
}

func runUsage(c *cli.Context, _ *flag.FlagSet, args []string) error {
	c, fs := commandFlags(c, "usage")
	runner := fs.String("runner", "", "copilot, claude, codex or opencode (default: all)")
	days := fs.Int("days", 7, "sessions updated within this many days; 0 = all history")
	if err := parseCommandFlags(fs, args); err != nil {
		return err
	}
	if fs.NArg() != 0 {
		return inputError("Wrong usage arguments", "select a runner with --runner", "pulse usage --help")
	}
	now := time.Time{}
	if c.Now != "" {
		var err error
		now, err = time.Parse(time.RFC3339, c.Now)
		if err != nil {
			return inputError("Invalid usage clock", "now must be UTC RFC3339", "pulse usage --days 7")
		}
	}
	reports, err := Usage(UsageOptions{Runner: *runner, Days: *days, Now: now})
	if err != nil {
		return commandError(err, "usage")
	}
	note := "Best-effort diagnostics, not billing. Days selects recently updated sessions; cumulative totals may include earlier turns."
	if c.JSON {
		return cli.WriteJSON(c.Out, map[string]any{"runners": reports, "days": *days, "note": note})
	}
	if c.Quiet {
		return nil
	}
	num := func(n *int64) string {
		if n == nil {
			return "unavailable"
		}
		return strconv.FormatInt(*n, 10)
	}
	for _, report := range reports {
		if report.Status == "not found" {
			fmt.Fprintln(c.Out, report.Runner+": not found")
		} else {
			fmt.Fprintln(c.Out, report.Runner)
			rows := [][]string{}
			for _, s := range report.Sessions {
				rows = append(rows, []string{s.ID, num(s.Input), num(s.Output), num(s.Cache), num(s.SubagentTotal), num(s.Total), s.Mode})
			}
			cli.Table(c.Out, c.Color, []string{"SESSION", "IN", "OUT", "CACHE", "SUBAGENT TOTAL", "TOTAL", "MODE"}, rows)
		}
		for _, warning := range report.Warnings {
			fmt.Fprintln(c.Out, " ", warning)
		}
	}
	fmt.Fprintln(c.Out, note)
	return nil
}

func printScores(w io.Writer, color bool, s *Scores) {
	rows := [][]string{}
	for _, f := range s.Fixtures {
		rows = append(rows, []string{f.Name, f.Kind, fmt.Sprintf("%.4f", f.Score), strconv.FormatFloat(f.Weight, 'g', -1, 64)})
	}
	cli.Table(w, color, []string{"FIXTURE", "KIND", "SCORE", "WEIGHT"}, rows)
	fmt.Fprintf(w, "Weighted total: %.4f; held-out count: %d\n", s.Total, s.Heldout)
	fmt.Fprintln(w, "Boundary: detached local clones; empty worker PULSE_HOME; no OS sandbox. Clones removed.")
}

func printArchive(w io.Writer, color bool, archive []Archived) {
	rows := [][]string{}
	for _, a := range archive {
		decision := "discarded"
		if a.Kept {
			decision = "kept"
		}
		rows = append(rows, []string{a.ID, a.Project, fmt.Sprintf("%.4f", a.BaselineScore), fmt.Sprintf("%.4f", a.VariantScore),
			fmt.Sprintf("%.4f", a.Tolerance), decision, a.SuggestionPost, a.RunID})
	}
	cli.Table(w, color, []string{"ID", "PROJECT", "BASELINE", "VARIANT", "TOLERANCE", "DECISION", "SUGGESTION", "RUN"}, rows)
}
