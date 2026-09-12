package runner

import "testing"

// skipSlow marks the tests that spawn real worker processes and daemon loops (several seconds of wall
// clock each, far more when the package runs under load). They still run in the full suite; `go test
// -short ./internal/runner/...` skips them so a quick check finishes in well under two minutes.
func skipSlow(t *testing.T) {
	t.Helper()
	if testing.Short() {
		t.Skip("slow worker/daemon test skipped under -short")
	}
}
