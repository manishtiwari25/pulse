package hub

import (
	"encoding/json"
	"errors"
	"net/http"
	"sort"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

func (h *Handler) projectAPI(w http.ResponseWriter, r *http.Request, slug, route string) {
	if !readMethod(w, r) {
		return
	}
	p, err := h.entry(slug)
	if err != nil {
		code := 503
		var missing *tally.NoProjectError
		if errors.As(err, &missing) {
			code = 404
		}
		writeError(w, code, "no such project", err.Error(), "Run pulse projects.")
		return
	}
	if !present(p) {
		writeError(w, 503, "registered but missing on disk", "The project path must be a directory in a Git checkout.", "Restore the registered checkout or update the project path.")
		return
	}
	if route == "health" {
		writeJSON(w, 200, object("ok time last_error", true, h.Store.Now(), ""))
		return
	}
	if route == "settings" {
		// parity: /p/<slug>/api/v1/settings - Python's Path.home collision is fixed; use owner store settings.
		data, err := h.settings()
		h.respond(w, data, err)
		return
	}
	if route != "project" && route != "refresh" && route != "overview" && route != "threads" &&
		route != "suggestions" && route != "amendments" && route != "evolution" && route != "timeline" &&
		route != "notices" && route != "proposals" && !strings.HasPrefix(route, "threads/") && !strings.HasPrefix(route, "posts/") {
		writeError(w, 404, "not found", "No such project API route.", "Use the API v1 project routes.")
		return
	}
	d, err := h.snapshot(slug, r.URL.Query().Get("area"))
	if err != nil {
		h.respond(w, nil, err)
		return
	}
	var value any
	switch {
	case route == "project" || route == "refresh":
		// parity: project metadata - actual store fingerprint/build identity, never a disposable projection.
		value = d.Meta
	case route == "overview":
		value = d.overview()
	case route == "threads":
		value = d.threads()
	case strings.HasPrefix(route, "threads/"):
		row := d.thread(strings.TrimPrefix(route, "threads/"))
		if row == nil {
			writeError(w, 404, "no such thread", "That thread is not in this project.", "Refresh the thread list.")
			return
		}
		value = row
	case strings.HasPrefix(route, "posts/"):
		row := d.post(strings.TrimPrefix(route, "posts/"))
		if row == nil {
			writeError(w, 404, "no such post", "That post is not in this project.", "Refresh the thread.")
			return
		}
		value = row
	case route == "suggestions":
		value = d.Suggestions
	case route == "amendments":
		value = d.Amendments
	case route == "proposals":
		value = d.Proposals
	case route == "notices":
		value = d.notices()
	case route == "timeline":
		value = timeline(d.Posts, str(d.Params["now"]))
	case route == "evolution":
		// parity: evolution - coordinator-approved SQL-native document replaces the filesystem report.
		value = d.Evolution
	}
	writeJSON(w, 200, value)
}

func (h *Handler) respond(w http.ResponseWriter, data any, err error) {
	if err != nil {
		writeError(w, 503, "The store view is unavailable", err.Error(), "Check pulse status and retry.")
		return
	}
	writeJSON(w, 200, data)
}

type projectView struct {
	Project *store.Project
	Data    *boardData
	Error   string
}

func (h *Handler) each() ([]projectView, error) {
	if h.Store == nil {
		return nil, errors.New("the store is unavailable")
	}
	projects, err := h.Store.Projects(true)
	if err != nil {
		return nil, err
	}
	commons, err := h.entry(store.CommonsSlug)
	if err != nil {
		return nil, err
	}
	projects = append(projects, *commons)
	out := make([]projectView, 0, len(projects))
	for _, p := range projects {
		view := projectView{Project: &p}
		if present(&p) {
			view.Data, err = h.snapshot(p.Slug, "")
			if err != nil {
				view.Error = err.Error()
			}
		} else {
			view.Error = "missing on disk"
			if p.Slug == store.CommonsSlug {
				view.Error = "not created yet"
			}
		}
		out = append(out, view)
	}
	return out, nil
}

func baseFor(slug string) string {
	if slug == store.CommonsSlug {
		return "/commons"
	}
	return "/p/" + slug
}

func (h *Handler) orgAPI(w http.ResponseWriter, r *http.Request, route string) {
	if route == "settings" {
		data, err := h.settings()
		h.respond(w, data, err)
		return
	}
	if route == "executors" {
		data, err := h.executors(r)
		h.respond(w, data, err)
		return
	}
	if route == "runner" {
		data, err := h.runner(r)
		h.respond(w, data, err)
		return
	}
	switch route {
	case "health", "org", "notices", "proposals", "roster", "votes", "activity", "timeline":
	default:
		writeError(w, 404, "not found", "No such organization API route.", "Use /api/v1/org.")
		return
	}
	if route == "health" {
		if h.Store == nil {
			writeError(w, 503, "The store is unavailable", "No PULSE store is open.", "Run pulse init.")
			return
		}
		projects, err := h.Store.Projects(true)
		if err != nil {
			h.respond(w, nil, err)
			return
		}
		slugs := []string{store.CommonsSlug}
		for _, p := range projects {
			slugs = append(slugs, p.Slug)
		}
		sort.Strings(slugs)
		writeJSON(w, 200, object("ok time projects errors", true, h.Store.Now(), slugs, map[string]any{}))
		return
	}
	views, err := h.each()
	if err != nil {
		h.respond(w, nil, err)
		return
	}
	var data any
	switch route {
	case "org":
		org := h.org(views)
		metadata, metadataErr := h.metadata()
		if metadataErr != nil {
			h.respond(w, nil, metadataErr)
			return
		}
		for key, value := range metadata {
			org[key] = value
		}
		data = org
	case "notices":
		data = h.org(views)["notices"]
	case "proposals":
		data = orgProposals(views)
	case "votes":
		data = orgVotes(views)
	case "activity":
		data = orgActivity(views, 150)
	case "timeline":
		data = orgTimeline(views, h.Store.Now())
	case "roster":
		data, err = h.roster(views)
	}
	h.respond(w, data, err)
}

func (h *Handler) summary(view projectView) map[string]any {
	p, d := view.Project, view.Data
	kind := "project"
	if p.Slug == store.CommonsSlug {
		kind = "commons"
	}
	if d == nil {
		return object("slug kind brain base path present missing error", p.Slug, kind, p.Brain, baseFor(p.Slug), p.Path, false, !present(p), view.Error)
	}
	overview := d.overview()
	evo := evolutionSummary(d.Evolution)
	recent := append([]map[string]any{}, d.Posts...)
	order(recent, "-date")
	var last any
	if len(recent) > 0 {
		last = pick(recent[0], "id thread_key kind agent name session date summary")
	}
	dayAgo := parseTime(h.Store.Now()).Add(-24 * time.Hour).Format(time.RFC3339)
	posts24 := 0
	identities := map[string]bool{}
	questions := map[string]bool{}
	for _, post := range d.Posts {
		identities[str(post["agent"])] = true
		if str(post["date"]) >= dayAgo {
			posts24++
		}
		if post["kind"] == "question" {
			questions[str(post["thread_key"])] = true
		}
	}
	questionsOpen := 0
	for _, t := range d.Threads {
		if (t["status"] == "open" || t["status"] == "stale") && questions[str(t["key"])] {
			questionsOpen++
		}
	}
	claims := []map[string]any{}
	for _, c := range overview["active_claims"].([]map[string]any) {
		claims = append(claims, pick(c, "id thread_key agent expires summary name session"))
	}
	proposals := []map[string]any{}
	for _, row := range d.Proposals {
		if row["status"] == "awaiting" {
			proposals = append(proposals, pick(row, "id thread_key agent name session date status summary proposal_text rel_path"))
		}
	}
	return object("slug kind brain base path remote name branch head dirty built_at board_present claims_active threads_open questions_open suggestions_eligible suggestions_needing_votes amendments_open proposals_awaiting posts posts_24h tally_mode can_decide agents identities evolution_present loop stop_present baseline last_post active_claims proposals present missing error",
		p.Slug, kind, p.Brain, baseFor(p.Slug), p.Path, p.Remote, d.Meta["repo_name"], d.Meta["branch"], d.Meta["head"],
		d.Meta["dirty"] == "1", d.Meta["built_at"], d.Meta["board_present"] == "1", overview["claims_active"], overview["threads_open"], questionsOpen,
		overview["suggestions_eligible"], overview["suggestions_needing_votes"], overview["amendments_open"], overview["proposals_awaiting"], len(d.Posts), posts24,
		"go", true, len(identities), sortedKeys(identities), evo["evolution_present"] == "1", evo["loop"], evo["stop_present"] == "1",
		evo["baseline"], last, claims, proposals, true, false, view.Error)
}

func (h *Handler) org(views []projectView) map[string]any {
	projects, all := []map[string]any{}, []map[string]any{}
	var commons map[string]any
	brains, stamps := []string{}, []string{}
	for _, view := range views {
		summary := h.summary(view)
		all = append(all, summary)
		stamps = append(stamps, str(summary["built_at"]))
		if view.Project.Slug == store.CommonsSlug {
			commons = summary
		} else {
			projects = append(projects, summary)
			if view.Project.Brain {
				brains = append(brains, view.Project.Slug)
			}
		}
	}
	brain := ""
	if len(brains) == 1 {
		brain = brains[0]
	}
	identities, activeAgents := map[string]bool{}, map[string]bool{}
	claims, proposals, ballot := []map[string]any{}, []map[string]any{}, []map[string]any{}
	votesOpen, posts24, posts, presentCount := 0, 0, 0, 0
	for _, p := range all {
		if p["present"] != true {
			continue
		}
		if p["kind"] == "project" {
			presentCount++
			ballot = append(ballot, object("slug base name brain suggestions amendments proposals", p["slug"], p["base"], p["name"], p["brain"],
				num(p["suggestions_eligible"])+num(p["suggestions_needing_votes"]), p["amendments_open"], p["proposals_awaiting"]))
		}
		votesOpen += num(p["suggestions_eligible"]) + num(p["suggestions_needing_votes"]) + num(p["amendments_open"])
		posts24 += num(p["posts_24h"])
		posts += num(p["posts"])
		for _, id := range p["identities"].([]string) {
			identities[id] = true
		}
		delete(p, "identities")
		for _, row := range p["active_claims"].([]map[string]any) {
			claim := clone(row)
			claim["slug"], claim["base"] = p["slug"], p["base"]
			claims = append(claims, claim)
			activeAgents[str(claim["agent"])+"\x00"+str(claim["session"])] = true
		}
		for _, row := range p["proposals"].([]map[string]any) {
			proposal := clone(row)
			proposal["slug"], proposal["base"], proposal["can_decide"] = p["slug"], p["base"], true
			proposals = append(proposals, proposal)
		}
		delete(p, "proposals")
	}
	order(claims, "expires")
	order(proposals, "-date", "-slug")
	commonsOK := commons != nil && commons["present"] == true
	questions, commonsBase, commonsDir := 0, "", ""
	var noticeQuestions any
	if commons != nil {
		commonsBase, commonsDir = str(commons["base"]), str(commons["path"])
		if commonsOK {
			questions = num(commons["questions_open"])
			noticeQuestions = questions
		}
	}
	ballotVotes, ballotProposals := 0, 0
	for _, row := range ballot {
		ballotVotes += num(row["suggestions"]) + num(row["amendments"])
		ballotProposals += num(row["proposals"])
	}
	return object("generated_at registry tally_mode can_decide commons_dir projects commons stamp notices totals active_claims proposals",
		h.Store.Now(), h.Store.Path, "go", true, commonsDir, projects, commons, strings.Join(stamps, "|"),
		object("ballot votes_open proposals_awaiting brain brains commons_present commons_base commons_questions", ballot, ballotVotes, ballotProposals, brain, brains, commonsOK, commonsBase, noticeQuestions),
		object("projects projects_present agents agents_active votes_open proposals_awaiting questions_open posts_24h posts brain brains",
			len(projects), presentCount, len(identities), len(activeAgents), votesOpen, len(proposals), questions, posts24, posts, brain, brains), claims, proposals)
}

func orgProposals(views []projectView) []map[string]any {
	out := []map[string]any{}
	for _, view := range views {
		if view.Data == nil || view.Project.Slug == store.CommonsSlug {
			continue
		}
		for _, p := range view.Data.Proposals {
			row := clone(p)
			delete(row, "body")
			row["slug"], row["base"] = view.Project.Slug, baseFor(view.Project.Slug)
			out = append(out, row)
		}
	}
	order(out, "-date", "-slug")
	sort.SliceStable(out, func(i, j int) bool { return out[i]["status"] == "awaiting" && out[j]["status"] != "awaiting" })
	return out
}

func orgVotes(views []projectView) map[string]any {
	suggestions, amendments := []map[string]any{}, []map[string]any{}
	for _, view := range views {
		if view.Data == nil {
			continue
		}
		for _, s := range view.Data.Suggestions {
			if s["status"] == "eligible" || s["status"] == "needs votes" {
				row := clone(s)
				row["slug"], row["base"] = view.Project.Slug, baseFor(view.Project.Slug)
				row["quorum"], row["expiry_days"] = view.Data.Params["quorum"], view.Data.Params["expiry_days"]
				suggestions = append(suggestions, row)
			}
		}
		for _, a := range view.Data.Amendments {
			if !strings.HasPrefix(str(a["status"]), "closed:") {
				row := clone(a)
				row["slug"], row["base"] = view.Project.Slug, baseFor(view.Project.Slug)
				amendments = append(amendments, row)
			}
		}
	}
	order(suggestions, "date", "slug")
	sort.SliceStable(suggestions, func(i, j int) bool {
		a, b := suggestions[i], suggestions[j]
		if a["status"] != b["status"] {
			return a["status"] == "eligible"
		}
		return num(a["score"]) > num(b["score"])
	})
	order(amendments, "date", "slug")
	sort.SliceStable(amendments, func(i, j int) bool {
		return strings.HasPrefix(str(amendments[i]["status"]), "passed") && !strings.HasPrefix(str(amendments[j]["status"]), "passed")
	})
	return object("suggestions amendments", suggestions, amendments)
}

func orgActivity(views []projectView, limit int) []map[string]any {
	out := []map[string]any{}
	for _, view := range views {
		if view.Data == nil {
			continue
		}
		projectPosts := []map[string]any{}
		for _, p := range view.Data.Posts {
			row := pick(p, "id thread_key kind agent name session project date summary vote resolution provenance_ok")
			row["slug"], row["base"] = view.Project.Slug, baseFor(view.Project.Slug)
			projectPosts = append(projectPosts, row)
		}
		order(projectPosts, "-date")
		if len(projectPosts) > limit {
			projectPosts = projectPosts[:limit]
		}
		out = append(out, projectPosts...)
	}
	order(out, "-date", "-id")
	if len(out) > limit {
		out = out[:limit]
	}
	return out
}

func parseTime(text string) time.Time {
	text = strings.ReplaceAll(strings.TrimSuffix(strings.TrimSpace(text), "Z"), " ", "T")
	if len(text) > 19 {
		text = text[:19]
	}
	for _, layout := range []string{"2006-01-02T15:04:05", "2006-01-02T15:04", "2006-01-02"} {
		if t, err := time.Parse(layout, text); err == nil {
			return t
		}
	}
	return time.Time{}
}

func timeline(posts []map[string]any, now string) map[string]any {
	end := parseTime(now)
	if end.IsZero() {
		end = time.Now().UTC()
	}
	hours, days := []map[string]any{}, []map[string]any{}
	byHour, byDay := map[string]map[string]any{}, map[string]map[string]any{}
	for i := 23; i >= 0; i-- {
		key := end.Add(-time.Duration(i) * time.Hour).Format("2006-01-02T15")
		row := object("t n kinds", key, 0, map[string]int{})
		hours = append(hours, row)
		byHour[key] = row
	}
	for i := 29; i >= 0; i-- {
		key := end.AddDate(0, 0, -i).Format("2006-01-02")
		row := object("t n kinds", key, 0, map[string]int{})
		days = append(days, row)
		byDay[key] = row
	}
	before, unparsed := 0, 0
	bump := func(row map[string]any, kind string) {
		row["n"] = num(row["n"]) + 1
		row["kinds"].(map[string]int)[kind]++
	}
	for _, p := range posts {
		dt := parseTime(str(p["date"]))
		if dt.IsZero() {
			unparsed++
			continue
		}
		if dt.After(end) {
			dt = end
		}
		day := byDay[dt.Format("2006-01-02")]
		if day == nil {
			before++
			continue
		}
		kind := str(p["kind"])
		if kind == "" {
			kind = "(none)"
		}
		bump(day, kind)
		if hour := byHour[dt.Format("2006-01-02T15")]; hour != nil {
			bump(hour, kind)
		}
	}
	return object("now hours days total before_window unparsed", end.Format(time.RFC3339), hours, days, len(posts), before, unparsed)
}

func orgTimeline(views []projectView, now string) map[string]any {
	posts := []map[string]any{}
	for _, view := range views {
		if view.Data != nil {
			posts = append(posts, view.Data.Posts...)
			if str(view.Data.Params["now"]) > now {
				now = str(view.Data.Params["now"])
			}
		}
	}
	total, series := timeline(posts, now), []map[string]any{}
	for _, view := range views {
		if view.Data != nil {
			row := timeline(view.Data.Posts, now)
			kind := "project"
			if view.Project.Slug == store.CommonsSlug {
				kind = "commons"
			}
			row["slug"], row["kind"], row["base"] = view.Project.Slug, kind, baseFor(view.Project.Slug)
			series = append(series, row)
		}
	}
	total["series"] = series
	return total
}

func jsonMap(value any) (map[string]any, error) {
	data, err := json.Marshal(value)
	if err != nil {
		return nil, err
	}
	var row map[string]any
	err = json.Unmarshal(data, &row)
	return row, err
}
