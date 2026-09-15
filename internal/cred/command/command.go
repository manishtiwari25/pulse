// Package command registers `pulse cred`, the owner's console for the daemon's push credential (ADR-041):
// keep a 1Password service-account token off argv, point push.credential at a key by reference after
// probing that the key opens the remote, show what is set without showing the key, and clear it.
package command

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"regexp"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/cred"
	"github.com/manishtiwari25/pulse/internal/store"
)

func init() {
	cli.Register(cli.Command{
		Name: "cred", Group: "human", Raw: true, NoStore: true,
		Summary: "the daemon's own push key, by reference: token, set, status, clear (ADR-041)",
		Usage: "pulse cred token [--from-file PATH]      (1Password service-account token from stdin, kept in the keychain or a 0600 file)\n" +
			"       pulse cred set REF [--no-probe]         (REF: " + cred.Shapes + ")\n" +
			"       pulse cred status [--json]\n" +
			"       pulse cred clear [--token]",
		Run: run,
	})
}

// Deps are the seams the tests replace: the resolver factory, stdin, and the probe.
type Deps struct {
	Resolver func(home string) *cred.Resolver
	Stdin    io.Reader
	Probe    func(ctx context.Context, root, remote, sshCommand string) error
}

var deps = Deps{
	Resolver: cred.New,
	Stdin:    os.Stdin,
	Probe:    probeRemote,
}

var actorPattern = regexp.MustCompile(`^[A-Za-z0-9._:-]{1,64}$`)

// requireOwner mirrors goals.RequireOwner: the CLI without PULSE_AGENT (or with a human:<surface> agent),
// never a worker, never --agent human from an agent session.
func requireOwner(home, actor string) error {
	envActor := os.Getenv("PULSE_AGENT")
	if !ownerActor(actor) || (envActor != "" && !ownerActor(envActor)) || store.WorkerGuard(home) {
		return cli.Errorf(2, "Only the owner handles the push credential",
			"pulse cred names the key the daemon pushes with (ADR-041); agents may read pulse cred status and suggest, never set or clear it.",
			"Run pulse cred from your own shell, without PULSE_AGENT.")
	}
	return nil
}

func ownerActor(actor string) bool {
	return store.HumanActor(actor) && actorPattern.MatchString(actor) && !store.SecretShaped(actor)
}

func run(c *cli.Context, _ *flag.FlagSet, args []string) error {
	sub := "status"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	fs := flag.NewFlagSet("cred "+sub, flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	var fromFile string
	var noProbe, token, jsonOut bool
	switch sub {
	case "token":
		fs.StringVar(&fromFile, "from-file", "", "read the token from this file instead of stdin")
	case "set":
		fs.BoolVar(&noProbe, "no-probe", false, "save without pushing a dry run to the remote")
	case "status":
		fs.BoolVar(&jsonOut, "json", false, "machine-readable")
	case "clear":
		fs.BoolVar(&token, "token", false, "also remove the stored service-account token")
	default:
		return cli.Errorf(2, "Unknown cred command "+sub, "Use token, set REF, status or clear.", "pulse help cred")
	}
	flagArgs, rest := []string{}, []string{}
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch {
		case a == "--":
			rest = append(rest, args[i+1:]...)
			i = len(args)
		case strings.HasPrefix(a, "-") && len(a) > 1 && !cred.IsPrivateKey([]byte(a)):
			flagArgs = append(flagArgs, a)
			if name := strings.TrimLeft(a, "-"); name == "from-file" && i+1 < len(args) {
				i++
				flagArgs = append(flagArgs, args[i])
			}
		default:
			rest = append(rest, a)
		}
	}
	if err := fs.Parse(flagArgs); err != nil {
		for _, a := range args {
			if cred.IsPrivateKey([]byte(a)) || store.SecretShaped(a) {
				return notAReference()
			}
		}
		return cli.Errorf(2, "pulse cred "+sub+": "+cred.Redact(err.Error()), "", "pulse help cred")
	}
	rest = append(rest, fs.Args()...)
	if (sub == "set" && len(rest) != 1) || (sub != "set" && len(rest) != 0) {
		return cli.Errorf(2, "Wrong cred arguments", "set takes one reference; token, status and clear take flags only.", "pulse help cred")
	}
	actor := c.Agent
	if actor == "" {
		actor = os.Getenv("PULSE_AGENT")
	}
	if actor == "" {
		actor = store.HumanAgent
	}
	home := c.Home
	if c.Store != nil {
		home = c.Store.Home
	}
	if sub != "status" {
		if err := requireOwner(home, actor); err != nil {
			return err
		}
	}
	s := c.Store
	if s == nil {
		var err error
		s, err = store.Open(store.Options{Home: home, ReadOnly: sub == "status", SkipRecords: true, Now: c.Now})
		if err != nil {
			return err
		}
		defer s.Close()
	}
	r := deps.Resolver(s.Home)
	ctx := context.Background()
	switch sub {
	case "token":
		return runToken(c, r, fromFile)
	case "set":
		return runSet(c, s, r, actor, rest[0], noProbe)
	case "status":
		return runStatus(c, s, r, jsonOut || c.JSON)
	case "clear":
		return runClear(ctx, c, s, r, actor, token)
	}
	return nil
}

