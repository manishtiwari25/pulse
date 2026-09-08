package runner

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Signing (macOS only). TCC keys a folder grant to the accessing binary's code requirement - not
// to the /bin/sh launcher, which is a platform binary macOS never holds responsible. A Go build
// is ad-hoc (linker-)signed, so its requirement is its own cdhash: every rebuild is a stranger
// and the Desktop/Documents dialog returns. Signed with one local certificate, the requirement
// becomes `identifier "pulse" and certificate leaf = H"..."`, the same for every build, and the
// answer the owner gave once holds across self-updates (ADR-035 rule 8).
//
// The certificate is self-signed and lives in its own keychain under <home>/private/signing with a
// random password beside it (0600; the keychain guards nothing but this local identity). macOS
// asks for the login password once to trust the certificate for code signing - the one act that
// needs the owner - so only `pulse service install` creates it, while the human is there. The
// daemon signs pulse.new with the identity when it exists and says so once when it does not.

const (
	signingName       = "PULSE local code signing"
	signingIdentifier = cli.SigningIdentifier
	signingTimeout    = 2 * time.Minute
	signingTrustWait  = 10 * time.Minute // the trust step waits on the owner's password dialog
)

type signing struct {
	dir, keychain, passwordFile, cert, key, bundle string
}

func signingFor(home string) signing {
	dir := filepath.Join(home, "private", "signing")
	return signing{dir: dir, keychain: filepath.Join(dir, "pulse.keychain-db"), passwordFile: filepath.Join(dir, "password"),
		cert: filepath.Join(dir, "cert.pem"), key: filepath.Join(dir, "key.pem"), bundle: filepath.Join(dir, "identity.p12")}
}

// signingSystem lets tests pretend; "" is runtime.GOOS.
var signingSystem = ""

func signingOS() string {
	if signingSystem != "" {
		return signingSystem
	}
	return runtime.GOOS
}

// ready: the identity exists and macOS trusts it for code signing (codesign refuses an untrusted one).
func (sg signing) ready() bool {
	if signingOS() != "darwin" || !fileExists(sg.keychain) {
		return false
	}
	out, err := runIn("", signingTimeout, "security", "find-identity", "-v", "-p", "codesigning", sg.keychain)
	return err == nil && strings.Contains(out, signingName)
}

// signed: path carries a certificate-based signature rather than the linker's ad-hoc one.
func (sg signing) signed(path string) bool {
	if signingOS() != "darwin" {
		return false
	}
	signed, _ := cli.CodeSigned(path)
	return signed
}

// sign signs a copy and renames it over path, so a running pulse keeps its inode until it restarts
// on the changed file; signing in place would invalidate the running code and macOS would kill it.
func (sg signing) sign(path string) error {
	password, err := os.ReadFile(sg.passwordFile)
	if err != nil {
		return err
	}
	tmp := path + ".signing"
	if err := copyFile(path, tmp); err != nil {
		return err
	}
	if out, err := runIn("", signingTimeout, "security", "unlock-keychain", "-p", strings.TrimSpace(string(password)), sg.keychain); err != nil {
		_ = os.Remove(tmp)
		return fmt.Errorf("unlock-keychain: %s", strings.TrimSpace(firstNonEmpty(out, err.Error())))
	}
	if out, err := runIn("", signingTimeout, "codesign", "--force", "--sign", signingName, "--keychain", sg.keychain, "--identifier", signingIdentifier, "--timestamp=none", tmp); err != nil {
		_ = os.Remove(tmp)
		return fmt.Errorf("codesign: %s", strings.TrimSpace(firstNonEmpty(out, err.Error())))
	}
	if err := os.Rename(tmp, path); err != nil {
		_ = os.Remove(tmp)
		return err
	}
	return nil
}

