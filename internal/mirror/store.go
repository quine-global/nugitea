// Package mirror implements pull/push mirroring: periodically running
// `git fetch --prune --tags <remote>` (pull) or `git push --mirror -f
// <remote>` (push) against a remote configured via `git remote add
// --mirror=fetch|push`, mirroring services/mirror/mirror_{pull,push}.go.
package mirror

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// Direction is which way a mirror syncs.
type Direction string

const (
	Pull Direction = "pull"
	Push Direction = "push"
)

// remoteName is the git remote name nugitea uses for every configured
// mirror; each repo has at most one mirror per direction.
const remoteName = "mirror"

// Entry is one configured mirror.
type Entry struct {
	Repo            string    `json:"repo"`
	RemoteURL       string    `json:"remote_url"`
	Direction       Direction `json:"direction"`
	IntervalSeconds int64     `json:"interval_seconds"`
	LastSync        time.Time `json:"last_sync"`
}

func (e *Entry) key() string { return string(e.Direction) + ":" + e.Repo }

// Store persists mirror configuration to a JSON file under the repo root.
type Store struct {
	path string
	mu   sync.Mutex
}

// NewStore returns a Store backed by mirrors.json under root.
func NewStore(root string) *Store {
	return &Store{path: filepath.Join(root, "mirrors.json")}
}

func (s *Store) load() ([]Entry, error) {
	data, err := os.ReadFile(s.path)
	if os.IsNotExist(err) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	var entries []Entry
	if err := json.Unmarshal(data, &entries); err != nil {
		return nil, err
	}
	return entries, nil
}

func (s *Store) save(entries []Entry) error {
	data, err := json.MarshalIndent(entries, "", "  ")
	if err != nil {
		return err
	}
	tmp := s.path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, s.path)
}

// Add registers a new mirror, replacing any existing one with the same
// repo+direction.
func (s *Store) Add(e Entry) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	entries, err := s.load()
	if err != nil {
		return err
	}
	out := entries[:0]
	for _, existing := range entries {
		if existing.key() != e.key() {
			out = append(out, existing)
		}
	}
	out = append(out, e)
	return s.save(out)
}

// All returns every configured mirror.
func (s *Store) All() ([]Entry, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.load()
}

// touch updates LastSync for the mirror matching key to now.
func (s *Store) touch(e Entry, when time.Time) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	entries, err := s.load()
	if err != nil {
		return err
	}
	found := false
	for i := range entries {
		if entries[i].key() == e.key() {
			entries[i].LastSync = when
			found = true
		}
	}
	if !found {
		return fmt.Errorf("mirror %s not found", e.key())
	}
	return s.save(entries)
}
