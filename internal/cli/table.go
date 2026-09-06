package cli

import (
	"bytes"
	"io"
	"strings"
	"text/tabwriter"
)

// Style colours text when On; otherwise it returns the text unchanged.
type Style struct{ On bool }

func (s Style) wrap(code, text string) string {
	if !s.On || text == "" {
		return text
	}
	return "\x1b[" + code + "m" + text + "\x1b[0m"
}

// Bold, Dim, Underline, Green, Yellow, Red, Cyan colour a string.
func (s Style) Bold(t string) string      { return s.wrap("1", t) }
func (s Style) Dim(t string) string       { return s.wrap("2", t) }
func (s Style) Underline(t string) string { return s.wrap("4", t) }
func (s Style) Green(t string) string     { return s.wrap("32", t) }
func (s Style) Yellow(t string) string    { return s.wrap("33", t) }
func (s Style) Red(t string) string       { return s.wrap("31", t) }
func (s Style) Cyan(t string) string      { return s.wrap("36", t) }

// Table writes an aligned table (two-space column padding, no trailing
// spaces). The header is bold and underlined when color is on; a nil header
// prints rows only.
func Table(w io.Writer, color bool, header []string, rows [][]string) {
	var buf bytes.Buffer
	tw := tabwriter.NewWriter(&buf, 0, 8, 2, ' ', 0)
	if header != nil {
		tw.Write([]byte(strings.Join(header, "\t") + "\n"))
	}
	for _, row := range rows {
		cells := make([]string, len(row))
		for i, c := range row {
			cells[i] = strings.ReplaceAll(strings.ReplaceAll(c, "\t", " "), "\n", " ")
		}
		tw.Write([]byte(strings.Join(cells, "\t") + "\n"))
	}
	tw.Flush()
	lines := strings.Split(strings.TrimRight(buf.String(), "\n"), "\n")
	if buf.Len() == 0 {
		return
	}
	style := Style{On: color}
	for i, line := range lines {
		line = strings.TrimRight(line, " ")
		if i == 0 && header != nil {
			line = style.Bold(style.Underline(line))
		}
		io.WriteString(w, line+"\n")
	}
}
