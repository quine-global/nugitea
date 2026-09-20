package mirror

import (
	"context"
	"fmt"
	"time"

	"nugitea/internal/gitcmd"
	"nugitea/internal/repo"
)

// AddPull configures repoName to periodically fetch from remoteURL,
// mirroring Gitea's pull-mirror setup: `git remote add --mirror=fetch`.
func AddPull(ctx context.Context, repos *repo.Store, mirrors *Store, repoName, remoteURL string, interval time.Duration) error {
	repoPath, err := repos.Path(repoName)
	if err != nil {
		return err
	}
	if !repos.Exists(repoName) {
		return fmt.Errorf("repo %q does not exist", repoName)
	}

	// Ignore the error: fine if no remote existed yet.
	_ = gitcmd.New("remote", "remove", remoteName).WithDir(repoPath).Run(ctx)

	if err := gitcmd.New("remote", "add", "--mirror=fetch", remoteName, remoteURL).WithDir(repoPath).Run(ctx); err != nil {
		return fmt.Errorf("git remote add: %w", err)
	}

	return mirrors.Add(Entry{
		Repo:            repoName,
		RemoteURL:       remoteURL,
		Direction:       Pull,
		IntervalSeconds: int64(interval.Seconds()),
	})
}

// SyncPull runs the actual fetch for a configured pull mirror, equivalent to
// Gitea's `git fetch --prune --tags <remote>` in services/mirror/mirror_pull.go.
func SyncPull(ctx context.Context, repos *repo.Store, e Entry) error {
	repoPath, err := repos.Path(e.Repo)
	if err != nil {
		return err
	}
	return gitcmd.New("fetch", "--prune", "--tags", remoteName).WithDir(repoPath).Run(ctx)
}
