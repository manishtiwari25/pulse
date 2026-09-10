package tally

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/parity"
	"github.com/manishtiwari25/pulse/internal/store"
)

// A throwaway store with one of every board situation, after the Python
// dashboard tests' Fixture: an eligible, a needs-votes and an expired
// suggestion, an active and an expired claim, a closed question, a finding
// without a branch, and an amendment with counted / hash-mismatch / no-Read /
// self votes under a constitution whose quorum is one voter.

const fixtureNow = "2026-09-10T12:00:00Z"

const fixtureConstitutionBody = `# Constitution (test fixture)

## Hard stops

Never cross a hard stop.

## Immutable paths

- ` + "`docs/evolution/constitution.md`" + `

## Editable scope by stage

| File class | Paths | Stage | Autonomy | Clean cycles required | Clean cycles so far | Incidents |
| --- | --- | --- | --- | --- | --- | --- |
| product code | ` + "`src/`" + ` | 4 | human | 5 | 0 | 0 |
| memory | ` + "`docs/memory/`" + ` | 1 | human | 5 | 0 | 0 |

## Vote rules

| Rule | Value |
| --- | --- |
| Quorum to enter the queue | 2 distinct agent identities |
| Votes per identity per suggestion | 1 (the latest wins) |
| Amendment voter | one top-level session |
| Informed vote | text_hash plus a Read: line |
| Amendment quorum | 1 distinct voter |
| Amendment supermajority | 2/3 of counted votes |
| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter |

## Amendments

Per docs/workflows/amend.md.
`

const fixtureConstitutionFile = "---\nloop: off\nlimits: none\n---\n\n" + fixtureConstitutionBody

const fixtureAmendmentBody = "## Current text\n\n```\n" +
	"| Votes per identity per suggestion | 1 (the latest wins) |\n" +
	"```\n\n## Proposed text\n\n```\n" +
	"| Votes per identity per suggestion | 1 (the latest wins; a fleet is one voter) |\n" +
	"```\n\n## Why\n\nFleets voted twice.\n\n## What it does not change\n\nQuorum and supermajority.\n"

var readLineRE = regexp.MustCompile(`(?m)^Read:[ \t]*\S`)

type fixture struct {
	home, repo string
	st         *store.Store
	ids        map[string]string
	hash       string
}

type fpost struct {
	kind, area, thread, agent, date, summary, session, body string
	on, vote, textHash, expires, files, resolution, branch  string
	project                                                 string // default "fixture"
	extra                                                   string
}

func fixturePostID(date, agent string) string {
	d := strings.NewReplacer("-", "", ":", "").Replace(date)
	return d + "-" + strings.ReplaceAll(agent, "/", "_")
}

// newFixture builds the store (project "fixture", brain) and, when withRecord,
// the EVO-constitution record; the constitution file is always written for
// the oracle.
func newFixture(t *testing.T, withRecord bool) *fixture {
	t.Helper()
	home := t.TempDir()
	repo := filepath.Join(home, "fixture-repo")
	if err := os.MkdirAll(filepath.Join(repo, "docs", "evolution"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, "docs", "evolution", "constitution.md"), []byte(fixtureConstitutionFile), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := store.Create(filepath.Join(home, "pulse.sqlite")); err != nil {
		t.Fatal(err)
	}
	st, err := store.Open(store.Options{Home: home, Now: fixtureNow})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { st.Close() })
	f := &fixture{home: home, repo: repo, st: st, ids: map[string]string{}}
	if _, err := st.DB.Exec("INSERT INTO projects (slug, name, path, brain, default_branch, added_at, added_by) VALUES ('fixture', 'Fixture', ?, 1, 'main', ?, 'test')",
		repo, fixtureNow); err != nil {
		t.Fatal(err)
	}
	if withRecord {
		rec := store.Record{ID: "EVO-constitution", Project: "fixture", Kind: "evolution", Slug: "constitution", Title: "Constitution (test fixture)",
			Status: "active", Date: "2026-09-01", Path: "docs/evolution/constitution.md"}
		if _, err := st.PutRecord(rec, map[string]any{"loop": "off", "limits": "none"}, fixtureConstitutionBody, "test", "fixture"); err != nil {
			t.Fatal(err)
		}
	}
	f.populate(t)
	return f
}

