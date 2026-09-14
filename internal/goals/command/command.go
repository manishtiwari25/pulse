// Package command registers the owner goal CLI without making goals depend on board.
package command

import (
	"flag"
	"fmt"
	"os"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/goals"
	"github.com/manishtiwari25/pulse/internal/store"
)

func init() {
	cli.Register(cli.Command{
		Name: "goal", Group: "human", NoStore: true,
		Summary: "read the shared project goal, or set/reset it as the owner",
		Usage: "pulse goal [show] [--project SLUG] [--json]\n" +
			"       pulse goal set --text TEXT [--project SLUG] [--json]\n" +
			"       pulse goal reset [--project SLUG] [--json]",
		Flags: func(fs *flag.FlagSet) { fs.String("text", "", "owner goal (1..4000 Unicode code points)") },
		Run:   run,
	})
}

func run(c *cli.Context, fs *flag.FlagSet, args []string) error {
	sub := "show"
	if len(args) == 1 {
		sub = args[0]
	}
	textGiven := false
	fs.Visit(func(f *flag.Flag) {
		if f.Name == "text" {
			textGiven = true
		}
	})
	if len(args) > 1 || (sub != "show" && sub != "set" && sub != "reset") || (textGiven != (sub == "set")) {
		return cli.Errorf(2, "Invalid goal command", "Use show, set --text TEXT, or reset.", "pulse help goal")
	}
	actor := c.Agent
	if actor == "" {
		actor = os.Getenv("PULSE_AGENT")
	}
	if actor == "" {
		actor = store.HumanAgent
	}
	s, home := c.Store, c.Home
	if s != nil {
		home = s.Home
	}
	if sub != "show" {
		if err := goals.RequireOwner(home, actor); err != nil {
			return err
		}
	}
	if s == nil {
		var err error
		s, err = store.Open(store.Options{Home: home, ReadOnly: sub == "show", SkipRecords: true, Now: c.Now})
		if err != nil {
			return err
		}
		defer s.Close()
	}
	p, err := board.CurrentProject(s, c.Project)
	if err != nil {
		return err
	}
	var goal goals.Goal
	switch sub {
	case "show":
		goal, err = goals.Get(s, p.Slug)
	case "set":
		goal, err = goals.Set(s, p.Slug, fs.Lookup("text").Value.String(), actor)
	case "reset":
		goal, err = goals.Reset(s, p.Slug, actor)
	}
	if err != nil {
		return err
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, goals.View{Project: p.Slug, Goal: goal, CanEdit: goals.CanEdit(s, p, actor)})
	}
	if c.Quiet {
		return nil
	}
	_, err = fmt.Fprint(c.Out, goals.Context(goal))
	return err
}
