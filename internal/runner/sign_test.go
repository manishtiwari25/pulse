package runner

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

// Off macOS signing is silent and does nothing; on macOS without an identity the daemon says so
// and installs the ad-hoc build anyway (ADR-035 rule 8). Creating the identity needs the owner's
// password, so setup itself is exercised live by `pulse service install`, not here.
func TestSigningIsQuietOffMacOSAndHonestWithoutAnIdentity(t *testing.T) {
	resetSelfUpdate()
	t.Cleanup(resetSelfUpdate)
	s := newStore(t)
	binary := filepath.Join(t.TempDir(), "pulse")
	if err := os.WriteFile(binary, []byte("not a mach-o"), 0o755); err != nil {
		t.Fatal(err)
	}

	signingSystem = "linux"
	t.Cleanup(func() { signingSystem = "" })
	sg := signingFor(s.Home)
	if sg.ready() || sg.signed(binary) {
		t.Fatal("nothing is signed off macOS")
	}
	if line, err := signNewBuild(s, binary); line != "" || err != nil {
		t.Fatalf("off macOS signNewBuild is silent: %q %v", line, err)
	}
	var out bytes.Buffer
	ensureSigned(&out, s.Home, binary)
	if out.Len() != 0 {
		t.Fatalf("off macOS ensureSigned says nothing: %q", out.String())
	}
	if err := sg.setup(&out); err != nil || out.Len() != 0 {
		t.Fatalf("off macOS setup is a no-op: %v %q", err, out.String())
	}

	if runtime.GOOS != "darwin" {
		return
	}
	if _, err := exec.LookPath("codesign"); err != nil {
		t.Skip("no codesign")
	}
	signingSystem = "darwin"
	if sg.ready() {
		t.Fatal("a fresh home has no identity")
	}
	if sg.signed(binary) {
		t.Fatal("a plain file is not signed")
	}
	line, err := signNewBuild(s, binary)
	if err != nil || !strings.Contains(line, "no local signing identity") || !strings.Contains(line, "pulse service install") {
		t.Fatalf("without an identity the daemon says so and goes on: %q %v", line, err)
	}
	if data, _ := os.ReadFile(binary); string(data) != "not a mach-o" {
		t.Fatal("the build was touched without an identity")
	}
	if err := sg.sign(binary); err == nil {
		t.Fatal("sign without a password file must fail")
	}
	if fileExists(binary + ".signing") {
		t.Fatal("a failed sign left its copy behind")
	}
	if got := filepath.Base(sg.keychain); got != "pulse.keychain-db" || !strings.HasPrefix(sg.dir, filepath.Join(s.Home, "private")) {
		t.Fatalf("the identity lives under the store's private dir: %+v", sg)
	}
}
