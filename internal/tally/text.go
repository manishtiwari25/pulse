package tally

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// The text helpers mirror legacy/pulse.py one for one (fenced_block, block_hash,
// table_cell, table_num, table_row, trunc, plural, day_number) so the tally
// produces the same bytes as the Python oracle.

// dayNumber is Python's date.fromisoformat(value[:10]).toordinal(): the
// proleptic Gregorian ordinal of the date, 0 when the text is not a date.
func dayNumber(value string) int {
	if len(value) > 10 {
		value = value[:10]
	}
	t, err := time.Parse("2006-01-02", value)
	if err != nil {
		return 0
	}
	// Duration overflows over ~292 years, so count from the Unix epoch:
	// 1970-01-01 is proleptic Gregorian ordinal 719163.
	return int(t.Unix()/86400) + 719163
}

// trunc keeps n-1 characters and a "~" when text is longer than n (code points,
// like Python).
func trunc(text string, n int) string {
	r := []rune(text)
	if len(r) > n {
		return string(r[:n-1]) + "~"
	}
	return text
}

func plural(n int, word string) string {
	if n == 1 {
		return word
	}
	return word + "s"
}

func sha256Text(text string) string {
	sum := sha256.Sum256([]byte(text))
	return hex.EncodeToString(sum[:])
}

// fencedBlock returns the lines of the fenced block under `## <heading>`,
// normalized as board.sh did: trailing whitespace stripped from every line,
// leading and trailing blank lines dropped. nil when the heading, the fence or
// the text is missing.
func fencedBlock(body, heading string) []string {
	state := 0
	fence := ""
	var lines []string
	for _, raw := range strings.Split(body, "\n") {
		switch state {
		case 0:
			if strings.HasPrefix(raw, "## "+heading) && strings.TrimRight(raw[len("## "+heading):], " \t") == "" {
				state = 1
			}
		case 1:
			if strings.HasPrefix(raw, "## ") {
				state = -1
				break
			}
			if strings.HasPrefix(raw, "`") {
				n := 0
				for n < len(raw) && raw[n] == '`' {
					n++
				}
				fence, state = raw[:n], 2
			}
		case 2:
			if strings.HasPrefix(raw, fence) && strings.TrimRight(raw[len(fence):], " \t") == "" {
				state = 3
				break
			}
			lines = append(lines, strings.TrimRight(raw, " \t"))
		}
		if state == -1 || state == 3 {
			break
		}
	}
	if state < 2 {
		return nil
	}
	for len(lines) > 0 && lines[0] == "" {
		lines = lines[1:]
	}
	for len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	if len(lines) == 0 {
		return nil
	}
	return lines
}

func blockHash(lines []string) string {
	return sha256Text(strings.Join(lines, "\n") + "\n")
}

// TextHash is the text_hash of an amendment's proposed text: the fenced block
// under "## Proposed text" when text is an amendment body, else the whole text
// normalized the same way (trailing whitespace off every line, blank edges
// dropped), hashed with one trailing newline.
func TextHash(text string) string {
	if block := fencedBlock(text, "Proposed text"); block != nil {
		return blockHash(block)
	}
	var lines []string
	for _, l := range strings.Split(text, "\n") {
		lines = append(lines, strings.TrimRight(l, " \t"))
	}
	for len(lines) > 0 && lines[0] == "" {
		lines = lines[1:]
	}
	for len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	return blockHash(lines)
}

// tableCell is the value cell of `| <name> | <value> |` anywhere in a Markdown
// text, backticks removed and stripped; ok is false when the row is absent.
func tableCell(text, name string) (string, bool) {
	re := regexp.MustCompile(`\|[ \t]*` + regexp.QuoteMeta(name) + `[ \t]*\|([^|]*)`)
	m := re.FindStringSubmatch(text)
	if m == nil {
		return "", false
	}
	return strings.TrimSpace(strings.ReplaceAll(m[1], "`", "")), true
}

var (
	fractionRE = regexp.MustCompile(`([0-9]+)/([0-9]+)`)
	integerRE  = regexp.MustCompile(`[0-9]+`)
)

// tableNum is tableCell parsed as a fraction (n/m) or an integer.
func tableNum(text, name string) (float64, bool) {
	cell, ok := tableCell(text, name)
	if !ok {
		return 0, false
	}
	if m := fractionRE.FindStringSubmatch(cell); m != nil {
		n, _ := strconv.Atoi(m[1])
		d, _ := strconv.Atoi(m[2])
		return float64(n) / float64(d), true
	}
	if m := integerRE.FindString(cell); m != "" {
		n, _ := strconv.Atoi(m)
		return float64(n), true
	}
	return 0, false
}

// tableRow is board.sh `row`: the third cell of the table line whose second
// cell is exactly name.
func tableRow(text, name string) (string, bool) {
	for _, line := range strings.Split(text, "\n") {
		parts := strings.Split(line, "|")
		if len(parts) >= 3 && strings.TrimSpace(parts[1]) == name {
			return strings.TrimSpace(parts[2]), true
		}
	}
	return "", false
}

// AmendmentCheck is what check_amendment computes for one amendment body.
type AmendmentCheck struct {
	Hash    string `json:"hash"`    // text_hash of the proposed text ("" when malformed)
	Anchor  string `json:"anchor"`  // how many times the current text occurs in the constitution ("" until both blocks parse)
	Section string `json:"section"` // "|"-joined sections the replaced lines fall in
	Verdict string `json:"verdict"` // "ok", "malformed: ...", "unchecked: ...", "lowers ...", 'protected section "..."', ...
}

