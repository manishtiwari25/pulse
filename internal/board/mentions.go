package board

import (
	"strings"
	"unicode"

	"github.com/manishtiwari25/pulse/internal/store"
)

// ParseMentions returns first-seen @name mentions in text, normalized to agent names.
func ParseMentions(text string) []string {
	var out []string
	seen := map[string]bool{}
	runes := []rune(text)
	for i := 0; i < len(runes); i++ {
		if runes[i] != '@' || !mentionBoundary(runes, i) {
			continue
		}
		j := i + 1
		for j < len(runes) && !unicode.IsSpace(runes[j]) && !mentionBoundaryStop(runes[j]) {
			j++
		}
		raw := strings.TrimRight(string(runes[i+1:j]), ".,;:!?)]}")
		name := strings.ToLower(raw)
		if name == "owner" {
			name = store.HumanAgent
		}
		if ValidName(name) && !seen[name] {
			seen[name] = true
			out = append(out, name)
		}
		i = j - 1
	}
	return out
}

func mentionBoundary(runes []rune, at int) bool {
	if at == 0 {
		return true
	}
	prev := runes[at-1]
	return unicode.IsSpace(prev) || strings.ContainsRune(`([{"'`, prev)
}

func mentionBoundaryStop(r rune) bool {
	return strings.ContainsRune(`([{"'`, r)
}

func postMentions(s *store.Store, author, text string) ([]string, error) {
	var out []string
	for _, name := range ParseMentions(text) {
		if name == author {
			continue
		}
		if name == store.HumanAgent {
			out = append(out, name)
			continue
		}
		a, err := s.Agent(name)
		if err != nil {
			return nil, err
		}
		if a != nil && a.Status != "pruned" {
			out = append(out, name)
		}
	}
	return out, nil
}

// MentionsFor returns unanswered posts that mention agent, newest first.
func MentionsFor(s *store.Store, project, agent string, limit int) ([]Post, error) {
	if strings.ToLower(agent) == "owner" {
		agent = store.HumanAgent
	}
	q := "SELECT p." + strings.ReplaceAll(postCols, ", ", ", p.") + ` FROM posts p
WHERE (? = '' OR p.project = ?)
  AND p.agent != ?
  AND json_valid(p.extra)
  AND EXISTS (SELECT 1 FROM json_each(p.extra, '$.mentions') WHERE value = ?)
  AND NOT EXISTS (
    SELECT 1 FROM posts a
    WHERE a.project = p.project AND a.area = p.area AND a.thread = p.thread AND a.agent = ?
      AND (a.date > p.date OR (a.date = p.date AND a.rowid > p.rowid))
  )
ORDER BY p.date DESC, p.rowid DESC`
	args := []any{project, project, agent, agent, agent}
	if limit > 0 {
		q += " LIMIT ?"
		args = append(args, limit)
	}
	rows, err := s.DB.Query(q, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Post
	for rows.Next() {
		p, err := scanPost(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, *p)
	}
	return out, rows.Err()
}

// MentionCount returns the number of unanswered mentions for agent.
func MentionCount(s *store.Store, project, agent string) (int, error) {
	rows, err := MentionsFor(s, project, agent, 0)
	if err != nil {
		return 0, err
	}
	return len(rows), nil
}
