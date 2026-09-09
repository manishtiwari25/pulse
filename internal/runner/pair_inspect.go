package runner

import (
	"bytes"
	"debug/buildinfo"
	"encoding/base64"
	"fmt"
	"regexp"
	"runtime/debug"
	"strings"

	"github.com/manishtiwari25/pulse/internal/cli"
)

const pairHandsModule = "github.com/manishtiwari25/pulse"

var (
	pairHandsVersionRe = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9.+_-]{0,79}$`)
	pairHandsMarkerRe  = regexp.MustCompile(`(?m)^# pulse-hands ([A-Za-z0-9][A-Za-z0-9.+_-]{0,79})[ \t]*\r?$`)
	pairHandsHashRe    = regexp.MustCompile(`^[a-f0-9]{64}$`)
)

// Exact shipped snapshots of docs/scripts/hosted/pulse-hands.sh, newest first.
// Those scripts predate version markers; their last-changing commit identifies
// them. A claimed version or a short hash alone must never earn "older".
var pairLegacyHands = []struct{ revision, hash string }{
	{"ccb0790", "c75aba6a43bc7db6f640fc145358e5f41304ca2975946e86f0ada9bb79da5118"},
	{"53a1580", "4121f2dcb5a343f68b8bc2bce7a8e44c8f4eb1c9248c8b46aa1000142c382061"},
	{"5eb76b5", "b7c859d6a0b25a34757d8b4b4ab4c18f14fb1292cc1e343098a94ec1bdeeed51"},
	{"90a5b53", "3a0d0b405e9129a12aaebcba56057bc61fcf22655fdc3a073a0ecac7afa03572"},
	{"71f4ea1", "02d0d9ca42e6721383ad6142b9795ed6284d8373ace3c8448a5396cd68edafba"},
	{"d483be9", "1840323a68d3ee8e9122f446eaf3c021da308bc3939d041c13d2c33370cca07d"},
	{"0627cb1", "980ee7cf744715ff4312813bf2d678edbbfb93730ea3b4a4b7d67a0c1f0b0977"},
	{"479c333", "a1ae2f51c1ffa48af5e8b9107a776944f544c6daa234340b77f9be64b48f9b75"},
	{"d80046e", "4f0005f607808ec307a9920b9198dd75728a5f16b0d429e9d26b84291378928c"},
	{"b31f5a7", "cbffffff45c36ff8b660a302de85762a40a554444106cf7aa452e6f0b93af2d7"},
	{"e181712", "82243fd782b61d9392deb399eefed7eea877f101c13a2ab838e949ef01e84c1b"},
}

func pairLegacyHandsIndex(hash string) int {
	for i, release := range pairLegacyHands {
		if release.hash == hash {
			return i
		}
	}
	return -1
}

func pairInspectHandsScript(path, hash string) string {
	return `HANDS_FILE=` + shellQuote(path) + `
HANDS_HASH=` + shellQuote(hash) + `
if [ ! -f "$HANDS_FILE" ] && [ -f "$HANDS_FILE.sh" ]; then HANDS_FILE="$HANDS_FILE.sh"; fi
if [ -f "$HANDS_FILE" ]; then
  if [ ! -r "$HANDS_FILE" ]; then
    printf 'cannot read hands file: %s\n' "$HANDS_FILE" >&2
    exit 1
  fi
  hands_sha256=unknown
  if command -v sha256sum >/dev/null 2>&1; then
    hands_sha256="$(sha256sum "$HANDS_FILE")" || exit 1
    hands_sha256="${hands_sha256%% *}"
  fi
  printf 'hands_sha256=%s\n' "$hands_sha256"
  printf 'hands_version=%s\n' "$(head -c 4096 "$HANDS_FILE" | LC_ALL=C sed -n 's/^# pulse-hands \([A-Za-z0-9][A-Za-z0-9.+_-]*\)[[:space:]]*$/\1/p' | head -n 1)"
  printf 'hands_buildinfo=%s\n' "$(LC_ALL=C grep -aE '^(mod[[:space:]]github.com/manishtiwari25/pulse[[:space:]]|build[[:space:]](-ldflags=|vcs\.revision=|vcs\.modified=))' "$HANDS_FILE" | head -c 8192 | base64 | tr -d '\n')"
  if [ -n "$HANDS_HASH" ] && [ "$hands_sha256" = "$HANDS_HASH" ]; then
    echo hands=present
  else
    echo hands=differs
  fi
else
  echo hands=absent
fi
`
}

func pairHandsBuildVersion(info *debug.BuildInfo) string {
	if info.Main.Path != pairHandsModule {
		return "unknown"
	}
	version := cli.BuildVersion(info)
	for _, setting := range info.Settings {
		if setting.Key != "-ldflags" {
			continue
		}
		args := splitShell(setting.Value)
		for i := 0; i < len(args); i++ {
			arg := args[i]
			if arg == "-X" && i+1 < len(args) {
				i++
				arg = args[i]
			} else if strings.HasPrefix(arg, "-X=") {
				arg = strings.TrimPrefix(arg, "-X=")
			} else {
				continue
			}
			if v, ok := strings.CutPrefix(arg, pairHandsModule+"/internal/cli.version="); ok {
				version = v
				if version == "" {
					version = cli.BuildVersion(info)
				}
			}
		}
	}
	if !pairHandsVersionRe.MatchString(version) {
		return "unknown"
	}
	return version
}

func pairHandsVersion(data []byte) string {
	if i := pairLegacyHandsIndex(sha256Bytes(data)); i >= 0 {
		return "legacy-" + pairLegacyHands[i].revision
	}
	if m := pairHandsMarkerRe.FindSubmatch(data[:min(len(data), 4096)]); m != nil {
		return string(m[1])
	}
	if info, err := buildinfo.Read(bytes.NewReader(data)); err == nil {
		return pairHandsBuildVersion(info)
	}
	return "unknown"
}

func pairInstalledHandsVersion(insp map[string]string) string {
	if version := insp["hands_version"]; pairHandsVersionRe.MatchString(version) {
		return version
	}
	encoded := insp["hands_buildinfo"]
	if len(encoded) > base64.StdEncoding.EncodedLen(8192) {
		return "unknown"
	}
	if raw, err := base64.StdEncoding.DecodeString(encoded); err == nil {
		if info, err := debug.ParseBuildInfo(string(raw)); err == nil {
			return pairHandsBuildVersion(info)
		}
	}
	return "unknown"
}

func pairHandsSHA7(hash string) string {
	if !pairHandsHashRe.MatchString(hash) {
		return "unknown"
	}
	return hash[:7]
}

func pairHandsSummary(insp map[string]string, template []byte) string {
	if insp["hands"] == "absent" {
		return "absent"
	}
	installedHash, templateHash := insp["hands_sha256"], ""
	if len(template) > 0 {
		templateHash = sha256Bytes(template)
		if installedHash == templateHash {
			return "present"
		}
	}
	status := "differs"
	installedVersion, templateVersion := pairInstalledHandsVersion(insp), pairHandsVersion(template)
	if i := pairLegacyHandsIndex(installedHash); i >= 0 {
		installedVersion = "legacy-" + pairLegacyHands[i].revision
		if templateVersion != "unknown" && i > pairLegacyHandsIndex(templateHash) {
			status = "older"
		}
	}
	return fmt.Sprintf("%s (installed %s %s, template %s %s)", status,
		installedVersion, pairHandsSHA7(installedHash), templateVersion, pairHandsSHA7(templateHash))
}
