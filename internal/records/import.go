package records

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// ImportOptions: Paths restricts the committed tree to those files or folders
// (git pathspecs relative to the repository); empty means every record folder.
type ImportOptions struct {
	DryRun   bool
	ForceRev bool
	Author   string
	Paths    []string
}

type KindCount struct {
	Imported int `json:"imported"`
	Skipped  int `json:"skipped"`
	Revised  int `json:"revised"`
}

type SkippedFile struct {
	Path   string `json:"path"`
	Kind   string `json:"kind"`
	Reason string `json:"reason"`
}

type Summary struct {
	Project  string               `json:"project"`
	Ref      string               `json:"ref"`
	ShortRef string               `json:"-"`
	DryRun   bool                 `json:"dry_run"`
	Kinds    map[string]KindCount `json:"kinds"`
	Imported int                  `json:"imported"`
	Revised  int                  `json:"revised"`
	Skipped  []SkippedFile        `json:"skipped"`
}

var importPaths = []string{
	"docs/decisions", "docs/plans", "docs/features", "docs/memory",
	"docs/workflows", "docs/skills", "docs/guides", "docs/context",
	"docs/architecture", "docs/evolution", "docs/usage", "docs/prompts",
	"docs/README.md", "docs/versions.md", "AGENTS.md", "CLAUDE.md",
	".github/copilot-instructions.md",
}

var folderKinds = map[string]string{
	"decisions": "adr", "plans": "plan", "features": "feature",
	"memory": "memory", "workflows": "workflow", "skills": "skill",
	"guides": "guide", "context": "context", "architecture": "architecture",
	"evolution": "evolution", "usage": "usage", "prompts": "prompt",
}

type importItem struct {
	record store.Record
	front  map[string]any
	body   string
	revise bool
}

