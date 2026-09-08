package runner

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

const decisionValidDays = 7

type decisionHit struct {
	PostID, Answer, DecidedAt string
	Valid                     bool
}

// authorizingDecision: the newest human decision on a proposal whose body carries the exact line `marker`.
func authorizingDecision(q querier, marker, now string) *decisionHit {
	pattern := regexp.MustCompile(`(?m)^[ \t]*` + regexp.QuoteMeta(marker) + `[ \t]*$`)
	rows := mustRows(q, "SELECT d.post_id, d.answer, d.decided_at, p.body FROM decisions d JOIN posts p ON p.id = d.post_id "+
		"WHERE d.decided_by = ? ORDER BY d.decided_at DESC, d.post_id DESC", humanAgent)
	for _, r := range rows {
		if !pattern.MatchString(r.str("body")) {
			continue
		}
		age := mustISO(now).Sub(mustISO(r.str("decided_at")))
		valid := r.str("answer") == "yes" && age >= 0 && age.Hours() <= float64(decisionValidDays*24)
		return &decisionHit{r.str("post_id"), r.str("answer"), r.str("decided_at"), valid}
	}
	return nil
}

// agentGate: who may run `pair` and `onboard` (ADR-021 section 3).
func agentGate(q querier, marker, actor, now string) (string, error) {
	if actor == "" || actor == humanAgent {
		return "", nil
	}
	found := authorizingDecision(q, marker, now)
	if found == nil {
		return "", fail(2, "An agent may run this only after the human's `pulse decide ... yes` on a board proposal whose body "+
			"carries the line `%s`; no decided proposal names it", marker)
	}
	if !found.Valid {
		return "", fail(2, "The newest decision naming `%s` (%s: %s on %s) does not authorize it; a yes at most %d days old is needed",
			marker, found.PostID, found.Answer, short(found.DecidedAt, 10), decisionValidDays)
	}
	return found.PostID, nil
}

func daemonStatus(s *store.Store, now string) string {
	r := mustOne(s.DB, "SELECT value FROM meta WHERE key = 'daemon'")
	if r == nil {
		return "never started"
	}
	var beat map[string]any
	if err := json.Unmarshal([]byte(r.str("value")), &beat); err != nil {
		return "unreadable heartbeat"
	}
	if beat["note"] == "exited" {
		return fmt.Sprintf("exited %v (pid %v)", beat["at"], beat["pid"])
	}
	at, _ := beat["at"].(string)
	age := int(mustISO(now).Sub(mustISO(at)).Seconds())
	interval := 300
	if v, ok := beat["interval"].(float64); ok && v > 0 {
		interval = int(v)
	}
	loops := 0
	if v, ok := beat["loops"].(float64); ok {
		loops = int(v)
	}
	state := "alive"
	if age > 2*interval+60 {
		state = "silent"
	}
	pid := ""
	if v, ok := beat["pid"].(float64); ok {
		pid = fmt.Sprintf("%d", int(v))
	}
	return fmt.Sprintf("%s pid %s, last loop %ds ago, %d loop(s), interval %ds", state, pid, age, loops, interval)
}

type onboardOpts struct {
	Name, Remote, VerifyCmd, Actor string
	Brain                          bool
}

