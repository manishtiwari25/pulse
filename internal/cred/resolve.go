package cred

import (
	"bytes"
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"time"
)

// Runner runs an external command: stdin is fed to the process, env replaces its environment when
// non-nil. Tests inject a fake; the daemon uses Exec.
type Runner func(ctx context.Context, name string, args []string, stdin string, env []string) (stdout, stderr string, err error)

// Exec is the Runner that starts real processes.
func Exec(ctx context.Context, name string, args []string, stdin string, env []string) (string, string, error) {
	cmd := exec.CommandContext(ctx, name, args...)
	if env != nil {
		cmd.Env = env
	}
	if stdin != "" {
		cmd.Stdin = strings.NewReader(stdin)
	}
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	err := cmd.Run()
	if err != nil && ctx.Err() != nil {
		err = fmt.Errorf("%s timed out", name)
	}
	return out.String(), errb.String(), err
}

const (
	// TokenEnv is the variable 1Password CLI reads a service-account token from.
	TokenEnv = "OP_SERVICE_ACCOUNT_TOKEN"
	// KeychainService is the macOS Keychain service under which pulse cred keeps the token; the account is
	// TokenEnv, so `security find-generic-password -s pulse -a OP_SERVICE_ACCOUNT_TOKEN -w` reads it.
	KeychainService = "pulse"
	tokenFileName   = "op-service-account-token"
)

// Resolver turns a Ref into an SSH private key.
type Resolver struct {
	Home    string              // PULSE_HOME: the token file and push-time key directories live under it
	Run     Runner              // nil: Exec
	Getenv  func(string) string // nil: os.Getenv
	GOOS    string              // "": runtime.GOOS
	Timeout time.Duration       // per external command; 0: 30s
	Worker  bool                // true inside a run: every resolution is refused (ADR-041 D4)
}

// New returns a Resolver with the real runner and environment.
func New(home string) *Resolver { return &Resolver{Home: home} }

func (r *Resolver) run() Runner {
	if r.Run == nil {
		return Exec
	}
	return r.Run
}

func (r *Resolver) getenv(k string) string {
	if r.Getenv == nil {
		return os.Getenv(k)
	}
	return r.Getenv(k)
}

func (r *Resolver) goos() string {
	if r.GOOS == "" {
		return runtime.GOOS
	}
	return r.GOOS
}

// OS is the operating system the resolver acts for (GOOS, or the injected one).
func (r *Resolver) OS() string { return r.goos() }

func (r *Resolver) ctx(parent context.Context) (context.Context, context.CancelFunc) {
	d := r.Timeout
	if d == 0 {
		d = 30 * time.Second
	}
	return context.WithTimeout(parent, d)
}

// Secret is a resolved push credential. Key is the private key; Source says which path answered.
type Secret struct {
	Key    []byte
	Source string // "1Password (token from the keychain)", "keychain", "file"
}

// ErrWorker is returned inside a run.
var ErrWorker = errors.New("push credentials are never resolved inside a run (ADR-041 D4)")

// Resolve reads the key the reference names. Errors never carry the secret.
func (r *Resolver) Resolve(parent context.Context, ref Ref) (*Secret, error) {
	if r.Worker || r.getenv("PULSE_JOB") != "" {
		return nil, ErrWorker
	}
	var key []byte
	var source string
	switch ref.Scheme {
	case "op":
		out, tokenSource, err := r.Op(parent, "read", "-n", ref.Raw)
		if err != nil {
			return nil, err
		}
		key, source = []byte(out), "1Password (token from "+tokenSource+")"
	case "keychain":
		if r.goos() != "darwin" {
			return nil, errors.New("keychain:// references need macOS; use file:// or op:// here")
		}
		ctx, cancel := r.ctx(parent)
		defer cancel()
		out, stderr, err := r.run()(ctx, "security", []string{"find-generic-password", "-s", ref.Service, "-a", ref.Account, "-w"}, "", nil)
		if err != nil {
			return nil, fmt.Errorf("keychain item %s/%s: %s", ref.Service, ref.Account, Redact(firstLine(stderr, err)))
		}
		key, source = keychainBytes(out), "keychain"
	case "file":
		info, err := os.Lstat(ref.Path)
		if err != nil {
			return nil, fmt.Errorf("%s: %v", ref.Path, errors.Unwrap(err))
		}
		if !info.Mode().IsRegular() {
			return nil, fmt.Errorf("%s is not a regular file (symlinks are refused)", ref.Path)
		}
		if info.Mode().Perm()&0o077 != 0 {
			return nil, fmt.Errorf("%s is readable by others (mode %04o); chmod 600 it", ref.Path, info.Mode().Perm())
		}
		key, err = os.ReadFile(ref.Path)
		if err != nil {
			return nil, fmt.Errorf("%s: %v", ref.Path, errors.Unwrap(err))
		}
		source = "file"
	default:
		return nil, fmt.Errorf("unknown scheme %q", ref.Scheme)
	}
	key = bytes.TrimRight(key, "\r\n\t ")
	if len(key) == 0 {
		return nil, fmt.Errorf("%s resolved to nothing", ref.Raw)
	}
	if !IsPrivateKey(key) {
		hint := ""
		if ref.Scheme == "op" && ref.Query == "" {
			hint = "; for an SSH Key item add ?ssh-format=openssh to the reference"
		}
		return nil, fmt.Errorf("%s is not an SSH private key (it does not start with -----BEGIN ... PRIVATE KEY-----)%s", ref.Raw, hint)
	}
	return &Secret{Key: append(key, '\n'), Source: source}, nil
}

