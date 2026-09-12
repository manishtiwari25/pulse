package runner

import (
	"fmt"
	"runtime"
	"testing"
	"time"
)

// skipSlow marks the tests that spawn real worker processes and daemon loops (several seconds of wall
// clock each, far more when the package runs under load). They still run in the full suite; `go test
// -short ./internal/runner/...` skips them so a quick check finishes in well under two minutes.
func skipSlow(t *testing.T) {
	t.Helper()
	if testing.Short() {
		t.Skip("slow worker/daemon test skipped under -short")
	}
}

// testDeadline fails the test with a goroutine dump when it runs past d, so a worker clone whose
// spawned git/worker subprocess blocks (credential prompt, stuck pipe) reports the stuck test by
// name instead of letting the whole package hit `go test -timeout` and kill every result with it.
// The full-package timeout stays the outer bound; d must sit well under it.
func testDeadline(t *testing.T, d time.Duration) {
	t.Helper()
	done := make(chan struct{})
	t.Cleanup(func() { close(done) })
	go func() {
		select {
		case <-done:
		case <-time.After(d):
			buf := make([]byte, 1<<20)
			n := runtime.Stack(buf, true)
			t.Errorf("%s ran past its %s deadline; goroutines:\n%s", t.Name(), d, buf[:n])
			panic(fmt.Sprintf("test %s exceeded %s deadline", t.Name(), d))
		}
	}()
}