func runToken(c *cli.Context, r *cred.Resolver, fromFile string) error {
	var raw []byte
	var err error
	if fromFile != "" {
		raw, err = os.ReadFile(fromFile)
	} else {
		if f, ok := deps.Stdin.(*os.File); ok && cli.IsTTY(f) {
			fmt.Fprintln(c.Err, "Paste the 1Password service-account token, then press Enter and Ctrl-D:")
		}
		raw, err = io.ReadAll(io.LimitReader(deps.Stdin, 64*1024))
	}
	if err != nil {
		return cli.Errorf(1, "Could not read the token", err.Error(), "pulse cred token < token.txt, or --from-file PATH")
	}
	where, err := r.StoreToken(context.Background(), string(raw))
	if err != nil {
		return cli.Errorf(2, "Token not stored", err.Error(), "Copy the token from 1Password > Developer > Service Accounts and paste it on one line.")
	}
	c.Printf("Service-account token kept in %s. It never appears on argv, in the store or in events.\n", where)
	c.Println("Next: pulse cred set \"op://VAULT/ITEM/private key?ssh-format=openssh\"")
	return nil
}

func notAReference() error {
	return cli.Errorf(2, "That is the key, not a reference to it", "pulse stores where the key lives, never the key (ADR-041).", "pulse cred set \"op://VAULT/ITEM/private key?ssh-format=openssh\"")
}

