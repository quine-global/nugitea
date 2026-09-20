package mirror

import (
	"context"
	"log"
	"sync"
	"time"

	"nugitea/internal/repo"
)

// tickInterval is how often the scheduler checks for due mirrors.
const tickInterval = 30 * time.Second

// Scheduler periodically syncs configured mirrors whose interval has elapsed.
type Scheduler struct {
	Repos   *repo.Store
	Mirrors *Store

	locksMu sync.Mutex
	locks   map[string]*sync.Mutex // per-repo+direction, prevents overlapping syncs
}

// NewScheduler returns a Scheduler for the given repo and mirror stores.
func NewScheduler(repos *repo.Store, mirrors *Store) *Scheduler {
	return &Scheduler{Repos: repos, Mirrors: mirrors, locks: map[string]*sync.Mutex{}}
}

// Run blocks, ticking until ctx is cancelled.
func (s *Scheduler) Run(ctx context.Context) {
	ticker := time.NewTicker(tickInterval)
	defer ticker.Stop()
	for {
		s.tick(ctx)
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}

func (s *Scheduler) tick(ctx context.Context) {
	entries, err := s.Mirrors.All()
	if err != nil {
		log.Printf("mirror scheduler: load mirrors: %v", err)
		return
	}
	now := time.Now()
	for _, e := range entries {
		due := e.LastSync.Add(time.Duration(e.IntervalSeconds) * time.Second)
		if now.Before(due) {
			continue
		}
		go s.sync(ctx, e)
	}
}

func (s *Scheduler) sync(ctx context.Context, e Entry) {
	lock := s.lockFor(e)
	if !lock.TryLock() {
		return // a sync for this mirror is already running
	}
	defer lock.Unlock()

	var err error
	switch e.Direction {
	case Pull:
		err = SyncPull(ctx, s.Repos, e)
	case Push:
		err = SyncPush(ctx, s.Repos, e)
	}
	if err != nil {
		log.Printf("mirror sync %s %s: %v", e.Direction, e.Repo, err)
	}
	if touchErr := s.Mirrors.touch(e, time.Now()); touchErr != nil {
		log.Printf("mirror sync %s %s: record last sync: %v", e.Direction, e.Repo, touchErr)
	}
}

func (s *Scheduler) lockFor(e Entry) *sync.Mutex {
	s.locksMu.Lock()
	defer s.locksMu.Unlock()
	key := e.key()
	if s.locks[key] == nil {
		s.locks[key] = &sync.Mutex{}
	}
	return s.locks[key]
}
