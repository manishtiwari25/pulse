package tally

import (
	"strings"
	"testing"
)

func TestCardClass(t *testing.T) {
	tests := []struct {
		name, summary, body, want string
	}{
		{"hub git dir", "GIT_DIR=/tmp/x/.git go test ./internal/hub goes FAIL -> ok", "Class: product\nHypothesis: passes under GIT_DIR", CardClassOperatingLayer},
		{"tools check", "check:tools/test.sh stays green", "Class: product\nFile: tools/test.sh\nVariable: shell check\nHypothesis: stays green", CardClassOperatingLayer},
		{"vitest case", "a new vitest case in __tests__/login.test.ts", "Class: product\nFile: __tests__/login.test.ts\nVariable: add a vitest case\nHypothesis: adds a vitest case", CardClassOperatingLayer},
		{"gofmt", "gofmt -l goes 1->0", "Class: product\nVariable: gofmt\nHypothesis: gofmt", CardClassOperatingLayer},
		{"grep helper", "grep -rn 'func gitEnv' shrinks by one", "Class: product\nVariable: helper removal\nHypothesis: check stays green", CardClassOperatingLayer},
		{"github workflow", "CI workflow stops failing", "Class: product\nFile: .github/workflows/test.yml\nVariable: runner setup", CardClassOperatingLayer},
		{"fixture", "fixture data is renamed", "Class: product\nFile: internal/testdata/store.json\nVariable: fixture", CardClassOperatingLayer},
		{"no class no path", "Make it better", "Variable: one constant\nHypothesis: score improves", CardClassOperatingLayer},
		{"no class signal only", "README gets clearer", "Signal: README.md\nVariable: wording\nHypothesis: users understand setup", CardClassOperatingLayer},
		{"oauth product", "German-locale user with a failed OAuth callback sees a localized error message", "Class: product\nFile: src/oauth/callback.go\nVariable: callback error copy\nHypothesis: a German-locale user sees the localized failure message", CardClassProduct},
		{"site data product by file", "CTA copy follows status", "File: lib/site-data.ts\nVariable: CTA copy per status\nHypothesis: users see the right CTA", CardClassProduct},
		{"declared operating wins", "Internal report wording", "Class: operating-layer\nFile: internal/runner/brief.go\nVariable: wording", CardClassOperatingLayer},
		// Real card shapes from the board: a product File: path does not save a card whose outcome and change are both check work.
		{"git env plumbing behind a product path", "GIT_DIR=/tmp/x go test -run TestEnvSetShowCheck ./internal/runner goes FAIL->ok",
			"Class: product\nFile: internal/runner/util.go\nVariable: isGitCheckout runs git with gitEnv(nil) so an inherited GIT_DIR does not redirect it\nHypothesis: GIT_DIR=/tmp/x go test -run TestEnvSetShowCheck ./internal/runner goes FAIL->ok\nTier: 1", CardClassOperatingLayer},
		{"testutil path", "GIT_INDEX_FILE=/tmp/x go test ./internal/hub goes FAIL->ok",
			"Class: product\nFile: internal/testutil/gitenv.go\nVariable: strip GIT_DIR from os.Environ() inside GitEnv()\nHypothesis: GIT_INDEX_FILE=/tmp/x go test ./internal/hub goes FAIL->ok", CardClassOperatingLayer},
		{"test speed refactor", "TestLauncherRestores drops from 10.5s to <1s",
			"Class: product\nFile: internal/runner/service.go\nVariable: make the launcher's early-exit pause a parameter (default 2s, test passes 0)\nHypothesis: TestLauncherRestoresThePreviousPulse drops from 10.5s to <1s and `go test ./internal/runner` gains ~10s headroom", CardClassOperatingLayer},
		{"line count refactor", "js/tutor.test.mjs stays 33/33 while Session shrinks toward ~180 lines",
			"File: js/demo/tutor.js\nVariable: extract one remaining large method body out of Session\nHypothesis: js/tutor.test.mjs stays 33/33 while Session shrinks from 216 toward ~180 lines, giving margin against future regressions", CardClassOperatingLayer},
		{"gofmt with go test still ok", "gofmt -l ./... goes from 1 file to 0",
			"Class: product\nFile: internal/runner/exec.go\nVariable: gofmt exec.go\nHypothesis: gofmt -l ./... goes from 1 file to 0 with go test ./internal/runner still ok", CardClassOperatingLayer},
		{"precondition clause", "GIT_DIR=/tmp/x/.git go test ./internal/board goes 47 FAIL -> ok",
			"Class: product\nFile: internal/testutil/gitenv.go\nVariable: gitChildEnv everywhere\nHypothesis: with board.go gitChildEnv landed, GIT_DIR=/tmp/x/.git go test ./internal/board goes 47 FAIL -> ok", CardClassOperatingLayer},
		{"typecheck errors in a test dir", "tsc drops from 6 errors to 0",
			"Class: product\nFile: src/backend/Amistio.Api.Tests/Amistio.Api.Tests.csproj\nVariable: bump Mvc.Testing\nHypothesis: dotnet test stays green", CardClassOperatingLayer},
		{"pulse control plane docs", "docs match check:test:run reality",
			"Class: product\nFile: docs/memory/web.md\nVariable: update the full-suite note\nHypothesis: docs match check:test:run reality (94/94), no check changes", CardClassOperatingLayer},
		{"product outcome then check", "fewer runs miss their finish call",
			"Class: product\nFile: cmd/pulse/main.go\nVariable: call finish on every exit path\nHypothesis: fewer runs miss their finish call, go test ./internal/store stays green", CardClassProduct},
		{"product outcome with typecheck trailer", "In development count drops 3 -> 1",
			"Class: product\nFile: lib/site-data.ts\nVariable: set Lamplit Light status to Live\nHypothesis: out/index.html \"In development\" count drops 3 -> 1 with build/typecheck green", CardClassProduct},
		{"product outcome measured under a check", "posts without image drop 8/16 -> 7/16",
			"Class: product\nFile: _posts/2022-03-21-GRPC.md\nVariable: add one relevant screenshot header image\nHypothesis: posts without image drop 8/16 -> 7/16 under check:tools/test.sh", CardClassProduct},
		{"blog screenshot of a workflow run", "image-coverage drops 5/16 -> 4/16 and stays green",
			"Class: product\nFile: _posts/2022-07-19-Azurite-GitHubAction.md\nVariable: add a header screenshot of the Azurite GitHub Action workflow run\nHypothesis: check:tools/test.sh image-coverage drops 5/16 -> 4/16 and stays green", CardClassProduct},
		{"product change measured by a test assertion", "check:test-fast stays green and gains an assertion on the hint text",
			"Class: product\nFile: js/demo/tutor.js\nVariable: use pre.tooLong in the deterministic write feedback to emit a \"too long\" hint line\nHypothesis: check:test-fast stays green and gains an assertion on the hint text", CardClassProduct},
		{"missing label makes a test pass", "go test ./internal/hub -run TestSettingLabelsCoverCatalogue moves FAIL->ok",
			"Class: product\nFile: web/hub/src/lib/setting-labels.ts\nVariable: add a human label for records.auto_adopt\nHypothesis: go test ./internal/hub -run TestSettingLabelsCoverCatalogue moves FAIL->ok", CardClassProduct},
		{"error propagation closing a gap", "go vet and go test stay green while a new unit test can assert runJob returns an error",
			"Class: product\nFile: internal/runner/run.go\nVariable: check and propagate the error from os.MkdirAll instead of discarding it\nHypothesis: go vet ./... and go test ./internal/runner stay green while a new unit test can assert runJob returns a fail() error when workDir creation fails, closing a silent-failure gap", CardClassProduct},
		{"trailing not-just aside", "check:npm run typecheck covers web-shared too",
			"Class: product\nFile: packages/web-shared/package.json\nVariable: add a typecheck script\nHypothesis: check:npm run typecheck (root) covers web-shared and web too, not just web-fachsprachtrainer", CardClassOperatingLayer},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := CardClass(tc.summary, tc.body); got != tc.want {
				t.Fatalf("CardClass = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestIsOperatingLayerPath(t *testing.T) {
	for _, p := range []string{"internal/runner/run_test.go", "src/a/__tests__/x.test.tsx", "internal/testutil/gitenv.go", "tools/test.sh",
		".github/workflows/ci.yml", "docs/memory/lessons.md", "docs/scripts/pulse.py", "AGENTS.md", "js/check-ui.mjs", "docs/scripts/check-links.sh",
		"src/backend/Amistio.Api.Tests/Amistio.Api.Tests.csproj", "src/web/tsconfig.json", "src/web/vitest.config.ts", "pnpm-lock.yaml", "Makefile"} {
		if !IsOperatingLayerPath(p) {
			t.Errorf("IsOperatingLayerPath(%q) = false", p)
		}
	}
	for _, p := range []string{"internal/runner/run.go", "src/web/app/page.tsx", "docs/runbooks/pretraining.md", "README.md", "_posts/2022-03-21-GRPC.md",
		"lib/site-data.ts", "package.json", "cmd/pulse/main.go", "web/hub/src/components/ui/sheet.tsx", "docs/testing.md", ""} {
		if IsOperatingLayerPath(p) {
			t.Errorf("IsOperatingLayerPath(%q) = true", p)
		}
	}
}

func TestDuplicateCardsGateAndLeaveBallot(t *testing.T) {
	f := newFixture(t, true)
	earlier := f.post(t, fpost{kind: "suggestion", area: "evolution", thread: "dup-a", agent: "runner-da/model", date: "2026-09-08T08:00:00Z", summary: "Fix OAuth callback copy", body: "File: src/oauth.go\nVariable: callback copy", session: "s-da"})
	later := f.post(t, fpost{kind: "suggestion", area: "evolution", thread: "dup-b", agent: "runner-db/model", date: "2026-09-08T09:00:00Z", summary: "`fix`  \"oauth\" callback   copy", body: "File: src/other.go\nVariable: other copy", session: "s-db"})
	pairEarlier := f.post(t, fpost{kind: "suggestion", area: "evolution", thread: "pair-a", agent: "runner-pa/model", date: "2026-09-08T10:00:00Z", summary: "Login message copy", body: "File: src/login.go\nVariable: callback copy", session: "s-pa"})
	pairLater := f.post(t, fpost{kind: "suggestion", area: "evolution", thread: "pair-b", agent: "runner-pb/model", date: "2026-09-08T11:00:00Z", summary: "Localized login failure", body: "File: src/login.go\nVariable: callback copy", session: "s-pb"})
	differentFile := f.post(t, fpost{kind: "suggestion", area: "evolution", thread: "not-dup", agent: "runner-nd/model", date: "2026-09-08T12:00:00Z", summary: "Localized billing failure", body: "File: src/billing.go\nVariable: callback copy", session: "s-nd"})

	res, err := Compute(f.st, "fixture", "", Thresholds{Now: fixtureNow})
	if err != nil {
		t.Fatal(err)
	}
	if got := DuplicateOf(res, Suggestion{ID: later}); got != earlier {
		t.Fatalf("summary duplicate = %q, want %q", got, earlier)
	}
	if got := DuplicateOf(res, Suggestion{ID: pairLater}); got != pairEarlier {
		t.Fatalf("File+Variable duplicate = %q, want %q", got, pairEarlier)
	}
	if got := DuplicateOf(res, Suggestion{ID: differentFile}); got != "" {
		t.Fatalf("different file duplicate = %q", got)
	}
	ctx := PlanContextFor(f.st, "fixture", fixtureNow, res)
	sgs := map[string]Suggestion{}
	posts := map[string]*Post{}
	for _, sg := range res.Suggestions {
		sgs[sg.ID] = sg
	}
	for i := range res.Posts {
		posts[res.Posts[i].ID] = &res.Posts[i]
	}
	if got := PlanGateWith(f.st, ctx, f.repo, sgs[later], posts[later]); got != DuplicateGate(earlier) {
		t.Fatalf("duplicate gate = %q, want %q", got, DuplicateGate(earlier))
	}
	var ballotIDs []string
	for _, it := range res.Ballot("", "") {
		if it.Suggestion != nil {
			ballotIDs = append(ballotIDs, it.Suggestion.ID)
		}
	}
	joined := strings.Join(ballotIDs, ",")
	if strings.Contains(joined, later) || strings.Contains(joined, pairLater) {
		t.Fatalf("ballot contains duplicate cards: %v", ballotIDs)
	}
	if !strings.Contains(joined, earlier) || !strings.Contains(joined, pairEarlier) || !strings.Contains(joined, differentFile) {
		t.Fatalf("ballot dropped non-duplicates: %v", ballotIDs)
	}
}

func TestOperatingLayerCapGate(t *testing.T) {
	f := newFixture(t, true)
	if err := f.st.SetSetting("test", "plan.operating_layer_per_day", "1"); err != nil {
		t.Fatal(err)
	}
	counted := f.post(t, fpost{kind: "suggestion", area: "evolution", thread: "counted", agent: "runner-counted/model", date: "2026-09-09T08:00:00Z", summary: "check:tools/test.sh stays green", body: "Class: operating-layer\nFile: tools/test.sh\nVariable: shell check", session: "s-counted"})
	op := addEligibleSuggestion(t, f, "7", "runner-op/model", "op-cap", "gofmt -l goes 1->0", "Class: product\nVariable: gofmt\nHypothesis: gofmt", "", []string{"up", "up"})
	product := addEligibleSuggestion(t, f, "8", "runner-product/model", "product-cap", "Localized OAuth error", "Class: product\nFile: src/oauth.go\nVariable: callback copy\nHypothesis: user sees localized error", "", []string{"up", "up"})
	if _, err := f.st.DB.Exec("INSERT INTO jobs (id, project, kind, title, brief, source, status, created_at) VALUES ('J-counted', 'fixture', 'improve', 'counted', '', ?, 'queued', ?)",
		"suggestion:"+counted, fixtureNow); err != nil {
		t.Fatal(err)
	}
	res, err := Compute(f.st, "fixture", "", Thresholds{Now: fixtureNow})
	if err != nil {
		t.Fatal(err)
	}
	ctx := PlanContextFor(f.st, "fixture", fixtureNow, res)
	sgs, posts := map[string]Suggestion{}, map[string]*Post{}
	for _, sg := range res.Suggestions {
		sgs[sg.ID] = sg
	}
	for i := range res.Posts {
		posts[res.Posts[i].ID] = &res.Posts[i]
	}
	want := OperatingLayerCapGate(1, 1)
	if got := PlanGateWith(f.st, ctx, f.repo, sgs[op], posts[op]); got != want {
		t.Fatalf("operating cap gate = %q, want %q", got, want)
	}
	if got := PlanGateWith(f.st, ctx, f.repo, sgs[product], posts[product]); got != "" {
		t.Fatalf("product card gated by cap: %q", got)
	}
	if err := f.st.SetSetting("test", "plan.operating_layer_per_day", "0"); err != nil {
		t.Fatal(err)
	}
	if got := PlanGateWith(f.st, PlanContextFor(f.st, "fixture", fixtureNow, res), f.repo, sgs[op], posts[op]); got != "" {
		t.Fatalf("cap 0 gate = %q, want none", got)
	}
	if err := f.st.SetSetting("test", "plan.operating_layer_per_day", "1"); err != nil {
		t.Fatal(err)
	}
	if _, err := f.st.DB.Exec("UPDATE jobs SET created_at = '2026-09-09T23:59:59Z' WHERE id = 'J-counted'"); err != nil {
		t.Fatal(err)
	}
	if got := PlanGateWith(f.st, PlanContextFor(f.st, "fixture", "2026-09-10T00:00:01Z", res), f.repo, sgs[op], posts[op]); got != "" {
		t.Fatalf("next UTC day gate = %q, want none", got)
	}
}
