package records

import (
	"errors"
	"flag"
	"io"
	"os"
	"strconv"
	"strings"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func init() {
	cli.Register(cli.Command{
		Name: "record", Group: "records", Raw: true,
		Summary: "list, read, create, revise, accept, deliver, or export records",
		Usage:   "pulse record list|show|new|amend|status|accept|deliver|export|lead-time|revisions [flags] [args]",
		Run:     runRecord,
	})
	cli.Register(cli.Command{
		Name: "guide", Group: "records", Raw: true, ReadOnly: true,
		Summary: "read the stored agent operating instructions",
		Usage:   "pulse guide [--full] [--project P] [--json]",
		Run:     runGuide,
	})
	cli.Register(cli.Command{
		Name: "migrate", Group: "records", Raw: true, NoStore: true,
		Summary: "import a repository's committed Markdown records into the store (the schema migrates itself on open)",
		Usage:   "pulse migrate [--records] --repo PATH [--ref HEAD] [--project P] [--dry-run] [--force-rev]",
		Run:     runMigrate,
	})
}

func runRecord(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		return inputError("Record command required", "choose list, show, new, amend, status, accept, deliver, export, lead-time, or revisions", "pulse help record")
	}
	sub, args := args[0], args[1:]
	fs := flag.NewFlagSet("record "+sub, flag.ContinueOnError)
	fs.SetOutput(c.Err)
	jsonOut := c.JSON
	fs.BoolVar(&jsonOut, "json", c.JSON, "print JSON")
	project := c.Project
	var kind, status, area, title, areas, tags, bodyFile, note, words, date string
	var revision, tier int
	var force bool
	switch sub {
	case "list":
		fs.StringVar(&project, "project", project, "project slug")
		fs.StringVar(&status, "status", "", "status")
		fs.StringVar(&area, "area", "", "area")
	case "show":
		fs.IntVar(&revision, "rev", 0, "revision (0 = current)")
	case "new":
		fs.StringVar(&project, "project", project, "project slug")
		fs.StringVar(&title, "title", "", "title")
		fs.StringVar(&status, "status", "", "status")
		fs.StringVar(&areas, "areas", "", "comma-separated areas")
		fs.StringVar(&tags, "tags", "", "comma-separated tags")
		fs.IntVar(&tier, "tier", 0, "lifecycle tier")
		fs.StringVar(&bodyFile, "body-file", "", "file, stdin, or -")
	case "amend":
		fs.StringVar(&bodyFile, "body-file", "", "file, stdin, or -")
		fs.StringVar(&note, "note", "", "revision note")
	case "accept":
		fs.StringVar(&words, "words", "", "the human's acceptance words")
	case "deliver":
		fs.StringVar(&date, "date", "", "delivery date (YYYY-MM-DD)")
	case "export":
		fs.StringVar(&kind, "kind", "", "record kind")
		fs.StringVar(&project, "project", project, "project slug")
		fs.BoolVar(&force, "force", false, "allow export inside a git checkout")
	case "lead-time":
		fs.StringVar(&project, "project", project, "project slug")
	case "status", "revisions":
	default:
		return inputError("Unknown record command "+sub, "choose a supported record command", "pulse help record")
	}
	if err := parseFlags(fs, args); err != nil {
		return err
	}
	args = fs.Args()
	if sub == "list" {
		if len(args) > 1 {
			return usageError(sub, "list [kind] [--project P] [--status S] [--area A] [--json]")
		}
		if len(args) == 1 {
			kind = args[0]
		}
		rows, err := List(c.Store, store.RecordFilter{Project: project, Kind: kind, Status: status, Area: area})
		if err != nil {
			return err
		}
		if jsonOut {
			return cli.WriteJSON(c.Out, map[string]any{"records": rows})
		}
		table := make([][]string, 0, len(rows))
		for _, row := range rows {
			table = append(table, []string{row.ID, row.Project, row.Status, row.Delivery, row.Title})
		}
		if !c.Quiet {
			cli.Table(c.Out, c.Color, []string{"ID", "PROJECT", "STATUS", "DELIVERY", "TITLE"}, table)
		}
		return nil
	}
	if sub == "lead-time" {
		if len(args) != 0 {
			return usageError(sub, "lead-time [--project P] [--json]")
		}
		rows, err := LeadTime(c.Store, project)
		if err != nil {
			return err
		}
		if jsonOut {
			return cli.WriteJSON(c.Out, map[string]any{"records": rows})
		}
		table := make([][]string, 0, len(rows))
		for _, r := range rows {
			table = append(table, []string{r.Kind, r.ID, strconv.Itoa(r.Tier), r.Status, r.Delivery, r.Proposed, r.Delivered, leadDays(r), r.Target})
		}
		if !c.Quiet {
			cli.Table(c.Out, c.Color, []string{"KIND", "ID", "TIER", "STATUS", "DELIVERY", "PROPOSED", "DELIVERED", "DAYS", "TARGET"}, table)
		}
		return nil
	}
	want := 1
	if sub == "status" || sub == "deliver" {
		want = 2
	}
	if len(args) != want {
		return usageError(sub, sub+" <ID or kind or directory> [flags]")
	}
	id := args[0]
	switch sub {
	case "show":
		if revision < 0 {
			return inputError("Cannot show revision", "--rev must be zero or a positive number", "pulse record revisions "+id)
		}
		rec, rev, err := Get(c.Store, id, revision)
		if err != nil {
			return recordError(id, err)
		}
		if jsonOut {
			revisions, err := c.Store.Revisions(id)
			if err != nil {
				return err
			}
			return cli.WriteJSON(c.Out, map[string]any{"record": rec, "frontmatter": rev.Frontmatter, "body": rev.Body, "rev": rev.Rev, "revisions": revisions})
		}
		if !c.Quiet {
			_, err = io.WriteString(c.Out, RenderMarkdown(rec, rev))
		}
		return err
	case "new":
		opts := NewOptions{Areas: splitCSV(areas), Tags: splitCSV(tags), Tier: tier, Status: status, Author: author(c)}
		fs.Visit(func(f *flag.Flag) {
			if f.Name == "tier" {
				opts.TierSet = true
			}
		})
		if bodyFile != "" {
			var err error
			opts.Body, err = readBody(bodyFile)
			if err != nil {
				return err
			}
			opts.BodySet = true
		}
		rec, err := New(c.Store, id, project, title, opts)
		if err != nil {
			return err
		}
		return printRecordWrite(c, rec, jsonOut)
	case "amend":
		if bodyFile == "" {
			return usageError(sub, "amend "+id+" --body-file F|stdin [--note TEXT]")
		}
		body, err := readBody(bodyFile)
		if err != nil {
			return err
		}
		if strings.TrimSpace(body) == "" {
			return inputError("Empty record body", "the body file has no content", "pulse record amend ID --body-file F with a non-empty file, or stdin")
		}
		rec, err := Amend(c.Store, id, body, author(c), note)
		if err != nil {
			return recordError(id, err)
		}
		return printRecordWrite(c, rec, jsonOut)
	case "status", "accept", "deliver":
		var err error
		switch sub {
		case "status":
			err = Status(c.Store, id, args[1], author(c))
		case "accept":
			err = Accept(c.Store, id, words)
		case "deliver":
			err = Deliver(c.Store, id, args[1], date, author(c))
		}
		if err != nil {
			return recordError(id, err)
		}
		rec, _, err := c.Store.GetRecord(id, 0)
		if err != nil {
			return err
		}
		return printRecordWrite(c, rec, jsonOut)
	case "export":
		n, err := Export(c.Store, id, ExportOptions{Kind: kind, Project: project, Force: force})
		if err != nil {
			return err
		}
		if jsonOut {
			return cli.WriteJSON(c.Out, map[string]any{"directory": id, "exported": n})
		}
		c.Printf("Exported %d records to %s\n", n, id)
		return nil
	case "revisions":
		if _, _, err := c.Store.GetRecord(id, 0); err != nil {
			return recordError(id, err)
		}
		rows, err := c.Store.Revisions(id)
		if err != nil {
			return err
		}
		if jsonOut {
			return cli.WriteJSON(c.Out, map[string]any{"revisions": rows})
		}
		table := make([][]string, 0, len(rows))
		for _, row := range rows {
			table = append(table, []string{strconv.Itoa(row.Rev), row.Author, row.At, row.Note})
		}
		if !c.Quiet {
			cli.Table(c.Out, c.Color, []string{"REV", "AUTHOR", "AT", "NOTE"}, table)
		}
	}
	return nil
}

