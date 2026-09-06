package records

import (
	"encoding/json"
	"fmt"
	"sort"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// ParseFrontmatter parses the YAML subset used by PULSE records. Body bytes
// (including the blank line after the closing delimiter) are kept unchanged.
// Scalars, flow collections, indented maps/lists, and literal/folded text are
// supported; malformed or unsupported YAML is an error, not discarded data.
func ParseFrontmatter(text string) (map[string]any, string, error) {
	front := map[string]any{}
	first, rest, ok := strings.Cut(text, "\n")
	if strings.TrimSuffix(first, "\r") != "---" {
		return front, text, nil
	}
	if !ok {
		return nil, "", fmt.Errorf("frontmatter: missing closing ---")
	}
	var lines []string
	for {
		line, remaining, newline := strings.Cut(rest, "\n")
		if strings.TrimSuffix(line, "\r") == "---" {
			p := yamlSubset{lines: lines}
			v, err := p.mapping(0)
			if err != nil {
				return nil, "", err
			}
			return v, remaining, nil
		}
		lines = append(lines, strings.TrimSuffix(line, "\r"))
		if !newline {
			return nil, "", fmt.Errorf("frontmatter: missing closing ---")
		}
		rest = remaining
	}
}

type yamlSubset struct {
	lines []string
	pos   int
}

func indentation(s string) int { return len(s) - len(strings.TrimLeft(s, " ")) }

func (p *yamlSubset) skip() {
	for p.pos < len(p.lines) {
		s := strings.TrimSpace(p.lines[p.pos])
		if s != "" && !strings.HasPrefix(s, "#") {
			return
		}
		p.pos++
	}
}

func (p *yamlSubset) fail(message string) error {
	return fmt.Errorf("frontmatter line %d: %s", p.pos+2, message)
}

func (p *yamlSubset) mapping(indent int) (map[string]any, error) {
	out := map[string]any{}
	for {
		p.skip()
		if p.pos == len(p.lines) || indentation(p.lines[p.pos]) < indent {
			return out, nil
		}
		line := p.lines[p.pos]
		if indentation(line) != indent || strings.HasPrefix(strings.TrimSpace(line), "- ") {
			return nil, p.fail("unexpected indentation or list item")
		}
		key, raw, found := strings.Cut(strings.TrimSpace(line), ":")
		key = strings.TrimSpace(key)
		if !found || key == "" || strings.ContainsAny(key, " \t[]{}&*!") {
			return nil, p.fail("expected key: value")
		}
		if _, exists := out[key]; exists {
			return nil, p.fail("duplicate key " + key)
		}
		p.pos++
		raw = strings.TrimSpace(stripComment(raw))
		var value any
		var err error
		switch {
		case isBlockScalar(raw):
			value, err = p.blockScalar(raw, indent)
		case raw == "":
			p.skip()
			if p.pos < len(p.lines) && indentation(p.lines[p.pos]) >= indent && isListItem(strings.TrimSpace(p.lines[p.pos])) {
				value, err = p.sequence(indentation(p.lines[p.pos]))
			} else if p.pos < len(p.lines) && indentation(p.lines[p.pos]) > indent {
				nestedIndent := indentation(p.lines[p.pos])
				value, err = p.mapping(nestedIndent)
			} else {
				value = nil
			}
		default:
			value, err = parseValue(raw)
		}
		if err != nil {
			return nil, p.fail(key + ": " + err.Error())
		}
		out[key] = value
	}
}

func (p *yamlSubset) sequence(indent int) ([]any, error) {
	out := []any{}
	for {
		p.skip()
		if p.pos == len(p.lines) || indentation(p.lines[p.pos]) < indent {
			return out, nil
		}
		line := strings.TrimSpace(p.lines[p.pos])
		if indentation(p.lines[p.pos]) == indent && !isListItem(line) {
			return out, nil
		}
		if indentation(p.lines[p.pos]) != indent || !isListItem(line) {
			return nil, p.fail("expected - item")
		}
		raw := strings.TrimSpace(stripComment(strings.TrimPrefix(line, "-")))
		p.pos++
		var value any
		var err error
		if isBlockScalar(raw) {
			value, err = p.blockScalar(raw, indent)
		} else {
			value, err = parseValue(raw)
		}
		if err != nil {
			return nil, p.fail(err.Error())
		}
		out = append(out, value)
	}
}

func isListItem(s string) bool { return s == "-" || strings.HasPrefix(s, "- ") }

func isBlockScalar(s string) bool {
	return s == "|" || s == "|-" || s == "|+" || s == ">" || s == ">-" || s == ">+"
}

func (p *yamlSubset) blockScalar(marker string, parentIndent int) (string, error) {
	start := p.pos
	indent := -1
	for p.pos < len(p.lines) {
		line := p.lines[p.pos]
		if strings.TrimSpace(line) != "" {
			n := indentation(line)
			if n <= parentIndent {
				break
			}
			if indent < 0 {
				indent = n
			}
			if n < indent {
				return "", p.fail("inconsistent block scalar indentation")
			}
		}
		p.pos++
	}
	if start == p.pos {
		return "", nil
	}
	lines := make([]string, p.pos-start)
	for i, s := range p.lines[start:p.pos] {
		if strings.TrimSpace(s) != "" {
			lines[i] = s[indent:]
		}
	}
	var b strings.Builder
	for i, s := range lines {
		b.WriteString(s)
		if marker[0] == '>' && i+1 < len(lines) {
			next := lines[i+1]
			switch {
			case s != "" && next != "" && !strings.HasPrefix(s, " ") && !strings.HasPrefix(next, " "):
				b.WriteByte(' ')
			case s == "" && next != "" && i > 0 && lines[i-1] != "" && !strings.HasPrefix(next, " "):
				// A blank line between ordinary folded paragraphs is one newline.
			default:
				b.WriteByte('\n')
			}
		} else {
			b.WriteByte('\n')
		}
	}
	text := b.String()
	switch {
	case strings.HasSuffix(marker, "-"):
		return strings.TrimRight(text, "\n"), nil
	case strings.HasSuffix(marker, "+"):
		return text, nil
	default:
		return strings.TrimRight(text, "\n") + "\n", nil
	}
}

func stripComment(s string) string {
	quote := byte(0)
	for i := 0; i < len(s); i++ {
		c := s[i]
		if quote != 0 {
			if c == '\\' && quote == '"' {
				i++
			} else if c == quote {
				if quote == '\'' && i+1 < len(s) && s[i+1] == '\'' {
					i++
				} else {
					quote = 0
				}
			}
		} else if (c == '\'' || c == '"') && (i == 0 || strings.ContainsRune(" \t[:,{", rune(s[i-1]))) {
			quote = c
		} else if c == '#' && (i == 0 || s[i-1] == ' ' || s[i-1] == '\t') {
			return strings.TrimRight(s[:i], " \t")
		}
	}
	return s
}

func parseValue(raw string) (any, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" || raw == "~" || strings.EqualFold(raw, "null") {
		return nil, nil
	}
	switch raw[0] {
	case '"':
		var s string
		if err := json.Unmarshal([]byte(raw), &s); err != nil {
			return nil, fmt.Errorf("invalid quoted string: %w", err)
		}
		return s, nil
	case '\'':
		if len(raw) < 2 || raw[len(raw)-1] != '\'' {
			return nil, fmt.Errorf("unterminated quoted string")
		}
		inside := raw[1 : len(raw)-1]
		for i := 0; i < len(inside); i++ {
			if inside[i] == '\'' {
				if i+1 >= len(inside) || inside[i+1] != '\'' {
					return nil, fmt.Errorf("single quote must be doubled")
				}
				i++
			}
		}
		return strings.ReplaceAll(inside, "''", "'"), nil
	case '[':
		if !strings.HasSuffix(raw, "]") {
			return nil, fmt.Errorf("unclosed list")
		}
		parts, err := splitFlow(raw[1 : len(raw)-1])
		if err != nil {
			return nil, err
		}
		out := []any{}
		for _, part := range parts {
			v, err := parseValue(part)
			if err != nil {
				return nil, err
			}
			out = append(out, v)
		}
		return out, nil
	case '{':
		if !strings.HasSuffix(raw, "}") {
			return nil, fmt.Errorf("unclosed map")
		}
		parts, err := splitFlow(raw[1 : len(raw)-1])
		if err != nil {
			return nil, err
		}
		out := map[string]any{}
		for _, part := range parts {
			key, val, ok := strings.Cut(part, ":")
			if !ok {
				return nil, fmt.Errorf("expected map key: value")
			}
			k, err := parseValue(strings.TrimSpace(key))
			if err != nil {
				return nil, err
			}
			name, ok := k.(string)
			if !ok || name == "" {
				return nil, fmt.Errorf("map keys must be strings")
			}
			if _, exists := out[name]; exists {
				return nil, fmt.Errorf("duplicate map key %s", name)
			}
			out[name], err = parseValue(val)
			if err != nil {
				return nil, err
			}
		}
		return out, nil
	case '&', '*', '!', '|', '>':
		return nil, fmt.Errorf("unsupported YAML value %q", raw)
	}
	if raw == "true" || raw == "false" {
		return raw == "true", nil
	}
	// JSON-compatible numbers survive SQLite's JSON round-trip with the same type.
	if json.Valid([]byte(raw)) {
		var number float64
		if err := json.Unmarshal([]byte(raw), &number); err == nil {
			return number, nil
		}
	}
	return raw, nil
}

