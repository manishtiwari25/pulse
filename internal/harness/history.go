package harness

import (
	"fmt"
	"strings"

	"github.com/manishtiwari25/pulse/internal/provider"
)

const (
	pruneMarker      = "characters elided - rerun the command to see them"
	outputElidedLine = "[output elided - rerun the command to see it]"
	outputOpenTag    = "<output>"
	outputCloseTag   = "</output>"
	returnCodePrefix = "<returncode>"
)

func pruneHistory(msgs []provider.Message, keep int, level int) (out []provider.Message, changed int) {
	out = cloneMessages(msgs)
	if keep < 1 {
		keep = 1
	}
	if level < 1 {
		return out, 0
	}
	var observations []int
	for i, msg := range out {
		if i < 2 {
			continue
		}
		if isObservationMessage(i, msg) {
			observations = append(observations, i)
		}
	}
	collapseThrough := len(observations) - keep
	if collapseThrough <= 0 {
		return out, 0
	}
	for _, idx := range observations[:collapseThrough] {
		old := out[idx].Content
		next := collapseObservation(old, level)
		if next != old {
			out[idx].Content = next
			changed++
		}
	}
	return out, changed
}

func isObservationMessage(index int, msg provider.Message) bool {
	if msg.Role == provider.RoleTool {
		return true
	}
	return msg.Role == provider.RoleUser && index >= 2 && strings.HasPrefix(msg.Content, returnCodePrefix)
}

func collapseObservation(content string, level int) string {
	if level == 1 && strings.Contains(content, pruneMarker) {
		return content
	}
	header, output, ok := splitObservation(content)
	if !ok {
		if level >= 2 {
			return headerLine(content) + "\n" + outputElidedLine
		}
		return content
	}
	if level >= 2 {
		next := header + "\n" + outputOpenTag + "\n" + outputElidedLine + "\n" + outputCloseTag
		if content == next {
			return content
		}
		return next
	}
	lines := strings.Split(output, "\n")
	if len(lines) <= 4 {
		return content
	}
	first := takeFirst(lines, 2)
	last := takeLast(lines, 2)
	kept := strings.Join(append(append([]string{}, first...), last...), "\n")
	elided := len(output) - len(kept)
	if elided <= 0 {
		return content
	}
	body := append([]string{}, first...)
	body = append(body, fmt.Sprintf("[%d characters elided - rerun the command to see them]", elided))
	body = append(body, last...)
	return header + "\n" + outputOpenTag + "\n" + strings.Join(body, "\n") + "\n" + outputCloseTag
}

func splitObservation(content string) (header, output string, ok bool) {
	lines := strings.Split(content, "\n")
	if len(lines) == 0 || !strings.HasPrefix(lines[0], returnCodePrefix) {
		return headerLine(content), "", false
	}
	open, close := -1, -1
	for i := 1; i < len(lines); i++ {
		if lines[i] == outputOpenTag && open == -1 {
			open = i
			continue
		}
		if lines[i] == outputCloseTag {
			close = i
			break
		}
	}
	if open == -1 || close == -1 || close < open {
		return lines[0], "", false
	}
	return lines[0], strings.Join(lines[open+1:close], "\n"), true
}

func headerLine(content string) string {
	line, _, _ := strings.Cut(content, "\n")
	return line
}

func takeFirst(lines []string, n int) []string {
	if len(lines) <= n {
		return append([]string{}, lines...)
	}
	return append([]string{}, lines[:n]...)
}

func takeLast(lines []string, n int) []string {
	if len(lines) <= n {
		return append([]string{}, lines...)
	}
	return append([]string{}, lines[len(lines)-n:]...)
}