// onboardProject is onboard_project() under D-L: register (init), queue the onboarding job whose brief asks
// for the environment recipe as a record, queue the first reflect, print what stays human. No files are
// written into the checkout: the loop's constitution and evaluators are records, seeded at registration
// (ADR-030: the checkout's committed docs/evolution files indexed, else the template's text copied) and
// reported here as the store holds them afterwards.
func onboardProject(s *store.Store, path, now string, o onboardOpts) ([]string, error) {
	root, err := filepath.Abs(expandUser(path))
	if err != nil {
		return nil, err
	}
	if rp, err := filepath.EvalSymlinks(root); err == nil {
		root = rp
	}
	if st, err := os.Stat(root); err != nil || !st.IsDir() {
		return nil, fail(2, "Not a directory: %s", root)
	}
	slug := ""
	if existing := mustOne(s.DB, "SELECT slug FROM projects WHERE path = ?", root); existing != nil {
		slug = existing.str("slug")
	} else {
		base := o.Name
		if base == "" {
			base = filepath.Base(root)
		}
		base = slugify(base, 40)
		if base == commonsSlug {
			base += "-project"
		}
		slug = uniqueSlug(s.DB, base, "projects", "slug")
	}
	decision, err := agentGate(s.DB, "onboard: "+slug, o.Actor, now)
	if err != nil {
		return nil, err
	}
	evolution := filepath.Join(root, "docs", "evolution")
	if st, err := os.Stat(evolution); err == nil {
		if !st.IsDir() {
			return nil, fail(2, "%s exists but is not a directory", evolution)
		}
		var missing []string
		for _, name := range []string{"constitution.md", "evaluators.md", "archive.md"} {
			if !fileExists(filepath.Join(evolution, name)) {
				missing = append(missing, name)
			}
		}
		if len(missing) > 0 {
			return nil, fail(2, "docs/evolution/ is partial; missing %s. A half-filled loop folder is the owner's to finish", strings.Join(missing, ", "))
		}
	}
	project, created, err := addProject(s, root, o.Name, o.Remote, o.Brain, o.VerifyCmd, o.Actor, "")
	if err != nil {
		return nil, err
	}
	onboardJob, onboardNew, err := queueOnboardingJob(s, project.str("slug"), "init")
	if err != nil {
		return nil, err
	}
	reflect := mustOne(s.DB, "SELECT * FROM jobs WHERE project = ? AND kind = 'reflect' ORDER BY created_at, id LIMIT 1", project.str("slug"))
	reflectNew := false
	if reflect == nil {
		by := o.Actor
		if by == "" {
			by = "onboard"
		}
		reflect, reflectNew, err = addJob(s, project.str("slug"), "reflect",
			"First reflect: turn the onboarding findings into one-variable suggestion cards",
			"First reflect after onboarding: read the onboarding findings and the loop's records (`pulse record show`), run the project's "+
				"own checks, and turn each useful signal into one-variable suggestion cards. Do not flip loop gates; record "+
				"human-only baseline or loop-on work as FINDING: lines.", 0, 19, "onboard-reflect:"+project.str("slug"), by, true)
		if err != nil {
			return nil, err
		}
	}
	actor := o.Actor
	if actor == "" {
		actor = humanAgent
	}
	eventAt(s.DB, now, actor, "project.onboard", project.str("slug"), map[string]any{"created": created, "scaffolded": []string{}, "decision": decision})
	verb := "refreshed"
	if created {
		verb = "registered"
	}
	brainNote := ""
	if project.int("brain") != 0 {
		brainNote = " (brain)"
	}
	lines := []string{fmt.Sprintf("%s project %s at %s%s", verb, project.str("slug"), project.str("path"), brainNote)}
	if onboardNew {
		lines = append(lines, fmt.Sprintf("queued onboarding job %s", onboardJob.str("id")))
	} else {
		lines = append(lines, fmt.Sprintf("already queued onboarding job %s", onboardJob.str("id")))
	}
	if reflectNew {
		lines = append(lines, fmt.Sprintf("queued first reflect job %s", reflect.str("id")))
	} else {
		lines = append(lines, fmt.Sprintf("first reflect already exists %s", reflect.str("id")))
	}
	if fileExists(evolution) {
		lines = append(lines, "docs/evolution/ already present, untouched")
	} else {
		lines = append(lines, "no docs/evolution/ scaffold written: the loop's constitution and evaluators live as records")
	}
	lines = append(lines, loopRecordLine(s, project.str("slug"), "constitution"), loopRecordLine(s, project.str("slug"), "evaluators"))
	if _, found := recipeText(s, project.str("slug")); found {
		lines = append(lines, "environment recipe present (CTX-environment), untouched")
	} else if fileExists(filepath.Join(root, recipePath)) {
		lines = append(lines, fmt.Sprintf("%s found in the checkout; `pulse env import --project %s` moves it into the store", recipePath, project.str("slug")))
	} else {
		lines = append(lines, "no environment recipe yet; the onboarding agent writes it with `pulse env set`, a human reviews it")
	}
	starters := starterCount(s, project.str("slug"))
	if created && starters > 0 {
		lines = append(lines, fmt.Sprintf("seeded %d starter fixtures (pulse evolve fixture list --project %s)", starters, project.str("slug")))
	} else if starters == 0 {
		lines = append(lines, fmt.Sprintf("starter fixtures absent: pulse evolve fixture seed --project %s", project.str("slug")))
	}
	lines = append(lines, "What stays human:",
		fmt.Sprintf("1. the baseline records itself: once evolve.loop is 1 the daemon measures %s with the enabled fixtures and archives the baseline (ADR-031; costs real agent runs); sooner by hand: pulse job add --project %s --kind baseline --title 'Record baseline'", project.str("default_branch"), project.str("slug")),
		heldoutStep(s),
		"3. set loop: on in the constitution record")
	if !strings.HasPrefix(daemonStatus(s, now), "alive ") {
		lines = append(lines, "4. keep the daemon and the hub alive with `pulse service install`")
	}
	lines = append(lines, "store: "+s.Path)
	return lines, nil
}