func splitFlow(s string) ([]string, error) {
	out := []string{}
	start, depth := 0, 0
	quote := byte(0)
	for i := 0; i < len(s); i++ {
		c := s[i]
		if quote != 0 {
			if c == '\\' && quote == '"' {
				i++
			} else if c == quote {
				if quote == '\'' && i+1 < len(s) && s[i+1] == '\'' {
					i++
				} else {
					quote = 0
				}
			}
			continue
		}
		switch c {
		case '\'', '"':
			if strings.TrimSpace(s[start:i]) == "" || (i > 0 && strings.ContainsRune(" [{:", rune(s[i-1]))) {
				quote = c
			}
		case '[', '{':
			depth++
		case ']', '}':
			depth--
			if depth < 0 {
				return nil, fmt.Errorf("unbalanced flow collection")
			}
		case ',':
			if depth == 0 {
				part := strings.TrimSpace(s[start:i])
				if part == "" {
					return nil, fmt.Errorf("empty flow collection item")
				}
				out = append(out, part)
				start = i + 1
			}
		}
	}
	if quote != 0 || depth != 0 {
		return nil, fmt.Errorf("unclosed quote or flow collection")
	}
	if part := strings.TrimSpace(s[start:]); part != "" {
		out = append(out, part)
	}
	return out, nil
}

// RenderMarkdown renders the selected revision, never mixing the current head's
// metadata into an older revision. JSON literals are also valid YAML scalars.
func RenderMarkdown(rec *store.Record, rev *store.Revision) string {
	if rev == nil {
		return ""
	}
	if len(rev.Frontmatter) == 0 {
		return rev.Body
	}
	var b strings.Builder
	b.WriteString("---\n")
	keys := make([]string, 0, len(rev.Frontmatter))
	for key := range rev.Frontmatter {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	for _, key := range keys {
		b.WriteString(key)
		b.WriteString(": ")
		b.WriteString(renderValue(rev.Frontmatter[key]))
		b.WriteByte('\n')
	}
	b.WriteString("---\n")
	b.WriteString(rev.Body)
	return b.String()
}

func renderValue(v any) string {
	encoded, err := json.Marshal(v)
	if err != nil {
		panic("records: frontmatter must contain JSON values: " + err.Error())
	}
	return string(encoded)
}
