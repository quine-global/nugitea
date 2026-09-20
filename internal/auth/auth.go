// Package auth defines the extension seam for access control. nugitea ships
// with no users or login, so the only implementation today is AllowAll, but
// the httpgit and sshgit transports depend on this interface rather than a
// concrete decision so real auth can be added later without touching them.
package auth

// Authorizer decides whether an operation on a repo is permitted.
type Authorizer interface {
	AllowPull(repo string) bool
	AllowPush(repo string) bool
}

// AllowAll permits every pull and push. It is the default Authorizer.
type AllowAll struct{}

func (AllowAll) AllowPull(string) bool { return true }
func (AllowAll) AllowPush(string) bool { return true }
