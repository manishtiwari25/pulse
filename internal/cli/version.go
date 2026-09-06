package cli

import "runtime/debug"

// version is set by `-ldflags "-X github.com/manishtiwari25/pulse/internal/cli.version=v1.2.3"`;
// otherwise Version() derives it from the build info.
var version = ""

// Version is the canonical pulse version every command and the hub report:
// the linked-in value, else the module version from the build info, else
// "dev-<12-char revision>[-dirty]", else "dev".
func Version() string {
	if version != "" {
		return version
	}
	info, ok := debug.ReadBuildInfo()
	if !ok {
		return "dev"
	}
	if v := info.Main.Version; v != "" && v != "(devel)" {
		return v
	}
	var rev, dirty string
	for _, s := range info.Settings {
		switch s.Key {
		case "vcs.revision":
			rev = s.Value
		case "vcs.modified":
			if s.Value == "true" {
				dirty = "-dirty"
			}
		}
	}
	if rev == "" {
		return "dev"
	}
	if len(rev) > 12 {
		rev = rev[:12]
	}
	return "dev-" + rev + dirty
}