// opEnv is the environment of `op read`: the service-account token and no Connect variables, which would
// take precedence over it.
// OpAvailable reports whether the 1Password CLI is on PATH (always true with an injected Runner).
func (r *Resolver) OpAvailable() bool {
	if r.Run != nil {
		return true
	}
	_, err := exec.LookPath("op")
	return err == nil
}

// Op runs the 1Password CLI with the service-account token in its environment and nothing else of
// 1Password's (OP_CONNECT_* would take precedence). It answers stdout, where the token came from, and a
// redacted error: `op read -n REF`, `op vault list --format json`, `op item list ... --format json`.
func (r *Resolver) Op(parent context.Context, args ...string) (stdout, tokenSource string, err error) {
	token, tokenSource, err := r.Token(parent)
	if err != nil {
		return "", "", err
	}
	if !r.OpAvailable() {
		return "", "", errors.New("op not found; install 1Password CLI (brew install 1password-cli)")
	}
	ctx, cancel := r.ctx(parent)
	defer cancel()
	out, stderr, err := r.run()(ctx, "op", args, "", opEnv(os.Environ(), token))
	if err != nil {
		verb := "op"
		if len(args) > 0 {
			verb = "op " + args[0]
		}
		return "", tokenSource, fmt.Errorf("%s failed: %s", verb, Redact(firstLine(stderr, err), token))
	}
	return out, tokenSource, nil
}

func opEnv(base []string, token string) []string {
	env := make([]string, 0, len(base)+1)
	for _, kv := range base {
		if strings.HasPrefix(kv, "OP_CONNECT_HOST=") || strings.HasPrefix(kv, "OP_CONNECT_TOKEN=") || strings.HasPrefix(kv, TokenEnv+"=") {
			continue
		}
		env = append(env, kv)
	}
	return append(env, TokenEnv+"="+token)
}

// keychainBytes undoes what `security -w` does to a multi-line secret: it prints hex.
func keychainBytes(out string) []byte {
	text := strings.TrimSpace(out)
	if len(text)%2 == 0 && text != "" && strings.Trim(text, "0123456789abcdef") == "" {
		if raw, err := hex.DecodeString(text); err == nil && IsPrivateKey(raw) {
			return raw
		}
	}
	return []byte(text)
}

// IsPrivateKey says whether the bytes look like a PEM or OpenSSH private key.
func IsPrivateKey(b []byte) bool {
	head := strings.TrimSpace(string(b))
	return strings.HasPrefix(head, "-----BEGIN ") && strings.Contains(head, "PRIVATE KEY-----")
}

// tokenFile is where the token lives when the Keychain is unavailable.
func (r *Resolver) tokenFile() string { return filepath.Join(r.Home, "secrets", tokenFileName) }

// Token finds the 1Password service-account token: the environment, the login Keychain (macOS), then the
// token file under PULSE_HOME. It reports which source answered.
func (r *Resolver) Token(parent context.Context) (token, source string, err error) {
	if t := strings.TrimSpace(r.getenv(TokenEnv)); t != "" {
		return t, "the environment", nil
	}
	if r.goos() == "darwin" {
		ctx, cancel := r.ctx(parent)
		out, _, err := r.run()(ctx, "security", []string{"find-generic-password", "-s", KeychainService, "-a", TokenEnv, "-w"}, "", nil)
		cancel()
		if err == nil && strings.TrimSpace(out) != "" {
			return strings.TrimSpace(out), "the keychain", nil
		}
	}
	if b, err := os.ReadFile(r.tokenFile()); err == nil && strings.TrimSpace(string(b)) != "" {
		return strings.TrimSpace(string(b)), "the token file", nil
	}
	return "", "", errors.New("no service-account token: pulse cred token")
}