// ImportRepo reads a committed tree, never the working tree, and uses the store
// record API for every write. Dry runs perform the same preflight without writes.
// A repeated origin path is skipped unless ForceRev sees a changed body.
func ImportRepo(s *store.Store, repo, ref, project string, opts ImportOptions) (Summary, error) {
	summary := Summary{Kinds: map[string]KindCount{}, Skipped: []SkippedFile{}, DryRun: opts.DryRun}
	if !opts.DryRun {
		unlock, err := lockWrites(s)
		if err != nil {
			return summary, err
		}
		defer unlock()
	}
	for _, kind := range store.RecordKinds {
		summary.Kinds[kind] = KindCount{}
	}
	if repo == "" {
		repo = "."
	}
	repo, err := filepath.Abs(repo)
	if err != nil {
		return summary, err
	}
	// Resolve once: a moving branch must not mix different source revisions.
	if ref == "" {
		ref = "HEAD"
	}
	// One call yields the full and abbreviated ids; callers print the short one.
	commit, err := gitRead(repo, "log", "-1", "--format=%H%n%h", "--end-of-options", ref+"^{commit}", "--")
	if err != nil {
		return summary, err
	}
	summary.Ref, summary.ShortRef, _ = strings.Cut(strings.TrimSpace(commit), "\n")
	p, err := resolveProject(s, project, repo)
	if err != nil {
		return summary, err
	}
	summary.Project = p.Slug
	paths := opts.Paths
	if len(paths) == 0 {
		paths = importPaths
	}
	// The tree listing carries each blob's object id, so the files' contents
	// come from one cat-file --batch call instead of one git show per file.
	args := append([]string{"ls-tree", "-r", "-z", summary.Ref, "--"}, paths...)
	tree, err := gitRead(repo, args...)
	if err != nil {
		return summary, err
	}
	files, oids := []string{}, map[string]string{}
	for _, entry := range strings.Split(tree, "\x00") {
		meta, file, found := strings.Cut(entry, "\t")
		if !found {
			continue
		}
		fields := strings.Fields(meta)
		if len(fields) < 3 || fields[1] != "blob" {
			continue
		}
		files = append(files, file)
		oids[file] = fields[2]
	}
	blobs, err := gitCatBlobs(repo, oids)
	if err != nil {
		return summary, err
	}
	existing, err := s.ListRecords(store.RecordFilter{})
	if err != nil {
		return summary, err
	}
	byID := map[string]store.Record{}
	byPath := map[string]store.Record{}
	for _, rec := range existing {
		byID[rec.ID] = rec
		if rec.Project == p.Slug && strings.HasPrefix(rec.Origin, "markdown:") {
			origin := strings.TrimPrefix(rec.Origin, "markdown:")
			if at := strings.LastIndex(origin, "@"); at >= 0 {
				origin = origin[:at]
			}
			byPath[origin] = rec
		}
	}
	items := []importItem{}
	for _, file := range files {
		kind, slug, reason := classifyPath(file)
		if reason != "" {
			summary.skip(file, kind, reason)
			continue
		}
		prev, imported := byPath[file]
		if imported && !opts.ForceRev {
			summary.skip(file, kind, "origin path already imported as "+prev.ID)
			continue
		}
		text, ok := blobs[oids[file]]
		if !ok {
			return summary, inputError("Cannot read repository history", "git cat-file returned no object "+oids[file]+" for "+file, "git -C "+strconv.Quote(repo)+" fsck")
		}
		front, body, err := ParseFrontmatter(text)
		if err != nil {
			summary.skip(file, kind, err.Error())
			continue
		}
		rec := store.Record{Project: p.Slug, Kind: kind, Slug: slug, Path: file, Origin: "markdown:" + file + "@" + summary.Ref}
		rec.ID = scopedID(kind, slug, p)
		if numbered(kind) {
			id := textValue(front["id"])
			if id == "" {
				// Old unnumbered filenames may still have a numbered frontmatter ID.
				parts := strings.Split(path.Base(file), "-")
				if len(parts) > 1 && parts[0] == store.RecordPrefix[kind] {
					id = strings.Join(parts[:2], "-")
				}
			}
			if strings.HasPrefix(id, store.RecordPrefix[kind]+"-") {
				n, err := strconv.Atoi(strings.TrimPrefix(id, store.RecordPrefix[kind]+"-"))
				if err != nil || n <= 0 {
					summary.skip(file, kind, "invalid numbered id "+id)
					continue
				}
				rec.ID, rec.Number = id, n
			} else if id != "" {
				summary.skip(file, kind, "id "+id+" does not match kind "+kind)
				continue
			}
		}
		if err := applyColumns(&rec, front, body); err != nil {
			summary.skip(file, kind, err.Error())
			continue
		}
		if rec.Date == "" {
			date, err := gitRead(repo, "log", "--diff-filter=A", "--format=%cs", "-1", summary.Ref, "--", file)
			if err != nil {
				return summary, err
			}
			rec.Date = strings.TrimSpace(date)
		}
		if imported {
			_, revision, err := s.GetRecord(prev.ID, 0)
			if err != nil {
				return summary, err
			}
			if revision.Body == body {
				summary.skip(file, kind, "body unchanged from "+prev.ID)
				continue
			}
			if prev.Kind != kind || prev.ID != rec.ID {
				summary.skip(file, kind, "origin path changed identity; existing record is "+prev.ID)
				continue
			}
			rec.CreatedAt, rec.CreatedBy, rec.Origin = prev.CreatedAt, prev.CreatedBy, prev.Origin
		} else if collision, found := byID[rec.ID]; found {
			summary.skip(file, kind, fmt.Sprintf("id %s already belongs to project %s, path %s", rec.ID, collision.Project, collision.Path))
			continue
		}
		items = append(items, importItem{record: rec, front: front, body: body, revise: imported})
		byID[rec.ID] = rec
	}
	for _, item := range items {
		if !opts.DryRun {
			note := "import " + item.record.Path + "@" + summary.Ref
			if _, err := s.PutRecord(item.record, item.front, item.body, opts.Author, note); err != nil {
				return summary, fmt.Errorf("import %s: %w", item.record.Path, err)
			}
		}
		count := summary.Kinds[item.record.Kind]
		if item.revise {
			count.Revised++
			summary.Revised++
		} else {
			count.Imported++
			summary.Imported++
		}
		summary.Kinds[item.record.Kind] = count
	}
	if !opts.DryRun {
		if err := s.Event(opts.Author, "records.migrated", p.Slug, summary); err != nil {
			return summary, err
		}
	}
	return summary, nil
}

func (s *Summary) skip(file, kind, reason string) {
	s.Skipped = append(s.Skipped, SkippedFile{Path: file, Kind: kind, Reason: reason})
	count := s.Kinds[kind]
	count.Skipped++
	s.Kinds[kind] = count
}