// Check is legacy/pulse.py check_amendment: the hash, anchor count, sections
// and verdict of an amendment body against the constitution text and the
// comma-separated protected sections.
func Check(body, constitution, protected string) AmendmentCheck {
	prop := fencedBlock(body, "Proposed text")
	if prop == nil {
		return AmendmentCheck{Verdict: `malformed: no fenced block under "## Proposed text"`}
	}
	hsh := blockHash(prop)
	cur := fencedBlock(body, "Current text")
	if cur == nil {
		return AmendmentCheck{Hash: hsh, Verdict: `malformed: no fenced block under "## Current text"`}
	}
	if constitution == "" {
		return AmendmentCheck{Hash: hsh, Verdict: "unchecked: no constitution record"}
	}
	curText := strings.Join(cur, "\n") + "\n"
	propText := strings.Join(prop, "\n") + "\n"
	verdict := "ok"
	cq, cqOK := tableNum(curText, "Amendment quorum")
	pq, pqOK := tableNum(propText, "Amendment quorum")
	if cqOK && (!pqOK || pq < cq) {
		verdict = "lowers or removes the amendment quorum"
	}
	cm, cmOK := tableNum(curText, "Amendment supermajority")
	pm, pmOK := tableNum(propText, "Amendment supermajority")
	if verdict == "ok" && cmOK && (!pmOK || pm < cm) {
		verdict = "lowers or removes the amendment supermajority"
	}
	for _, guard := range []string{"Amendment voter", "Informed vote", "Not amendable by vote"} {
		if verdict != "ok" {
			break
		}
		_, inCur := tableCell(curText, guard)
		_, inProp := tableCell(propText, guard)
		if inCur && !inProp {
			verdict = fmt.Sprintf(`removes the "%s" row`, guard)
		}
	}
	cp, cpOK := tableCell(curText, "Not amendable by vote")
	pp, ppOK := tableCell(propText, "Not amendable by vote")
	if verdict == "ok" && cpOK && ppOK {
		for _, item := range strings.Split(cp, ",") {
			it := strings.TrimSpace(item)
			if it != "" && !strings.Contains(","+pp+",", it) {
				verdict = fmt.Sprintf(`drops "%s" from the protected sections`, it)
			}
		}
	}
	lines := strings.Split(constitution, "\n")
	for i := range lines {
		lines[i] = strings.TrimRight(lines[i], " \t")
	}
	if len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	hay := "\n" + strings.Join(lines, "\n") + "\n"
	needle := "\n" + curText
	count, pos, first := 0, 0, -1
	for {
		k := strings.Index(hay[pos:], needle)
		if k < 0 {
			break
		}
		k += pos
		count++
		if first < 0 {
			first = k
		}
		pos = k + 1
	}
	var sections []string
	if count == 1 {
		ln := strings.Count(hay[:first+1], "\n")
		span := strings.Count(curText, "\n")
		n := len(lines)
		fmEnd := 0
		if n > 0 && isRule(lines[0]) {
			for i := 1; i < n; i++ {
				if isRule(lines[i]) {
					fmEnd = i + 1
					break
				}
			}
		}
		secOf := make([]string, 0, n)
		sec := "preamble"
		for i, line := range lines {
			if strings.HasPrefix(line, "## ") {
				sec = strings.TrimRight(line[3:], " \t")
			}
			if fmEnd > 0 && i+1 <= fmEnd {
				secOf = append(secOf, "frontmatter")
			} else {
				secOf = append(secOf, sec)
			}
		}
		var items []string
		for _, it := range strings.Split(strings.ReplaceAll(protected, "`", ""), ",") {
			items = append(items, strings.TrimSpace(it))
		}
		end := ln + span
		if end > n+1 {
			end = n + 1
		}
		for j := ln; j < end; j++ {
			if j < 1 || j > n {
				continue
			}
			s := secOf[j-1]
			if !contains(sections, s) {
				sections = append(sections, s)
			}
			for _, it := range items {
				if verdict == "ok" && it != "" && strings.EqualFold(it, s) {
					verdict = fmt.Sprintf(`protected section "%s"`, s)
				}
			}
		}
	}
	return AmendmentCheck{Hash: hsh, Anchor: strconv.Itoa(count), Section: strings.Join(sections, "|"), Verdict: verdict}
}

// isRule matches Python's ^---[ \t]*$ (a frontmatter delimiter).
func isRule(line string) bool {
	return strings.HasPrefix(line, "---") && strings.TrimRight(line[3:], " \t") == ""
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

// CheckAmendment runs Check and returns nil when the amendment is well formed,
// checked and touches no protected section; otherwise an error carrying the
// Python verdict text ("malformed: ...", "unchecked: ...", 'protected section "..."', ...).
func CheckAmendment(body, constitution string, protected []string) error {
	c := Check(body, constitution, strings.Join(protected, ", "))
	if c.Verdict == "ok" {
		return nil
	}
	return &VerdictError{Check: c}
}

// VerdictError is CheckAmendment's failure; Error() is the Python verdict.
type VerdictError struct{ Check AmendmentCheck }

func (e *VerdictError) Error() string { return e.Check.Verdict }
