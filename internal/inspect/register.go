// Package inspect implements the read-only store views and the human's settings
// and runner kill switch.
package inspect

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"strings"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func init() {
	for _, cmd := range []cli.Command{
		{Name: "status", Group: "inspect", Summary: "one screen: store, projects, agents, jobs, runner", Usage: "pulse status [--json]", ReadOnly: true, Run: runStatus},
		{Name: "projects", Group: "inspect", Summary: "registered projects, including inactive projects", Usage: "pulse projects [--json]", ReadOnly: true, Run: runProjects},
		{Name: "jobs", Group: "inspect", Summary: "the runner's queue", Usage: "pulse jobs [--project P] [--all] [--json]", ReadOnly: true, Flags: allFlag, Run: runJobs},
		{Name: "agents", Group: "inspect", Summary: "agent scorecards; --all includes pruned agents", Usage: "pulse agents [--all] [--json]", ReadOnly: true, Flags: allFlag, Run: runAgents},
		{Name: "events", Group: "inspect", Summary: "the append-only event log, newest first", Usage: "pulse events [--limit N] [--project P] [--since SEQ|TIME] [--json]", ReadOnly: true, Flags: func(fs *flag.FlagSet) {
			fs.Int("limit", 40, "maximum events; a negative value means all")
			fs.String("since", "", "events after this sequence number or UTC timestamp")
		}, Run: runEvents},
		{Name: "settings", Group: "human", Summary: "read or change a setting", Usage: settingsUsage(), NoStore: true, Run: runSettings},
		{Name: "schema", Group: "inspect", Summary: "print the schema", Usage: "pulse schema [--json]", NoStore: true, Run: runSchema},
		{Name: "verify", Group: "inspect", Summary: "verify a commit's signature and trailers against its run", Usage: "pulse verify <ref> [--project P] [--json]", ReadOnly: true, Run: runVerify},
		{Name: "signers", Group: "inspect", Summary: "an allowed-signers file for every run's key", Usage: "pulse signers [--project P] [--json]", ReadOnly: true, Run: runSigners},
		{Name: "stop", Group: "human", Summary: "stop the runner using the store's kill switch", Usage: "pulse stop", Run: runStop},
		{Name: "start", Group: "human", Summary: "let the runner run again", Usage: "pulse start", Run: runStart},
	} {
		cli.Register(cmd)
	}
}

func allFlag(fs *flag.FlagSet) {
	fs.Bool("all", false, "include finished jobs or pruned agents")
}

func boolFlag(fs *flag.FlagSet, name string) bool {
	return fs.Lookup(name).Value.(flag.Getter).Get().(bool)
}

func stringFlag(fs *flag.FlagSet, name string) string {
	return fs.Lookup(name).Value.String()
}

func noArgs(args []string) error {
	if len(args) != 0 {
		return cli.Errorf(2, "pulse: unrecognized arguments: "+strings.Join(args, " "), "", "")
	}
	return nil
}

func actor(c *cli.Context) string {
	if c.Agent != "" {
		return c.Agent
	}
	if a := os.Getenv("PULSE_AGENT"); a != "" {
		return a
	}
	return store.HumanAgent
}

func inputError(err error) error {
	var in *store.InputError
	if errors.As(err, &in) {
		return cli.Errorf(2, "pulse: "+in.Error(), "", "")
	}
	return err
}

func jsonOutput(c *cli.Context, value any) error {
	if c.Quiet {
		return nil
	}
	return cli.WriteJSON(c.Out, value)
}

func runSchema(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if err := noArgs(args); err != nil {
		return err
	}
	if c.JSON {
		return jsonOutput(c, struct {
			Schema string `json:"schema"`
		}{store.DDL})
	}
	c.Printf("%s", store.DDL)
	return nil
}

func runStop(c *cli.Context, _ *flag.FlagSet, args []string) error {
	return setStopped(c, args, true)
}

func runStart(c *cli.Context, _ *flag.FlagSet, args []string) error {
	return setStopped(c, args, false)
}

func setStopped(c *cli.Context, args []string, stopped bool) error {
	if err := noArgs(args); err != nil {
		return err
	}
	value, state := "0", "allowed"
	if stopped {
		value, state = "1", "stopped"
	}
	if err := c.Store.SetSetting(actor(c), "stop", value); err != nil {
		return inputError(err)
	}
	if c.JSON {
		return jsonOutput(c, struct {
			Stopped bool `json:"stopped"`
		}{stopped})
	}
	c.Printf("runner %s\n", state)
	return nil
}

func trunc(text string, n int) string {
	runes := []rune(text)
	if len(runes) > n {
		return string(runes[:n-1]) + "~"
	}
	return text
}

func orDash(text string) string {
	if text == "" {
		return "-"
	}
	return text
}

func currentProject(c *cli.Context) (*store.Project, error) {
	explicit := c.Project
	if explicit == "" {
		explicit = os.Getenv("PULSE_PROJECT")
	}
	if explicit == store.CommonsSlug {
		return &store.Project{Slug: explicit, Name: "the commons"}, nil
	}
	if explicit != "" {
		p, err := c.Store.Project(explicit)
		if err != nil {
			return nil, err
		}
		if p == nil {
			return nil, cli.Errorf(2, fmt.Sprintf("pulse: No project %s (pulse projects lists them)", pyRepr(explicit)), "", "")
		}
		return p, nil
	}
	dir, err := os.Getwd()
	if err != nil {
		return nil, err
	}
	p, err := c.Store.ProjectForDir(dir)
	if err != nil {
		return nil, err
	}
	if p == nil {
		return nil, cli.Errorf(2, "pulse: The current directory is not inside a registered project; pass --project SLUG, set PULSE_PROJECT, or run `pulse init` here", "", "")
	}
	return p, nil
}

func pyRepr(text string) string {
	quote := "'"
	if strings.Contains(text, "'") && !strings.Contains(text, `"`) {
		quote = `"`
	}
	text = strings.NewReplacer(`\`, `\\`, "\n", `\n`, "\r", `\r`, "\t", `\t`, quote, `\`+quote).Replace(text)
	return quote + text + quote
}
