package runner

import (
	"bytes"
	"context"
	"errors"
	"os"
	"os/exec"
	"syscall"
	"time"
)

// outcome is what a worker or check run produced.
type outcome struct {
	Timeout  bool
	ExitCode int // -1 when unknown
	Stdout   string
	Stderr   string
}

// runGroup runs argv in its own process group and kills the whole group when
// the budget ends or ctx is cancelled (Python: subprocess timeout; the hands'
// timeout(1) --kill-after). stdin is closed. A budget <= 0 means no limit.
func runGroup(ctx context.Context, argv []string, dir string, env []string, stdin []byte, budget time.Duration) (outcome, error) {
	if ctx == nil {
		ctx = context.Background()
	}
	cmd := exec.Command(argv[0], argv[1:]...)
	cmd.Dir = dir
	cmd.Env = env
	cmd.Stdin = bytes.NewReader(stdin)
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := cmd.Start(); err != nil {
		return outcome{ExitCode: -1}, err
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	var timer <-chan time.Time
	if budget > 0 {
		t := time.NewTimer(budget)
		defer t.Stop()
		timer = t.C
	}
	timedOut := false
	select {
	case err := <-done:
		return finish(err, out.String(), errb.String(), false), nil
	case <-timer:
		timedOut = true
	case <-ctx.Done():
		timedOut = true
	}
	killGroup(cmd.Process.Pid, done)
	return finish(nil, out.String(), errb.String(), timedOut), nil
}

// killGroup is timeout(1) --kill-after: TERM the group, wait, then KILL it.
func killGroup(pid int, done <-chan error) {
	_ = syscall.Kill(-pid, syscall.SIGTERM)
	select {
	case <-done:
		return
	case <-time.After(killAfter):
	}
	_ = syscall.Kill(-pid, syscall.SIGKILL)
	select {
	case <-done:
	case <-time.After(5 * time.Second):
	}
}

// killAfter is the grace between TERM and KILL; tests shorten it.
var killAfter = 5 * time.Second

func finish(err error, stdout, stderr string, timedOut bool) outcome {
	o := outcome{Stdout: stdout, Stderr: stderr, Timeout: timedOut, ExitCode: -1}
	if timedOut {
		return o
	}
	if err == nil {
		o.ExitCode = 0
		return o
	}
	var ee *exec.ExitError
	if errors.As(err, &ee) {
		o.ExitCode = ee.ExitCode()
		if o.ExitCode < 0 && ee.ProcessState != nil {
			if ws, ok := ee.ProcessState.Sys().(syscall.WaitStatus); ok && ws.Signaled() {
				o.ExitCode = 128 + int(ws.Signal())
			}
		}
	}
	return o
}

func envWith(base []string, extra map[string]string) []string {
	if len(extra) == 0 {
		return base
	}
	out := make([]string, 0, len(base)+len(extra))
	for _, kv := range base {
		k := kv
		if i := indexByte(kv, '='); i >= 0 {
			k = kv[:i]
		}
		if _, replaced := extra[k]; !replaced {
			out = append(out, kv)
		}
	}
	for _, k := range sortedKeys(extra) {
		out = append(out, k+"="+extra[k])
	}
	return out
}

func indexByte(s string, c byte) int {
	for i := 0; i < len(s); i++ {
		if s[i] == c {
			return i
		}
	}
	return -1
}

func sortedKeys(m map[string]string) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sortStrings(keys)
	return keys
}

func sortStrings(a []string) {
	for i := 1; i < len(a); i++ {
		for j := i; j > 0 && a[j] < a[j-1]; j-- {
			a[j], a[j-1] = a[j-1], a[j]
		}
	}
}

func environ() []string { return os.Environ() }
