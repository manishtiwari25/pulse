package hub

import (
	"os"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

// TestMain keeps the compute caps' login-source check (ADR-034) off the hub's own tests: the runner installs a
// check that would ask `gh auth token` and GitHub, and no hub test is about that.
func TestMain(m *testing.M) {
	tasks.LoginSourceCheck = func(*store.Store) bool { return true }
	os.Exit(m.Run())
}
