// Command nugitea is a minimal git server: no users, no auth, no web UI —
// just smart-HTTP and SSH clone/push against bare repos, plus pull/push
// mirroring, implemented by shelling out to git the same way Gitea does.
package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"nugitea/internal/auth"
	"nugitea/internal/httpgit"
	"nugitea/internal/mirror"
	"nugitea/internal/repo"
	"nugitea/internal/sshgit"
)

func main() {
	if len(os.Args) < 2 {
		usage()
		os.Exit(1)
	}

	var err error
	switch os.Args[1] {
	case "serve":
		err = cmdServe(os.Args[2:])
	case "repo":
		err = cmdRepo(os.Args[2:])
	case "mirror":
		err = cmdMirror(os.Args[2:])
	case "hook":
		err = cmdHook(os.Args[2:])
	default:
		usage()
		os.Exit(1)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "nugitea:", err)
		os.Exit(1)
	}
}

func usage() {
	fmt.Fprintln(os.Stderr, `usage: nugitea <command> [args]

commands:
  serve                                 run the HTTP + SSH git server
  repo create <name>                    create a new bare repo
  repo list                             list repos
  mirror add-pull [--interval 5m] <repo> <url>   periodic fetch from a remote
  mirror add-push [--interval 5m] <repo> <url>   periodic mirror-push to a remote
  hook <pre-receive|update|post-receive>          internal, invoked by git hooks`)
}

func cmdServe(args []string) error {
	fs := flag.NewFlagSet("serve", flag.ExitOnError)
	httpAddr := fs.String("http", ":3080", "HTTP listen address")
	sshAddr := fs.String("ssh", ":2222", "SSH listen address")
	repoRoot := fs.String("repo-root", "./data", "directory containing bare repos")
	hostKeyPath := fs.String("ssh-host-key", "./ssh_host_ed25519", "path to SSH host key (created if missing)")
	if err := fs.Parse(args); err != nil {
		return err
	}

	repos, err := repo.NewStore(*repoRoot)
	if err != nil {
		return err
	}

	authz := auth.AllowAll{}
	mirrors := mirror.NewStore(repos.Root)
	scheduler := mirror.NewScheduler(repos, mirrors)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	go scheduler.Run(ctx)

	httpHandler := httpgit.New(repos, authz)
	httpServer := &http.Server{Addr: *httpAddr, Handler: httpHandler}
	go func() {
		log.Printf("http: listening on %s", *httpAddr)
		if err := httpServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Fatalf("http server: %v", err)
		}
	}()

	sshServer := sshgit.New(repos, authz, *sshAddr, *hostKeyPath)
	go func() {
		if err := sshServer.ListenAndServe(); err != nil {
			log.Fatalf("ssh server: %v", err)
		}
	}()

	<-ctx.Done()
	log.Println("shutting down")
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	return httpServer.Shutdown(shutdownCtx)
}

func cmdRepo(args []string) error {
	if len(args) < 1 {
		return fmt.Errorf("usage: nugitea repo <create|list> ...")
	}
	repos, err := repo.NewStore(repoRootFromEnv())
	if err != nil {
		return err
	}

	switch args[0] {
	case "create":
		if len(args) != 2 {
			return fmt.Errorf("usage: nugitea repo create <name>")
		}
		path, err := repos.InitBare(context.Background(), args[1])
		if err != nil {
			return err
		}
		fmt.Println(path)
		return nil
	case "list":
		names, err := repos.List()
		if err != nil {
			return err
		}
		for _, n := range names {
			fmt.Println(n)
		}
		return nil
	default:
		return fmt.Errorf("unknown repo subcommand %q", args[0])
	}
}

func cmdMirror(args []string) error {
	if len(args) < 1 {
		return fmt.Errorf("usage: nugitea mirror <add-pull|add-push> [--interval 5m] <repo> <url>")
	}
	sub := args[0]
	fs := flag.NewFlagSet("mirror "+sub, flag.ExitOnError)
	interval := fs.Duration("interval", 5*time.Minute, "sync interval")
	if err := fs.Parse(args[1:]); err != nil {
		return err
	}
	rest := fs.Args()
	if len(rest) != 2 {
		return fmt.Errorf("usage: nugitea mirror %s [--interval 5m] <repo> <url>", sub)
	}
	repoName, remoteURL := rest[0], rest[1]

	repos, err := repo.NewStore(repoRootFromEnv())
	if err != nil {
		return err
	}
	mirrors := mirror.NewStore(repos.Root)
	ctx := context.Background()

	switch sub {
	case "add-pull":
		return mirror.AddPull(ctx, repos, mirrors, repoName, remoteURL, *interval)
	case "add-push":
		return mirror.AddPush(ctx, repos, mirrors, repoName, remoteURL, *interval)
	default:
		return fmt.Errorf("unknown mirror subcommand %q", sub)
	}
}

// cmdHook implements the `nugitea hook <name>` entry point that installed
// git hook delegator scripts call back into. There is no policy to enforce
// yet (no protected branches, no webhooks) — this drains stdin so it plays
// nicely with git's hook protocol and exits 0, leaving a stub for future
// extension.
func cmdHook(args []string) error {
	if len(args) < 1 {
		return fmt.Errorf("usage: nugitea hook <pre-receive|update|post-receive>")
	}
	switch args[0] {
	case "pre-receive", "post-receive":
		_, _ = io.Copy(io.Discard, os.Stdin)
	case "update":
		// git invokes this as `update <refname> <oldrev> <newrev>`, no stdin
	default:
		return fmt.Errorf("unknown hook %q", args[0])
	}
	return nil
}

func repoRootFromEnv() string {
	if v := os.Getenv("NUGITEA_REPO_ROOT"); v != "" {
		return v
	}
	return "./data"
}
