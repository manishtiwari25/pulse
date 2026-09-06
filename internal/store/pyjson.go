package store

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"strings"
	"unicode/utf16"
)

// PyJSON encodes v the way legacy/pulse.py does with
// json.dumps(v, sort_keys=True): ", " and ": " separators, keys sorted,
// non-ASCII escaped as \uXXXX. Go's encoding/json already sorts map keys;
// struct fields keep their declared order, so order them accordingly.
func PyJSON(v any) (string, error) {
	var compact bytes.Buffer
	enc := json.NewEncoder(&compact)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return "", err
	}
	dec := json.NewDecoder(bytes.NewReader(compact.Bytes()))
	dec.UseNumber()
	var out strings.Builder
	// stack of container states: 'o' object expecting key, 'v' object expecting value, 'a' array
	type frame struct {
		kind  byte
		count int
	}
	var stack []frame
	before := func() {
		if len(stack) == 0 {
			return
		}
		f := &stack[len(stack)-1]
		switch f.kind {
		case 'a':
			if f.count > 0 {
				out.WriteString(", ")
			}
			f.count++
		case 'k':
			if f.count > 0 {
				out.WriteString(", ")
			}
			f.count++
			f.kind = 'v'
		case 'v':
			out.WriteString(": ")
			f.kind = 'k'
		}
	}
	for {
		tok, err := dec.Token()
		if err == io.EOF {
			break
		}
		if err != nil {
			return "", err
		}
		switch t := tok.(type) {
		case json.Delim:
			switch t {
			case '{':
				before()
				out.WriteByte('{')
				stack = append(stack, frame{kind: 'k'})
			case '[':
				before()
				out.WriteByte('[')
				stack = append(stack, frame{kind: 'a'})
			case '}', ']':
				out.WriteByte(byte(t))
				stack = stack[:len(stack)-1]
			}
		case string:
			before()
			writePyString(&out, t)
		case json.Number:
			before()
			out.WriteString(t.String())
		case bool:
			before()
			if t {
				out.WriteString("true")
			} else {
				out.WriteString("false")
			}
		case nil:
			before()
			out.WriteString("null")
		default:
			return "", fmt.Errorf("unexpected JSON token %T", tok)
		}
	}
	return out.String(), nil
}

func writePyString(w *strings.Builder, s string) {
	w.WriteByte('"')
	for _, r := range s {
		switch r {
		case '"':
			w.WriteString(`\"`)
		case '\\':
			w.WriteString(`\\`)
		case '\n':
			w.WriteString(`\n`)
		case '\r':
			w.WriteString(`\r`)
		case '\t':
			w.WriteString(`\t`)
		case '\b':
			w.WriteString(`\b`)
		case '\f':
			w.WriteString(`\f`)
		default:
			switch {
			case r < 0x20 || (r >= 0x80 && r <= 0xffff):
				fmt.Fprintf(w, `\u%04x`, r)
			case r > 0xffff:
				hi, lo := utf16.EncodeRune(r)
				fmt.Fprintf(w, `\u%04x\u%04x`, hi, lo)
			default:
				w.WriteRune(r)
			}
		}
	}
	w.WriteByte('"')
}
