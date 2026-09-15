// Package setup is the one implementation behind `pulse cred` and the hub's cred route (ADR-041, ADR-042):
// apply a reference (resolve the key, inspect it, probe every upstream, save only when they answer), report
// the state, clear it, mint a key of the daemon's own, and browse what a provider holds. Nothing here returns
// a secret: results carry references, fingerprints, public keys and redacted reasons.
package setup

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/cred"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Deps are the seams tests replace; nil fields mean the real thing.
type Deps struct {
	Probe  func(ctx context.Context, root, remote, sshCommand string) error
	Keygen func(ctx context.Context, path, comment string) error
}

func (d Deps) probe() func(context.Context, string, string, string) error {
	if d.Probe != nil {
		return d.Probe
	}
	return ProbeRemote
}

func (d Deps) keygen() func(context.Context, string, string) error {
	if d.Keygen != nil {
		return d.Keygen
	}
	return keygen
}

// Refusal is why a step did not happen, in words safe to show: What/Why/Fix as the CLI prints them, the exit
// code it uses (1: the environment, 2: the input), and, when the key was read, what the owner needs to add
// somewhere - its public key.
type Refusal struct {
	Code        int     `json:"-"`
	What        string  `json:"what"`
	Why         string  `json:"why"`
	Fix         string  `json:"fix"`
	Fingerprint string  `json:"fingerprint,omitempty"`
	PublicKey   string  `json:"public_key,omitempty"`
	Probes      []Probe `json:"probes,omitempty"`
}

func (r *Refusal) Error() string { return r.What + ": " + r.Why }

// Probe is one project's upstream asked whether the key may push.
type Probe struct {
	Project string `json:"project"`
	Branch  string `json:"branch"`
	Remote  string `json:"remote,omitempty"`
	OK      bool   `json:"ok"`
	Skipped string `json:"skipped,omitempty"` // why it was not asked
	Why     string `json:"why,omitempty"`     // the remote's refusal, redacted
}

// Result is a saved reference and what was learned on the way.
type Result struct {
	Reference   string  `json:"reference"`
	Fingerprint string  `json:"fingerprint"`
	PublicKey   string  `json:"public_key"`
	Source      string  `json:"source"`
	Probed      bool    `json:"probed"`
	Probes      []Probe `json:"probes"`
	LandingPush string  `json:"landing_push"`
}

// NotAReference is the refusal for a pasted key or token.
func NotAReference() *Refusal {
	return &Refusal{Code: 2, What: "That is the key, not a reference to it", Why: "pulse stores where the key lives, never the key (ADR-041).", Fix: "pulse cred set \"op://VAULT/ITEM/private key?ssh-format=openssh\""}
}

