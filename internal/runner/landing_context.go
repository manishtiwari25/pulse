package runner

import (
	"context"
	"path/filepath"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

func landingStop(ctx context.Context, s *store.Store, root string) string {
	if why := runnerStopped(s); why != "" {
		return why
	}
	if fileExists(filepath.Join(root, "docs", "evolution", "STOP")) {
		return "docs/evolution/STOP is present in " + root
	}
	if ctx != nil && ctx.Err() != nil {
		return ctx.Err().Error()
	}
	return ""
}

func shWithContext(ctx context.Context, dir, cmd string, budget time.Duration) (rc *int, stdout, stderr string) {
	out, err := runGroup(ctx, []string{"/bin/sh", "-c", cmd}, dir, environ(), nil, budget)
	if err != nil {
		code := 127
		return &code, "", err.Error()
	}
	if out.Timeout {
		return nil, out.Stdout, out.Stderr
	}
	code := out.ExitCode
	return &code, out.Stdout, out.Stderr
}