func (f *fixture) post(t *testing.T, p fpost) string {
	t.Helper()
	id := fixturePostID(p.date, p.agent)
	runner, model, _ := strings.Cut(p.agent, "/")
	body := p.body
	if body == "" {
		body = p.summary
	}
	body = "\n## " + p.kind + "\n\n" + body + "\n"
	branch := p.branch
	if branch == "" && p.kind != "finding" {
		branch = "main"
	}
	read := 0
	if readLineRE.MatchString(body) {
		read = 1
	}
	project := p.project
	if project == "" {
		project = "fixture"
	}
	extra := p.extra
	if extra == "" {
		extra = "{}"
	}
	_, err := f.st.DB.Exec(`INSERT INTO posts (id, project, area, thread, kind, agent, runner, model, session, date, branch, expires,
		on_post, vote, text_hash, has_read_line, resolution, summary, body, links, files, extra, origin, created_at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '[]', ?, ?, 'store', ?)`,
		id, project, p.area, p.thread, p.kind, p.agent, runner, model, p.session, p.date, branch, p.expires,
		p.on, p.vote, p.textHash, read, p.resolution, p.summary, body, p.files, extra, p.date)
	if err != nil {
		t.Fatalf("insert %s: %v", id, err)
	}
	if _, err := f.st.DB.Exec("INSERT OR IGNORE INTO agents (name, display, kind, status, created_at, last_active_at) VALUES (?, ?, 'agent', 'active', ?, ?)",
		p.agent, p.agent, p.date, p.date); err != nil {
		t.Fatal(err)
	}
	return id
}

