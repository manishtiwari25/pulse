// Package cred resolves the daemon's push credential (ADR-041): a reference the owner keeps in the
// `push.credential` setting - op:// (1Password, read as a service account), keychain:// (a macOS Keychain
// item) or file:// (a private file) - to an SSH private key that exists on disk only for the seconds of a
// push. The package stores no secret, prints none, and never runs inside a worker.
package cred

import (
	"errors"
	"fmt"
	"net/url"
	"path/filepath"
	"strings"
)

// Ref is a parsed push credential reference.
type Ref struct {
	Raw    string
	Scheme string // op, keychain, file

	// op://<vault>/<item>/<field>[?query]
	Vault, Item, Field, Query string
	// keychain://<service>/<account>
	Service, Account string
	// file://<absolute path>
	Path string
}

// Shapes lists the accepted reference shapes for error messages and help.
const Shapes = "op://VAULT/ITEM/FIELD[?ssh-format=openssh], keychain://SERVICE/ACCOUNT, or file:///ABSOLUTE/PATH"

// Parse validates a reference. An empty reference is an error: the caller decides what empty means.
func Parse(text string) (Ref, error) {
	raw := strings.TrimSpace(text)
	ref := Ref{Raw: raw}
	if raw == "" {
		return ref, errors.New("empty push credential reference")
	}
	if strings.ContainsAny(raw, "\n\r\t") {
		return ref, errors.New("a reference is one line")
	}
	scheme, rest, ok := strings.Cut(raw, "://")
	if !ok {
		return ref, fmt.Errorf("%q is not a reference; use %s", raw, Shapes)
	}
	ref.Scheme = strings.ToLower(scheme)
	switch ref.Scheme {
	case "op":
		path, query, _ := strings.Cut(rest, "?")
		parts := strings.Split(path, "/")
		if len(parts) < 3 || parts[0] == "" || parts[1] == "" || parts[len(parts)-1] == "" {
			return ref, fmt.Errorf("%q needs a vault, an item and a field: op://VAULT/ITEM/FIELD", raw)
		}
		ref.Vault, ref.Item, ref.Field = parts[0], parts[1], strings.Join(parts[2:], "/")
		if query != "" {
			if _, err := url.ParseQuery(query); err != nil {
				return ref, fmt.Errorf("%q has a malformed query: %v", raw, err)
			}
		}
		ref.Query = query
	case "keychain":
		service, account, ok := strings.Cut(rest, "/")
		if !ok || service == "" || account == "" || strings.Contains(account, "/") {
			return ref, fmt.Errorf("%q needs a service and an account: keychain://SERVICE/ACCOUNT", raw)
		}
		ref.Service, ref.Account = service, account
	case "file":
		if !filepath.IsAbs(rest) {
			return ref, fmt.Errorf("%q needs an absolute path: file:///ABSOLUTE/PATH", raw)
		}
		ref.Path = filepath.Clean(rest)
	default:
		return ref, fmt.Errorf("%q has an unknown scheme %q; use %s", raw, scheme, Shapes)
	}
	return ref, nil
}

// String returns the reference as the setting stores it.
func (r Ref) String() string { return r.Raw }
