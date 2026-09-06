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
	"syscall"

	"github.com/manishtiwari25/pulse/internal/cli"
)

func init() {
	for _, name := range []string{"hub", "serve"} {
		cli.Register(cli.Command{
			Name: name, Group: "hub", Summary: "Serve the local owner console",
			Usage: "pulse " + name + " [--project SLUG] [--port N] [--no-browser] [--host 127.0.0.1]",
			Flags: func(fs *flag.FlagSet) {
				fs.Int("port", 8765, "TCP port (0 chooses a free port)")
				fs.String("host", "127.0.0.1", "listen address (127.0.0.1 only)")
				fs.Bool("no-browser", false, "do not open the browser")
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
				h := New(c.Store, nil)
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
				address := "http://" + listener.Addr().String()
				fmt.Fprintf(c.Out, "Serving on %s (127.0.0.1 only; writes: decide, settings, tasks, messages, compute; Ctrl-C to stop)\n", address)
				if fs.Lookup("no-browser").Value.String() != "true" {
					if err := openBrowser(address); err != nil {
						fmt.Fprintf(c.Err, "Could not open the browser: %v; open %s manually\n", err, address)
					}
				}
				ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
				defer cancel()
				return Serve(ctx, listener, h)
			},
		})
	}
}