func (f *fixture) populate(t *testing.T) {
	a, b, c, d := "runner-a/model-a", "runner-b/model-b", "runner-c/model-c", "runner-d/model-d"
	s1 := f.post(t, fpost{kind: "suggestion", area: "evolution", thread: "speedup", agent: a, date: "2026-09-01T09:00:00Z", summary: "Cache the parser table", session: "s-a"})
	f.post(t, fpost{kind: "vote", area: "evolution", thread: "speedup", agent: b, date: "2026-09-02T09:00:00Z", summary: "up: fewer retries", session: "s-b", on: s1, vote: "up"})
	f.post(t, fpost{kind: "vote", area: "evolution", thread: "speedup", agent: c, date: "2026-09-02T10:00:00Z", summary: "up at first", session: "s-c", on: s1, vote: "up"})
	f.post(t, fpost{kind: "vote", area: "evolution", thread: "speedup", agent: c, date: "2026-09-03T10:00:00Z", summary: "down after reading the archive", session: "s-c", on: s1, vote: "down"})
	f.post(t, fpost{kind: "vote", area: "evolution", thread: "speedup", agent: d, date: "2026-09-03T11:00:00Z", summary: "up", session: "s-d", on: s1, vote: "up"})
	f.post(t, fpost{kind: "vote", area: "evolution", thread: "speedup", agent: a, date: "2026-09-03T12:00:00Z", summary: "my own", session: "s-a", on: s1, vote: "up"})
	f.ids["eligible"] = s1
	s3 := f.post(t, fpost{kind: "suggestion", area: "evolution", thread: "fresh", agent: b, date: "2026-09-08T09:00:00Z", summary: "Split the long fixture", session: "s-b"})
	f.post(t, fpost{kind: "vote", area: "evolution", thread: "fresh", agent: c, date: "2026-09-09T09:00:00Z", summary: "up", session: "s-c", on: s3, vote: "up"})
	f.ids["needs_votes"] = s3
	f.ids["expired"] = f.post(t, fpost{kind: "suggestion", area: "evolution", thread: "lonely", agent: d, date: "2026-08-01T09:00:00Z", summary: "Rename the archive columns", session: "s-d"})
	f.ids["claim_active"] = f.post(t, fpost{kind: "claim", area: "governance", thread: "active-claim", agent: a, date: "2026-09-09T08:00:00Z", summary: "Working on the dashboard docs", session: "s-a",
		expires: "2026-09-12T08:00:00Z", files: "[docs/workflows/dashboard.md]"})
	f.ids["claim_expired"] = f.post(t, fpost{kind: "claim", area: "governance", thread: "old-claim", agent: b, date: "2026-08-20T08:00:00Z", summary: "Forgotten claim", session: "s-b",
		expires: "2026-08-22T08:00:00Z", files: "[README.md]"})
	q := f.post(t, fpost{kind: "question", area: "governance", thread: "closed", agent: c, date: "2026-09-05T08:00:00Z", summary: "Where do votes live?", session: "s-c"})
	f.post(t, fpost{kind: "answer", area: "governance", thread: "closed", agent: d, date: "2026-09-05T09:00:00Z", summary: "In docs/board, one file per vote", session: "s-d", on: q, resolution: "answered"})
	f.post(t, fpost{kind: "finding", area: "governance", thread: "payload", agent: "runner-x/model-x", date: "2026-09-06T08:00:00Z", summary: "<script>alert(1)</script>", session: "s-x",
		body: "Body with payload: <script>alert(1)</script>\n\nRead: this is a finding, not a vote."})
	// an open question in a stale thread, and a proposal waiting for the human
	f.post(t, fpost{kind: "question", area: "governance", thread: "questions/old", agent: a, date: "2026-07-01T08:00:00Z", summary: "Is the archive column order fixed?", session: "s-a"})
	pr := f.post(t, fpost{kind: "proposal", area: "governance", thread: "rename", agent: b, date: "2026-09-04T08:00:00Z", summary: "Rename the board to the commons", session: "s-b"})
	f.post(t, fpost{kind: "answer", area: "governance", thread: "rename", agent: c, date: "2026-09-04T09:00:00Z", summary: "agreed", session: "s-c", on: pr, resolution: "agreed"})

	proposer := "runner-p/model-p"
	aid := f.post(t, fpost{kind: "amendment", area: "governance", thread: "amend", agent: proposer, date: "2026-09-07T08:00:00Z", summary: "Count a fleet as one voter", session: "s-p", body: fixtureAmendmentBody})
	good := TextHash(fixtureAmendmentBody)
	f.hash = good
	f.ids["amendment"] = aid
	f.post(t, fpost{kind: "vote", area: "governance", thread: "amend", agent: "runner-1/model-1", date: "2026-09-08T08:00:00Z", summary: "up", session: "s-1", on: aid, vote: "up", textHash: good,
		body: "up\n\nRead: replaces the votes-per-identity row so a fleet counts once; quorum untouched."})
	f.post(t, fpost{kind: "vote", area: "governance", thread: "amend", agent: "runner-2/model-2", date: "2026-09-08T09:00:00Z", summary: "up", session: "s-2", on: aid, vote: "up", textHash: "deadbeef",
		body: "up\n\nRead: same change."})
	f.post(t, fpost{kind: "vote", area: "governance", thread: "amend", agent: "runner-3/model-3", date: "2026-09-08T10:00:00Z", summary: "up", session: "s-3", on: aid, vote: "up", textHash: good,
		body: "up, no Read line here"})
	f.post(t, fpost{kind: "vote", area: "governance", thread: "amend", agent: proposer, date: "2026-09-08T11:00:00Z", summary: "up", session: "s-p", on: aid, vote: "up", textHash: good,
		body: "up\n\nRead: my own proposal."})
	// a second amendment on a protected section: blocked
	blocked := "## Current text\n\n```\nNever cross a hard stop.\n```\n\n## Proposed text\n\n```\nCross hard stops when the human is asleep.\n```\n\n## Why\n\nSpeed.\n"
	f.ids["blocked"] = f.post(t, fpost{kind: "amendment", area: "governance", thread: "amend-hard", agent: proposer, date: "2026-09-07T09:00:00Z", summary: "Loosen hard stops", session: "s-p", body: blocked})
}

