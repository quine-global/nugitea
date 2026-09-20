// Package repo manages the on-disk layout of bare repositories: a flat
// namespace of <root>/<name>.git directories (no users/orgs, since nugitea
// has none), mirroring the bare-repo shape Gitea itself creates via
// `git init --bare`.
package repo

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
)

// validName matches safe repo names: letters, digits, dash, underscore, dot,
// not starting with a dot, no path separators. This is the only defense
// against path traversal since there is no auth layer to rely on.
var validName = regexp.MustCompile(`^[A-Za-z0-9_][A-Za-z0-9_.-]*$`)

// Store resolves repo names to on-disk paths under a root directory.
type Store struct {
	Root string
}

// NewStore returns a Store rooted at root, creating the directory if needed.
func NewStore(root string) (*Store, error) {
	abs, err := filepath.Abs(root)
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(abs, 0o755); err != nil {
		return nil, fmt.Errorf("create repo root: %w", err)
	}
	return &Store{Root: abs}, nil
}

// ValidateName reports whether name is safe to use as a repo name.
func ValidateName(name string) error {
	if !validName.MatchString(name) {
		return fmt.Errorf("invalid repo name %q", name)
	}
	return nil
}

// Path returns the absolute path to the bare repo directory for name
// (without requiring it to exist).
func (s *Store) Path(name string) (string, error) {
	if err := ValidateName(name); err != nil {
		return "", err
	}
	return filepath.Join(s.Root, name+".git"), nil
}

// Exists reports whether the named repo exists on disk.
func (s *Store) Exists(name string) bool {
	path, err := s.Path(name)
	if err != nil {
		return false
	}
	info, err := os.Stat(path)
	return err == nil && info.IsDir()
}

// List returns the names of all repos under the root.
func (s *Store) List() ([]string, error) {
	entries, err := os.ReadDir(s.Root)
	if err != nil {
		return nil, err
	}
	var names []string
	for _, e := range entries {
		if e.IsDir() && filepath.Ext(e.Name()) == ".git" {
			names = append(names, e.Name()[:len(e.Name())-len(".git")])
		}
	}
	return names, nil
}
