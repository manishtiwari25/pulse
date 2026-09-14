package codedelivery

import (
	"encoding/json"
	"fmt"
	"path/filepath"
	"slices"
	"strings"
)

const (
	EvidenceVersion     = 1
	EvidenceRecorded    = "recorded"
	EvidenceGit         = "git"
	EvidenceUnavailable = "unavailable"

	DefaultCodePathLimit = 50
	DefaultFileLimit     = 1000
)

const (
	ReasonNoTrustworthyEvidence = "No trustworthy changed-file evidence"
	ReasonMissingTransition     = "Missing landing transition evidence"
	ReasonGitUnavailable        = "Git changed-file evidence is unavailable"
	ReasonEvidenceMalformed     = "Changed-file evidence is malformed"
	ReasonEvidenceTooLarge      = "Changed-file evidence exceeded the bounded scan"
)

type FileClass string

const (
	FileCode    FileClass = "code"
	FileDocs    FileClass = "docs"
	FileNonCode FileClass = "non_code"
)

type Change struct {
	Status  string `json:"status"`
	Path    string `json:"path"`
	OldPath string `json:"old_path,omitempty"`
}

type Evidence struct {
	Version          int      `json:"version"`
	Evidence         string   `json:"evidence"`
	Before           string   `json:"before,omitempty"`
	After            string   `json:"after,omitempty"`
	FilesChanged     int      `json:"files_changed"`
	CodeFiles        int      `json:"code_files"`
	DocsFiles        int      `json:"docs_files"`
	NonCodeFiles     int      `json:"non_code_files"`
	CodePaths        []string `json:"code_paths"`
	PathsTruncated   bool     `json:"paths_truncated"`
	ChangesTruncated bool     `json:"changes_truncated,omitempty"`
	Reason           string   `json:"reason,omitempty"`
}

type JobSnapshot struct {
	ID         string `json:"id"`
	Title      string `json:"title"`
	Kind       string `json:"kind"`
	Status     string `json:"status"`
	Source     string `json:"source,omitempty"`
	Generation int    `json:"generation,omitempty"`
}

type WorkerSnapshot struct {
	Name     string `json:"name"`
	Runner   string `json:"runner"`
	Model    string `json:"model"`
	Session  string `json:"session,omitempty"`
	RunID    string `json:"run_id,omitempty"`
	Executor string `json:"executor,omitempty"`
}

type WorkSnapshot struct {
	Job    *JobSnapshot    `json:"job,omitempty"`
	Worker *WorkerSnapshot `json:"worker,omitempty"`
}

func UnavailableEvidence(before, after, reason string) Evidence {
	return Evidence{
		Version:   EvidenceVersion,
		Evidence:  EvidenceUnavailable,
		Before:    before,
		After:     after,
		CodePaths: []string{},
		Reason:    PublicReason(reason),
	}
}

func PublicReason(reason string) string {
	reason = strings.Join(strings.Fields(reason), " ")
	switch {
	case reason == "":
		return ReasonNoTrustworthyEvidence
	case reason == ReasonNoTrustworthyEvidence, reason == ReasonMissingTransition, reason == ReasonGitUnavailable,
		reason == ReasonEvidenceMalformed, reason == ReasonEvidenceTooLarge:
		return reason
	case strings.Contains(strings.ToLower(reason), "exceeded"):
		return ReasonEvidenceTooLarge
	case strings.Contains(strings.ToLower(reason), "malformed") || strings.Contains(strings.ToLower(reason), "name-status"):
		return ReasonEvidenceMalformed
	case strings.Contains(strings.ToLower(reason), "git"):
		return ReasonGitUnavailable
	case strings.Contains(strings.ToLower(reason), "missing"):
		return ReasonMissingTransition
	default:
		return ReasonNoTrustworthyEvidence
	}
}