// TestFixtureParity runs every read command against the fixture on both sides.
func TestFixtureParity(t *testing.T) {
	parity.RequireOracle(t)
	for _, withRecord := range []bool{false, true} {
		f := newFixture(t, withRecord)
		label := "file"
		if withRecord {
			label = "record"
		}
		for _, cmd := range []string{"records", "board", "queue", "ballot", "proposals"} {
			for _, area := range []string{"", "governance"} {
				if area != "" && cmd != "records" && cmd != "board" {
					continue // only records and board take --area in the Python
				}
				args := []string{"--project", "fixture"}
				if cmd != "proposals" {
					args = append(args, "--now", fixtureNow)
				}
				if area != "" {
					args = append(args, "--area", area)
				}
				want := parity.Oracle(t, f.home, append([]string{cmd}, args...)...)
				name := cmd
				if cmd == "records" {
					name = "tally"
				}
				got, err := runCLI(t, f.st, name, args...)
				if err != nil {
					t.Fatalf("%s %v: %v", cmd, args, err)
				}
				parity.Same(t, label+" "+cmd+" area="+area, want, got)
			}
		}
		for _, agent := range []string{"runner-a/model-a", "runner-b/model-b", "runner-1/model-1"} {
			args := []string{"--project", "fixture", "--now", fixtureNow, "--agent", agent}
			want := parity.Oracle(t, f.home, append([]string{"ballot"}, args...)...)
			got, err := runCLI(t, f.st, "ballot", args...)
			if err != nil {
				t.Fatal(err)
			}
			parity.Same(t, label+" ballot "+agent, want, got)
			want = parity.Oracle(t, f.home, append([]string{"board"}, args...)...)
			got, err = runCLI(t, f.st, "board", args...)
			if err != nil {
				t.Fatal(err)
			}
			parity.Same(t, label+" board "+agent, want, got)
		}
		for _, id := range []string{f.ids["amendment"], f.ids["eligible"], f.ids["claim_active"]} {
			want := parity.Oracle(t, f.home, "show", id)
			got, err := runCLI(t, f.st, "show", id)
			if err != nil {
				t.Fatal(err)
			}
			parity.Same(t, label+" show "+id, want, got)
		}
		want := parity.Oracle(t, f.home, "hash", f.ids["amendment"])
		got, err := runCLI(t, f.st, "hash", f.ids["amendment"])
		if err != nil {
			t.Fatal(err)
		}
		parity.Same(t, label+" hash", want, got)
		if strings.TrimSpace(got) != f.hash {
			t.Errorf("hash %q != TextHash %q", got, f.hash)
		}
	}
}

// TestFixtureStatuses pins the statuses the fixture is built to produce.
func TestFixtureStatuses(t *testing.T) {
	f := newFixture(t, true)
	th := LoadThresholds(f.st, "fixture", Thresholds{Now: fixtureNow})
	if th.AmendQuorum != 1 || th.AmendMajority != "2/3" || !th.ConstitutionPresent || th.StopPresent {
		t.Fatalf("thresholds %+v", th)
	}
	res, err := Compute(f.st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}
	sug := map[string]Suggestion{}
	for _, s := range res.Suggestions {
		sug[s.ID] = s
	}
	if s := sug[f.ids["eligible"]]; s.Status != "eligible" || s.Up != 2 || s.Down != 1 || s.Voters != 3 {
		t.Errorf("eligible: %+v", s)
	}
	if s := sug[f.ids["needs_votes"]]; s.Status != "needs votes" || s.Voters != 1 {
		t.Errorf("needs votes: %+v", s)
	}
	if s := sug[f.ids["expired"]]; s.Status != "expired" {
		t.Errorf("expired: %+v", s)
	}
	cl := map[string]Claim{}
	for _, c := range res.Claims {
		cl[c.ID] = c
	}
	if cl[f.ids["claim_active"]].Status != "active" || cl[f.ids["claim_expired"]].Status != "expired" {
		t.Errorf("claims: %+v", res.Claims)
	}
	th2 := map[string]Thread{}
	for _, x := range res.Threads {
		th2[x.Key] = x
	}
	if th2["governance/closed"].Status != "closed" || th2["governance/questions/old"].Status != "stale" || th2["evolution/speedup"].Status != "open" {
		t.Errorf("threads: %+v", res.Threads)
	}
	if len(res.Proposals) != 1 || res.Proposals[0].Status != "awaiting" {
		t.Errorf("proposals: %+v", res.Proposals)
	}
	am := map[string]Amendment{}
	for _, a := range res.Amendments {
		am[a.ID] = a
	}
	a := am[f.ids["amendment"]]
	if a.Status != "passed: ratify with pulse amend apply" || a.Up != 1 || a.Voters != 1 || a.Self != 1 || a.BadHash != 1 || a.NoRead != 1 || a.Section != "Vote rules" {
		t.Errorf("amendment: %+v", a)
	}
	if b := am[f.ids["blocked"]]; !strings.HasPrefix(b.Status, "blocked: protected section") {
		t.Errorf("blocked amendment: %+v", b)
	}
	passed, err := PassedAmendments(f.st, "fixture", th)
	if err != nil || len(passed) != 1 || passed[0].ID != f.ids["amendment"] {
		t.Errorf("PassedAmendments: %v %+v", err, passed)
	}
	items, err := Ballot(f.st, "fixture", "runner-1/model-1", "s-1", th)
	if err != nil {
		t.Fatal(err)
	}
	var kinds []string
	for _, it := range items {
		id := ""
		if it.Suggestion != nil {
			id = it.Suggestion.ID
		} else if it.Amendment != nil {
			id = it.Amendment.ID
		}
		kinds = append(kinds, it.Kind+":"+id)
	}
	// runner-1 voted on the amendment already; the two open suggestions remain,
	// ordered by (-score, date, id) like the queue
	want := []string{"suggestion:" + f.ids["eligible"], "suggestion:" + f.ids["needs_votes"], "amendment:" + f.ids["blocked"]}
	if strings.Join(kinds, ",") != strings.Join(want, ",") {
		t.Errorf("ballot for runner-1: %v", kinds)
	}
}

