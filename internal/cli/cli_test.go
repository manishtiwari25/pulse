package cli

import (
	"bytes"
	"flag"
	"strings"
	"testing"
)

func noop(*Context, *flag.FlagSet, []string) error { return nil }

func reset() {
	commands = map[string]Command{}
	aliases = map[string]string{}
}

func TestRegisterLookupAll(t *testing.T) {
	reset()
	t.Cleanup(reset)
	Register(Command{Name: "post", Group: "agents", Summary: "write", Run: noop})
	Register(Command{Name: "tally", Group: "inspect", Summary: "tally", Aliases: []string{"records"}, Run: noop})
	Register(Command{Name: "board", Group: "agents", Summary: "digest", Run: noop})
	if _, ok := Lookup("records"); !ok {
		t.Fatal("alias lookup failed")
	}
	if _, ok := Lookup("nope"); ok {
		t.Fatal("unknown found")
	}
	all := All()
	got := []string{}
	for _, c := range all {
		got = append(got, c.Name)
	}
	if strings.Join(got, ",") != "board,post,tally" {
		t.Fatalf("All() = %v", got)
	}
	for _, dup := range []Command{
		{Name: "post", Group: "agents", Run: noop},
		{Name: "records", Group: "agents", Run: noop},
		{Name: "x", Group: "agents", Aliases: []string{"board"}, Run: noop},
		{Name: "y", Group: "nogroup", Run: noop},
		{Name: "z", Group: "agents"},
	} {
		func() {
			defer func() {
				if recover() == nil {
					t.Errorf("Register(%s) did not panic", dup.Name)
				}
			}()
			Register(dup)
		}()
	}
	if s := Suggest("recrds", 3); s[0] != "records" {
		t.Fatalf("Suggest = %v", s)
	}
}

func TestTable(t *testing.T) {
	var b bytes.Buffer
	Table(&b, false, []string{"NAME", "SUMMARY"}, [][]string{{"a", "one"}, {"longer", "two"}})
	want := "NAME    SUMMARY\na       one\nlonger  two\n"
	if b.String() != want {
		t.Fatalf("plain table:\n%q\nwant\n%q", b.String(), want)
	}
	b.Reset()
	Table(&b, true, []string{"N", "S"}, [][]string{{"a", "b"}})
	if !strings.HasPrefix(b.String(), "\x1b[1m\x1b[4mN  S\x1b[0m\x1b[0m\n") || !strings.HasSuffix(b.String(), "a  b\n") {
		t.Fatalf("colour table: %q", b.String())
	}
	b.Reset()
	Table(&b, false, nil, nil)
	if b.Len() != 0 {
		t.Fatalf("empty table wrote %q", b.String())
	}
}

func TestErrorAndStyle(t *testing.T) {
	e := Errorf(2, "No store at /x", "this machine has no PULSE store yet", "pulse init <project-path>")
	want := "No store at /x\n  why: this machine has no PULSE store yet\n  fix: pulse init <project-path>"
	if e.Error() != want || e.ExitCode() != 2 {
		t.Fatalf("Error() = %q code %d", e.Error(), e.ExitCode())
	}
	if (&Error{What: "x"}).ExitCode() != 1 || (&Error{What: "x"}).Error() != "x" {
		t.Fatal("defaults wrong")
	}
	off, on := Style{}, Style{On: true}
	if off.Red("x") != "x" || on.Green("x") != "\x1b[32mx\x1b[0m" {
		t.Fatal("style wrong")
	}
	var b bytes.Buffer
	if err := WriteJSON(&b, map[string]any{"b": 1, "a": "<x>"}); err != nil || b.String() != "{\n  \"a\": \"<x>\",\n  \"b\": 1\n}\n" {
		t.Fatalf("WriteJSON = %q %v", b.String(), err)
	}
	t.Setenv("NO_COLOR", "1")
	if ColorEnabled(&b, false) {
		t.Fatal("colour on a buffer")
	}
}

func TestHelp(t *testing.T) {
	reset()
	t.Cleanup(reset)
	Register(Command{Name: "post", Group: "agents", Summary: "write a post", Usage: "pulse post <kind> --summary S", Run: noop,
		Flags: func(fs *flag.FlagSet) { fs.String("summary", "", "one line") }})
	Register(Command{Name: "work", Group: "runner", Summary: "lease one job", Run: noop, Raw: true})
	var b bytes.Buffer
	Help(&b, false)
	out := b.String()
	if !strings.HasPrefix(out, Title+"\n") {
		t.Fatalf("help title: %q", out)
	}
	ia, il := strings.Index(out, GroupTitles["agents"]), strings.Index(out, GroupTitles["runner"])
	if ia < 0 || il < 0 || ia > il {
		t.Fatalf("group order wrong:\n%s", out)
	}
	if strings.Contains(out, GroupTitles["hub"]) {
		t.Fatal("empty group printed")
	}
	b.Reset()
	cmd, _ := Lookup("post")
	HelpCommand(&b, false, cmd)
	if !strings.Contains(b.String(), "Usage: pulse post <kind> --summary S") || !strings.Contains(b.String(), "-summary string") {
		t.Fatalf("command help:\n%s", b.String())
	}
}