var tokenShape = regexp.MustCompile(`^[A-Za-z0-9_.=-]{20,}$`)

// StoreToken keeps a service-account token argv-free: `security -i` reads the command on stdin on macOS;
// elsewhere a 0600 file under PULSE_HOME/secrets. It returns where the token went.
func (r *Resolver) StoreToken(parent context.Context, token string) (string, error) {
	token = strings.TrimSpace(token)
	if !tokenShape.MatchString(token) {
		return "", errors.New("that does not look like a 1Password service-account token (one line, letters, digits, _ . = -)")
	}
	if r.goos() == "darwin" {
		ctx, cancel := r.ctx(parent)
		defer cancel()
		cmd := fmt.Sprintf("add-generic-password -a %s -s %s -U -w '%s'\n", TokenEnv, KeychainService, token)
		if _, stderr, err := r.run()(ctx, "security", []string{"-i"}, cmd, nil); err != nil {
			return "", fmt.Errorf("keychain write failed: %s", Redact(firstLine(stderr, err), token))
		}
		return "the keychain (service " + KeychainService + ", account " + TokenEnv + ")", nil
	}
	dir := filepath.Dir(r.tokenFile())
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", err
	}
	_ = os.Chmod(dir, 0o700)
	if err := os.WriteFile(r.tokenFile(), []byte(token+"\n"), 0o600); err != nil {
		return "", err
	}
	return r.tokenFile(), nil
}

// ClearToken removes the stored token from the Keychain and the token file; a missing token is not an error.
func (r *Resolver) ClearToken(parent context.Context) ([]string, error) {
	var removed []string
	if r.goos() == "darwin" {
		ctx, cancel := r.ctx(parent)
		_, _, err := r.run()(ctx, "security", []string{"delete-generic-password", "-s", KeychainService, "-a", TokenEnv}, "", nil)
		cancel()
		if err == nil {
			removed = append(removed, "the keychain item")
		}
	}
	if err := os.Remove(r.tokenFile()); err == nil {
		removed = append(removed, r.tokenFile())
	} else if !errors.Is(err, os.ErrNotExist) {
		return removed, err
	}
	return removed, nil
}

// Inspect derives the public key and fingerprint of a materialized key with ssh-keygen.
func (r *Resolver) Inspect(parent context.Context, keyPath string) (publicKey, fingerprint string, err error) {
	ctx, cancel := r.ctx(parent)
	defer cancel()
	pub, stderr, err := r.run()(ctx, "ssh-keygen", []string{"-y", "-f", keyPath}, "", nil)
	if err != nil {
		return "", "", fmt.Errorf("ssh-keygen could not read the key: %s", Redact(firstLine(stderr, err)))
	}
	fp, stderr, err := r.run()(ctx, "ssh-keygen", []string{"-l", "-f", keyPath}, "", nil)
	if err != nil {
		return "", "", fmt.Errorf("ssh-keygen could not fingerprint the key: %s", Redact(firstLine(stderr, err)))
	}
	fields := strings.Fields(fp)
	if len(fields) >= 2 {
		fingerprint = fields[1]
	}
	return strings.TrimSpace(pub), fingerprint, nil
}

var (
	pemBlock   = regexp.MustCompile(`(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(-----END [A-Z ]*PRIVATE KEY-----|\z)`)
	tokenBlock = regexp.MustCompile(`\bops_[A-Za-z0-9_.=-]{16,}`)
)

// Redact removes private key blocks, service-account tokens and the given secrets from text meant for a
// log, an event or a hold note.
func Redact(text string, secrets ...string) string {
	text = pemBlock.ReplaceAllString(text, "[private key]")
	text = tokenBlock.ReplaceAllString(text, "[token]")
	for _, s := range secrets {
		if s = strings.TrimSpace(s); s != "" {
			text = strings.ReplaceAll(text, s, "[redacted]")
		}
	}
	return text
}

func firstLine(stderr string, err error) string {
	for _, line := range strings.Split(stderr, "\n") {
		if line = strings.TrimSpace(line); line != "" {
			return line
		}
	}
	if err != nil {
		return err.Error()
	}
	return "unknown error"
}