// starterCount is how many starter fixtures the project carries, enabled or
// not; -1 when the fixture table cannot be read, so onboarding says nothing.
func starterCount(s *store.Store, slug string) int {
	fixtures, err := evolve.ListFixtures(s, slug, false)
	if err != nil {
		return -1
	}
	n := 0
	for _, f := range fixtures {
		if evolve.IsStarter(f.Name) {
			n++
		}
	}
	return n
}

// loopRecordLine describes one of the loop's records as the store holds it:
// still the seeded copy (first revision marked seeded_from), the checkout's
// file as indexed, present with its revision, or absent with the command
// that seeds it.
func loopRecordLine(s *store.Store, slug, name string) string {
	rows, err := s.ListRecords(store.RecordFilter{Project: slug, Kind: "evolution", Slug: name})
	if err != nil || len(rows) == 0 {
		return fmt.Sprintf("%s: absent: pulse evolve seed --project %s", name, slug)
	}
	rec := rows[0]
	if rec.CurrentRev == 1 {
		if _, rev, err := s.GetRecord(rec.ID, 0); err == nil {
			if from, _ := rev.Frontmatter["seeded_from"].(string); from != "" {
				return fmt.Sprintf("%s: copied from %s (%s)", name, from[strings.LastIndex(from, "/")+1:], rec.ID)
			}
		}
		if origin, imported := strings.CutPrefix(rec.Origin, "markdown:"); imported {
			if at := strings.LastIndex(origin, "@"); at >= 0 {
				origin = origin[:at]
			}
			return fmt.Sprintf("%s: imported from %s (%s)", name, origin, rec.ID)
		}
	}
	return fmt.Sprintf("%s: present (%s r%d)", name, rec.ID, rec.CurrentRev)
}

// heldoutStep is step 2 of the onboarding summary: the daemon writes the sealed held-out set itself
// (ADR-033) unless the owner chose evolve.heldout = owner, in which case the fixtures are theirs to keep.
func heldoutStep(s *store.Store) string {
	if evolve.HeldoutMode(s) == "owner" {
		return "2. keep at least two private fixtures in evolve.heldout_dir: pulse evolve setup DIR"
	}
	return "2. nothing for the held-out set: the daemon writes two sealed fixtures itself (ADR-033); evolve.heldout = owner to hand-write them instead"
}
