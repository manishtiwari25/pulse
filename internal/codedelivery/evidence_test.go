package codedelivery

import (
	"strings"
	"testing"
)

func TestSummarizeNameStatusClassifiesCodeDocsGeneratedRenamesAndDeletions(t *testing.T) {
	raw := strings.Join([]string{
		"A", "internal/api/server.go",
		"M", "README.md",
		"M", "web/hub/src/card.css",
		"M", "src/main.mjs",
		"M", "src/main.cjs",
		"R100", "internal/old.go", "internal/new.go",
		"R100", "internal/code.go", "docs/code.md",
		"R100", "docs/guide.md", "internal/guide.go",
		"D", "cmd/deleted/main.go",
		"M", "web/hub/out/index.js",
		"M", "package.json",
		"M", "next.config.ts",
		"M", "eslint.config.js",
		"M", "api/service.pb.go",
		"M", "scripts/run.sh",
		"M", "web/hub/src/minified.min.js",
		"M", "assets/logo.png",
		"",
	}, "\x00")
	evidence, err := SummarizeNameStatus([]byte(raw), "before", "after", 20, 100, map[string]bool{"assets/logo.png": true})
	if err != nil {
		t.Fatal(err)
	}
	if evidence.FilesChanged != 17 || evidence.CodeFiles != 9 || evidence.DocsFiles != 1 || evidence.NonCodeFiles != 7 {
		t.Fatalf("counts = changed:%d code:%d docs:%d non:%d", evidence.FilesChanged, evidence.CodeFiles, evidence.DocsFiles, evidence.NonCodeFiles)
	}
	wantPaths := "cmd/deleted/main.go,internal/api/server.go,internal/code.go,internal/guide.go,internal/new.go,scripts/run.sh,src/main.cjs,src/main.mjs,web/hub/src/card.css"
	if got := strings.Join(evidence.CodePaths, ","); got != wantPaths {
		t.Fatalf("code paths = %s, want %s", got, wantPaths)
	}
	if evidence.PathsTruncated || evidence.ChangesTruncated || evidence.Evidence != EvidenceRecorded || evidence.Version != EvidenceVersion {
		t.Fatalf("unexpected evidence flags: %+v", evidence)
	}
}

func TestSummarizeNameStatusBoundsPreviewAndFileCount(t *testing.T) {
	raw := strings.Join([]string{
		"M", "a.go",
		"M", "b.go",
		"M", "c.go",
		"",
	}, "\x00")
	evidence, err := SummarizeNameStatus([]byte(raw), "before", "after", 2, 2)
	if err != nil {
		t.Fatal(err)
	}
	if evidence.FilesChanged != 2 || evidence.CodeFiles != 2 || strings.Join(evidence.CodePaths, ",") != "a.go,b.go" {
		t.Fatalf("bounded evidence: %+v", evidence)
	}
	if !evidence.ChangesTruncated || !evidence.PathsTruncated {
		t.Fatalf("truncation flags missing: %+v", evidence)
	}
}

func TestParseNameStatusRejectsUnsafeOrMalformedPaths(t *testing.T) {
	for _, raw := range [][]string{
		{"M"},
		{"M", "../outside.go"},
		{"R100", "old.go"},
		{"R100", "old.go", "/tmp/new.go"},
	} {
		t.Run(strings.Join(raw, ","), func(t *testing.T) {
			_, _, err := ParseNameStatus([]byte(strings.Join(append(raw, ""), "\x00")), 100)
			if err == nil {
				t.Fatal("unsafe or malformed name-status output was accepted")
			}
		})
	}
}

func TestParseNumstatBinaryPaths(t *testing.T) {
	raw := strings.Join([]string{
		"1\t0\tinternal/api.go",
		"-\t-\tassets/logo.png",
		"-\t-\tassets/other.png",
		"",
	}, "\x00")
	binaries, truncated, err := ParseNumstatBinaryPaths([]byte(raw), 1)
	if err != nil {
		t.Fatal(err)
	}

	if !truncated || !binaries["assets/logo.png"] || binaries["assets/other.png"] {
		t.Fatalf("binary parsing: binaries=%v truncated=%t", binaries, truncated)
	}
}

func TestCodeDeliveryConfigAndGeneratedConventions(t *testing.T) {
	for _, path := range []string{
		".eslintrc.cjs", ".babelrc.js", ".prettierrc.mjs", ".stylelintrc.ts",
		".lintstagedrc.js", ".commitlintrc.cjs", ".mocharc.cjs",
		"next-env.d.ts", "src/schema.generated.ts", "src/service_pb.js",
		"internal/zz_generated.deepcopy.go",
	} {
		if got := ClassifyPath(path); got != FileNonCode {
			t.Errorf("%s classified as %s", path, got)
		}
	}
	for _, path := range []string{"src/main.mjs", "src/main.cjs", "src/types.d.ts", "src/editor.tsx"} {
		if got := ClassifyPath(path); got != FileCode {
			t.Errorf("source %s classified as %s", path, got)
		}
	}
}
