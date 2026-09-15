package hub

import (
	"context"
	"errors"
	"net/http"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/cred"
	"github.com/manishtiwari25/pulse/internal/cred/setup"
)

// The daemon's push key from the console (ADR-042): the hub is the owner's pen for it, as it is for the
// held-out fixtures. GET is the state and the providers, never a secret. Every POST needs a loopback Host
// and X-Pulse-Cred: yes, acts as human:dashboard and runs the same code as pulse cred (internal/cred/setup):
//
//	GET  /api/v1/cred                 state (reference, fingerprint, token source), providers, os, can_edit
//	POST /api/v1/cred/token {token}   keep a 1Password service-account token (Keychain / 0600 file)
//	POST /api/v1/cred/op/vaults {}    the vaults the service account sees
//	POST /api/v1/cred/op/items {vault}  the SSH Key items of one vault, each with its op:// reference
//	POST /api/v1/cred/set {reference, probe}  resolve, inspect, probe every upstream, save when they answer
//	POST /api/v1/cred/clear {token}   unset push.credential (and remove the token)
//	POST /api/v1/cred/generate {replace}  mint an ed25519 key at $PULSE_HOME/secrets/push_key
//
// A refusal answers 409 (the environment or the remote) or 400 (the input) with the CLI's what/why/fix and,
// when the key was read, its public key so the owner can add it as a deploy key.

const credHeader = "X-Pulse-Cred"

// credResolver builds the resolver; tests replace it with fake tools.
var credResolver = func(home string) *cred.Resolver { return cred.New(home) }

// credDeps are the setup seams; tests replace the probe and the keygen.
var credDeps = setup.Deps{}

func (h *Handler) additionCred(w http.ResponseWriter, r *http.Request, parts []string) {
	sub := strings.Join(parts[1:], "/")
	if sub == "" {
		if !additionMethod(w, r, http.MethodGet) || !h.additionAvailable(w, r) {
			return
		}
		h.credState(w, r)
		return
	}
	switch sub {
	case "token", "op/vaults", "op/items", "set", "clear", "generate":
	default:
		additionNotFound(w)
		return
	}
	if !additionMethod(w, r, http.MethodPost) || !h.additionAvailable(w, r) {
		return
	}
	if !loopbackHost(r) {
		writeError(w, http.StatusForbidden, "The push key is set only on loopback", "The request named a host other than this machine.", "Open the hub on 127.0.0.1.")
		return
	}
	if r.Header.Get(credHeader) != "yes" {
		writeError(w, http.StatusForbidden, "missing "+credHeader+": yes", "An explicit confirmation header is required.", "Confirm the change in the hub.")
		return
	}
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	resolver := credResolver(h.Store.Home)
	ctx := r.Context()
	switch sub {
	case "token":
		token, ok := additionString(w, payload, "token", true)
		if !ok {
			return
		}
		additionWrites.Lock()
		defer additionWrites.Unlock()
		where, err := resolver.StoreToken(ctx, token)
		if err != nil {
			writeError(w, http.StatusBadRequest, "Token not stored", cred.Redact(err.Error(), token), "Copy the token from 1Password > Developer > Service Accounts and paste it on one line.")
			return
		}
		writeJSON(w, http.StatusOK, object("stored_in report", where, setup.Status(ctx, h.Store, resolver)))
	case "op/vaults":
		vaults, err := setup.Vaults(ctx, resolver)
		if err != nil {
			credFailure(w, err, "Add a service-account token first (pulse cred token or the Connect step), and check op is installed.")
			return
		}
		writeJSON(w, http.StatusOK, object("vaults", vaults))
	case "op/items":
		vault, ok := additionString(w, payload, "vault", true)
		if !ok {
			return
		}
		items, err := setup.Items(ctx, resolver, vault)
		if err != nil {
			credFailure(w, err, "Pick a vault the service account can read.")
			return
		}
		writeJSON(w, http.StatusOK, object("vault items", vault, items))
	case "set":
		reference, ok := additionString(w, payload, "reference", true)
		if !ok {
			return
		}
		probe, ok := additionFlag(w, payload, "probe", true)
		if !ok {
			return
		}
		additionWrites.Lock()
		defer additionWrites.Unlock()
		res, err := setup.Apply(ctx, h.Store, resolver, "human:dashboard", reference, probe, credDeps)
		if err != nil {
			credFailure(w, err, "Fix what the message names and try again; nothing was saved.")
			return
		}
		writeJSON(w, http.StatusOK, object("result report", res, setup.Status(ctx, h.Store, resolver)))
	case "clear":
		token, ok := additionFlag(w, payload, "token", false)
		if !ok {
			return
		}
		additionWrites.Lock()
		defer additionWrites.Unlock()
		cleared, err := setup.Clear(ctx, h.Store, resolver, "human:dashboard", token)
		if err != nil {
			credFailure(w, err, "Check the store and try again.")
			return
		}
		writeJSON(w, http.StatusOK, object("cleared report", cleared, setup.Status(ctx, h.Store, resolver)))
	case "generate":
		replace, ok := additionFlag(w, payload, "replace", false)
		if !ok {
			return
		}
		additionWrites.Lock()
		defer additionWrites.Unlock()
		gen, err := setup.Generate(ctx, resolver, replace, credDeps)
		if err != nil {
			credFailure(w, err, "Install OpenSSH's ssh-keygen, check PULSE_HOME is writable, and try again.")
			return
		}
		writeJSON(w, http.StatusOK, object("generated", gen))
	}
}

func (h *Handler) credState(w http.ResponseWriter, r *http.Request) {
	resolver := credResolver(h.Store.Home)
	ctx, cancel := context.WithTimeout(r.Context(), resolverTimeout)
	defer cancel()
	writeJSON(w, http.StatusOK, object("available can_edit os report providers shapes", true, true, resolver.OS(), setup.Status(ctx, h.Store, resolver), setup.Providers(resolver), cred.Shapes))
}

// resolverTimeout bounds the GET: a Keychain read that hangs must not hang the page.
const resolverTimeout = 15 * time.Second

// credFailure answers a setup refusal with its words and public key, anything else as a conflict.
func credFailure(w http.ResponseWriter, err error, fix string) {
	var refusal *setup.Refusal
	if errors.As(err, &refusal) {
		code := http.StatusConflict
		if refusal.Code == 2 {
			code = http.StatusBadRequest
		}
		body := map[string]any{"error": refusal.What, "why": refusal.Why, "fix": refusal.Fix}
		if refusal.PublicKey != "" {
			body["public_key"], body["fingerprint"] = refusal.PublicKey, refusal.Fingerprint
		}
		if len(refusal.Probes) > 0 {
			body["probes"] = refusal.Probes
		}
		writeJSON(w, code, body)
		return
	}
	if errors.Is(err, cred.ErrWorker) {
		writeError(w, http.StatusForbidden, "Only the owner sets the push key", err.Error(), "Open the hub yourself; agents read the state and suggest.")
		return
	}
	writeError(w, http.StatusConflict, cred.Redact(err.Error()), "The step did not complete.", fix)
}

// additionFlag reads an optional boolean with a default.
func additionFlag(w http.ResponseWriter, payload map[string]any, key string, dflt bool) (bool, bool) {
	value, present := payload[key]
	if !present {
		return dflt, true
	}
	b, ok := value.(bool)
	if !ok {
		writeError(w, http.StatusBadRequest, "Invalid "+key, key+" must be true or false.", "Send "+key+":true or "+key+":false.")
		return false, false
	}
	return b, true
}