func SummarizeNameStatus(raw []byte, before, after string, pathLimit, fileLimit int, binaryPaths ...map[string]bool) (Evidence, error) {
	if pathLimit <= 0 {
		pathLimit = DefaultCodePathLimit
	}
	if fileLimit <= 0 {
		fileLimit = DefaultFileLimit
	}
	changes, truncated, err := ParseNameStatus(raw, fileLimit)
	if err != nil {
		return Evidence{}, err
	}
	binaries := map[string]bool{}
	if len(binaryPaths) > 0 && binaryPaths[0] != nil {
		binaries = binaryPaths[0]
	}
	e := Evidence{
		Version:          EvidenceVersion,
		Evidence:         EvidenceRecorded,
		Before:           before,
		After:            after,
		CodePaths:        []string{},
		ChangesTruncated: truncated,
	}
	seenCodePaths := map[string]bool{}
	for _, change := range changes {
		class, codePath := classifyChange(change, binaries)
		e.FilesChanged++
		switch class {
		case FileCode:
			e.CodeFiles++
			if !seenCodePaths[codePath] {
				seenCodePaths[codePath] = true
				if len(e.CodePaths) < pathLimit {
					e.CodePaths = append(e.CodePaths, codePath)
				} else {
					e.PathsTruncated = true
				}
			}
		case FileDocs:
			e.DocsFiles++
		default:
			e.NonCodeFiles++
		}
	}
	if truncated && len(e.CodePaths) >= pathLimit {
		e.PathsTruncated = true
	}
	slices.Sort(e.CodePaths)
	return e, nil
}

func ParseNameStatus(raw []byte, fileLimit int) ([]Change, bool, error) {
	text := string(raw)
	changes := []Change{}
	truncated := false
	for text != "" {
		var field string
		field, text, _ = strings.Cut(text, "\x00")
		status := strings.TrimSpace(field)
		if status == "" {
			continue
		}
		if text == "" {
			return nil, false, fmt.Errorf("git name-status output ended after status %q", status)
		}
		code := status[0]
		if code == 'R' || code == 'C' {
			var rawOld, rawPath string
			rawOld, text, _ = strings.Cut(text, "\x00")
			oldPath, ok := cleanRepoPath(rawOld)
			if !ok {
				return nil, false, fmt.Errorf("git reported an unsafe repository path")
			}
			if text == "" {
				return nil, false, fmt.Errorf("git name-status output ended during %q", status)
			}
			rawPath, text, _ = strings.Cut(text, "\x00")
			path, ok := cleanRepoPath(rawPath)
			if !ok {
				return nil, false, fmt.Errorf("git reported an unsafe repository path")
			}
			if len(changes) < fileLimit {
				changes = append(changes, Change{Status: status, Path: path, OldPath: oldPath})
			} else {
				truncated = true
			}
			continue
		}
		var rawPath string
		rawPath, text, _ = strings.Cut(text, "\x00")
		path, ok := cleanRepoPath(rawPath)
		if !ok {
			return nil, false, fmt.Errorf("git reported an unsafe repository path")
		}
		if len(changes) < fileLimit {
			changes = append(changes, Change{Status: status, Path: path})
		} else {
			truncated = true
		}
	}
	return changes, truncated, nil
}

func ClassifyPath(path string) FileClass {
	clean, ok := cleanRepoPath(path)
	if !ok {
		return FileNonCode
	}
	lower := strings.ToLower(clean)
	if generatedOrVendor(lower) || strings.Contains(lower, ".min.") {
		return FileNonCode
	}
	base := strings.ToLower(filepath.Base(lower))
	ext := strings.ToLower(filepath.Ext(base))
	if generatedFile(base) || configFile(base) {
		return FileNonCode
	}
	switch ext {
	case ".md", ".mdx", ".rst", ".adoc", ".txt":
		return FileDocs
	case ".go", ".py", ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".css", ".scss", ".sass", ".less", ".java", ".kt", ".kts",
		".rs", ".c", ".h", ".cc", ".cpp", ".cxx", ".hpp", ".hh", ".m", ".mm", ".swift", ".rb", ".php", ".cs",
		".fs", ".fsx", ".scala", ".sh", ".bash", ".zsh", ".fish", ".ps1", ".vue", ".svelte", ".html", ".htm":
		return FileCode
	}
	switch base {
	case "dockerfile", "containerfile":
		return FileNonCode
	}
	return FileNonCode
}

