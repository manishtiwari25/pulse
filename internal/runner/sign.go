package runner

import (
	"fmt"
	"io"
	"os"
	"runtime"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Signing (macOS only). TCC keys a folder grant to the accessing binary's designated requirement -
// not to the /bin/sh launcher, which is a platform binary macOS never holds responsible. A Go build
// is ad-hoc (linker-)signed with the default requirement, its own cdhash: every rebuild is a
// stranger and the Desktop/Documents dialog returns. pulse re-signs each build ad-hoc with an
// explicit requirement, `identifier "pulse"`, which every build meets, so the answer the owner gave
// once holds across self-updates (ADR-035 rule 8). No certificate, no keychain, no password: the
// only binary that can meet the requirement at ~/.pulse/bin/pulse is one written there by this user,
// who already owns the grant.

const (
	signingIdentifier  = cli.SigningIdentifier
	signingRequirement = `=designated => identifier "` + signingIdentifier + `"`
	signingTimeout     = 2 * time.Minute
)

// signingSystem lets tests pretend; "" is runtime.GOOS.
var signingSystem = ""

func signingOS() string {
	if signingSystem != "" {
		return signingSystem
	}
	return runtime.GOOS
}

// signed: path carries pulse's requirement rather than the linker's cdhash one.
func signed(path string) bool {
	if signingOS() != "darwin" {
		return false
	}
	ok, _ := cli.CodeSigned(path)
	return ok
}

// sign re-signs the file at path; running is true when a pulse may be executing it, in which case a
// copy is signed and renamed over path (signing in place would invalidate the running code and macOS
// would kill it; the running process keeps its inode until it restarts on the changed file).
func sign(path string, running bool) error {
	if signingOS() != "darwin" {
		return nil
	}
	target := path
	if running {
		target = path + ".signing"
		if err := copyFile(path, target); err != nil {
			return err
		}
	}
	if out, err := runIn("", signingTimeout, "codesign", "--force", "--sign", "-", "--identifier", signingIdentifier, "--requirements", signingRequirement, target); err != nil {
		if running {
			_ = os.Remove(target)
		}
		return fmt.Errorf("codesign: %s", strings.TrimSpace(firstNonEmpty(out, err.Error())))
	}
	if running {
		if err := os.Rename(target, path); err != nil {
			_ = os.Remove(target)
			return err
		}
	}
	return nil
}

// ensureSigned is the `pulse service install` step: the installed binary carries the requirement before
// the units load, so the first daemon to run is the one whose folder answer sticks. Never fatal.
func ensureSigned(out io.Writer, binary string) {
	if signingOS() != "darwin" {
		return
	}
	if signed(binary) {
		fmt.Fprintf(out, "macOS: %s is signed as pulse; folder answers outlive rebuilds.\n", binary)
		return
	}
	if err := sign(binary, true); err != nil {
		fmt.Fprintf(out, "macOS: could not sign %s (%s); each rebuild asks about your folders again.\n", binary, err)
		return
	}
	fmt.Fprintf(out, "macOS: signed %s as pulse; macOS asks about your folders one last time, and every later build keeps the answer.\n", binary)
}

// signNewBuild is the daemon's step between build and smoke (rule 8): pulse.new is not running, so it is
// signed in place. A failure fails the update at stage "sign" (the ad-hoc build would only bring the
// dialog back).
func signNewBuild(_ *store.Store, path string) error {
	return sign(path, false)
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
