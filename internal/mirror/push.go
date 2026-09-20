package mirror

import (
	"context"
	"fmt"
	"time"

	"nugitea/internal/gitcmd"
	"nugitea/internal/repo"
)

// AddPush configures repoName to periodically mirror-push to remoteURL,
// mirroring Gitea's push-mirror setup: `git remote add --mirror=push`.
func AddPush(ctx context.Context, repos *repo.Store, mirrors *Store, repoName, remoteURL string, interval time.Duration) error {
	repoPath, err := repos.Path(repoName)
	if err != nil {
		return err
	}
	if !repos.Exists(repoName) {
		return fmt.Errorf("repo %q does not exist", repoName)
	}

	// Ignore the error: fine if no remote existed yet.
	_ = gitcmd.New("remote", "remove", remoteName).WithDir(repoPath).Run(ctx)

	if err := gitcmd.New("remote", "add", "--mirror=push", remoteName, remoteURL).WithDir(repoPath).Run(ctx); err != nil {
		return fmt.Errorf("git remote add: %w", err)
	}

	return mirrors.Add(Entry{
		Repo:            repoName,
		RemoteURL:       remoteURL,
		Direction:       Push,
		IntervalSeconds: int64(interval.Seconds()),
	})
}

// SyncPush runs the actual mirror push, equivalent to Gitea's
// `git push --mirror -f <remote>` in services/mirror/mirror_push.go.
func SyncPush(ctx context.Context, repos *repo.Store, e Entry) error {
	repoPath, err := repos.Path(e.Repo)
	if err != nil {
		return err
	}
	return gitcmd.New("push", "--mirror", "-f", remoteName).WithDir(repoPath).Run(ctx)
}