// setup creates the keychain, the certificate and the trust setting; the last step is where macOS
// asks the owner for the login password. It is idempotent: a ready identity is left alone.
func (sg signing) setup(out io.Writer) error {
	if signingOS() != "darwin" {
		return nil
	}
	if sg.ready() {
		return nil
	}
	if err := os.MkdirAll(sg.dir, 0o700); err != nil {
		return err
	}
	openssl := "/usr/bin/openssl" // LibreSSL: its PKCS#12 is the shape `security import` reads
	if !fileExists(openssl) {
		openssl = "openssl"
	}
	if !fileExists(sg.cert) || !fileExists(sg.key) {
		if o, err := runIn(sg.dir, signingTimeout, openssl, "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", sg.key, "-out", sg.cert, "-days", "3650",
			"-subj", "/CN="+signingName, "-addext", "keyUsage=critical,digitalSignature", "-addext", "extendedKeyUsage=critical,codeSigning", "-addext", "basicConstraints=critical,CA:false"); err != nil {
			return fmt.Errorf("openssl req: %s", strings.TrimSpace(firstNonEmpty(o, err.Error())))
		}
		_ = os.Chmod(sg.key, 0o600)
		_ = os.Remove(sg.keychain)
	}
	password, err := os.ReadFile(sg.passwordFile)
	if err != nil {
		password = []byte(randomHex(16))
		if err := os.WriteFile(sg.passwordFile, password, 0o600); err != nil {
			return err
		}
		_ = os.Remove(sg.keychain)
	}
	pw := strings.TrimSpace(string(password))
	if !fileExists(sg.keychain) {
		if o, err := runIn(sg.dir, signingTimeout, openssl, "pkcs12", "-export", "-inkey", sg.key, "-in", sg.cert, "-out", sg.bundle, "-passout", "pass:"+pw, "-name", signingName); err != nil {
			return fmt.Errorf("openssl pkcs12: %s", strings.TrimSpace(firstNonEmpty(o, err.Error())))
		}
		steps := [][]string{
			{"security", "create-keychain", "-p", pw, sg.keychain},
			{"security", "set-keychain-settings", sg.keychain}, // never auto-lock
			{"security", "import", sg.bundle, "-k", sg.keychain, "-P", pw, "-T", "/usr/bin/codesign"},
			{"security", "set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", pw, sg.keychain},
		}
		for _, step := range steps {
			if o, err := runIn(sg.dir, signingTimeout, step[0], step[1:]...); err != nil {
				_ = os.Remove(sg.keychain)
				return fmt.Errorf("%s %s: %s", step[0], step[1], strings.TrimSpace(firstNonEmpty(o, err.Error())))
			}
		}
		_ = os.Remove(sg.bundle)
	}
	fmt.Fprintln(out, "macOS: trusting the local signing certificate for code signing - macOS asks for your login password once; every pulse build signed with it keeps the folder answer you give.")
	if o, err := runIn(sg.dir, signingTrustWait, "security", "add-trusted-cert", "-r", "trustRoot", "-p", "codeSign", "-k", sg.keychain, sg.cert); err != nil {
		return fmt.Errorf("add-trusted-cert: %s", strings.TrimSpace(firstNonEmpty(o, err.Error())))
	}
	if !sg.ready() {
		return errors.New("the certificate is in the keychain but macOS does not list it as valid for code signing")
	}
	return nil
}

// ensureSigned is `pulse service install`'s macOS step: set the identity up if needed and sign the
// installed binary once. Failures are reported, never fatal: the services still work; each rebuild
// just asks about the folders again until this succeeds.
func ensureSigned(out io.Writer, home, binary string) {
	if signingOS() != "darwin" {
		return
	}
	sg := signingFor(home)
	if err := sg.setup(out); err != nil {
		fmt.Fprintf(out, "macOS: pulse is not signed with a local identity (%s); each rebuild will ask about your folders again. Run pulse service install again to retry.\n", err)
		return
	}
	if sg.signed(binary) {
		fmt.Fprintf(out, "macOS: %s is signed with the local identity; folder answers outlive rebuilds.\n", binary)
		return
	}
	if err := sg.sign(binary); err != nil {
		fmt.Fprintf(out, "macOS: could not sign %s (%s); the next rebuild asks about your folders again.\n", binary, err)
		return
	}
	fmt.Fprintf(out, "macOS: signed %s with the local identity; the running daemon and hub restart on the changed file and macOS asks about your folders one last time.\n", binary)
}

// signNewBuild is the daemon's step between build and smoke (rule 8): sign when the identity is there,
// say once per daemon when it is not, never block the update on it.
func signNewBuild(s *store.Store, path string) (line string, err error) {
	if signingOS() != "darwin" {
		return "", nil
	}
	sg := signingFor(s.Home)
	if !sg.ready() {
		return "self-update: macOS: no local signing identity, so this build keeps the linker's ad-hoc signature and the folder dialog returns once it runs; `pulse service install` sets the identity up (one password prompt) and every later build is silent", nil
	}
	if err := sg.sign(path); err != nil {
		return "", err
	}
	return "", nil
}

func randomHex(n int) string {
	b := make([]byte, n)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)
}

func copyFile(src, dst string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	info, err := in.Stat()
	if err != nil {
		return err
	}
	out, err := os.OpenFile(dst, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, info.Mode().Perm())
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		_ = out.Close()
		return err
	}
	return out.Close()
}
