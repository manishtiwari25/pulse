package board

import (
	"regexp"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

const amendmentBody = "## Current text\n\n```\nold line\n```\n\n## Proposed text\n\n```\nnew line\n```\n"

func seedPosts(t *testing.T, s *store.Store) (suggestion, amendment, finding *Post) {
	t.Helper()
	var err error
	suggestion, err = CreatePost(s, PostInput{Kind: "suggestion", Project: "demo", Area: "runner", Thread: "faster-tests", Summary: "Cache the build", Agent: "alice", Date: "2026-09-06T09:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	amendment, err = CreatePost(s, PostInput{Kind: "amendment", Project: "demo", Area: "evolution", Thread: "constitution", Summary: "Amend a line", Body: amendmentBody, Agent: "alice", Date: "2026-09-06T09:00:01Z"})
	if err != nil {
		t.Fatal(err)
	}
	finding, err = CreatePost(s, PostInput{Kind: "finding", Project: "demo", Area: "runner", Thread: "faster-tests", Summary: "Found it", Agent: "bob", Date: "2026-09-06T09:00:02Z"})
	if err != nil {
		t.Fatal(err)
	}
	return
}

func TestCreatePostRuleMatrix(t *testing.T) {
	noTTY(t)
	home, s, _ := fixture(t)
	sug, am, fin := seedPosts(t, s)
	if am.TextHash != BlockHash([]string{"new line"}) {
		t.Fatalf("amendment hash %q", am.TextHash)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	pyHome := cloneHome(t, home)
	s = openHome(t, home)

	long := strings.Repeat("word ", 60)
	cases := []struct {
		name string
		args []string // after "post"; --agent/--project/--session travel as flags for both tools
		want string   // the refusal (What); "" = success
		skip bool     // argparse refuses before create_post: compare the Go message only
	}{
		{"kind", []string{"rumour", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "alice"}, "Kind must be one of: " + strings.Join(Kinds, ", "), true},
		{"area", []string{"finding", "--area", "!!!", "--thread", "b", "--summary", "s", "--agent", "alice"}, "Both --area and --thread are needed (lowercase words with dashes)", false},
		{"thread", []string{"finding", "--area", "a", "--thread", "!!", "--summary", "s", "--agent", "alice"}, "Both --area and --thread are needed (lowercase words with dashes)", false},
		{"summary-empty", []string{"finding", "--area", "a", "--thread", "b", "--summary", "   ", "--agent", "alice"}, "A one-line --summary is required", false},
		{"summary-long", []string{"finding", "--area", "a", "--thread", "b", "--summary", long, "--agent", "alice"}, "summary is 299 chars; limit 240 - move detail to --body", false},
		{"project", []string{"finding", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "alice", "--project", "zzz"}, "No project 'zzz' (pulse projects lists them)", false},
		{"who", []string{"finding", "--area", "a", "--thread", "b", "--summary", "s"}, "Who are you? Pass --agent NAME or set PULSE_AGENT (join first: pulse agent join --name NAME)", false},
		{"unknown-agent", []string{"finding", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "Ghost/One"}, "No agent named 'Ghost/One'. Join first: pulse agent join --name ghost-one --runner R --model M", false},
		{"resolution", []string{"finding", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "alice", "--resolution", "maybe"}, "Resolution must be one of: " + strings.Join(Resolutions, ", "), true},
		{"vote-on", []string{"vote", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "bob", "--vote", "up"}, "A vote needs --on POST_ID (the suggestion or amendment it answers)", false},
		{"vote-dir", []string{"vote", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "bob", "--on", sug.ID}, "A vote needs --vote up or --vote down", false},
		{"only-vote", []string{"finding", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "bob", "--vote", "up"}, "Only a vote carries --vote", false},
		{"on-missing", []string{"answer", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "bob", "--on", "nope"}, "No post 'nope' to answer", false},
		{"self-vote", []string{"vote", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "alice", "--on", sug.ID, "--vote", "up"}, "No self-votes: alice wrote " + sug.ID, false},
		{"same-session", []string{"vote", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "bob", "--on", sug.ID, "--vote", "up", "--session", "sess-alice"}, "No self-votes: " + sug.ID + " was written in your session sess-alice", false},
		{"vote-finding", []string{"vote", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "alice", "--on", fin.ID, "--vote", "down"}, "Votes answer a suggestion or an amendment, not a finding", false},
		{"amend-hash", []string{"vote", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "bob", "--on", am.ID, "--vote", "up"}, "A vote on an amendment carries --text-hash (see `pulse hash " + am.ID + "`)", false},
		{"amend-read", []string{"vote", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "bob", "--on", am.ID, "--vote", "up", "--text-hash", am.TextHash, "--body", "no read line"}, "A vote on an amendment carries a `Read:` line restating the change in the body", false},
		{"amend-blocks", []string{"amendment", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "bob", "--body", "just text"}, "An amendment body needs `## Current text` and `## Proposed text`, each with a fenced block", false},
		{"expires", []string{"finding", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "bob", "--expires", "tomorrow"}, "Not a UTC time like 2026-09-05T08:00:00Z: 'tomorrow'", false},
		{"ok-finding", []string{"finding", "--area", "Runner Speed", "--thread", "b", "--summary", "  two   spaces  ", "--agent", "bob", "--body", "text"}, "", false},
		{"ok-vote", []string{"vote", "--area", "a", "--thread", "b", "--summary", "s", "--agent", "bob", "--on", am.ID, "--vote", "up", "--text-hash", am.TextHash, "--body", "Read: the new line"}, "", false},
		{"ok-claim", []string{"claim", "--area", "a", "--thread", "b", "--summary", "mine", "--agent", "bob", "--claim-hours", "2"}, "", false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			args := append([]string{"post"}, tc.args...)
			if !containsFlag(tc.args, "--project") {
				args = append(args, "--project", "demo")
			}
			out, err := runCmd(t, s, &cli.Context{}, args...)
			if tc.want == "" {
				if err != nil {
					t.Fatalf("go: %v", err)
				}
				id := strings.TrimSpace(out)
				if !strings.HasPrefix(id, stampOf(testNow)+"-bob") {
					t.Fatalf("go id %q", id)
				}
			} else if got := what(err); got != tc.want || code(err) != 2 {
				t.Fatalf("go: got %q (code %d)\nwant %q", got, code(err), tc.want)
			}
			if tc.skip {
				return
			}
			pyOut, pyErr, pyCode := oracle(t, pyHome, nil, args...)
			if tc.want == "" {
				if pyCode != 0 {
					t.Fatalf("oracle failed: %s", pyErr)
				}
				if !regexp.MustCompile(`^\d{8}T\d{6}Z-bob(-\d+)?$`).MatchString(strings.TrimSpace(pyOut)) {
					t.Fatalf("oracle id %q", pyOut)
				}
				return
			}
			if got := oracleMessage(pyErr); got != tc.want || pyCode != 2 {
				t.Fatalf("oracle: got %q (code %d)\nwant %q", got, pyCode, tc.want)
			}
		})
	}
	// the successful writes left the same shape in both stores
	pyStore, err := store.Open(store.Options{Home: pyHome, SkipRecords: true})
	if err != nil {
		t.Fatal(err)
	}
	defer pyStore.Close()
	for _, q := range []string{
		"SELECT kind, project, area, thread, agent, summary, body, vote, on_post, text_hash, has_read_line, resolution, session, name, origin FROM posts WHERE agent = 'bob' AND kind != 'finding' OR (agent = 'bob' AND body = 'text') ORDER BY kind, summary",
		"SELECT name, posts, last_useful_at != '' FROM agents WHERE name = 'bob'",
		"SELECT action, actor, detail FROM events WHERE action = 'post' AND actor = 'bob' ORDER BY seq",
	} {
		g, p := dumpQuery(t, s, q), dumpQuery(t, pyStore, q)
		if g != p {
			t.Errorf("rows differ for %s\n go: %s\n py: %s", q, g, p)
		}
	}
}

func containsFlag(args []string, name string) bool {
	for _, a := range args {
		if a == name {
			return true
		}
	}
	return false
}

func dumpQuery(t *testing.T, s *store.Store, q string) string {
	t.Helper()
	rows, err := s.DB.Query(q)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	cols, _ := rows.Columns()
	var b strings.Builder
	for rows.Next() {
		vals := make([]any, len(cols))
		ptrs := make([]any, len(cols))
		for i := range vals {
			ptrs[i] = &vals[i]
		}
		if err := rows.Scan(ptrs...); err != nil {
			t.Fatal(err)
		}
		for _, v := range vals {
			switch x := v.(type) {
			case []byte:
				b.WriteString(string(x))
			default:
				b.WriteString(strings.TrimSpace(strings.ReplaceAll(sqlLiteral(v), "'", "")))
			}
			b.WriteString("|")
		}
		b.WriteString("\n")
	}
	return b.String()
}

func TestPostIDCollisionSuffix(t *testing.T) {
	_, s, _ := fixture(t)
	var ids []string
	for i := 0; i < 3; i++ {
		p, err := CreatePost(s, PostInput{Kind: "finding", Project: "demo", Area: "a", Thread: "b", Summary: "again", Agent: "alice"})
		if err != nil {
			t.Fatal(err)
		}
		ids = append(ids, p.ID)
	}
	base := stampOf(testNow) + "-alice"
	want := []string{base, base + "-2", base + "-3"}
	for i := range want {
		if ids[i] != want[i] {
			t.Fatalf("ids %v, want %v", ids, want)
		}
	}
	a, _ := s.Agent("alice")
	if a.Posts != 3 || a.LastUsefulAt != testNow || a.LastActiveAt != testNow {
		t.Fatalf("agent counters %+v", a)
	}
	if n := countRows(t, s, "events", "action = 'post' AND actor = 'alice'"); n != 3 {
		t.Fatalf("%d post events", n)
	}
}

func TestClaimExpiry(t *testing.T) {
	noTTY(t)
	_, s, _ := fixture(t)
	out, err := runCmd(t, s, &cli.Context{Agent: "alice", Project: "demo"}, "claim", "--area", "a", "--thread", "b", "--summary", "mine")
	if err != nil {
		t.Fatal(err)
	}
	p, _ := PostByID(s, strings.TrimSpace(out))
	if p == nil || p.Kind != "claim" || p.Expires != "2026-09-06T14:00:00Z" {
		t.Fatalf("claim %+v", p)
	}
	out, err = runCmd(t, s, &cli.Context{Agent: "bob", Project: "demo"}, "claim", "--area", "a", "--thread", "b", "--summary", "mine", "--hours", "1.5")
	if err != nil {
		t.Fatal(err)
	}
	p, _ = PostByID(s, strings.TrimSpace(out))
	if p.Expires != "2026-09-06T11:30:00Z" {
		t.Fatalf("expires %q", p.Expires)
	}
	if err := s.SetSetting("human", "claim_hours", "6"); err != nil {
		t.Fatal(err)
	}
	out, err = runCmd(t, s, &cli.Context{Agent: "bob", Project: "demo"}, "post", "claim", "--area", "a", "--thread", "c", "--summary", "mine")
	if err != nil {
		t.Fatal(err)
	}
	p, _ = PostByID(s, strings.TrimSpace(out))
	if p.Expires != "2026-09-06T16:00:00Z" {
		t.Fatalf("expires %q", p.Expires)
	}
	// posts are immutable and the branch of the checkout is recorded
	if p.Branch != "main" {
		t.Fatalf("branch %q", p.Branch)
	}
	if _, err := s.DB.Exec("UPDATE posts SET summary = 'x' WHERE id = ?", p.ID); err == nil {
		t.Fatal("posts must be immutable")
	}
}

func TestVoteCommand(t *testing.T) {
	noTTY(t)
	_, s, _ := fixture(t)
	sug, am, _ := seedPosts(t, s)
	_, err := runCmd(t, s, &cli.Context{Agent: "bob"}, "vote", sug.ID, "sideways", "--summary", "r")
	if code(err) != 2 {
		t.Fatalf("direction: %v", err)
	}
	_, err = runCmd(t, s, &cli.Context{Agent: "bob"}, "vote", "nope", "up", "--summary", "r")
	if what(err) != "No post 'nope'" {
		t.Fatalf("missing: %v", err)
	}
	_, err = runCmd(t, s, &cli.Context{Agent: "alice"}, "vote", sug.ID, "up", "--summary", "r")
	if what(err) != "No self-votes: alice wrote "+sug.ID {
		t.Fatalf("self: %v", err)
	}
	out, err := runCmd(t, s, &cli.Context{Agent: "bob"}, "vote", sug.ID, "down", "--summary", "too slow")
	if err != nil {
		t.Fatal(err)
	}
	v, _ := PostByID(s, strings.TrimSpace(out))
	if v.Kind != "vote" || v.Vote != "down" || v.On != sug.ID || v.Project != "demo" || v.Area != "runner" || v.Thread != "faster-tests" || v.Session != "sess-bob" {
		t.Fatalf("vote %+v", v)
	}
	_, err = runCmd(t, s, &cli.Context{Agent: "bob"}, "vote", am.ID, "up", "--summary", "r", "--text-hash", am.TextHash)
	if !strings.Contains(what(err), "`Read:` line") {
		t.Fatalf("read line: %v", err)
	}
	out, err = runCmd(t, s, &cli.Context{Agent: "bob"}, "vote", am.ID, "up", "--summary", "r", "--text-hash", am.TextHash, "--body", "Read: new line")
	if err != nil {
		t.Fatal(err)
	}
	v, _ = PostByID(s, strings.TrimSpace(out))
	if !v.HasReadLine || v.TextHash != am.TextHash {
		t.Fatalf("amendment vote %+v", v)
	}
}

func TestHashParityWithOracle(t *testing.T) {
	home, s := snapshotHome(t)
	var id, body string
	err := s.DB.QueryRow("SELECT id, body FROM posts WHERE kind = 'amendment' AND text_hash != '' ORDER BY id DESC LIMIT 1").Scan(&id, &body)
	if err != nil {
		t.Skip("no amendment in the snapshot")
	}
	block := FencedBlock(body, "Proposed text")
	if block == nil {
		t.Fatal("no proposed text block")
	}
	out, errText, c := oracle(t, home, nil, "hash", id)
	if c != 0 {
		t.Fatal(errText)
	}
	if got := BlockHash(block); got != strings.TrimSpace(out) {
		t.Fatalf("hash %s, oracle %s", got, strings.TrimSpace(out))
	}
}

func TestHelpers(t *testing.T) {
	for in, want := range map[string]string{"Runner Speed": "runner-speed", "!!!": "x", "": "x", "A--B": "a-b",
		strings.Repeat("a", 50): strings.Repeat("a", 40), strings.Repeat("a", 39) + "-bcd": strings.Repeat("a", 39)} {
		if got := Slugify(in); got != want {
			t.Errorf("slugify(%q) = %q, want %q", in, got, want)
		}
	}
	for in, ok := range map[string]bool{"alice": true, "a": false, "1abc": false, "Alice": false, "a-1": true, strings.Repeat("a", 41): false} {
		if ValidName(in) != ok {
			t.Errorf("valid_name(%q) != %v", in, ok)
		}
	}
	if pyRepr("it's") != `"it's"` || pyRepr("plain") != "'plain'" || pyRepr(`a"b'c`) != `'a"b\'c'` {
		t.Errorf("pyRepr: %s %s %s", pyRepr("it's"), pyRepr("plain"), pyRepr(`a"b'c`))
	}
	if trunc("abcdef", 4) != "abc~" || trunc("abc", 4) != "abc" {
		t.Error("trunc")
	}
	if got, _ := PlusHours("2026-09-06T10:00:00Z", -0.5); got != "2026-09-06T09:30:00Z" {
		t.Errorf("plus_hours %s", got)
	}
	if FencedBlock("## Proposed text\n```\n\n  x  \n\n```\n", "Proposed text")[0] != "  x" {
		t.Error("fenced block normalization")
	}
	if FencedBlock("## Proposed text\n## Other\n```\nx\n```\n", "Proposed text") != nil {
		t.Error("heading before fence must end the search")
	}
	if !HasReadLine("a\nRead: yes\n") || HasReadLine("Read:\n") {
		t.Error("read line")
	}
}