// Apply resolves text as a reference, checks the secret is an SSH private key, probes each active project's
// upstream with a dry-run push through that key alone (unless probe is false) and saves the reference as
// push.credential only when every probed remote answers. Errors are *Refusal or the store's.
func Apply(parent context.Context, s *store.Store, r *cred.Resolver, actor, text string, probe bool, deps Deps) (*Result, error) {
	text = strings.TrimSpace(text)
	if cred.IsPrivateKey([]byte(text)) || store.SecretShaped(text) {
		return nil, NotAReference()
	}
	ref, err := cred.Parse(text)
	if err != nil {
		return nil, &Refusal{Code: 2, What: "Not a push credential reference", Why: err.Error(), Fix: "pulse help cred"}
	}
	ctx, cancel := context.WithTimeout(parent, 2*time.Minute)
	defer cancel()
	secret, err := r.Resolve(ctx, ref)
	if err != nil {
		return nil, &Refusal{Code: 1, What: "The key could not be read", Why: cred.Redact(err.Error()), Fix: "Fix the reference or the item it names, then try again. Nothing was saved."}
	}
	material, err := r.Materialize(secret)
	if err != nil {
		return nil, &Refusal{Code: 1, What: "Could not write the key for the check", Why: cred.Redact(err.Error()), Fix: "Check PULSE_HOME is writable."}
	}
	defer material.Cleanup()
	pub, fp, err := r.Inspect(ctx, material.KeyPath)
	if err != nil {
		return nil, &Refusal{Code: 1, What: "The secret is not a usable SSH private key", Why: cred.Redact(err.Error()), Fix: "For a 1Password SSH Key item the reference ends in /private key?ssh-format=openssh. Nothing was saved."}
	}
	res := &Result{Reference: ref.String(), Fingerprint: fp, PublicKey: pub, Source: secret.Source, Probed: probe, Probes: []Probe{}}
	if probe {
		projects, err := s.Projects(true)
		if err != nil {
			return nil, err
		}
		for _, p := range projects {
			if p.Slug == store.CommonsSlug || p.Path == "" {
				continue
			}
			branch := DefaultBranch(p.DefaultBranch)
			pr := Probe{Project: p.Slug, Branch: branch, Remote: UpstreamRemote(p.Path, branch)}
			if pr.Remote == "" {
				pr.Skipped = fmt.Sprintf("no upstream on %s (git branch --set-upstream-to=origin/%s %s)", branch, branch, branch)
				res.Probes = append(res.Probes, pr)
				continue
			}
			if err := deps.probe()(ctx, p.Path, pr.Remote, material.SSHCommand); err != nil {
				pr.Why = cred.Redact(err.Error())
				res.Probes = append(res.Probes, pr)
				return nil, &Refusal{Code: 1, What: fmt.Sprintf("The key does not open %s's remote %s", p.Slug, pr.Remote), Why: pr.Why,
					Fix:         "Add the public key to the repository as a deploy key with write access (or to a machine user), then try again. Nothing was saved.",
					Fingerprint: fp, PublicKey: pub, Probes: res.Probes}
			}
			pr.OK = true
			res.Probes = append(res.Probes, pr)
		}
	}
	if err := s.SetSetting(actor, "push.credential", ref.String()); err != nil {
		return nil, err
	}
	_ = s.Event(actor, "cred.set", "push.credential", map[string]any{"reference": ref.String(), "fingerprint": fp, "source": secret.Source, "probed": probe})
	res.LandingPush = s.Setting("landing.push", "off")
	return res, nil
}

// Report is the state without secrets: what pulse cred status prints and GET /api/v1/cred answers.
type Report struct {
	Reference   string `json:"reference"`
	Scheme      string `json:"scheme,omitempty"`
	LandingPush string `json:"landing_push"`
	Token       string `json:"token"` // where the service-account token was found, or "none"
	SetAt       string `json:"set_at,omitempty"`
	SetBy       string `json:"set_by,omitempty"`
	Fingerprint string `json:"fingerprint,omitempty"`
}

// Status reads the state. The fingerprint comes from the cred.set event that saved this reference; nothing
// is resolved.
func Status(ctx context.Context, s *store.Store, r *cred.Resolver) Report {
	rep := Report{Reference: s.Setting("push.credential", ""), LandingPush: s.Setting("landing.push", "off"), Token: "none"}
	if rep.Reference != "" {
		if ref, err := cred.Parse(rep.Reference); err == nil {
			rep.Scheme = ref.Scheme
		}
		_ = s.DB.QueryRow("SELECT updated_at, updated_by FROM settings WHERE name = 'push.credential'").Scan(&rep.SetAt, &rep.SetBy)
		_ = s.DB.QueryRow("SELECT json_extract(detail, '$.fingerprint') FROM events WHERE action = 'cred.set' AND json_extract(detail, '$.reference') = ? ORDER BY seq DESC LIMIT 1", rep.Reference).Scan(&rep.Fingerprint)
	}
	if _, source, err := r.Token(ctx); err == nil {
		rep.Token = source
	}
	return rep
}

// Cleared says what Clear removed.
type Cleared struct {
	Was          string   `json:"was"`           // the reference that was set, or ""
	TokenRemoved []string `json:"token_removed"` // where the token was removed from
}

// Clear unsets push.credential (the daemon pushes with git's own credentials again, ADR-037) and, when
// token is true, removes the stored service-account token.
func Clear(ctx context.Context, s *store.Store, r *cred.Resolver, actor string, token bool) (*Cleared, error) {
	out := &Cleared{Was: s.Setting("push.credential", ""), TokenRemoved: []string{}}
	if out.Was != "" {
		if err := s.SetSetting(actor, "push.credential", ""); err != nil {
			return nil, err
		}
		_ = s.Event(actor, "cred.clear", "push.credential", map[string]any{"was": out.Was})
	}
	if token {
		removed, err := r.ClearToken(ctx)
		if err != nil {
			return nil, err
		}
		if removed != nil {
			out.TokenRemoved = removed
		}
	}
	return out, nil
}

