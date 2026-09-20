// Package sshgit implements the SSH transport for git clone/push, mirroring
// the approach in Gitea's modules/ssh/ssh.go + cmd/serv.go: read the raw SSH
// command, validate the verb, and exec the matching git subcommand with
// stdio wired directly to the session. Unlike Gitea, nugitea execs git
// directly from the session handler instead of re-invoking its own binary,
// since there is no permission database to hop into.
package sshgit

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/pem"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"

	gliderssh "github.com/gliderlabs/ssh"
	gossh "golang.org/x/crypto/ssh"

	"nugitea/internal/auth"
	"nugitea/internal/gitcmd"
	"nugitea/internal/repo"
)

// Server serves git-upload-pack/git-receive-pack over SSH.
type Server struct {
	Store   *repo.Store
	Auth    auth.Authorizer
	Addr    string
	HostKey string // path to a PEM private key file, created if missing
}

// New returns a Server for the given repo store. If a is nil, AllowAll is used.
func New(store *repo.Store, a auth.Authorizer, addr, hostKeyPath string) *Server {
	if a == nil {
		a = auth.AllowAll{}
	}
	return &Server{Store: store, Auth: a, Addr: addr, HostKey: hostKeyPath}
}

// ListenAndServe starts the SSH server and blocks.
func (s *Server) ListenAndServe() error {
	signer, err := loadOrCreateHostKey(s.HostKey)
	if err != nil {
		return fmt.Errorf("ssh host key: %w", err)
	}

	srv := &gliderssh.Server{
		Addr: s.Addr,
		// No users/login: accept any offered key. This is a stub seam,
		// structured like auth.Authorizer, for real key-checking later.
		PublicKeyHandler: func(ctx gliderssh.Context, key gliderssh.PublicKey) bool { return true },
		Handler:          s.handleSession,
	}
	srv.AddHostKey(signer)

	log.Printf("ssh: listening on %s", s.Addr)
	return srv.ListenAndServe()
}

func (s *Server) handleSession(session gliderssh.Session) {
	cmdLine := session.RawCommand()
	verb, repoName, err := parseGitCommand(cmdLine)
	if err != nil {
		fmt.Fprintf(session.Stderr(), "nugitea: %v\n", err)
		session.Exit(1)
		return
	}

	if err := repo.ValidateName(repoName); err != nil {
		fmt.Fprintf(session.Stderr(), "nugitea: %v\n", err)
		session.Exit(1)
		return
	}
	if !s.Store.Exists(repoName) {
		fmt.Fprintf(session.Stderr(), "nugitea: repository %q not found\n", repoName)
		session.Exit(1)
		return
	}
	repoPath, err := s.Store.Path(repoName)
	if err != nil {
		fmt.Fprintf(session.Stderr(), "nugitea: %v\n", err)
		session.Exit(1)
		return
	}

	var allowed bool
	var gitArg string
	switch verb {
	case "git-upload-pack":
		allowed = s.Auth.AllowPull(repoName)
		gitArg = "upload-pack"
	case "git-receive-pack":
		allowed = s.Auth.AllowPush(repoName)
		gitArg = "receive-pack"
	}
	if !allowed {
		fmt.Fprintf(session.Stderr(), "nugitea: forbidden\n")
		session.Exit(1)
		return
	}

	ctx := session.Context()
	cmd := gitcmd.New(gitArg, repoPath).WithStdio(session, session, session.Stderr())
	if err := cmd.Run(ctx); err != nil {
		log.Printf("ssh %s %s: %v", repoName, verb, err)
		session.Exit(1)
		return
	}
	session.Exit(0)
}

// allowedVerbs are the only git-* commands nugitea will exec over SSH.
var allowedVerbs = map[string]bool{
	"git-upload-pack":  true,
	"git-receive-pack": true,
}

// parseGitCommand extracts the verb and repo name from a raw SSH command
// line such as `git-upload-pack '/demo.git'`.
func parseGitCommand(cmdLine string) (verb, repoName string, err error) {
	fields, err := shellSplit(cmdLine)
	if err != nil || len(fields) != 2 {
		return "", "", fmt.Errorf("unsupported command")
	}
	verb = fields[0]
	if !allowedVerbs[verb] {
		return "", "", fmt.Errorf("unsupported command %q", verb)
	}
	path := strings.Trim(fields[1], "'\"")
	path = strings.TrimPrefix(path, "/")
	path = strings.TrimSuffix(path, ".git")
	return verb, path, nil
}

func loadOrCreateHostKey(path string) (gossh.Signer, error) {
	if data, err := os.ReadFile(path); err == nil {
		return gossh.ParsePrivateKey(data)
	}

	_, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return nil, err
	}
	block, err := gossh.MarshalPrivateKey(priv, "nugitea host key")
	if err != nil {
		return nil, err
	}
	data := pem.EncodeToMemory(block)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, err
	}
	if err := os.WriteFile(path, data, 0o600); err != nil {
		return nil, err
	}
	return gossh.ParsePrivateKey(data)
}

// shellSplit does minimal POSIX-ish word splitting for the raw SSH command
// git clients send (e.g. `git-upload-pack '/demo.git'`), just enough to
// strip a single layer of matching quotes around the repo path.
func shellSplit(s string) ([]string, error) {
	var fields []string
	var cur strings.Builder
	var quote rune
	inField := false
	for _, r := range s {
		switch {
		case quote != 0:
			if r == quote {
				quote = 0
			} else {
				cur.WriteRune(r)
			}
		case r == '\'' || r == '"':
			quote = r
			inField = true
		case r == ' ' || r == '\t':
			if inField {
				fields = append(fields, cur.String())
				cur.Reset()
				inField = false
			}
		default:
			cur.WriteRune(r)
			inField = true
		}
	}
	if quote != 0 {
		return nil, fmt.Errorf("unterminated quote")
	}
	if inField {
		fields = append(fields, cur.String())
	}
	return fields, nil
}