func runMigrate(c *cli.Context, _ *flag.FlagSet, args []string) error {
	fs := flag.NewFlagSet("migrate", flag.ContinueOnError)
	fs.SetOutput(c.Err)
	fs.Bool("records", true, "accepted for compatibility: the import is the only migration left")
	repo := fs.String("repo", ".", "source repository")
	ref := fs.String("ref", "HEAD", "source git ref")
	project := fs.String("project", c.Project, "project slug")
	dryRun := fs.Bool("dry-run", false, "preview without writes")
	forceRev := fs.Bool("force-rev", false, "revise already imported paths whose body changed")
	jsonOut := fs.Bool("json", c.JSON, "print JSON")
	if err := parseFlags(fs, args); err != nil {
		return err
	}
	if fs.NArg() != 0 {
		return inputError("Unexpected migrate arguments", strings.Join(fs.Args(), " "), "pulse migrate --records --repo .")
	}
	s := c.Store
	if s == nil {
		var err error
		s, err = store.Open(store.Options{Home: c.Home, ReadOnly: *dryRun, Now: c.Now})
		if err != nil {
			return err
		}
		defer s.Close()
	}
	summary, err := ImportRepo(s, *repo, *ref, *project, ImportOptions{DryRun: *dryRun, ForceRev: *forceRev, Author: author(c)})
	if err != nil {
		return err
	}
	if *jsonOut {
		return cli.WriteJSON(c.Out, summary)
	}
	if c.Quiet {
		return nil
	}
	if summary.DryRun {
		c.Println("Dry run: no records or events written.")
	}
	table := [][]string{}
	for _, kind := range store.RecordKinds {
		count := summary.Kinds[kind]
		table = append(table, []string{kind, strconv.Itoa(count.Imported), strconv.Itoa(count.Skipped), strconv.Itoa(count.Revised)})
	}
	cli.Table(c.Out, c.Color, []string{"KIND", "IMPORTED", "SKIPPED", "REVISED"}, table)
	if len(summary.Skipped) > 0 {
		c.Println("\nSkipped files:")
		for _, skipped := range summary.Skipped {
			c.Printf("  %s: %s\n", skipped.Path, skipped.Reason)
		}
	}
	return nil
}

