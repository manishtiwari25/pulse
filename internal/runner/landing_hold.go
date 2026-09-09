package runner

import (
	"encoding/json"
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// landingHeld says whether a landing verdict stops the sweep and holds every later branch of the project:
// the checkout is off its default branch or has uncommitted changes, another landing has the lock, the
// folder cannot be read, or the runner is stopped (ADR-024). Such a hold repeats every tick until the owner
// acts, so the daemon keeps the why where the owner looks - pulse status and the hub read meta.landing -
// and logs and emits it once per change rather than once per tick, as the planner does for "nothing to
// plan". Verdicts that are not holds (landed, missing, conflict, refused, dropped, ...) are each one action
// and stay one log line and one event apiece.
func landingHeld(state string) bool {
	switch state {
	case "deferred", "locked", "stopped", "unreadable":
		return true
	}
	return false
}

// tendLandings lands every ready project's waiting branches, oldest first, and returns the daemon's log
// lines together with the hold per project after the sweep. prev is the previous tick's answer: a hold that
// starts or changes is logged and written as a land.held event; a swept project whose hold has cleared is
// logged and written as land.resumed. A project not in projects this tick (its checkout did not answer the
// probe) keeps its previous hold, since nothing about it was learned.
func tendLandings(s *store.Store, projects []row, actor, now string, prev map[string]string) (lines []string, cur map[string]string) {
	cur = map[string]string{}
	swept := map[string]bool{}
	waiting := map[string]int{}
	states := map[string]string{}
	for _, proj := range projects {
		slug := proj.str("slug")
		swept[slug] = true
		landed := false
		for _, lr := range landPending(s, proj, actor, now, true, 0, false) {
			if lr.State == "landed" {
				landed = true
			}
			if !landingHeld(lr.State) {
				lines = append(lines, fmt.Sprintf("landing %s: %s", lr.State, lr.Note))
				continue
			}
			n := len(pendingLandings(s.DB, slug))
			noun := "branches"
			if n == 1 {
				noun = "branch"
			}
			waiting[slug], states[slug] = n, lr.State
			cur[slug] = fmt.Sprintf("%d %s waiting; %s: %s", n, noun, lr.State, lr.Note)
		}
		// ADR-037: what landed goes to the remote when the owner said so; a push that did not happen is a
		// note beside the landing hold, said once per change and retried next tick.
		line, note := pushLanded(s, proj, actor, now, landed)
		if line != "" {
			lines = append(lines, line)
		}
		if note != "" {
			if cur[slug] != "" {
				cur[slug] += "; " + note
			} else {
				cur[slug] = note
			}
			if states[slug] == "" {
				states[slug] = "push-failed"
			}
		}
	}
	for slug, note := range prev {
		if !swept[slug] {
			cur[slug] = note
		}
	}
	for _, slug := range sortedKeys(cur) {
		if !swept[slug] || prev[slug] == cur[slug] {
			continue
		}
		lines = append(lines, fmt.Sprintf("landing held %s: %s", slug, cur[slug]))
		eventAt(s.DB, now, actor, "land.held", slug, map[string]any{"project": slug, "waiting": waiting[slug], "state": states[slug], "note": cur[slug]})
	}
	for _, slug := range sortedKeys(prev) {
		if _, still := cur[slug]; still || !swept[slug] {
			continue
		}
		lines = append(lines, fmt.Sprintf("landing resumed %s", slug))
		eventAt(s.DB, now, actor, "land.resumed", slug, map[string]any{"project": slug, "was": prev[slug]})
	}
	recordLandingHolds(s, cur, now)
	return lines, cur
}

// recordLandingHolds writes the holds to meta.landing as {slug: {at, note}}, the shape pulse status and the
// hub read for the planner's idle notes; an empty map clears it.
func recordLandingHolds(s *store.Store, holds map[string]string, now string) {
	type note struct {
		At   string `json:"at"`
		Note string `json:"note"`
	}
	out := map[string]note{}
	for slug, text := range holds {
		out[slug] = note{At: now, Note: text}
	}
	data, _ := json.Marshal(out)
	exec1(s.DB, "INSERT INTO meta (key, value) VALUES ('landing', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", string(data))
}

// pushOn says whether the owner has asked for landed work to go to the remote (ADR-037 D1).
func pushOn(s *store.Store) bool { return s.Setting("landing.push", "off") == "main" }

// pushOwed says whether a landing of the project is newer than its last push (ADR-037 D4, D6): the push
// that failed, or that no one made because the setting was off, is still owed.
func pushOwed(s *store.Store, slug string) bool {
	r := mustOne(s.DB, `SELECT COALESCE((SELECT MAX(seq) FROM events WHERE action = 'land.landed' AND json_extract(detail, '$.project') = ?), 0) AS landed,
		COALESCE((SELECT MAX(seq) FROM events WHERE action = 'land.pushed' AND subject = ?), 0) AS pushed`, slug, slug)
	return r != nil && r.int("landed") > r.int("pushed")
}

// pushLanded pushes the project's default branch to its upstream when ADR-037 asks for it: after a sweep
// that landed something (landedNow), or while an earlier landing is still unpushed. Fast-forward only - a
// plain push of the one branch, never force - with the credentials git already has on this machine and no
// prompt. It returns the log line for a push that happened (empty otherwise) and the hold note for a push
// that was owed and did not happen (empty on success or when nothing was owed); the note never undoes the
// landing and is retried on the next tick.
func pushLanded(s *store.Store, project row, actor, now string, landedNow bool) (line, note string) {
	if !pushOn(s) {
		return "", ""
	}
	slug, root := project.str("slug"), project.str("path")
	def := project.str("default_branch")
	if def == "" {
		def = "main"
	}
	if !landedNow && !pushOwed(s, slug) {
		return "", ""
	}
	up := strings.Fields(gitTimeout(root, 10*time.Second, "for-each-ref", "--format=%(upstream:remotename) %(upstream:remoteref)", "refs/heads/"+def).Stdout)
	if len(up) != 2 {
		return "", fmt.Sprintf("landed but not pushed; push skipped: %s has no upstream (git branch --set-upstream-to=origin/%s %s)", def, def, def)
	}
	remote, remoteRef := up[0], up[1]
	remoteBranch := strings.TrimPrefix(remoteRef, "refs/heads/")
	upstream := "refs/remotes/" + remote + "/" + remoteBranch
	n, _ := strconv.Atoi(strings.TrimSpace(gitTimeout(root, 30*time.Second, "rev-list", "--count", upstream+"..refs/heads/"+def).Stdout))
	if n == 0 {
		return "", ""
	}
	from := strings.TrimSpace(gitTimeout(root, 10*time.Second, "rev-parse", upstream).Stdout)
	to := strings.TrimSpace(gitTimeout(root, 10*time.Second, "rev-parse", "refs/heads/"+def).Stdout)
	res := gitTimeoutEnv(root, 180*time.Second, pushEnv(root), "push", remote, "refs/heads/"+def+":"+remoteRef)
	if !res.ok() {
		why := squeeze(res.Stderr+" "+res.Stdout, 300)
		if res.Err != nil {
			why = errMessage(res.Err)
		}
		return "", fmt.Sprintf("%d %s landed but not pushed; push failed: %s", n, plural(n, "commit"), why)
	}
	eventAt(s.DB, now, actor, "land.pushed", slug, map[string]any{"project": slug, "remote": remote, "branch": def, "from": from, "to": to, "commits": n})
	return fmt.Sprintf("landing pushed %s: %s -> %s/%s (%d %s, %s..%s)", slug, def, remote, remoteBranch, n, plural(n, "commit"), short(from, 12), short(to, 12)), ""
}

// pushEnv keeps a push from waiting on a prompt the daemon cannot answer: ssh in batch mode when the owner
// has not chosen an ssh command of their own (GIT_SSH_COMMAND or core.sshCommand are left alone).
func pushEnv(root string) []string {
	if os.Getenv("GIT_SSH_COMMAND") != "" || strings.TrimSpace(gitTimeout(root, 10*time.Second, "config", "--get", "core.sshCommand").Stdout) != "" {
		return nil
	}
	return []string{"GIT_SSH_COMMAND=ssh -oBatchMode=yes"}
}