func runSet(c *cli.Context, s *store.Store, r *cred.Resolver, actor, text string, noProbe bool) error {
	if cred.IsPrivateKey([]byte(text)) || store.SecretShaped(text) {
		return notAReference()
	}
	ref, err := cred.Parse(text)
	if err != nil {
		return cli.Errorf(2, "Not a push credential reference", err.Error(), "pulse help cred")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	secret, err := r.Resolve(ctx, ref)
	if err != nil {
		return cli.Errorf(1, "The key could not be read", cred.Redact(err.Error()), "Fix the reference or the item it names, then run pulse cred set again. Nothing was saved.")
	}
	material, err := r.Materialize(secret)
	if err != nil {
		return cli.Errorf(1, "Could not write the key for the check", cred.Redact(err.Error()), "Check PULSE_HOME is writable.")
	}
	defer material.Cleanup()
	pub, fp, err := r.Inspect(ctx, material.KeyPath)
	if err != nil {
		return cli.Errorf(1, "The secret is not a usable SSH private key", cred.Redact(err.Error()), "For a 1Password SSH Key item the reference ends in /private key?ssh-format=openssh. Nothing was saved.")
	}
	c.Printf("Key read from %s: %s\n", secret.Source, fp)
	c.Printf("Public key: %s\n", pub)
	if !noProbe {
		probed := 0
		projects, err := s.Projects(true)
		if err != nil {
			return err
		}
		for _, p := range projects {
			if p.Slug == store.CommonsSlug || p.Path == "" {
				continue
			}
			remote := upstreamRemote(p.Path, p.DefaultBranch)
			if remote == "" {
				c.Printf("  %s: no upstream on %s; not probed (git branch --set-upstream-to=origin/%s %s)\n", p.Slug, defaultBranch(p.DefaultBranch), defaultBranch(p.DefaultBranch), defaultBranch(p.DefaultBranch))
				continue
			}
			probed++
			if err := deps.Probe(ctx, p.Path, remote, material.SSHCommand); err != nil {
				return cli.Errorf(1, fmt.Sprintf("The key does not open %s's remote %s", p.Slug, remote), cred.Redact(err.Error()),
					"Add the public key above to the repository as a deploy key with write access (or to a machine user), then run pulse cred set again. Nothing was saved.")
			}
			c.Printf("  %s: %s answers this key\n", p.Slug, remote)
		}
		if probed == 0 {
			c.Println("  no project with an upstream to probe; saved unprobed")
		}
	}
	if err := s.SetSetting(actor, "push.credential", ref.String()); err != nil {
		return err
	}
	_ = s.Event(actor, "cred.set", "push.credential", map[string]any{"reference": ref.String(), "fingerprint": fp, "source": secret.Source, "probed": !noProbe})
	c.Printf("push.credential=%s\n", ref.String())
	if s.Setting("landing.push", "off") != "main" {
		c.Println("The daemon pushes only with landing.push=main: pulse settings set landing.push main")
	}
	return nil
}

type statusReport struct {
	Reference   string `json:"reference"`
	Scheme      string `json:"scheme,omitempty"`
	LandingPush string `json:"landing_push"`
	Token       string `json:"token"` // where the service-account token was found, or "none"
	SetAt       string `json:"set_at,omitempty"`
	SetBy       string `json:"set_by,omitempty"`
	Fingerprint string `json:"fingerprint,omitempty"`
}

func runStatus(c *cli.Context, s *store.Store, r *cred.Resolver, jsonOut bool) error {
	rep := statusReport{Reference: s.Setting("push.credential", ""), LandingPush: s.Setting("landing.push", "off"), Token: "none"}
	if rep.Reference != "" {
		if ref, err := cred.Parse(rep.Reference); err == nil {
			rep.Scheme = ref.Scheme
		}
		_ = s.DB.QueryRow("SELECT updated_at, updated_by FROM settings WHERE name = 'push.credential'").Scan(&rep.SetAt, &rep.SetBy)
		_ = s.DB.QueryRow("SELECT json_extract(detail, '$.fingerprint') FROM events WHERE action = 'cred.set' ORDER BY seq DESC LIMIT 1").Scan(&rep.Fingerprint)
	}
	if _, source, err := r.Token(context.Background()); err == nil {
		rep.Token = source
	}
	if jsonOut {
		return cli.WriteJSON(c.Out, rep)
	}
	if rep.Reference == "" {
		c.Println("push.credential: not set - the daemon pushes with the git credentials this machine has (ADR-037)")
	} else {
		c.Printf("push.credential: %s", rep.Reference)
		if rep.Fingerprint != "" {
			c.Printf(" (%s)", rep.Fingerprint)
		}
		c.Println()
		if rep.SetAt != "" {
			c.Printf("set %s by %s\n", rep.SetAt, rep.SetBy)
		}
	}
	c.Printf("landing.push: %s\n", rep.LandingPush)
	c.Printf("1Password service-account token: %s\n", rep.Token)
	return nil
}

func runClear(ctx context.Context, c *cli.Context, s *store.Store, r *cred.Resolver, actor string, token bool) error {
	had := s.Setting("push.credential", "")
	if had != "" {
		if err := s.SetSetting(actor, "push.credential", ""); err != nil {
			return err
		}
		_ = s.Event(actor, "cred.clear", "push.credential", map[string]any{"was": had})
		c.Println("push.credential cleared - the daemon pushes with the git credentials this machine has again (ADR-037)")
	} else {
		c.Println("push.credential was not set")
	}
	if token {
		removed, err := r.ClearToken(ctx)
		if err != nil {
			return err
		}
		if len(removed) == 0 {
			c.Println("no stored service-account token to remove")
		} else {
			c.Printf("service-account token removed from %s\n", strings.Join(removed, " and "))
		}
		c.Println("Revoke the service account in 1Password too: Developer > Service Accounts.")
	}
	return nil
}

func defaultBranch(b string) string {
	if b == "" {
		return "main"
	}
	return b
}

// upstreamRemote is the remote the default branch tracks, or "".
func upstreamRemote(root, branch string) string {
	cmd := exec.Command("git", "-C", root, "for-each-ref", "--format=%(upstream:remotename)", "refs/heads/"+defaultBranch(branch))
	out, err := cmd.Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// probeRemote checks that the key opens the remote for writing without changing it: a dry-run push of the
// default branch to itself asks the server for write access and moves nothing.
func probeRemote(ctx context.Context, root, remote, sshCommand string) error {
	ctx, cancel := context.WithTimeout(ctx, 60*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", "-C", root, "push", "--dry-run", "--porcelain", remote, "HEAD:refs/heads/pulse-cred-probe-does-not-exist")
	cmd.Env = append(os.Environ(), "GIT_SSH_COMMAND="+sshCommand, "GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0")
	var out strings.Builder
	cmd.Stdout, cmd.Stderr = &out, &out
	if err := cmd.Run(); err != nil {
		text := strings.TrimSpace(out.String())
		if text == "" {
			text = err.Error()
		}
		if ctx.Err() != nil {
			text = "timed out after 60s: " + text
		}
		return errors.New(text)
	}
	return nil
}
