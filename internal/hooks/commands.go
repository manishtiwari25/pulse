package hooks

import (
	"context"
	"flag"
	"fmt"
	"io"
	"os"
	"strconv"
	"strings"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// stdin is what `hooks stop` reads; tests replace it.
var stdin io.Reader = os.Stdin

func init() {
	cli.Register(cli.Command{
		Name:    "hook",
		Group:   "agents",
		Summary: "scripts and webhooks that fire on store events: add, list, remove, enable, test",
		Usage: "pulse hook add <event-glob> --script PATH | --url URL [--project P]\n" +
			"pulse hook list [--all] [--json]\n" +
			"pulse hook deliveries <id> [--limit N] [--json]\n" +
			"pulse hook remove <id>      (disables; rows are never deleted)\n" +
			"pulse hook enable <id>\n" +
			"pulse hook test <id>        (fires it with a synthetic event)",
		Flags: func(fs *flag.FlagSet) {
			fs.String("script", "", "executable to run (env PULSE_EVENT_*, JSON on stdin, 30 s)")
			fs.String("url", "", "URL to POST the event JSON to (10 s)")
			fs.Bool("all", false, "list removed hooks too")
			fs.Int("limit", 50, "delivery rows to show")
		},
		Run: runHook,
	})
	cli.Register(cli.Command{
		Name:    "hooks",
		Group:   "human",
		Summary: "harness hooks for interactive agents: install copilot|claude (session start reads the board, stop reminds to reflect)",
		Usage: "pulse hooks install copilot|claude [--dir PATH] [--dry-run]\n" +
			"pulse hooks uninstall copilot|claude [--dir PATH] [--dry-run]\n" +
			"pulse hooks context --harness copilot|claude   (what the installed sessionStart hook runs)\n" +
			"pulse hooks stop --harness copilot|claude      (what the installed stop hook runs)",
		NoStore: true,
		Flags: func(fs *flag.FlagSet) {
			fs.String("dir", "", "repository to install into (default: the user-level location)")
			fs.Bool("dry-run", false, "print what would be written, write nothing")
			fs.String("harness", "copilot", "copilot or claude (context, stop)")
		},
		Run: runHooks,
	})
}

func str(fs *flag.FlagSet, name string) string { return fs.Lookup(name).Value.String() }
func boolean(fs *flag.FlagSet, name string) bool {
	return fs.Lookup(name).Value.String() == "true"
}

func actor(c *cli.Context) string {
	if c.Agent != "" {
		return c.Agent
	}
	return store.HumanAgent
}

func parseID(arg string) (int64, error) {
	id, err := strconv.ParseInt(arg, 10, 64)
	if err != nil {
		return 0, cli.Errorf(2, "A hook id is a number", "got "+strconv.Quote(arg), "pulse hook list")
	}
	return id, nil
}

func runHook(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		return cli.Errorf(2, "pulse hook needs a subcommand", "add, list, deliveries, remove, enable or test", "pulse help hook")
	}
	sub, rest := args[0], args[1:]
	switch sub {
	case "add":
		if len(rest) < 1 {
			return cli.Errorf(2, "Missing event glob", "the first argument names the actions the hook fires on", "pulse hook add 'task.*' --script PATH")
		}
		kind, target := "", ""
		if s := str(fs, "script"); s != "" {
			kind, target = "script", s
		}
		if u := str(fs, "url"); u != "" {
			if kind != "" {
				return cli.Errorf(2, "A hook is a --script or a --url", "not both", "pulse help hook")
			}
			kind, target = "webhook", u
		}
		h, err := Add(c.Store, rest[0], kind, target, c.Project, actor(c))
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"hook": h})
		}
		c.Printf("hook %d added: %s -> %s %s\n", h.ID, h.Event, h.Kind, h.Target)
		return nil
	case "list":
		list, err := List(c.Store, !boolean(fs, "all"))
		if err != nil {
			return err
		}
		if list == nil {
			list = []Hook{}
		}
		states, err := DeliveryStates(c.Store)
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"hooks": list, "deliveries": states})
		}
		if len(list) == 0 {
			c.Println("No hooks. Add one: pulse hook add 'task.*' --script PATH")
			return nil
		}
		rows := make([][]string, 0, len(list))
		for _, h := range list {
			on := "on"
			if !h.Enabled {
				on = "off"
			}
			state := states[h.ID]
			rows = append(rows, []string{strconv.FormatInt(h.ID, 10), on, h.Event, h.Project, h.Kind, h.Target, strconv.Itoa(h.Fires), strconv.Itoa(state.Pending), h.LastFiredAt, h.LastStatus, state.LastError})
		}
		cli.Table(c.Out, c.Color, []string{"ID", "ENABLED", "EVENT", "PROJECT", "KIND", "TARGET", "FIRES", "PENDING", "LAST", "STATUS", "LAST ERROR"}, rows)
		return nil
	case "deliveries":
		if len(rest) < 1 {
			return cli.Errorf(2, "Missing hook id", "which hook?", "pulse hook list")
		}
		id, err := parseID(rest[0])
		if err != nil {
			return err
		}
		if _, err := Get(c.Store, id); err != nil {
			return err
		}
		limit, _ := strconv.Atoi(fs.Lookup("limit").Value.String())
		deliveries, err := Deliveries(c.Store, id, limit)
		if err != nil {
			return err
		}
		if deliveries == nil {
			deliveries = []Delivery{}
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"deliveries": deliveries})
		}
		rows := make([][]string, 0, len(deliveries))
		for _, d := range deliveries {
			rows = append(rows, []string{strconv.FormatInt(d.ID, 10), strconv.FormatInt(d.EventSeq, 10), d.EventAction, strconv.Itoa(d.Attempt), d.Status, d.NextAt, d.Detail})
		}
		cli.Table(c.Out, c.Color, []string{"ID", "EVENT", "ACTION", "ATTEMPT", "STATUS", "NEXT", "DETAIL"}, rows)
		return nil
	case "remove", "enable":
		if len(rest) < 1 {
			return cli.Errorf(2, "Missing hook id", "which hook?", "pulse hook list")
		}
		id, err := parseID(rest[0])
		if err != nil {
			return err
		}
		h, err := SetEnabled(c.Store, id, sub == "enable", actor(c))
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"hook": h})
		}
		if sub == "enable" {
			c.Printf("hook %d enabled\n", h.ID)
		} else {
			c.Printf("hook %d removed (disabled; pulse hook enable %d brings it back)\n", h.ID, h.ID)
		}
		return nil
	case "test":
		if len(rest) < 1 {
			return cli.Errorf(2, "Missing hook id", "which hook?", "pulse hook list")
		}
		id, err := parseID(rest[0])
		if err != nil {
			return err
		}
		h, err := Get(c.Store, id)
		if err != nil {
			return err
		}
		// a synthetic action that matches the glob: task.* -> task.test
		action := strings.NewReplacer("*", "test", "?", "x").Replace(h.Event)
		if strings.ContainsAny(action, "[]") || action == "" {
			action = "hook.test"
		}
		e := bus.Event{At: c.Store.Now(), Actor: actor(c), Action: action, Subject: "hook-test", Detail: `{"project": "` + h.Project + `", "test": true}`}
		status, err := Fire(context.Background(), c.Store, *h, e)
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"hook": h.ID, "status": status, "ok": err == nil})
		}
		if err != nil {
			return cli.Errorf(1, fmt.Sprintf("hook %d failed", h.ID), err.Error(), "fix the script or the URL, then pulse hook test "+rest[0])
		}
		c.Printf("hook %d fired: %s\n", h.ID, status)
		return nil
	}
	return cli.Errorf(2, "pulse hook: unknown subcommand "+strconv.Quote(sub), "add, list, deliveries, remove, enable or test", "pulse help hook")
}