func runGuide(c *cli.Context, _ *flag.FlagSet, args []string) error {
	fs := flag.NewFlagSet("guide", flag.ContinueOnError)
	fs.SetOutput(c.Err)
	full := fs.Bool("full", false, "include Claude and Copilot instructions")
	project := fs.String("project", c.Project, "project slug")
	jsonOut := fs.Bool("json", c.JSON, "print JSON")
	if err := parseFlags(fs, args); err != nil {
		return err
	}
	if fs.NArg() != 0 {
		return inputError("Unexpected guide arguments", strings.Join(fs.Args(), " "), "pulse guide --full")
	}
	var p *store.Project
	var err error
	if *project != "" {
		p, err = resolveProject(c.Store, *project, "")
	} else {
		p, err = c.Store.ProjectForDir(store.Cwd())
		if err == nil && p == nil {
			p, err = c.Store.Brain()
		}
	}
	if err != nil {
		return err
	}
	slugs := []string{"agents"}
	if *full {
		slugs = append(slugs, "claude", "copilot-instructions")
	}
	output := []map[string]any{}
	var texts []string
	if p != nil {
		for _, slug := range slugs {
			rec, rev, err := c.Store.GetRecord(scopedID("instruction", slug, p), 0)
			if errors.Is(err, store.ErrNoRecord) {
				continue
			}
			if err != nil {
				return err
			}
			texts = append(texts, RenderMarkdown(rec, rev))
			output = append(output, map[string]any{"record": rec, "frontmatter": rev.Frontmatter, "body": rev.Body})
		}
	}
	hint := "No instruction records; run pulse migrate --records --repo PATH."
	if *jsonOut {
		result := map[string]any{"records": output}
		if len(output) == 0 {
			result["hint"] = hint
		}
		return cli.WriteJSON(c.Out, result)
	}
	if len(texts) == 0 {
		c.Println(hint)
		return nil
	}
	if !c.Quiet {
		_, err = io.WriteString(c.Out, strings.Join(texts, "\n\n"))
	}
	return err
}

