// Package httpgit implements the git smart-HTTP protocol (info/refs
// discovery, git-upload-pack, git-receive-pack), mirroring the subprocess
// invocation Gitea uses in routers/web/repo/githttp.go: git run with
// --stateless-rpc, stdin/stdout wired directly to the HTTP request/response.
package httpgit

import (
	"compress/gzip"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"regexp"
	"strings"

	"nugitea/internal/auth"
	"nugitea/internal/gitcmd"
	"nugitea/internal/repo"
)

// Handler serves the smart-HTTP git protocol under a single mount point,
// expecting paths of the form /{repo}.git/info/refs, /{repo}.git/git-upload-pack,
// /{repo}.git/git-receive-pack.
type Handler struct {
	Store *repo.Store
	Auth  auth.Authorizer
}

// New returns a Handler for the given repo store. If a is nil, AllowAll is used.
func New(store *repo.Store, a auth.Authorizer) *Handler {
	if a == nil {
		a = auth.AllowAll{}
	}
	return &Handler{Store: store, Auth: a}
}

var pathRE = regexp.MustCompile(`^/([A-Za-z0-9_][A-Za-z0-9_.-]*)\.git/(info/refs|git-upload-pack|git-receive-pack)$`)

// safeProtocolHeader matches the values git actually sends for Git-Protocol,
// e.g. "version=2", to avoid forwarding arbitrary header content into the
// subprocess environment.
var safeProtocolHeader = regexp.MustCompile(`^[0-9a-zA-Z=;,._-]+$`)

func (h *Handler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	m := pathRE.FindStringSubmatch(r.URL.Path)
	if m == nil {
		http.NotFound(w, r)
		return
	}
	repoName, endpoint := m[1], m[2]

	if err := repo.ValidateName(repoName); err != nil {
		http.NotFound(w, r)
		return
	}
	if !h.Store.Exists(repoName) {
		http.NotFound(w, r)
		return
	}
	repoPath, err := h.Store.Path(repoName)
	if err != nil {
		http.NotFound(w, r)
		return
	}

	switch endpoint {
	case "info/refs":
		h.infoRefs(w, r, repoName, repoPath)
	case "git-upload-pack":
		h.service(w, r, repoName, repoPath, "upload-pack", h.Auth.AllowPull)
	case "git-receive-pack":
		h.service(w, r, repoName, repoPath, "receive-pack", h.Auth.AllowPush)
	}
}

func (h *Handler) infoRefs(w http.ResponseWriter, r *http.Request, repoName, repoPath string) {
	service := strings.TrimPrefix(r.URL.Query().Get("service"), "git-")
	var allowed func(string) bool
	switch service {
	case "upload-pack":
		allowed = h.Auth.AllowPull
	case "receive-pack":
		allowed = h.Auth.AllowPush
	default:
		http.Error(w, "smart HTTP only: service must be git-upload-pack or git-receive-pack", http.StatusBadRequest)
		return
	}
	if !allowed(repoName) {
		http.Error(w, "forbidden", http.StatusForbidden)
		return
	}

	var stdout strings.Builder
	var stderr strings.Builder
	cmd := gitcmd.New(service, "--stateless-rpc", "--advertise-refs", ".").
		WithDir(repoPath).
		WithStdio(nil, &stdout, &stderr)
	if err := cmd.Run(r.Context()); err != nil {
		log.Printf("info/refs %s %s: %v: %s", repoName, service, err, stderr.String())
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", fmt.Sprintf("application/x-git-%s-advertisement", service))
	w.Header().Set("Cache-Control", "no-cache")
	w.WriteHeader(http.StatusOK)

	pkt := fmt.Sprintf("# service=git-%s\n", service)
	fmt.Fprintf(w, "%04x%s0000", len(pkt)+4, pkt)
	io.WriteString(w, stdout.String())
}

func (h *Handler) service(w http.ResponseWriter, r *http.Request, repoName, repoPath, service string, allowed func(string) bool) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	if !allowed(repoName) {
		http.Error(w, "forbidden", http.StatusForbidden)
		return
	}

	body := r.Body
	if r.Header.Get("Content-Encoding") == "gzip" {
		gz, err := gzip.NewReader(r.Body)
		if err != nil {
			http.Error(w, "bad gzip body", http.StatusBadRequest)
			return
		}
		defer gz.Close()
		body = gz
	}

	env := buildEnv(r)

	w.Header().Set("Content-Type", fmt.Sprintf("application/x-git-%s-result", service))
	w.Header().Set("Cache-Control", "no-cache")
	w.WriteHeader(http.StatusOK)

	var stderr strings.Builder
	cmd := gitcmd.New(service, "--stateless-rpc", ".").
		WithDir(repoPath).
		WithEnv(env).
		WithStdio(body, w, &stderr)
	if err := cmd.Run(r.Context()); err != nil {
		log.Printf("%s %s: %v: %s", repoName, service, err, stderr.String())
	}
}

// buildEnv forwards the client's requested git wire protocol version, same
// as Gitea's handling of the Git-Protocol header in routers/web/repo/githttp.go.
func buildEnv(r *http.Request) []string {
	env := os.Environ()
	if protocol := r.Header.Get("Git-Protocol"); protocol != "" && safeProtocolHeader.MatchString(protocol) {
		env = append(env, "GIT_PROTOCOL="+protocol)
	}
	return env
}