func runHooks(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		return cli.Errorf(2, "pulse hooks needs a subcommand", "install, uninstall, context or stop", "pulse help hooks")
	}
	sub, rest := args[0], args[1:]
	switch sub {
	case "install", "uninstall":
		if len(rest) < 1 {
			return cli.Errorf(2, "Which harness?", "copilot or claude", "pulse hooks "+sub+" copilot")
		}
		var res *Install
		var err error
		if sub == "install" {
			res, err = InstallHarness(rest[0], str(fs, "dir"), boolean(fs, "dry-run"))
		} else {
			res, err = UninstallHarness(rest[0], str(fs, "dir"), boolean(fs, "dry-run"))
		}
		if err != nil {
			return cli.Errorf(2, "Cannot "+sub+" "+rest[0]+" hooks", err.Error(), "pulse help hooks")
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, res)
		}
		st := c.Style()
		switch {
		case sub == "install" && !res.Changed:
			c.Printf("%s already has the PULSE hooks; nothing written.\n", res.Path)
		case sub == "install" && boolean(fs, "dry-run"):
			c.Printf("%s %s\n\n%s\n", st.Bold("would write"), res.Path, res.Content)
		case sub == "install":
			c.Printf("%s %s\n\n%s\n", st.Bold("wrote"), res.Path, res.Content)
		case !res.Changed:
			c.Printf("%s has no PULSE hooks; nothing to remove.\n", res.Path)
		case boolean(fs, "dry-run"):
			c.Printf("%s the PULSE hooks from %s\n", st.Bold("would remove"), res.Path)
		default:
			c.Printf("%s the PULSE hooks from %s\n", st.Bold("removed"), res.Path)
		}
		if sub == "install" {
			c.Printf("What it does: session start -> `pulse guide` + `pulse board` become context; stop -> one reminder per session to record findings.\n")
			c.Printf("Takes effect on the next %s session. Undo: %s\n", rest[0], res.Undo)
		}
		return nil
	case "context":
		harness := str(fs, "harness")
		home := c.Home
		fmt.Fprintln(c.Out, ContextJSON(harness, func(args ...string) string { return RunPulse(home, args...) }))
		return nil
	case "stop":
		// best effort, read-only: the reminder may name what this store
		// wants recorded; with no store it falls back to the static text
		var s *store.Store
		if opened, err := store.Open(store.Options{Home: c.Home, ReadOnly: true}); err == nil {
			s = opened
			defer s.Close()
		}
		fmt.Fprintln(c.Out, StopJSON(str(fs, "harness"), c.Home, s, stdin))
		return nil
	}
	return cli.Errorf(2, "pulse hooks: unknown subcommand "+strconv.Quote(sub), "install, uninstall, context or stop", "pulse help hooks")
}