// GeneratedKeyName is the file under $PULSE_HOME/secrets the console mints (ADR-042 D4).
const GeneratedKeyName = "push_key"

// Generated is a key of the daemon's own on this machine: never the private half.
type Generated struct {
	Path        string `json:"path"`
	Reference   string `json:"reference"`
	Fingerprint string `json:"fingerprint"`
	PublicKey   string `json:"public_key"`
	Existed     bool   `json:"existed"` // true: a key was already there and was kept (replace was false)
}

// Generate mints an ed25519 key without a passphrase at $PULSE_HOME/secrets/push_key (0600 in a 0700
// directory) and answers with its public key and file:// reference. An existing key is kept and described
// unless replace is true. Refused inside a run.
func Generate(parent context.Context, r *cred.Resolver, replace bool, deps Deps) (*Generated, error) {
	if r.Worker || os.Getenv("PULSE_JOB") != "" {
		return nil, cred.ErrWorker
	}
	dir := filepath.Join(r.Home, "secrets")
	path := filepath.Join(dir, GeneratedKeyName)
	ctx, cancel := context.WithTimeout(parent, time.Minute)
	defer cancel()
	out := &Generated{Path: path, Reference: "file://" + path}
	if _, err := os.Lstat(path); err == nil {
		if !replace {
			pub, fp, err := r.Inspect(ctx, path)
			if err != nil {
				return nil, &Refusal{Code: 1, What: "A file is already at " + path + " but is not a usable key", Why: cred.Redact(err.Error()), Fix: "Generate again with replace to overwrite it."}
			}
			out.Existed, out.PublicKey, out.Fingerprint = true, pub, fp
			return out, nil
		}
		_ = os.Remove(path)
		_ = os.Remove(path + ".pub")
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, &Refusal{Code: 1, What: "Could not create " + dir, Why: err.Error(), Fix: "Check PULSE_HOME is writable."}
	}
	_ = os.Chmod(dir, 0o700)
	host, _ := os.Hostname()
	comment := "pulse push key"
	if host != "" {
		comment += " (" + host + ")"
	}
	if err := deps.keygen()(ctx, path, comment); err != nil {
		return nil, &Refusal{Code: 1, What: "ssh-keygen could not make the key", Why: cred.Redact(err.Error()), Fix: "Install OpenSSH's ssh-keygen and try again."}
	}
	_ = os.Chmod(path, 0o600)
	pub, fp, err := r.Inspect(ctx, path)
	if err != nil {
		return nil, &Refusal{Code: 1, What: "The generated key could not be read back", Why: cred.Redact(err.Error()), Fix: "Remove " + path + " and generate again."}
	}
	out.PublicKey, out.Fingerprint = pub, fp
	return out, nil
}

func keygen(ctx context.Context, path, comment string) error {
	cmd := exec.CommandContext(ctx, "ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", comment, "-f", path)
	var out strings.Builder
	cmd.Stdout, cmd.Stderr = &out, &out
	if err := cmd.Run(); err != nil {
		if text := strings.TrimSpace(out.String()); text != "" {
			return errors.New(text)
		}
		return err
	}
	return nil
}

// Vault is one 1Password vault the service account may read.
type Vault struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

// Item is one SSH Key item in a vault.
type Item struct {
	ID        string `json:"id"`
	Title     string `json:"title"`
	Category  string `json:"category"`
	UpdatedAt string `json:"updated_at,omitempty"`
	Reference string `json:"reference"` // the op:// reference to its private key
}

// Vaults lists what the service account can see: `op vault list`.
func Vaults(ctx context.Context, r *cred.Resolver) ([]Vault, error) {
	out, _, err := r.Op(ctx, "vault", "list", "--format", "json")
	if err != nil {
		return nil, err
	}
	var vaults []Vault
	if err := json.Unmarshal([]byte(out), &vaults); err != nil {
		return nil, errors.New("op vault list answered something that is not JSON")
	}
	if vaults == nil {
		vaults = []Vault{}
	}
	return vaults, nil
}