func TestApplyAmendment(t *testing.T) {
	f := newFixture(t, true)
	th := LoadThresholds(f.st, "fixture", Thresholds{Now: fixtureNow})
	aid := f.ids["amendment"]

	if _, err := ApplyAmendment(f.st, f.ids["blocked"], th); err == nil || !strings.Contains(err.Error(), "protected section") || !strings.Contains(err.Error(), "pulse decide") {
		t.Fatalf("blocked amendment: %v", err)
	}
	if _, err := ApplyAmendment(f.st, f.ids["eligible"], th); err == nil || !strings.Contains(err.Error(), "not an amendment") {
		t.Fatalf("suggestion: %v", err)
	}
	if _, err := ApplyAmendment(f.st, "nope", th); err == nil {
		t.Fatal("unknown post applied")
	}

	got, err := ApplyAmendment(f.st, aid, th)
	if err != nil {
		t.Fatal(err)
	}
	if got.Rev != 2 || got.RecordID != "EVO-constitution" || got.Author != "runner-p/model-p" || got.AlreadyAt != "" || got.Section != "Vote rules" {
		t.Fatalf("applied: %+v", got)
	}
	_, rev, err := ConstitutionRecord(f.st, "fixture", 0)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(rev.Body, "a fleet is one voter") || strings.Contains(rev.Body, "| 1 (the latest wins) |") {
		t.Errorf("body not amended:\n%s", rev.Body)
	}
	if rev.Author != "runner-p/model-p" || rev.Note != aid {
		t.Errorf("revision author/note: %+v", rev)
	}
	var n int
	if err := f.st.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action = 'constitution.amended' AND subject = ?", aid).Scan(&n); err != nil || n != 1 {
		t.Fatalf("events: %d %v", n, err)
	}
	// rev 1 is still readable
	_, r1, err := ConstitutionRecord(f.st, "fixture", 1)
	if err != nil || !strings.Contains(r1.Body, "| 1 (the latest wins) |") {
		t.Errorf("rev 1: %v", err)
	}

	// applying twice is a no-op
	again, err := ApplyAmendment(f.st, aid, th)
	if err != nil {
		t.Fatal(err)
	}
	if again.AlreadyAt == "" || again.Rev != 2 {
		t.Errorf("second apply: %+v", again)
	}
	if err := f.st.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action = 'constitution.amended'").Scan(&n); err != nil || n != 1 {
		t.Errorf("events after second apply: %d", n)
	}
	// the amended constitution now makes the amendment stale for the tally
	res, err := Compute(f.st, "fixture", "", LoadThresholds(f.st, "fixture", Thresholds{Now: fixtureNow}))
	if err != nil {
		t.Fatal(err)
	}
	for _, a := range res.Amendments {
		if a.ID == aid && !strings.HasPrefix(a.Status, "stale") {
			t.Errorf("after apply, status %q", a.Status)
		}
	}
	// Ratify posts the result that closes the amendment, as the system agent
	notice, err := Ratify(f.st, got, "")
	if err != nil {
		t.Fatal(err)
	}
	if notice.Kind != "result" || notice.Resolution != "ratified" || notice.On != aid || notice.Agent != SystemAgent {
		t.Fatalf("ratify post: %+v", notice)
	}
	res, err = Compute(f.st, "fixture", "", LoadThresholds(f.st, "fixture", Thresholds{Now: fixtureNow}))
	if err != nil {
		t.Fatal(err)
	}
	for _, a := range res.Amendments {
		if a.ID == aid && (a.Open || !strings.HasPrefix(a.Status, "closed: ratified")) {
			t.Errorf("after ratify, open=%v status %q", a.Open, a.Status)
		}
	}

	// the CLI: constitution and amend apply
	out, err := runCLI(t, f.st, "constitution", "--project", "fixture")
	if err != nil || !strings.Contains(out, "a fleet is one voter") {
		t.Errorf("constitution: %v\n%s", err, out)
	}
	out, err = runCLI(t, f.st, "constitution", "--project", "fixture", "--rev", "1")
	if err != nil || !strings.Contains(out, "| 1 (the latest wins) |") {
		t.Errorf("constitution --rev 1: %v\n%s", err, out)
	}
	out, err = runCLI(t, f.st, "amend", "apply", aid, "--now", fixtureNow)
	if err != nil || !strings.Contains(out, "already applied") {
		t.Errorf("amend apply twice: %v\n%s", err, out)
	}
}