func DecodeEvidence(value any) (Evidence, bool) {
	var e Evidence
	if value == nil {
		return e, false
	}
	data, err := json.Marshal(value)
	if err != nil {
		return e, false
	}
	if err := json.Unmarshal(data, &e); err != nil {
		return e, false
	}
	if e.Version != EvidenceVersion || (e.Evidence != EvidenceRecorded && e.Evidence != EvidenceGit && e.Evidence != EvidenceUnavailable) {
		return e, false
	}
	if e.CodePaths == nil {
		e.CodePaths = []string{}
	}
	return e, true
}

func DecodeWorkSnapshot(value any) WorkSnapshot {
	var w WorkSnapshot
	if value == nil {
		return w
	}
	data, err := json.Marshal(value)
	if err != nil {
		return w
	}
	_ = json.Unmarshal(data, &w)
	return w
}

func classifyChange(change Change, binaryPaths map[string]bool) (FileClass, string) {
	if binaryPaths[change.Path] || (change.OldPath != "" && binaryPaths[change.OldPath]) {
		return FileNonCode, ""
	}
	class := ClassifyPath(change.Path)
	codePath := change.Path
	if class != FileCode && change.OldPath != "" {
		oldClass := ClassifyPath(change.OldPath)
		if oldClass == FileCode {
			return FileCode, change.OldPath
		}
		if class == FileNonCode && oldClass == FileDocs {
			return FileDocs, ""
		}
	}
	return class, codePath
}

func ParseNumstatBinaryPaths(raw []byte, fileLimit int) (map[string]bool, bool, error) {
	text := string(raw)
	out := map[string]bool{}
	truncated := false
	seen := 0
	for text != "" {
		var record string
		record, text, _ = strings.Cut(text, "\x00")
		if record == "" {
			continue
		}
		fields := strings.SplitN(record, "\t", 3)
		if len(fields) != 3 {
			return nil, false, fmt.Errorf("git numstat output is malformed")
		}
		if fields[0] != "-" || fields[1] != "-" {
			continue
		}
		seen++
		if seen > fileLimit {
			truncated = true
			continue
		}
		path, ok := cleanRepoPath(fields[2])
		if !ok {
			return nil, false, fmt.Errorf("git reported an unsafe repository path")
		}
		out[path] = true
	}
	return out, truncated, nil
}

func generatedFile(base string) bool {
	if base == "next-env.d.ts" || strings.HasPrefix(base, "zz_generated.") {
		return true
	}
	stem := strings.TrimSuffix(base, filepath.Ext(base))
	return strings.HasSuffix(base, ".pb.go") || strings.HasSuffix(base, ".gen.go") ||
		strings.HasSuffix(base, "_generated.go") || strings.HasSuffix(base, ".generated.go") ||
		strings.HasSuffix(stem, ".generated") || strings.HasSuffix(stem, ".gen") ||
		strings.HasSuffix(stem, ".pb") || strings.HasSuffix(stem, "_pb")
}

func configFile(base string) bool {
	if strings.Contains(base, ".config.") {
		return true
	}
	for _, prefix := range []string{".eslintrc.", ".babelrc.", ".prettierrc.", ".stylelintrc.", ".lintstagedrc.", ".commitlintrc.", ".mocharc."} {
		if strings.HasPrefix(base, prefix) {
			return true
		}
	}
	switch base {
	case "dockerfile", "containerfile":
		return true
	}
	return false
}

func generatedOrVendor(path string) bool {
	if path == "web/hub/out" || strings.HasPrefix(path, "web/hub/out/") {
		return true
	}
	for _, part := range strings.Split(path, "/") {
		switch part {
		case "node_modules", "vendor", ".next", "coverage", "generated", ".generated":
			return true
		}
	}
	return false
}

func cleanRepoPath(path string) (string, bool) {
	if strings.ContainsRune(path, '\x00') || strings.TrimSpace(path) == "" || filepath.IsAbs(path) {
		return "", false
	}
	clean := filepath.ToSlash(filepath.Clean(path))
	if clean == "." || clean == "" || strings.HasPrefix(clean, "../") || strings.Contains(clean, "/../") || clean == ".." {
		return "", false
	}
	return clean, true
}
