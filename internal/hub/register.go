// Package hub registers its pulse commands in init(); the placeholder keeps the build green until they land.
package hub

import (
	"context"
	"flag"
	"fmt"
	"net"
	"os"
	"os/signal"
	"strconv"
	"sync"
	"syscall"
	"time"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/records"
	"github.com/manishtiwari25/pulse/internal/store"
)

// restartPoll is how often a service-run hub looks at its own binary; tests shorten it.
var restartPoll = 30 * time.Second

func init() {
	for _, name := range []string{"hub", "serve"} {
		cli.Register(cli.Command{
			Name: name, Group: "hub", Summary: "Serve the local owner console",
			Usage: "pulse " + name + " [--project SLUG] [--port N] [--no-browser] [--host 127.0.0.1] [--restart-on-change]",
			Flags: func(fs *flag.FlagSet) {
				fs.Int("port", 8765, "TCP port (0 chooses a free port)")
				fs.String("host", "127.0.0.1", "listen address (127.0.0.1 only)")
				fs.Bool("no-browser", false, "do not open the browser")
				fs.Bool("restart-on-change", false, "exit 3 when the pulse binary changes on disk (the user service restarts it)")
				fs.String("project", "", "project slug for serve")
			},
			Run: func(c *cli.Context, fs *flag.FlagSet, args []string) error {
				if len(args) != 0 {
					return cli.Errorf(2, "Unexpected arguments", "hub and serve take flags only", "pulse help "+name)
				}
				host := fs.Lookup("host").Value.String()
				if host != "127.0.0.1" {
					return cli.Errorf(2, "Remote hub serving is not supported", "The owner write routes are loopback-only; hub.allow_remote is not supported yet", "pulse "+name+" --host 127.0.0.1")
				}
				port, _ := strconv.Atoi(fs.Lookup("port").Value.String())
				if port < 0 || port > 65535 {
					return cli.Errorf(2, "Invalid port", "The port must be between 0 and 65535", "pulse "+name+" --port 8765")
				}
				// ADR-038: a store the console opens has the framework's records before anything reads them.
				if seeded, err := records.EnsureFramework(c.Store, store.HumanAgent); err != nil {
					fmt.Fprintf(c.Err, "framework records: %v\n", err)
				} else if len(seeded.Written) > 0 {
					fmt.Fprintf(c.Out, "seeded %d framework record(s) from %s\n", len(seeded.Written), seeded.From)
				}
				h := New(c.Store, nil)
				h.Service = fs.Lookup("restart-on-change").Value.String() == "true"
				if name == "serve" {
					h.Project = fs.Lookup("project").Value.String()
					if h.Project == "" {
						h.Project = c.Project
					}
					if h.Project == "" {
						cwd, err := os.Getwd()
						if err != nil {
							return err
						}
						p, err := c.Store.ProjectForDir(cwd)
						if err != nil {
							return err
						}
						if p != nil {
							h.Project = p.Slug
						}
					}
					if h.Project == "" {
						return cli.Errorf(2, "No project selected", "This directory is not a registered project", "pulse serve --project SLUG")
					}
					if _, err := h.entry(h.Project); err != nil {
						return err
					}
				}
				listener, err := net.Listen("tcp4", net.JoinHostPort(host, strconv.Itoa(port)))
				if err != nil {
					return cli.Errorf(1, "The hub could not listen", err.Error(), "pulse "+name+" --port 0")
				}
				defer listener.Close()
				if addr, ok := listener.Addr().(*net.TCPAddr); ok {
					h.Port = addr.Port
				}
				address := "http://" + listener.Addr().String()
				fmt.Fprintf(c.Out, "Serving on %s (127.0.0.1 only; writes: decide, settings, tasks, messages, compute; Ctrl-C to stop)\n", address)
				if fs.Lookup("no-browser").Value.String() != "true" {
					if err := openBrowser(address); err != nil {
						fmt.Fprintf(c.Err, "Could not open the browser: %v; open %s manually\n", err, address)
					}
				}
				ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
				defer cancel()
				changed := make(chan struct{}, 1)
				if fs.Lookup("restart-on-change").Value.String() == "true" {
					go watchBinary(ctx, cli.BinaryStamp(), func() {
						fmt.Fprintln(c.Err, "pulse changed on disk; exiting 3 so the service restarts the hub")
						changed <- struct{}{}
						cancel()
					})
				}
				// ADR-038: the console installed the hub service from this foreground hub;
				// stop serving so the service's hub can bind this port, then load its unit.
				var handover func() error
				var handoverOnce sync.Once
				h.Handover = func(loadHub func() error) {
					handoverOnce.Do(func() {
						handover = loadHub
						// Let the response reach the page before the listener closes.
						time.Sleep(handoverDelay)
						fmt.Fprintln(c.Out, "handing over to the pulse.hub service")
						cancel()
					})
				}
				err = Serve(ctx, listener, h)
				if handover != nil {
					listener.Close()
					if loadErr := handover(); loadErr != nil {
						return cli.Errorf(1, "The hub service did not load", loadErr.Error(), "pulse service install")
					}
					fmt.Fprintf(c.Out, "the pulse.hub service now serves %s; this hub exits\n", address)
					return nil
				}
				select {
				case <-changed:
					return cli.Errorf(3, "", "", "")
				default:
					return err
				}
			},
		})
	}
}

// handoverDelay is how long a foreground hub keeps serving after it answers the
// service-install request, so the page receives the response before the port moves.
var handoverDelay = 300 * time.Millisecond

// watchBinary calls onChange once when the executable's stamp differs from stamp; it stops with ctx.
func watchBinary(ctx context.Context, stamp string, onChange func()) {
	if stamp == "" {
		return
	}
	t := time.NewTicker(restartPoll)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			if now := cli.BinaryStamp(); now != "" && now != stamp {
				onChange()
				return
			}
		}
	}
}
