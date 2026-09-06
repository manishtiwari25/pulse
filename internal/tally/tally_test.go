package tally

import (
	"crypto/sha256"
	"encoding/hex"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func amendment(current, proposed string) string {
	return "## Current text\n\n```\n" + current + "\n```\n\n## Proposed text\n\n```\n" + proposed + "\n```\n\n## Why\n\nBecause.\n"
}

func TestCheckAmendmentMessages(t *testing.T) {
	protected := []string{"Hard stops", "Kill switch", "Amendments", "frontmatter"}
	cases := []struct {
		name, body, constitution, want string
	}{
		{"no proposed", "## Current text\n\n```\nx\n```\n", fixtureConstitutionFile, `malformed: no fenced block under "## Proposed text"`},
		{"no current", "## Proposed text\n\n```\nx\n```\n", fixtureConstitutionFile, `malformed: no fenced block under "## Current text"`},
		{"no constitution", amendment("a", "b"), "", "unchecked: no constitution record"},
		{"lower quorum", amendment("| Amendment quorum | 1 distinct voter |", "| Amendment quorum | 0 distinct voters |"), fixtureConstitutionFile,
			"lowers or removes the amendment quorum"},
		{"remove quorum", amendment("| Amendment quorum | 1 distinct voter |", "| Quorum | gone |"), fixtureConstitutionFile,
			"lowers or removes the amendment quorum"},
		{"lower majority", amendment("| Amendment supermajority | 2/3 of counted votes |", "| Amendment supermajority | 1/2 of counted votes |"), fixtureConstitutionFile,
			"lowers or removes the amendment supermajority"},
		{"remove guard", amendment("| Informed vote | text_hash plus a Read: line |", "| Informed | anything |"), fixtureConstitutionFile,
			`removes the "Informed vote" row`},
		{"drop protected", amendment("| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter |",
			"| Not amendable by vote | Hard stops, Amendments, frontmatter |"), fixtureConstitutionFile,
			`drops "Kill switch" from the protected sections`},
		{"stale anchor", amendment("this line is not in the constitution", "nor this"), fixtureConstitutionFile, "ok"},
		{"protected section", amendment("Never cross a hard stop.", "Cross them."), fixtureConstitutionFile, `protected section "Hard stops"`},
		{"protected case-insensitive", amendment("Per docs/workflows/amend.md.", "Per nothing."), fixtureConstitutionFile, `protected section "Amendments"`},
		{"frontmatter", amendment("loop: off", "loop: on"), fixtureConstitutionFile, `protected section "frontmatter"`},
		{"ok", amendment("| Votes per identity per suggestion | 1 (the latest wins) |", "| Votes per identity per suggestion | 1 |"), fixtureConstitutionFile, "ok"},
	}
	for _, c := range cases {
		err := CheckAmendment(c.body, c.constitution, protected)
		got := "ok"
		if err != nil {
			got = err.Error()
		}
		if got != c.want {
			t.Errorf("%s: got %q want %q", c.name, got, c.want)
		}
	}
	// anchor and section details
	ck := Check(amendment("this line is not in the constitution", "nor this"), fixtureConstitutionFile, ProtectedDefault)
	if ck.Anchor != "0" || ck.Verdict != "ok" || ck.Hash == "" {
		t.Errorf("stale anchor: %+v", ck)
	}
	twice := fixtureConstitutionFile + "\n## Extra\n\nNever cross a hard stop.\n"
	ck = Check(amendment("Never cross a hard stop.", "Cross them."), twice, ProtectedDefault)
	// sections (and the protected verdict) are only resolved for a unique anchor
	if ck.Anchor != "2" || ck.Section != "" || ck.Verdict != "ok" {
		t.Errorf("ambiguous anchor: %+v", ck)
	}
	ck = Check(amendment("| Votes per identity per suggestion | 1 (the latest wins) |", "| Votes per identity per suggestion | 1 |"), fixtureConstitutionFile, ProtectedDefault)
	if ck.Anchor != "1" || ck.Section != "Vote rules" {
		t.Errorf("ok anchor: %+v", ck)
	}
	// the frontmatter counts as protected only when the constitution has one:
	// a record body carries none, so the same amendment is merely stale there
	ck = Check(amendment("loop: off", "loop: on"), fixtureConstitutionBody, ProtectedDefault)
	if ck.Anchor != "0" || ck.Verdict != "ok" {
		t.Errorf("frontmatter against a record body: %+v", ck)
	}
}

func TestTextHash(t *testing.T) {
	sum := func(s string) string {
		h := sha256.Sum256([]byte(s))
		return hex.EncodeToString(h[:])
	}
	// an amendment body hashes the proposed block, normalized like board.sh
	body := amendment("old", "  new line  \n\nsecond")
	if got := TextHash(body); got != sum("  new line\n\nsecond\n") {
		t.Errorf("proposed block hash %s", got)
	}
	// plain text: trailing whitespace off, blank edges dropped, one newline
	if got := TextHash("\n\nhello  \nworld\t\n\n"); got != sum("hello\nworld\n") {
		t.Errorf("plain hash %s", got)
	}
	if TextHash("") != sum("\n") {
		t.Errorf("empty hash %s", TextHash(""))
	}
	// the fixture hash agrees with the oracle-checked value in TestFixtureParity
	if TextHash(fixtureAmendmentBody) != sum("| Votes per identity per suggestion | 1 (the latest wins; a fleet is one voter) |\n") {
		t.Error("fixture hash")
	}
}

func TestParseMajority(t *testing.T) {
	n, d, err := ParseMajority("2/3")
	if err != nil || n != 2 || d != 3 {
		t.Errorf("2/3: %d %d %v", n, d, err)
	}
	// like the Python: any n/m with m > 0 parses, only the shape and a zero denominator are refused
	if n, d, err := ParseMajority(" 4 / 3 "); err != nil || n != 4 || d != 3 {
		t.Errorf("4/3: %d %d %v", n, d, err)
	}
	for _, bad := range []string{"", "2", "a/b", "3/0", "-1/2", "2/3/4"} {
		if _, _, err := ParseMajority(bad); err == nil {
			t.Errorf("%q parsed", bad)
		}
	}
}

func TestLoadThresholds(t *testing.T) {
	home := t.TempDir()
	if err := store.Create(filepath.Join(home, "pulse.sqlite")); err != nil {
		t.Fatal(err)
	}
	st, err := store.Open(store.Options{Home: home, Now: fixtureNow})
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()
	if _, err := st.DB.Exec("INSERT INTO projects (slug, name, path, added_at) VALUES ('p', 'P', ?, ?)", filepath.Join(home, "p"), fixtureNow); err != nil {
		t.Fatal(err)
	}
	// defaults from the settings, no constitution
	th := LoadThresholds(st, "p", Thresholds{Now: fixtureNow})
	if th.Quorum != 2 || th.ExpiryDays != 14 || th.StaleDays != 30 || th.AmendQuorum != 40 || th.AmendMajority != "2/3" ||
		th.AmendNum != 2 || th.AmendDen != 3 || th.Protected != ProtectedDefault || th.ConstitutionPresent || th.StopPresent || th.Now != fixtureNow {
		t.Errorf("defaults: %+v", th)
	}
	// flags win
	th = LoadThresholds(st, "p", Thresholds{Quorum: 5, ExpiryDays: 9, StaleDays: 3, AmendQuorum: 7, AmendMajority: "3/4"})
	if th.Quorum != 5 || th.ExpiryDays != 9 || th.StaleDays != 3 || th.AmendQuorum != 7 || th.AmendNum != 3 || th.AmendDen != 4 {
		t.Errorf("flags: %+v", th)
	}
	// settings
	for k, v := range map[string]string{"quorum": "3", "stale_days": "10", "amend_majority": "3/5", "stop": "1"} {
		if _, err := st.DB.Exec("UPDATE settings SET value = ? WHERE name = ?", v, k); err != nil {
			t.Fatal(err)
		}
	}
	th = LoadThresholds(st, "p", Thresholds{})
	if th.Quorum != 3 || th.StaleDays != 10 || th.AmendMajority != "3/5" || th.AmendNum != 3 || !th.StopPresent {
		t.Errorf("settings: %+v", th)
	}
	// an unparsable setting falls back to the default fraction
	if _, err := st.DB.Exec("UPDATE settings SET value = 'most' WHERE name = 'amend_majority'"); err != nil {
		t.Fatal(err)
	}
	th = LoadThresholds(st, "p", Thresholds{})
	if th.AmendMajority != "2/3" || th.AmendNum != 2 || th.AmendDen != 3 {
		t.Errorf("bad setting: %+v", th)
	}
	// the constitution record's Vote rules beat the settings but not the flags
	rec := store.Record{ID: "EVO-constitution-p", Project: "p", Kind: "evolution", Slug: "constitution", Title: "C", Status: "active", Date: "2026-09-01"}
	if _, err := st.PutRecord(rec, nil, fixtureConstitutionBody, "test", ""); err != nil {
		t.Fatal(err)
	}
	th = LoadThresholds(st, "p", Thresholds{})
	if th.AmendQuorum != 1 || th.AmendMajority != "2/3" || !th.ConstitutionPresent || th.Constitution != fixtureConstitutionBody {
		t.Errorf("record: %+v", th)
	}
	th = LoadThresholds(st, "p", Thresholds{AmendQuorum: 9})
	if th.AmendQuorum != 9 {
		t.Errorf("flag over record: %+v", th)
	}
	// the commons has no constitution
	th = LoadThresholds(st, store.CommonsSlug, Thresholds{})
	if th.ConstitutionPresent {
		t.Errorf("commons: %+v", th)
	}
}

func TestDayNumberAndBoundaries(t *testing.T) {
	if dayNumber("2026-09-06T21:00:00Z") != 739865 || dayNumber("1970-01-01") != 719163 || dayNumber("0001-01-01") != 1 || dayNumber("garbage") != 0 {
		t.Errorf("dayNumber: %d %d %d %d", dayNumber("2026-09-06T21:00:00Z"), dayNumber("1970-01-01"), dayNumber("0001-01-01"), dayNumber("garbage"))
	}
	home := t.TempDir()
	if err := store.Create(filepath.Join(home, "pulse.sqlite")); err != nil {
		t.Fatal(err)
	}
	st, err := store.Open(store.Options{Home: home, Now: fixtureNow})
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()
	if _, err := st.DB.Exec("INSERT INTO projects (slug, name, path, added_at) VALUES ('fixture', 'P', ?, ?)", filepath.Join(home, "p"), fixtureNow); err != nil {
		t.Fatal(err)
	}
	f := &fixture{st: st, ids: map[string]string{}}
	// unvoted suggestions: age == expiry_days is still "needs votes", one day more expires
	edge := f.post(t, fpost{kind: "suggestion", area: "evolution", thread: "edge", agent: "r/a", date: "2026-09-08T00:00:00Z", summary: "two days old", session: "s"})
	over := f.post(t, fpost{kind: "suggestion", area: "evolution", thread: "over", agent: "r/a", date: "2026-09-07T23:59:59Z", summary: "three days old", session: "s"})
	// claims: expiry is compared as ISO strings against now
	live := f.post(t, fpost{kind: "claim", area: "governance", thread: "c1", agent: "r/a", date: "2026-09-09T00:00:00Z", summary: "still live", session: "s", expires: "2026-09-10T12:00:01Z"})
	dead := f.post(t, fpost{kind: "claim", area: "governance", thread: "c2", agent: "r/a", date: "2026-09-09T01:00:00Z", summary: "just dead", session: "s", expires: "2026-09-10T12:00:00Z"})
	noexp := f.post(t, fpost{kind: "claim", area: "governance", thread: "c3", agent: "r/a", date: "2026-09-09T02:00:00Z", summary: "no expiry", session: "s"})
	// threads: stale when today - last > stale_days
	f.post(t, fpost{kind: "finding", area: "governance", thread: "fresh", agent: "r/a", date: "2026-09-05T00:00:00Z", summary: "five days", session: "s"})
	f.post(t, fpost{kind: "finding", area: "governance", thread: "old", agent: "r/a", date: "2026-09-04T23:00:00Z", summary: "six days", session: "s"})

	th := LoadThresholds(st, "fixture", Thresholds{Now: fixtureNow, ExpiryDays: 2, StaleDays: 5})
	res, err := Compute(st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}
	status := map[string]string{}
	for _, s := range res.Suggestions {
		status[s.ID] = s.Status
	}
	for _, c := range res.Claims {
		status[c.ID] = c.Status
	}
	for _, x := range res.Threads {
		status[x.Key] = x.Status
	}
	want := map[string]string{
		edge: "needs votes", over: "expired",
		live: "active", dead: "expired", noexp: "no expiry",
		"governance/fresh": "open", "governance/old": "stale",
	}
	for k, v := range want {
		if status[k] != v {
			t.Errorf("%s: got %q want %q", k, status[k], v)
		}
	}
	// a vote keeps an old suggestion alive; the voter's own card is not counted
	f.post(t, fpost{kind: "vote", area: "evolution", thread: "over", agent: "r/b", date: "2026-09-10T00:00:00Z", summary: "up", session: "t", on: over, vote: "up"})
	res, err = Compute(st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}
	for _, s := range res.Suggestions {
		if s.ID == over && (s.Status != "needs votes" || s.Voters != 1) {
			t.Errorf("voted suggestion: %+v", s)
		}
	}
	// the Records TSV is what the dashboard reads: params first, one record per line
	tsv := res.Records()
	if !strings.HasPrefix(tsv, "param\tboard_present\t1\n") || !strings.Contains(tsv, "\nparam\texpiry_days\t2\n") || !strings.Contains(tsv, "\nclaim\t"+dead+"\t") {
		t.Errorf("records:\n%s", tsv)
	}
	tsv2, err := Records(st, "fixture", "", th)
	if err != nil || tsv2 != tsv {
		t.Errorf("Records() differs from Result.Records(): %v", err)
	}
}