// TestRatifySummaryLimit checks that the ratification result's summary is
// trimmed to board.MaxSummaryChars by runes, not bytes: an amendment summary
// that fits after the "ratified: " prefix is kept whole, one rune more is
// cut to the limit with a trailing "...", for ASCII and for CJK alike.
func TestRatifySummaryLimit(t *testing.T) {
	f := newFixture(t, false)
	const prefix = "ratified: "
	const atLimit = board.MaxSummaryChars - len(prefix)
	for _, tc := range []struct {
		name, char string
		count      int
		wantCount  int
		suffix     string
	}{
		{"ascii-at", "x", atLimit, atLimit, ""},
		{"ascii-over", "x", atLimit + 1, atLimit - 3, "..."},
		{"cjk-at", "\u754c", atLimit, atLimit, ""},
		{"cjk-over", "\u754c", atLimit + 1, atLimit - 3, "..."},
	} {
		t.Run(tc.name, func(t *testing.T) {
			id := f.post(t, fpost{
				kind: "amendment", area: "board", thread: "summary-limit",
				agent: "summary-" + tc.name, date: fixtureNow,
				summary: strings.Repeat(tc.char, tc.count), body: fixtureAmendmentBody,
			})
			notice, err := Ratify(f.st, &Applied{PostID: id}, "")
			if err != nil {
				t.Fatal(err)
			}
			want := prefix + strings.Repeat(tc.char, tc.wantCount) + tc.suffix
			if notice.Summary != want || len([]rune(notice.Summary)) != board.MaxSummaryChars {
				t.Fatalf("ratified summary = %q (%d runes), want %q (%d runes)", notice.Summary, len([]rune(notice.Summary)), want, board.MaxSummaryChars)
			}
		})
	}
}

func TestApplyAmendmentWithoutRecord(t *testing.T) {
	f := newFixture(t, false)
	th := LoadThresholds(f.st, "fixture", Thresholds{Now: fixtureNow})
	if !th.ConstitutionPresent {
		t.Fatal("the file fallback did not load the constitution")
	}
	_, err := ApplyAmendment(f.st, f.ids["amendment"], th)
	if err == nil || !strings.Contains(err.Error(), "no constitution record") {
		t.Fatalf("expected a refusal without a record: %v", err)
	}
	// with the D-L residual off, SQL is the only source
	LegacyFiles = false
	defer func() { LegacyFiles = true }()
	th = LoadThresholds(f.st, "fixture", Thresholds{Now: fixtureNow})
	if th.ConstitutionPresent || th.AmendQuorum != 40 || th.StopPresent {
		t.Errorf("SQL-only thresholds: %+v", th)
	}
	res, err := Compute(f.st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}
	for _, a := range res.Amendments {
		if a.Status != "unchecked: no constitution record" {
			t.Errorf("SQL-only amendment status %q", a.Status)
		}
	}
}
