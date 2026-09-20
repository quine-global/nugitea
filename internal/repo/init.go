package repo

import (
	"context"
	"fmt"
	"os"

	"nugitea/internal/gitcmd"
)

// InitBare creates a new bare repository named name under the store's root
// (git init --bare, then hook installation), mirroring Gitea's
// git.InitRepository + gitrepo.CreateDelegateHooks flow.
func (s *Store) InitBare(ctx context.Context, name string) (string, error) {
	path, err := s.Path(name)
	if err != nil {
		return "", err
	}
	if _, err := os.Stat(path); err == nil {
		return "", fmt.Errorf("repo %q already exists", name)
	}
	if err := os.MkdirAll(path, 0o755); err != nil {
		return "", fmt.Errorf("create repo dir: %w", err)
	}

	if err := gitcmd.New("init", "--bare", ".").WithDir(path).Run(ctx); err != nil {
		return "", fmt.Errorf("git init --bare: %w", err)
	}

	nugiteaBin, err := os.Executable()
	if err != nil {
		return "", fmt.Errorf("resolve nugitea binary path: %w", err)
	}
	if err := installHooks(path, nugiteaBin); err != nil {
		return "", err
	}

	return path, nil
}
