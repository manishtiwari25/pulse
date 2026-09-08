package cli

import (
	"runtime/debug"
	"testing"
)

func TestBuildVersion(t *testing.T) {
	for _, tc := range []struct {
		name, version, revision, modified, want string
	}{
		{"module", "v1.2.3", "", "", "v1.2.3"},
		{"module-wins", "v1.2.3", "abcdef1234567890", "true", "v1.2.3"},
		{"clean", "(devel)", "abcdef1234567890", "false", "dev-abcdef123456"},
		{"dirty", "(devel)", "abcdef1234567890", "true", "dev-abcdef123456-dirty"},
		{"short-revision", "", "abcdef", "", "dev-abcdef"},
		{"no-revision", "(devel)", "", "true", "dev"},
		{"no-metadata", "", "", "", "dev"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			info := &debug.BuildInfo{
				Main: debug.Module{Version: tc.version},
				Settings: []debug.BuildSetting{
					{Key: "vcs.revision", Value: tc.revision},
					{Key: "vcs.modified", Value: tc.modified},
				},
			}
			if got := BuildVersion(info); got != tc.want {
				t.Errorf("BuildVersion = %q, want %q", got, tc.want)
			}
		})
	}
}
