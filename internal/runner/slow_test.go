package runner

import (
	"fmt"
	"os"
	"runtime"
	"strconv"
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

// defaultTestDeadline is the per-test bound newStore installs; the slowest test here takes ~8s alone.
const defaultTestDeadline = 60 * time.Second

// testDeadline fails the test with a goroutine dump when it runs past d, so a worker clone whose
// spawned git/worker subprocess blocks (credential prompt, stuck pipe) reports the stuck test by
// name instead of letting the whole package hit `go test -timeout` and kill every result with it.
// The full-package timeout stays the outer bound; d must sit well under it.
//
// d is scaled by deadlineMult so a loaded CI host does not turn a merely slow test into a false FAIL:
// PULSE_TEST_DEADLINE_MULT=3 triples every budget; under -short the budget doubles by default, since
// -short runs are the ones that share the host with other jobs. Raise `go test -timeout` accordingly.
func testDeadline(t *testing.T, d time.Duration) {
	t.Helper()
	d = time.Duration(float64(d) * deadlineMult())
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

// deadlineMult reads PULSE_TEST_DEADLINE_MULT (a positive float, e.g. "2.5"); when unset or invalid it
// returns 2 under -short and 1 otherwise. Values below 1 are clamped to 1 so a budget can only grow.
func deadlineMult() float64 {
	if v := os.Getenv("PULSE_TEST_DEADLINE_MULT"); v != "" {
		if f, err := strconv.ParseFloat(v, 64); err == nil && f > 0 {
			if f < 1 {
				return 1
			}
			return f
		}
	}
	if testing.Short() {
		return 2
	}
	return 1
}

func TestDeadlineMult(t *testing.T) {
	for _, tc := range []struct {
		env  string
		want float64
	}{{"3", 3}, {"2.5", 2.5}, {"0.5", 1}, {"0", 0}, {"abc", 0}} {
		t.Setenv("PULSE_TEST_DEADLINE_MULT", tc.env)
		want := tc.want
		if want == 0 {
			want = 1
			if testing.Short() {
				want = 2
			}
		}
		if got := deadlineMult(); got != want {
			t.Errorf("PULSE_TEST_DEADLINE_MULT=%q: got %v want %v", tc.env, got, want)
		}
	}
}
