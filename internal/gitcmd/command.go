// Package gitcmd wraps invocation of the git subprocess, mirroring the
// pattern Gitea itself uses in modules/git/gitcmd: a small builder around
// os/exec with a fixed baseline environment and context-based cancellation.
package gitcmd

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
)

// Command builds a single git invocation.
type Command struct {
	args   []string
	dir    string
	env    []string
	stdin  io.Reader
	stdout io.Writer
	stderr io.Writer
}

// New starts a git command with the given arguments (e.g. "upload-pack",
// "--stateless-rpc", ".").
func New(args ...string) *Command {
	return &Command{args: args}
}

// WithDir sets the working directory the command runs in (the bare repo path).
func (c *Command) WithDir(dir string) *Command {
	c.dir = dir
	return c
}

// WithEnv sets the environment passed to the subprocess. The baseline env
// (see baseEnv) is always appended on top of this in Run.
func (c *Command) WithEnv(env []string) *Command {
	c.env = env
	return c
}

// WithStdio wires the subprocess's stdin/stdout/stderr.
func (c *Command) WithStdio(stdin io.Reader, stdout, stderr io.Writer) *Command {
	c.stdin = stdin
	c.stdout = stdout
	c.stderr = stderr
	return c
}

// baseEnv returns the environment variables Gitea always sets for git
// subprocesses, to keep behavior deterministic and free of surprise
// user/system config interference.
func baseEnv() []string {
	home, _ := os.UserHomeDir()
	return []string{
		"HOME=" + home,
		"GIT_CONFIG_NOSYSTEM=1",
		"GIT_TERMINAL_PROMPT=0",
		"LC_ALL=C",
	}
}

// Run executes the git command, waiting for it to complete or ctx to be
// cancelled.
func (c *Command) Run(ctx context.Context) error {
	cmd := exec.CommandContext(ctx, "git", c.args...)
	cmd.Dir = c.dir
	cmd.Stdin = c.stdin
	cmd.Stdout = c.stdout
	cmd.Stderr = c.stderr

	env := os.Environ()
	if c.env != nil {
		env = c.env
	}
	cmd.Env = append(env, baseEnv()...)

	if err := cmd.Run(); err != nil {
		return fmt.Errorf("git %v: %w", c.args, err)
	}
	return nil
}

// RunStdString runs the command and returns its combined stdout as a string,
// with stderr included in the error on failure.
func RunStdString(ctx context.Context, dir string, args ...string) (string, error) {
	var stdout, stderr strings.Builder
	err := New(args...).WithDir(dir).WithStdio(nil, &stdout, &stderr).Run(ctx)
	if err != nil {
		return "", fmt.Errorf("%w: %s", err, stderr.String())
	}
	return stdout.String(), nil
}
