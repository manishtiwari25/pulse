package harness

import (
	"bytes"
	"context"
	"errors"
	"os"
	"os/exec"
	"runtime"
	"syscall"
	"time"
)

type LocalEnv struct {
	cwd string
	env []string
}

func NewLocalEnv(cwd string, env []string) *LocalEnv {
	if env == nil {
		env = os.Environ()
	}
	return &LocalEnv{cwd: cwd, env: append([]string(nil), env...)}
}

func (e *LocalEnv) Execute(ctx context.Context, command string, timeout time.Duration) Observation {
	if ctx == nil {
		ctx = context.Background()
	}
	start := time.Now()
	cmd := exec.Command("/bin/sh", "-c", command)
	cmd.Dir = e.cwd
	cmd.Env = append([]string(nil), e.env...)
	cmd.Stdin = nil
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	var buf bytes.Buffer
	cmd.Stdout = &buf
	cmd.Stderr = &buf
	if err := cmd.Start(); err != nil {
		return Observation{Output: err.Error(), ExitCode: 127, Duration: time.Since(start)}
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	var timer <-chan time.Time
	if timeout > 0 {
		t := time.NewTimer(timeout)
		defer t.Stop()
		timer = t.C
	}
	select {
	case err := <-done:
		return Observation{Output: buf.String(), ExitCode: exitCode(err), Duration: time.Since(start)}
	case <-timer:
		killProcessGroup(cmd.Process.Pid, done)
		return Observation{Output: buf.String(), ExitCode: -1, TimedOut: true, Duration: time.Since(start)}
	case <-ctx.Done():
		killProcessGroup(cmd.Process.Pid, done)
		return Observation{Output: buf.String(), ExitCode: -1, TimedOut: true, Duration: time.Since(start)}
	}
}

func (e *LocalEnv) Info() map[string]string {
	return map[string]string{
		"cwd":   e.cwd,
		"os":    runtime.GOOS,
		"shell": "/bin/sh",
	}
}

func exitCode(err error) int {
	if err == nil {
		return 0
	}
	var ee *exec.ExitError
	if errors.As(err, &ee) {
		code := ee.ExitCode()
		if code < 0 && ee.ProcessState != nil {
			if ws, ok := ee.ProcessState.Sys().(syscall.WaitStatus); ok && ws.Signaled() {
				return 128 + int(ws.Signal())
			}
		}
		return code
	}
	return 127
}

func killProcessGroup(pid int, done <-chan error) {
	_ = syscall.Kill(-pid, syscall.SIGTERM)
	select {
	case <-done:
		return
	case <-time.After(100 * time.Millisecond):
	}
	_ = syscall.Kill(-pid, syscall.SIGKILL)
	select {
	case <-done:
	case <-time.After(2 * time.Second):
	}
}
