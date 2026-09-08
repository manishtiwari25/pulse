package cli

import (
	"context"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"time"
)

// SigningIdentifier is the code-signing identifier pulse gives its builds on macOS (ADR-035 rule 8):
// TCC ties a folder answer to the binary's designated requirement, and pulse signs every build ad-hoc
// with the explicit requirement `identifier "pulse"`, the same for every build.
const SigningIdentifier = "pulse"

// CodeSigned reports whether the binary at path carries pulse's requirement (true) or only the
// linker's cdhash one or none (false). applicable is false off macOS or when the file is missing, and
// the hub shows nothing then.
func CodeSigned(path string) (signed, applicable bool) {
	if runtime.GOOS != "darwin" || path == "" {
		return false, false
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, "codesign", "-d", "-r-", path).CombinedOutput()
	if err != nil {
		if _, statErr := os.Stat(path); statErr != nil {
			return false, false
		}
		return false, true // present but unsigned: codesign exits 1 ("code object is not signed at all")
	}
	// codesign prints `designated => identifier pulse` (quotes only around identifiers that need them).
	text := strings.ReplaceAll(string(out), `"`, "")
	return strings.Contains(text, "designated => identifier "+SigningIdentifier) && !strings.Contains(text, "cdhash"), true
}

// Executable is the running binary's resolved path, "" when unknown.
func Executable() string {
	exe, err := os.Executable()
	if err != nil {
		return ""
	}
	return exe
}
