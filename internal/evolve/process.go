package evolve

import (
	"bytes"
	"context"
	"errors"
	"io"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/gitenv"
)

// commandWords is intentionally not a shell: quotes and escapes are accepted,
// but expansion and shell operators never execute while building harness argv.
func commandWords(text string) ([]string, error) {
	var words []string
	var word strings.Builder
	var quote rune
	escaped, active := false, false
	for _, r := range text {
		if escaped {
			word.WriteRune(r)
			escaped, active = false, true
			continue
		}
		if r == '\\' && quote != '\'' {
			escaped, active = true, true
			continue
		}
		if quote != 0 {
			if r == quote {
				quote = 0
			} else {
				word.WriteRune(r)
			}
			continue
		}
		switch r {
		case '\'', '"':
			quote, active = r, true
		case ' ', '\t', '\n':
			if active {
				words = append(words, word.String())
				word.Reset()
				active = false
			}
		case 0:
			return nil, errors.New("NUL in command")
		default:
			word.WriteRune(r)
			active = true
		}
	}
	if quote != 0 || escaped {
		return nil, errors.New("unclosed quote or escape in command")
	}
	if active {
		words = append(words, word.String())
	}
	if len(words) == 0 || words[0] == "" {
		return nil, errors.New("empty command")
	}
	return words, nil
}

func harnessArgs(command, request string) ([]string, error) {
	args, err := commandWords(command)
	if err != nil {
		return nil, inputError("Cannot start measurement harness", "runner_cmd is not a quoted executable and arguments", "pulse settings set runner_cmd copilot")
	}
	replaced := false
	for i := 1; i < len(args); i++ {
		if args[i] == "<request>" || args[i] == "{prompt}" {
			args[i], replaced = request, true
		}
	}
	if !replaced {
		for i, arg := range args {
			if arg == "-p" || arg == "--prompt" {
				if i != len(args)-1 {
					return nil, inputError("Cannot start measurement harness", "use {prompt} for an existing prompt argument in runner_cmd", "pulse settings set runner_cmd 'copilot -p {prompt} --allow-all-tools'")
				}
				args = append(args, request)
				replaced = true
				break
			}
		}
	}
	if !replaced {
		args = append(args, "-p", request)
	}
	return args, nil
}

func isolatedEnv(home string) []string {
	env := []string{}
	for _, entry := range os.Environ() {
		key, _, _ := strings.Cut(entry, "=")
		if strings.HasPrefix(key, "PULSE_") || strings.HasPrefix(key, "GIT_") {
			continue
		}
		env = append(env, entry)
	}
	// Preserve the harness's login environment, but never let its ordinary
	// pulse commands discover the real store. This is not an OS sandbox.
	return append(env, "PULSE_HOME="+home, "PULSE_JOB=measurement",
		"GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull,
		"GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0")
}

type boundedOutput struct {
	buf      bytes.Buffer
	limit    int
	overflow bool
}

func (b *boundedOutput) Write(p []byte) (int, error) {
	n := len(p)
	remaining := b.limit - b.buf.Len()
	if n > remaining {
		b.overflow = true
		p = p[:max(0, remaining)]
	}
	_, _ = b.buf.Write(p)
	return n, nil
}

func runProcess(ctx context.Context, cwd string, env, argv []string, stdout io.Writer) error {
	return runProcessTo(ctx, cwd, env, argv, stdout, io.Discard)
}

// runProcessTo is runProcess with the command's stderr kept as well; the
// harness and git never need it, a scorer's one-line reason for a 0 does.
func runProcessTo(ctx context.Context, cwd string, env, argv []string, stdout, stderr io.Writer) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	cmd := exec.CommandContext(ctx, argv[0], argv[1:]...)
	cmd.Dir, cmd.Env = cwd, env
	cmd.Stdout, cmd.Stderr = stdout, stderr
	cmd.WaitDelay = 2 * time.Second
	cleanup := containProcess(cmd)
	defer cleanup()
	err := cmd.Run()
	if ctx.Err() != nil {
		return ctx.Err()
	}
	return err
}

func gitOutput(ctx context.Context, repo string, args ...string) (string, error) {
	out, err := gitRaw(ctx, repo, args...)
	return strings.TrimSpace(out), err
}

// gitRaw is gitOutput without trimming, for -z output whose first byte may be
// a significant space.
func gitRaw(ctx context.Context, repo string, args ...string) (string, error) {
	out := &boundedOutput{limit: 8 << 20}
	argv := append([]string{"git", "-c", "core.hooksPath=" + os.DevNull}, args...)
	err := runProcess(ctx, repo, gitenv.Env(), argv, out)
	if out.overflow {
		return "", errors.New("git output exceeds the diagnostic limit")
	}
	return out.buf.String(), err
}
