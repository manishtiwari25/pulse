package records

import (
	"fmt"
	"regexp"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Record drafts (ADR-046): a proposed body anyone may file against a record,
// never the record's text until a human adopts it. The loop's records
// (constitution, evaluators) are checked when the draft is filed, so the
// agent that wrote it learns at once what to fix: a placeholder it left, a
// human-owned section it changed, a section it dropped.

// DraftInput is what Draft files: the body, who wrote it, one line on what
// it was based on, and the job whose run filed it (PULSE_JOB, may be "").
type DraftInput struct {
	Body   string
	Author string
	Note   string
	JobID  string
}

// Draft files or replaces the record's draft after the checks. The author
// defaults to the human, as every record write does.
func Draft(s *store.Store, id string, in DraftInput) (*store.RecordDraft, error) {
	if in.Author == "" {
		in.Author = store.HumanAgent
	}
	if strings.TrimSpace(in.Body) == "" {
		return nil, inputError("Empty draft", "the body has no content", "pulse record draft "+id+" --body-file F with the complete text")
	}
	rec, rev, err := s.GetRecord(id, 0)
	if err != nil {
		return nil, err
	}
	if strings.TrimSpace(in.Body) == strings.TrimSpace(rev.Body) {
		return nil, inputError("Draft changes nothing", "the body is the record's current text", "write the text the record should have and file that")
	}
	if rec.Kind == "evolution" && (rec.Slug == "constitution" || rec.Slug == "evaluators") {
		if problem := checkLoopDraft(rec.Slug, rev.Body, in.Body); problem != "" {
			return nil, inputError("Draft refused for "+id, problem, "fix the text and file it again with pulse record draft "+id+" --body-file F")
		}
	}
	unlock, err := lockWrites(s)
	if err != nil {
		return nil, err
	}
	defer unlock()
	return s.PutDraft(id, in.Body, in.Author, strings.TrimSpace(in.Note), in.JobID)
}

// Adopt writes the record's draft as its next revision, authored by the
// human with the drafting agent in the note, and clears the draft. It is
// the human's alone: an agent identity is refused, as ADR acceptance is.
func Adopt(s *store.Store, id, actor, words string) (*store.Record, error) {
	if actor != "" && actor != store.HumanAgent {
		return nil, inputError("Adoption is human only", "agents may not adopt a draft into "+id, "leave it for the owner: the hub shows it under the record, or pulse record adopt "+id)
	}
	unlock, err := lockWrites(s)
	if err != nil {
		return nil, err
	}
	defer unlock()
	rec, rev, err := s.GetRecord(id, 0)
	if err != nil {
		return nil, err
	}
	draft, err := s.GetDraft(id)
	if err != nil {
		return nil, err
	}
	if draft == nil {
		return nil, inputError("No draft to adopt", id+" has no draft filed", "pulse record drafts")
	}
	note := "adopted the draft by " + draft.Author
	if draft.BaseRev != rec.CurrentRev {
		note += fmt.Sprintf(" (written against r%d)", draft.BaseRev)
	}
	if words = strings.TrimSpace(words); words != "" {
		note += ": " + words
	}
	if _, err := s.PutRecord(*rec, rev.Frontmatter, draft.Body, store.HumanAgent, note); err != nil {
		return nil, err
	}
	if _, err := s.DeleteDraft(id, store.HumanAgent, "record.adopted", map[string]any{"author": draft.Author, "job": draft.JobID, "base_rev": draft.BaseRev}); err != nil {
		return nil, err
	}
	rec, _, err = s.GetRecord(id, 0)
	return rec, err
}

// Discard clears the record's draft; false when none was filed.
func Discard(s *store.Store, id, actor, words string) (bool, error) {
	if actor == "" {
		actor = store.HumanAgent
	}
	if _, _, err := s.GetRecord(id, 0); err != nil {
		return false, err
	}
	unlock, err := lockWrites(s)
	if err != nil {
		return false, err
	}
	defer unlock()
	return s.DeleteDraft(id, actor, "record.draft.discarded", map[string]any{"words": strings.TrimSpace(words)})
}

// Drafts lists the waiting drafts for one project or all.
func Drafts(s *store.Store, project string) ([]store.RecordDraft, error) {
	return s.ListDrafts(project)
}

// defaultProtected is the "Not amendable by vote" list the template ships.
const defaultProtected = "Hard stops, Kill switch, Amendments, frontmatter"

// placeholderRe is an angle-bracket placeholder the template leaves for the
// owner: <Project>, <N>, <product paths, e.g. `src/`>. Code spans are
// stripped first, so `<parent>..<variant>` in a command is not one.
var placeholderRe = regexp.MustCompile(`<[A-Za-z][^<>\n]*>`)

var codeSpanRe = regexp.MustCompile("`[^`\n]*`")

// checkLoopDraft is the reason a constitution or evaluators draft is
// refused, or "". It proves the agent completed the text and left the
// human-owned sections alone; whether the facts are right is the owner's
// reading before Adopt.
func checkLoopDraft(slug, current, draft string) string {
	if left := Placeholders(draft); len(left) > 0 {
		if len(left) > 3 {
			left = append(left[:3], fmt.Sprintf("and %d more", len(left)-3))
		}
		return fmt.Sprintf("a placeholder is still in the text: %s - replace it with the project's fact", strings.Join(left, ", "))
	}
	if slug != "constitution" {
		return ""
	}
	have, was := sectionsOf(draft), sectionsOf(current)
	for _, name := range sectionOrder(current) {
		if _, ok := have[name]; !ok {
			return fmt.Sprintf("the \"## %s\" section is missing - a constitution keeps every section of the current text", sectionTitle(current, name))
		}
	}
	protected, ok := tableCellOf(current, "Not amendable by vote")
	if !ok {
		protected = defaultProtected
	}
	for _, item := range strings.Split(strings.ReplaceAll(protected, "`", ""), ",") {
		name := strings.ToLower(strings.TrimSpace(item))
		if name == "" || name == "frontmatter" {
			continue
		}
		before, existed := was[name]
		if !existed {
			continue
		}
		if after, present := have[name]; !present || after != before {
			return fmt.Sprintf("the \"%s\" section differs from the current text; it is protected (Not amendable by vote) and changes only by a human's own edit - keep it verbatim", strings.TrimSpace(item))
		}
	}
	return ""
}

// sectionsOf maps each `## ` heading (lowercased) to its normalised text.
func sectionsOf(body string) map[string]string {
	out := map[string]string{}
	name := ""
	var lines []string
	flush := func() {
		if name != "" {
			out[name] = strings.TrimSpace(strings.Join(lines, "\n"))
		}
	}
	for _, line := range strings.Split(body, "\n") {
		if strings.HasPrefix(line, "## ") {
			flush()
			name, lines = strings.ToLower(strings.TrimSpace(line[3:])), nil
			continue
		}
		if strings.HasPrefix(line, "# ") {
			flush()
			name, lines = "", nil
			continue
		}
		lines = append(lines, strings.TrimRight(line, " \t"))
	}
	flush()
	return out
}

// sectionOrder lists the lowercased `## ` headings of a body in order.
func sectionOrder(body string) []string {
	var out []string
	for _, line := range strings.Split(body, "\n") {
		if strings.HasPrefix(line, "## ") {
			out = append(out, strings.ToLower(strings.TrimSpace(line[3:])))
		}
	}
	return out
}

// sectionTitle is the heading as written for a lowercased section name.
func sectionTitle(body, name string) string {
	for _, line := range strings.Split(body, "\n") {
		if strings.HasPrefix(line, "## ") && strings.ToLower(strings.TrimSpace(line[3:])) == name {
			return strings.TrimSpace(line[3:])
		}
	}
	return name
}

// tableCellOf is the second cell of the table row whose first cell is name.
func tableCellOf(text, name string) (string, bool) {
	for _, line := range strings.Split(text, "\n") {
		cells := strings.Split(strings.Trim(line, " \t|"), "|")
		if len(cells) > 1 && strings.EqualFold(strings.Trim(strings.TrimSpace(cells[0]), "*` "), name) {
			return strings.TrimSpace(cells[1]), true
		}
	}
	return "", false
}

// Placeholders lists the angle-bracket placeholders a body still carries,
// outside code spans, each once in order of first appearance.
func Placeholders(body string) []string {
	// Code spans are blanked to spaces of the same length, so the matches
	// index the original text and are reported as written.
	masked := codeSpanRe.ReplaceAllStringFunc(body, func(span string) string { return strings.Repeat(" ", len(span)) })
	seen := map[string]bool{}
	var out []string
	for _, at := range placeholderRe.FindAllStringIndex(masked, -1) {
		p := body[at[0]:at[1]]
		if !seen[p] {
			seen[p] = true
			out = append(out, p)
		}
	}
	return out
}

// Seeded reports whether a record is still the text ADR-030 seeded - its
// first revision, marked seeded_from - and that mark
// (amistio-template/TPL-evolution-constitution@r1; "" when never seeded).
func Seeded(head *store.Record, revision *store.Revision) (bool, string) {
	if head == nil || revision == nil {
		return false, ""
	}
	from, _ := revision.Frontmatter["seeded_from"].(string)
	return head.CurrentRev == 1 && from != "", from
}
