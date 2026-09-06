package evolve

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestFixturesCRUDAndSecretRefusal(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	f := fixture(t, s, "demo", "F01", "printf '0.5\\n'", 2)
	if err := SetFixtureEnabled(s, "demo", f.Name, false, "human"); err != nil {
		t.Fatal(err)
	}
	enabled, err := ListFixtures(s, "demo", true)
	if err != nil || len(enabled) != 0 {
		t.Fatal("disabled fixture remained selected")
	}
	if err := SetFixtureEnabled(s, "demo", f.Name, true, "human"); err != nil {
		t.Fatal(err)
	}
	enabled, err = ListFixtures(s, "demo", true)
	if err != nil || len(enabled) != 1 || len(enabled[0].AnswerKeys) != 1 {
		t.Fatal("enabled fixture did not retain its answer keys")
	}
	base := FixtureInput{Project: "demo", Name: "bad", Request: "safe request", Scorer: "printf 1", Weight: 1}
	cases := []FixtureInput{}
	x := base
	x.Request = "ghp_" + strings.Repeat("a", 40)
	cases = append(cases, x)
	x = base
	x.Scorer = "printf ghp_" + strings.Repeat("b", 40)
	cases = append(cases, x)
	x = base
	x.Weight = 0
	cases = append(cases, x)
	x = base
	x.Kind = "heldout"
	cases = append(cases, x)
	x = base
	x.AnswerKeys = []string{"../outside"}
	cases = append(cases, x)
	for i, in := range cases {
		if _, err := AddFixture(s, in); err == nil {
			t.Errorf("invalid public fixture case %d accepted", i)
		}
	}
	if countRows(t, s, "evolution_fixtures") != 1 {
		t.Fatal("invalid fixtures changed the table")
	}
	if _, err := s.DB.Exec("DELETE FROM evolution_fixtures WHERE id = ?", f.ID); err == nil {
		t.Fatal("fixture deletion bypassed disable-only history")
	}
}

func TestPrivateSetupCountsOnlyNonOverwriting(t *testing.T) {
	s := testStore(t)
	repo := testProject(t, s, "demo")
	dir := filepath.Join(t.TempDir(), "owner")
	first, err := Setup(s, dir)
	if err != nil || first.Pending != 2 || first.Created != 4 {
		t.Fatal("private template setup failed")
	}
	request := filepath.Join(dir, "H01.request")
	putFile(t, request, "owner-controlled request", 0o600)
	second, err := Setup(s, dir)
	if err != nil || second.Created != 0 {
		t.Fatal("repeated setup overwrote an existing template")
	}
	content, err := os.ReadFile(request)
	if err != nil || string(content) != "owner-controlled request" {
		t.Fatal("setup overwrote owner content")
	}
	encoded, err := json.Marshal(second)
	if err != nil {
		t.Fatal(err)
	}
	assertPrivateAbsent(t, string(encoded), []string{dir, "H01", "H02", "owner-controlled request"})
	if s.Setting("evolve.loop", "") != "0" || countRows(t, s, "evolution_archive") != 0 {
		t.Fatal("setup enabled evolution or invented a baseline")
	}
	if _, err := Setup(s, filepath.Join(repo, "private")); err == nil {
		t.Fatal("private setup accepted a registered checkout")
	}
	link := filepath.Join(t.TempDir(), "alias")
	if err := os.Symlink(repo, link); err != nil {
		t.Fatal(err)
	}
	if _, err := Setup(s, filepath.Join(link, "private")); err == nil {
		t.Fatal("private setup accepted a checkout through a symlink")
	}
	other := testProject(t, s, "second")
	if _, err := Setup(s, filepath.Join(other, "private")); err == nil {
		t.Fatal("private setup checked only the selected project")
	}
}

func TestPrivateValidationAndImportGuard(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	dir, forbidden := privateSet(t, s)
	state := HeldoutCount(s)
	if state.Count != 2 || state.Problem != "" {
		t.Fatal("complete private set was not recognized")
	}
	if _, err := readRequest(s, filepath.Join(dir, "owner-private-first.request")); err == nil {
		t.Fatal("private request could be imported into the public table")
	} else {
		assertPrivateAbsent(t, err.Error(), forbidden)
	}
	file := filepath.Join(dir, "owner-private-first.score")
	if err := os.Chmod(file, 0o744); err != nil {
		t.Fatal(err)
	}
	_, _, err := loadPrivate(s)
	if err == nil {
		t.Fatal("world-readable private scorer accepted")
	}
	assertPrivateAbsent(t, err.Error(), forbidden)
	encoded, _ := json.Marshal(HeldoutCount(s))
	assertPrivateAbsent(t, string(encoded), forbidden)
}

func TestPrivateSetupFailurePreservesTheConfiguredSet(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	good, forbidden := privateSet(t, s)
	bad := filepath.Join(t.TempDir(), "unpaired")
	putFile(t, filepath.Join(bad, "unpaired.request"), "Incomplete fixture", 0o600)
	if _, err := Setup(s, bad); err == nil {
		t.Fatal("setup accepted an unpaired private set")
	} else {
		assertPrivateAbsent(t, err.Error(), append(forbidden, bad, "unpaired"))
	}
	if s.Setting("evolve.heldout_dir", "") != good || HeldoutCount(s).Count != 2 {
		t.Fatal("failed setup replaced a working private configuration")
	}
}
