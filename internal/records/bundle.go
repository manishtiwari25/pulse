package records

import (
	"bytes"
	_ "embed"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// The framework's records travel inside the binary (ADR-038). seed/bundle.json
// is a committed build artifact like web/hub/out: `pulse seed export` writes
// it from the brain's store, EnsureFramework applies it to any store that
// lacks its records. The store stays the system of record - the bundle is
// what a copy starts from, never what it reads afterwards.
//
//go:embed seed/bundle.json
var bundleJSON []byte

// FrameworkKinds are the record kinds every copy of PULSE needs and the brain
// owns for everyone: how agents operate, what to read, how to work, what to
// say, what a record looks like, how PULSE is built. Decisions, plans, work
// units, features, memory, context, usage and the loop's records are one
// organization's history and never ship.
var FrameworkKinds = []string{"instruction", "guide", "skill", "workflow", "prompt", "template", "architecture"}

// bundleFile is the bundle's path inside the PULSE checkout.
const bundleFile = "internal/records/seed/bundle.json"

// BundleVersion is the format the binary reads; a bundle of another version is refused.
const BundleVersion = 1

// BundleRecord is one framework record as shipped: the head columns come
// from the frontmatter through applyColumns, as an import's do.
type BundleRecord struct {
	ID          string         `json:"id"`
	Kind        string         `json:"kind"`
	Slug        string         `json:"slug"`
	Frontmatter map[string]any `json:"frontmatter"`
	Body        string         `json:"body"`
}

// Bundle is the file: a version and the records sorted by kind (FrameworkKinds
// order) then id, so two exports of the same store are byte-identical.
type Bundle struct {
	Version int            `json:"version"`
	Records []BundleRecord `json:"records"`
}

// FrameworkBundle decodes the embedded bundle.
func FrameworkBundle() (*Bundle, error) {
	return decodeBundle(bundleJSON)
}

func decodeBundle(data []byte) (*Bundle, error) {
	var b Bundle
	if err := json.Unmarshal(data, &b); err != nil {
		return nil, fmt.Errorf("framework bundle: %w", err)
	}
	if b.Version != BundleVersion {
		return nil, fmt.Errorf("framework bundle: version %d, this pulse reads %d", b.Version, BundleVersion)
	}
	for _, r := range b.Records {
		if r.ID == "" || r.Kind == "" || r.Slug == "" {
			return nil, fmt.Errorf("framework bundle: a record lacks id, kind or slug")
		}
		if !frameworkKind(r.Kind) {
			return nil, fmt.Errorf("framework bundle: %s has kind %q, not one of %s", r.ID, r.Kind, strings.Join(FrameworkKinds, ", "))
		}
	}
	return &b, nil
}

func frameworkKind(kind string) bool {
	for _, k := range FrameworkKinds {
		if k == kind {
			return true
		}
	}
	return false
}

// seededFrom names the build that seeded a record: pulse@<12-char revision>,
// or pulse@dev for a build the toolchain could not stamp.
func seededFrom() string {
	rev := cli.Revision()
	if rev == "" {
		return "pulse@dev"
	}
	if len(rev) > 12 {
		rev = rev[:12]
	}
	return "pulse@" + rev
}

// EnsureResult is what EnsureFramework did: how many records it wrote and
// their ids, and how many were already there.
type EnsureResult struct {
	Written []string `json:"written"`
	Kept    int      `json:"kept"`
	From    string   `json:"from"`
}

// EnsureFramework gives a store every framework record it lacks (ADR-038).
// A record absent by id is written under the commons scope - the ids are the
// brain's unscoped ones, so templateBody, seedSource and pulse guide find
// them whatever project owns them - with the origin naming this build. A
// record present, whatever its project or text, is kept (ADR-030's rule).
// The row's origin (seed:pulse@rev) and the revision's note say where the
// text came from; the frontmatter stays as bundled.
// The hub, the daemon and project registration call it; pulse seed apply runs
// it by hand. A store that has everything is read only.
func EnsureFramework(s *store.Store, actor string) (EnsureResult, error) {
	result := EnsureResult{Written: []string{}, From: seededFrom()}
	b, err := FrameworkBundle()
	if err != nil {
		return result, err
	}
	if actor == "" {
		actor = store.HumanAgent
	}
	missing := make([]BundleRecord, 0)
	for _, r := range b.Records {
		if _, _, err := s.GetRecord(r.ID, 0); err == nil {
			result.Kept++
			continue
		} else if !errors.Is(err, store.ErrNoRecord) {
			return result, err
		}
		missing = append(missing, r)
	}
	if len(missing) == 0 {
		return result, nil
	}
	unlock, err := lockWrites(s)
	if err != nil {
		return result, err
	}
	defer unlock()
	for _, r := range missing {
		// Another process may have written it between the read and the lock.
		if _, _, err := s.GetRecord(r.ID, 0); err == nil {
			result.Kept++
			continue
		} else if !errors.Is(err, store.ErrNoRecord) {
			return result, err
		}
		// The text stays as bundled - a record without frontmatter (INSTR-agents is AGENTS.md) renders
		// bare from pulse guide - so where it came from is the row's origin and the revision's note.
		front := make(map[string]any, len(r.Frontmatter))
		for key, value := range r.Frontmatter {
			front[key] = value
		}
		rec := store.Record{ID: r.ID, Project: store.CommonsSlug, Kind: r.Kind, Slug: r.Slug, Origin: "seed:" + result.From}
		if err := applyColumns(&rec, front, r.Body); err != nil {
			return result, fmt.Errorf("%s: %w", r.ID, err)
		}
		if _, err := s.PutRecord(rec, front, r.Body, actor, "seeded from the framework bundle (ADR-038)"); err != nil {
			return result, err
		}
		result.Written = append(result.Written, r.ID)
	}
	if len(result.Written) > 0 {
		if err := s.Event(actor, "records.framework.seeded", store.CommonsSlug, map[string]any{"written": len(result.Written), "from": result.From}); err != nil {
			return result, err
		}
	}
	return result, nil
}

// secretShapes are the token shapes an export refuses: a framework record
// that carries one is a leak, whatever the record says it is.
var secretShapes = []*regexp.Regexp{
	regexp.MustCompile(`\bgh[pousr]_[A-Za-z0-9]{20,}`),
	regexp.MustCompile(`\bgithub_pat_[A-Za-z0-9_]{20,}`),
	regexp.MustCompile(`\bsk-[A-Za-z0-9_-]{20,}`),
	regexp.MustCompile(`\bAKIA[0-9A-Z]{16}\b`),
	regexp.MustCompile(`\bxox[abprs]-[A-Za-z0-9-]{10,}`),
	regexp.MustCompile(`\bAIza[0-9A-Za-z_-]{35}`),
	regexp.MustCompile(`-----BEGIN [A-Z ]*PRIVATE KEY-----`),
	regexp.MustCompile(`\beyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{10,}`),
}

// secretShaped names the first secret shape found in text, or "".
func secretShaped(text string) string {
	for _, re := range secretShapes {
		if m := re.FindString(text); m != "" {
			if len(m) > 12 {
				m = m[:12] + "…"
			}
			return m
		}
	}
	return ""
}

// retiredStatus reports a record whose text is no longer current; it is
// left out of the bundle. Draft and Proposed are not retired - templates
// carry the status their copies start with.
func retiredStatus(status string) bool {
	switch strings.ToLower(strings.TrimSpace(status)) {
	case "deprecated", "retired", "superseded":
		return true
	}
	return false
}

// BundleSummary is ExportBundle's account of the file it wrote against the
// one it replaced.
type BundleSummary struct {
	Path      string   `json:"path"`
	Records   int      `json:"records"`
	Added     []string `json:"added"`
	Removed   []string `json:"removed"`
	Changed   []string `json:"changed"`
	Unchanged int      `json:"unchanged"`
	Skipped   []string `json:"skipped"`
}

// Same reports whether the export changed nothing.
func (b BundleSummary) Same() bool {
	return len(b.Added) == 0 && len(b.Removed) == 0 && len(b.Changed) == 0
}

// BuildBundle collects the framework's records at their current revision:
// FrameworkKinds only, retired statuses left out, secret-shaped text refused.
// They are the brain's, or the commons' where an earlier bundle seeded them
// before the brain registered (pulse migrate keeps an id where it is). Records
// whose id carries a project suffix never occur for either (scopedID), so the
// ids are the ones every store looks up.
func BuildBundle(s *store.Store) (*Bundle, []string, error) {
	brain, err := s.Brain()
	if err != nil {
		return nil, nil, err
	}
	if brain == nil {
		return nil, nil, inputError("Cannot build the seed bundle", "no active project is the brain; the framework's records are the brain's", "pulse init PATH --brain, then pulse seed export")
	}
	rows, err := s.ListRecords(store.RecordFilter{Project: brain.Slug})
	if err != nil {
		return nil, nil, err
	}
	commons, err := s.ListRecords(store.RecordFilter{Project: store.CommonsSlug})
	if err != nil {
		return nil, nil, err
	}
	rows = append(rows, commons...)
	b := &Bundle{Version: BundleVersion, Records: []BundleRecord{}}
	skipped := []string{}
	for _, row := range rows {
		if !frameworkKind(row.Kind) {
			continue
		}
		if retiredStatus(row.Status) {
			skipped = append(skipped, row.ID+" ("+row.Status+")")
			continue
		}
		_, rev, err := s.GetRecord(row.ID, 0)
		if err != nil {
			return nil, nil, err
		}
		front := make(map[string]any, len(rev.Frontmatter))
		for key, value := range rev.Frontmatter {
			if key == "seeded_from" || key == "seeded_by" {
				continue
			}
			front[key] = value
		}
		if shape := secretShaped(rev.Body); shape != "" {
			return nil, nil, inputError("Refusing to bundle "+row.ID, "its body carries a secret-shaped value ("+shape+")", "remove the value with pulse record amend "+row.ID+", then pulse seed export")
		}
		for key, value := range front {
			if shape := secretShaped(textValue(value)); shape != "" {
				return nil, nil, inputError("Refusing to bundle "+row.ID, "frontmatter "+key+" carries a secret-shaped value ("+shape+")", "remove the value with pulse record amend "+row.ID+", then pulse seed export")
			}
		}
		b.Records = append(b.Records, BundleRecord{ID: row.ID, Kind: row.Kind, Slug: row.Slug, Frontmatter: front, Body: rev.Body})
	}
	sortBundle(b)
	return b, skipped, nil
}

func sortBundle(b *Bundle) {
	order := map[string]int{}
	for i, k := range FrameworkKinds {
		order[k] = i
	}
	sort.Slice(b.Records, func(i, j int) bool {
		if order[b.Records[i].Kind] != order[b.Records[j].Kind] {
			return order[b.Records[i].Kind] < order[b.Records[j].Kind]
		}
		return b.Records[i].ID < b.Records[j].ID
	})
}

// encodeBundle renders the bundle as indented JSON with a trailing newline;
// maps encode with sorted keys, so the text is stable across exports.
func encodeBundle(b *Bundle) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(b); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// BundlePath is where the bundle lives: --out, else the brain checkout's
// internal/records/seed/bundle.json.
func BundlePath(s *store.Store, out string) (string, error) {
	if strings.TrimSpace(out) != "" {
		return filepath.Clean(out), nil
	}
	brain, err := s.Brain()
	if err != nil {
		return "", err
	}
	if brain == nil || brain.Path == "" {
		return "", inputError("Cannot place the seed bundle", "no brain checkout to write it into", "pulse seed export --out PATH")
	}
	return filepath.Join(brain.Path, filepath.FromSlash(bundleFile)), nil
}

// ExportBundle writes the brain's framework records to path (`pulse seed
// export`, a maintainer's command) and reports what changed against the file
// it replaced. The file is a committed build artifact by decision (ADR-038),
// so unlike pulse record export it writes inside the checkout.
func ExportBundle(s *store.Store, path string) (BundleSummary, error) {
	summary := BundleSummary{Path: path, Added: []string{}, Removed: []string{}, Changed: []string{}, Skipped: []string{}}
	b, skipped, err := BuildBundle(s)
	if err != nil {
		return summary, err
	}
	summary.Records, summary.Skipped = len(b.Records), skipped
	data, err := encodeBundle(b)
	if err != nil {
		return summary, err
	}
	if old, err := os.ReadFile(path); err == nil {
		if prev, err := decodeBundle(old); err == nil {
			diffBundles(prev, b, &summary)
		} else {
			for _, r := range b.Records {
				summary.Added = append(summary.Added, r.ID)
			}
		}
	} else {
		for _, r := range b.Records {
			summary.Added = append(summary.Added, r.ID)
		}
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return summary, err
	}
	if err := os.WriteFile(path, data, 0o644); err != nil {
		return summary, err
	}
	return summary, nil
}

// diffBundles fills Added, Removed, Changed and Unchanged by id.
func diffBundles(prev, next *Bundle, summary *BundleSummary) {
	before := map[string]BundleRecord{}
	for _, r := range prev.Records {
		before[r.ID] = r
	}
	after := map[string]bool{}
	for _, r := range next.Records {
		after[r.ID] = true
		old, ok := before[r.ID]
		switch {
		case !ok:
			summary.Added = append(summary.Added, r.ID)
		case sameBundleRecord(old, r):
			summary.Unchanged++
		default:
			summary.Changed = append(summary.Changed, r.ID)
		}
	}
	for _, r := range prev.Records {
		if !after[r.ID] {
			summary.Removed = append(summary.Removed, r.ID)
		}
	}
	sort.Strings(summary.Removed)
}

func sameBundleRecord(a, b BundleRecord) bool {
	if a.Kind != b.Kind || a.Slug != b.Slug || a.Body != b.Body {
		return false
	}
	fa, _ := json.Marshal(a.Frontmatter)
	fb, _ := json.Marshal(b.Frontmatter)
	return bytes.Equal(fa, fb)
}

// BundleStatus compares the embedded bundle with a store: which records the
// store lacks, and - when the store has a brain - which of the brain's
// framework records now differ from the shipped text, so the maintainer
// knows the bundle is stale before the next build ships it.
type BundleStatus struct {
	Records   int      `json:"records"`
	Present   int      `json:"present"`
	Missing   []string `json:"missing"`
	Stale     []string `json:"stale"`
	Unshipped []string `json:"unshipped"`
	Brain     string   `json:"brain,omitempty"`
}

func FrameworkStatus(s *store.Store) (BundleStatus, error) {
	status := BundleStatus{Missing: []string{}, Stale: []string{}, Unshipped: []string{}}
	b, err := FrameworkBundle()
	if err != nil {
		return status, err
	}
	status.Records = len(b.Records)
	shipped := map[string]BundleRecord{}
	for _, r := range b.Records {
		shipped[r.ID] = r
		if _, _, err := s.GetRecord(r.ID, 0); err == nil {
			status.Present++
		} else if errors.Is(err, store.ErrNoRecord) {
			status.Missing = append(status.Missing, r.ID)
		} else {
			return status, err
		}
	}
	brain, err := s.Brain()
	if err != nil || brain == nil {
		return status, err
	}
	status.Brain = brain.Slug
	current, _, err := BuildBundle(s)
	if err != nil {
		return status, err
	}
	for _, r := range current.Records {
		old, ok := shipped[r.ID]
		switch {
		case !ok:
			status.Unshipped = append(status.Unshipped, r.ID)
		case !sameBundleRecord(old, r):
			status.Stale = append(status.Stale, r.ID)
		}
	}
	return status, nil
}