// Items lists the SSH Key items of one vault (by id or name): `op item list --categories "SSH Key"`.
func Items(ctx context.Context, r *cred.Resolver, vault string) ([]Item, error) {
	vault = strings.TrimSpace(vault)
	if vault == "" {
		return nil, &Refusal{Code: 2, What: "A vault is required", Why: "op item list needs --vault.", Fix: "Pick a vault first."}
	}
	out, _, err := r.Op(ctx, "item", "list", "--vault", vault, "--categories", "SSH Key", "--format", "json")
	if err != nil {
		return nil, err
	}
	var raw []struct {
		ID       string `json:"id"`
		Title    string `json:"title"`
		Category string `json:"category"`
		Updated  string `json:"updated_at"`
		Vault    Vault  `json:"vault"`
	}
	if err := json.Unmarshal([]byte(out), &raw); err != nil {
		return nil, errors.New("op item list answered something that is not JSON")
	}
	items := make([]Item, 0, len(raw))
	for _, it := range raw {
		v := it.Vault
		if v.ID == "" && v.Name == "" {
			v = Vault{ID: vault, Name: vault}
		}
		items = append(items, Item{ID: it.ID, Title: it.Title, Category: it.Category, UpdatedAt: it.Updated, Reference: OpReference(v, it.Title, it.ID)})
	}
	return items, nil
}

// OpReference builds the op:// reference to an SSH Key item's private key in OpenSSH form. Names are used
// while they survive the reference syntax (no "/" or "?", no edge spaces); otherwise the ids are.
func OpReference(v Vault, title, id string) string {
	vault := v.Name
	if !plainName(vault) {
		vault = v.ID
	}
	item := title
	if !plainName(item) {
		item = id
	}
	return "op://" + vault + "/" + item + "/private key?ssh-format=openssh"
}

func plainName(s string) bool {
	return s != "" && s == strings.TrimSpace(s) && !strings.ContainsAny(s, "/?\n")
}

// Field is something the owner types for a provider that has no listing.
type Field struct {
	Name        string `json:"name"`
	Label       string `json:"label"`
	Placeholder string `json:"placeholder"`
}

// Provider is one place a push key can live, as the console renders it (ADR-042 D2).
type Provider struct {
	ID          string  `json:"id"`
	Title       string  `json:"title"`
	Blurb       string  `json:"blurb"`
	Available   bool    `json:"available"`
	Unavailable string  `json:"unavailable,omitempty"`
	NeedsToken  bool    `json:"needs_token"`
	Browse      bool    `json:"browse"`
	Generate    bool    `json:"generate"`
	Fields      []Field `json:"fields"`
	Example     string  `json:"example"`
}

// Providers lists the providers on this machine, the unavailable ones included with the reason.
func Providers(r *cred.Resolver) []Provider {
	op := Provider{ID: "op", Title: "1Password", Blurb: "An SSH Key item in a vault of its own, read with a service account that sees only that vault and never needs the app unlocked.", Available: r.OpAvailable(), NeedsToken: true, Browse: true, Fields: []Field{}, Example: "op://PULSE/pulse push key/private key?ssh-format=openssh"}
	if !op.Available {
		op.Unavailable = "op not found; install 1Password CLI (brew install 1password-cli)"
	}
	keychain := Provider{ID: "keychain", Title: "macOS Keychain", Blurb: "A generic password item in the login Keychain holding the private key.", Available: r.OS() == "darwin", Fields: []Field{{Name: "service", Label: "Service", Placeholder: "pulse"}, {Name: "account", Label: "Account", Placeholder: "push-key"}}, Example: "keychain://pulse/push-key"}
	if !keychain.Available {
		keychain.Unavailable = "the Keychain is macOS only; use 1Password or a file here"
	}
	file := Provider{ID: "file", Title: "A file on this machine", Blurb: "A private key file only this user can read (mode 0600). The console can make one for the daemon.", Available: true, Generate: true, Fields: []Field{{Name: "path", Label: "Path", Placeholder: filepath.Join(r.Home, "secrets", GeneratedKeyName)}}, Example: "file://" + filepath.Join(r.Home, "secrets", GeneratedKeyName)}
	return []Provider{op, keychain, file}
}

// DefaultBranch is "main" when a project names none.
func DefaultBranch(b string) string {
	if b == "" {
		return "main"
	}
	return b
}

// UpstreamRemote is the remote the branch tracks, or "".
func UpstreamRemote(root, branch string) string {
	cmd := exec.Command("git", "-C", root, "for-each-ref", "--format=%(upstream:remotename)", "refs/heads/"+DefaultBranch(branch))
	out, err := cmd.Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// ProbeRemote checks that the key opens the remote for writing without changing it: a dry-run push to a
// branch that does not exist asks the server for write access and moves nothing.
func ProbeRemote(ctx context.Context, root, remote, sshCommand string) error {
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
