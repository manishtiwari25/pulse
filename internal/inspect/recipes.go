package inspect

import (
	"github.com/manishtiwari25/pulse/internal/runner"
	"github.com/manishtiwari25/pulse/internal/store"
)

// RecipeBlocks supplies the placement labels shown by status: {project slug:
// reason} for the active projects whose environment recipe (ADR-026) keeps
// them off `executor`. The rules live with the runner's recipe parser
// (runner.RecipeBlocks): the CTX-environment record first (D-K), else the
// default branch's docs/context/environment.md, never a dirty worktree.
func RecipeBlocks(s *store.Store, executor string) (map[string]string, error) {
	return runner.RecipeBlocks(s, executor, nil), nil
}