func classifyPath(file string) (kind, slug, reason string) {
	switch file {
	case "AGENTS.md":
		return "instruction", "agents", ""
	case "CLAUDE.md":
		return "instruction", "claude", ""
	case ".github/copilot-instructions.md":
		return "instruction", "copilot-instructions", ""
	case "docs/README.md":
		return "guide", "docs", ""
	case "docs/versions.md":
		return "usage", "versions", ""
	}
	parts := strings.Split(file, "/")
	if len(parts) < 3 || parts[0] != "docs" {
		return "", "", "outside record folders"
	}
	kind = folderKinds[parts[1]]
	if kind == "" {
		return "", "", "not a record folder"
	}
	base := path.Base(file)
	if base == "README.md" {
		return kind, "", "folder index; use pulse record list"
	}
	if kind == "skill" && base != "SKILL.md" {
		return kind, "", "skill companion file; only SKILL.md is a record"
	}
	if !strings.HasSuffix(base, ".md") {
		return kind, "", "not a Markdown record"
	}
	if strings.HasPrefix(base, "_template") {
		name := strings.TrimPrefix(strings.TrimSuffix(base, ".md"), "_template")
		name = strings.TrimPrefix(name, "-")
		if kind == "plan" && name == "work-unit" {
			return "template", "wu", ""
		}
		slug = kind
		if name != "" {
			slug += "-" + name
		}
		return "template", slug, ""
	}
	if kind == "skill" {
		return kind, slugify(path.Base(path.Dir(file))), ""
	}
	if kind == "plan" && strings.HasPrefix(base, "WU-") {
		kind = "wu"
	}
	slug = strings.TrimSuffix(strings.TrimSuffix(base, ".md"), ".prompt")
	if numbered(kind) {
		parts := strings.SplitN(slug, "-", 3)
		if len(parts) == 3 && parts[0] == store.RecordPrefix[kind] {
			slug = parts[2]
		} else if kind == "feature" {
			_, after, found := strings.Cut(slug, "-")
			if found {
				slug = after
			}
		}
	}
	return kind, slugify(slug), ""
}

// gitCatBlobs reads every object id in oids (values) through one
// `git cat-file --batch` process and returns the contents keyed by id.
func gitCatBlobs(repo string, oids map[string]string) (map[string]string, error) {
	blobs := make(map[string]string, len(oids))
	if len(oids) == 0 {
		return blobs, nil
	}
	var input strings.Builder
	for _, oid := range oids {
		if _, seen := blobs[oid]; seen {
			continue
		}
		blobs[oid] = ""
		input.WriteString(oid + "\n")
	}
	cmd := exec.Command("git", "-C", repo, "cat-file", "--batch")
	cmd.Env = gitRepoEnv()
	cmd.Stdin = strings.NewReader(input.String())
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	output, err := cmd.Output()
	if err != nil {
		if errors.Is(err, exec.ErrNotFound) {
			return nil, inputError("Cannot read repository history", "git is not installed", "git --version")
		}
		return nil, inputError("Cannot read repository history", strings.TrimSpace(stderr.String())+" ("+err.Error()+")", "git -C "+strconv.Quote(repo)+" status")
	}
	rest := string(output)
	for rest != "" {
		header, body, found := strings.Cut(rest, "\n")
		if !found {
			break
		}
		fields := strings.Fields(header)
		if len(fields) == 2 && fields[1] == "missing" {
			delete(blobs, fields[0])
			rest = body
			continue
		}
		if len(fields) != 3 {
			return nil, inputError("Cannot read repository history", "unexpected git cat-file --batch header "+strconv.Quote(header), "git -C "+strconv.Quote(repo)+" fsck")
		}
		size, err := strconv.Atoi(fields[2])
		if err != nil || size < 0 || size > len(body) {
			return nil, inputError("Cannot read repository history", "unexpected git cat-file --batch header "+strconv.Quote(header), "git -C "+strconv.Quote(repo)+" fsck")
		}
		blobs[fields[0]] = body[:size]
		rest = strings.TrimPrefix(body[size:], "\n")
	}
	return blobs, nil
}

// gitRepoEnv returns the process environment without the repository
// selection variables git hooks export (GIT_DIR, GIT_WORK_TREE,
// GIT_INDEX_FILE, GIT_PREFIX), so `git -C repo` reads the repository at
// repo instead of the one hosting the hook.
func gitRepoEnv() []string {
	env := make([]string, 0, len(os.Environ()))
	for _, kv := range os.Environ() {
		key, _, _ := strings.Cut(kv, "=")
		switch key {
		case "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX":
			continue
		}
		env = append(env, kv)
	}
	return env
}

func gitRead(repo string, args ...string) (string, error) {
	cmd := exec.Command("git", append([]string{"-C", repo}, args...)...)
	cmd.Env = gitRepoEnv()
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	output, err := cmd.Output()
	if err != nil {
		if errors.Is(err, exec.ErrNotFound) {
			return "", inputError("Cannot read repository history", "git is not installed", "git --version")
		}
		return "", inputError("Cannot read repository history", strings.TrimSpace(stderr.String())+" ("+err.Error()+")", "git -C "+strconv.Quote(repo)+" status")
	}
	return string(output), nil
}