// The standard flag package stops at the first positional argument. Commands
// accept flags on either side of IDs/kinds, without treating flag values as flags.
func parseFlags(fs *flag.FlagSet, args []string) error {
	flags, positional := []string{}, []string{}
	for i := 0; i < len(args); i++ {
		arg := args[i]
		if arg == "--" {
			positional = append(positional, args[i+1:]...)
			break
		}
		if !strings.HasPrefix(arg, "-") || arg == "-" {
			positional = append(positional, arg)
			continue
		}
		name, _, equal := strings.Cut(strings.TrimLeft(arg, "-"), "=")
		f := fs.Lookup(name)
		if f == nil {
			if name == "h" || name == "help" {
				return fs.Parse([]string{"-h"})
			}
			return inputError("Unknown flag "+arg, "this command does not accept that flag", "pulse help record")
		}
		flags = append(flags, arg)
		boolean, ok := f.Value.(interface{ IsBoolFlag() bool })
		if !equal && !(ok && boolean.IsBoolFlag()) {
			if i+1 == len(args) {
				return inputError("Missing value for "+arg, "the flag requires a value", "pulse help record")
			}
			i++
			flags = append(flags, args[i])
		}
	}
	return fs.Parse(append(append(flags, "--"), positional...))
}

func splitCSV(s string) []string {
	out := []string{}
	for _, part := range strings.Split(s, ",") {
		if part = strings.TrimSpace(part); part != "" {
			out = append(out, part)
		}
	}
	return out
}

func readBody(file string) (string, error) {
	if file == "stdin" || file == "-" {
		b, err := io.ReadAll(os.Stdin)
		return string(b), err
	}
	b, err := os.ReadFile(file)
	if err != nil {
		return "", inputError("Cannot read record body", err.Error(), "pulse record amend ID --body-file stdin")
	}
	return string(b), nil
}

func printRecordWrite(c *cli.Context, rec *store.Record, jsonOut bool) error {
	if jsonOut {
		return cli.WriteJSON(c.Out, map[string]any{"record": rec})
	}
	c.Printf("%s revision %d: %s\n", rec.ID, rec.CurrentRev, rec.Status)
	return nil
}

func recordError(id string, err error) error {
	if errors.Is(err, store.ErrNoRecord) {
		return inputError("Unknown record "+id, "no record has this id", "pulse record list")
	}
	return err
}

func usageError(sub, usage string) error {
	return inputError("Invalid record "+sub+" arguments", "expected "+usage, "pulse help record")
}

// author is the agent this process acts as, else the human (as the board does).
func author(c *cli.Context) string {
	if c.Agent != "" {
		return c.Agent
	}
	return store.HumanAgent
}
