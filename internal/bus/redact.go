package bus

import (
	"bytes"
	"encoding/json"
	"sort"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// PrivateValue replaces an owner-private value in anything agents read.
const PrivateValue = "[owner-private]"

// PrivateSettings names the settings whose values are the owner's alone. The
// store keeps their `setting` events raw (the log is immutable and the owner
// reads it); every agent-facing projection - `pulse watch`, hook payloads,
// the hub's API and SSE - masks them. evolve.heldout_dir points at the
// held-out fixtures the evolution loop must never see.
var PrivateSettings = map[string]bool{
	"evolve.heldout_dir": true,
}

// Redact masks e for agents: a `setting` event on a private setting loses
// its value. It does not need the store; Redactor also scrubs the configured
// private paths wherever they appear.
func Redact(e Event) Event {
	if e.Action == "setting" && PrivateSettings[e.Subject] && e.Detail != "" {
		e.Detail = PrivateValue
	}
	return e
}

// Redactor returns Redact extended with the store's private values: every
// value a private setting ever held (the immutable `setting` events, so a
// path cleared from the configuration is still hidden in older events) is
// replaced wherever it appears in a subject or detail - raw, and in each JSON
// string encoding a writer could have used (Python json.dumps with
// ensure_ascii, Go encoding/json with and without HTML escaping, and one
// level of double encoding), so a masked detail stays valid JSON. s may be nil.
func Redactor(s *store.Store) func(Event) Event {
	needles := privateNeedles(s)
	return func(e Event) Event {
		e = Redact(e)
		for _, n := range needles {
			e.Subject = strings.ReplaceAll(e.Subject, n, PrivateValue)
			e.Detail = strings.ReplaceAll(e.Detail, n, PrivateValue)
		}
		return e
	}
}

// privateNeedles collects the current and historical values of every private
// setting and expands each into the strings to scrub, longest first.
func privateNeedles(s *store.Store) []string {
	if s == nil {
		return nil
	}
	seen := map[string]bool{}
	var values []string
	add := func(v string) {
		v = strings.TrimSpace(v)
		if len(v) >= 4 && !seen[v] {
			seen[v] = true
			values = append(values, v)
		}
	}
	names := make([]string, 0, len(PrivateSettings))
	for name := range PrivateSettings {
		names = append(names, name)
		add(s.Setting(name, ""))
	}
	sort.Strings(names)
	rows, err := s.DB.Query("SELECT detail FROM events WHERE action = 'setting' AND subject IN ("+
		strings.TrimSuffix(strings.Repeat("?,", len(names)), ",")+")", toAny(names)...)
	if err == nil {
		defer rows.Close()
		for rows.Next() {
			var v string
			if rows.Scan(&v) == nil {
				add(v)
			}
		}
	}
	var needles []string
	seen = map[string]bool{}
	for _, v := range values {
		for _, enc := range encodings(v) {
			if !seen[enc] {
				seen[enc] = true
				needles = append(needles, enc)
			}
		}
	}
	sort.Slice(needles, func(i, j int) bool { return len(needles[i]) > len(needles[j]) })
	return needles
}

// encodings is v as it can appear inside text: raw and as the body of a JSON
// string under the encoders PULSE writers use, each also encoded once more.
func encodings(v string) []string {
	first := []string{v, pyEncoded(v), goEncoded(v, true), goEncoded(v, false)}
	out := append([]string(nil), first...)
	for _, f := range first[1:] {
		out = append(out, pyEncoded(f), goEncoded(f, true), goEncoded(f, false))
	}
	return out
}

// pyEncoded is the inside of Python's json.dumps(v) (ensure_ascii=True).
func pyEncoded(v string) string {
	q, err := store.PyJSON(v)
	if err != nil || len(q) < 2 {
		return v
	}
	return q[1 : len(q)-1]
}

// goEncoded is the inside of Go's json string for v, with or without HTML escaping.
func goEncoded(v string, html bool) string {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(html)
	if enc.Encode(v) != nil {
		return v
	}
	q := strings.TrimRight(buf.String(), "\n")
	if len(q) < 2 {
		return v
	}
	return q[1 : len(q)-1]
}

func toAny(ss []string) []any {
	out := make([]any, len(ss))
	for i, s := range ss {
		out[i] = s
	}
	return out
}
