package hub

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"github.com/manishtiwari25/pulse/internal/gitenv"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

type TallySource interface {
	Records(project, area string) (string, error)
}

type nativeTally struct{ store *store.Store }

func (n nativeTally) Records(project, area string) (string, error) {
	return tally.Records(n.store, project, area, tally.Thresholds{})
}

type boardData struct {
	Project     *store.Project
	Meta        map[string]any
	Params      map[string]any
	Posts       []map[string]any
	ByID        map[string]map[string]any
	Claims      []map[string]any
	Threads     []map[string]any
	Proposals   []map[string]any
	Suggestions []map[string]any
	Amendments  []map[string]any
	Evolution   map[string]any
}

func str(v any) string {
	if v == nil {
		return ""
	}
	return fmt.Sprint(v)
}

func num(v any) int {
	n, _ := strconv.Atoi(str(v))
	return n
}

func flagInt(b bool) int {
	if b {
		return 1
	}
	return 0
}

func object(keys string, values ...any) map[string]any {
	result := map[string]any{}
	for i, key := range strings.Fields(keys) {
		result[key] = values[i]
	}
	return result
}

func pick(row map[string]any, keys string) map[string]any {
	out := map[string]any{}
	for _, key := range strings.Fields(keys) {
		out[key] = row[key]
	}
	return out
}

func clone(row map[string]any) map[string]any {
	out := make(map[string]any, len(row))
	for k, v := range row {
		out[k] = v
	}
	return out
}

func order(rows []map[string]any, fields ...string) {
	sort.SliceStable(rows, func(i, j int) bool {
		for _, field := range fields {
			desc := strings.HasPrefix(field, "-")
			field = strings.TrimPrefix(field, "-")
			a, b := str(rows[i][field]), str(rows[j][field])
			if a != b {
				if desc {
					return a > b
				}
				return a < b
			}
		}
		return false
	})
}

func gitValue(repo string, args ...string) string {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, "git", append([]string{"-C", repo}, args...)...)
	// Drop hook-exported GIT_INDEX_FILE/GIT_DIR/GIT_WORK_TREE/GIT_PREFIX so "git -C repo" reads repo,
	// not the clone whose pre-commit hook is running this process.
	command.Env = gitenv.Env()
	out, err := command.Output()
	if err != nil {
		// An unborn checkout has no branch/head; a missing remote is normal.
		return ""
	}
	return strings.TrimSpace(string(out))
}

func (h *Handler) entry(slug string) (*store.Project, error) {
	if h.Store == nil {
		return nil, errors.New("the store is unavailable")
	}
	if slug == store.CommonsSlug {
		value, _, err := h.Store.Meta("commons_path")
		if err != nil {
			return nil, err
		}
		if value == "" {
			value = filepath.Join(h.Store.Home, "commons")
		}
		return &store.Project{Slug: slug, Name: slug, Path: value, Status: "active"}, nil
	}
	p, err := h.Store.Project(slug)
	if err != nil {
		return nil, err
	}
	if p == nil || p.Status != "active" {
		return nil, &tally.NoProjectError{Slug: slug}
	}
	return p, nil
}

// D-L removed docs/: presence is a directory in a Git checkout, not docs/.
func present(p *store.Project) bool {
	info, err := os.Stat(p.Path)
	return err == nil && info.IsDir() && gitValue(p.Path, "rev-parse", "--is-inside-work-tree") == "true"
}

func (h *Handler) snapshot(project, area string) (*boardData, error) {
	p, err := h.entry(project)
	if err != nil {
		return nil, err
	}
	source := h.Source
	if source == nil {
		source = nativeTally{h.Store}
	}
	text, err := source.Records(project, area)
	if err != nil {
		return nil, err
	}
	rawPosts, err := tally.PostsFor(h.Store, project, "")
	if err != nil {
		return nil, err
	}
	openJobs, err := tally.OpenSuggestionJobs(h.Store)
	if err != nil {
		return nil, err
	}
	dbPosts := make(map[string]tally.Post, len(rawPosts))
	for _, post := range rawPosts {
		dbPosts[post.ID] = post
	}
	d := &boardData{Project: p, Params: map[string]any{}, ByID: map[string]map[string]any{},
		Posts: []map[string]any{}, Claims: []map[string]any{}, Threads: []map[string]any{},
		Proposals: []map[string]any{}, Suggestions: []map[string]any{}, Amendments: []map[string]any{}}
	params := map[string]string{}
	svotes, avotes := map[string][]map[string]any{}, map[string][]map[string]any{}
	nokind := "0"
	for _, line := range strings.Split(strings.TrimRight(text, "\n"), "\n") {
		if line == "" {
			continue
		}
		fields := strings.Split(line, "\t")
		kind, f := fields[0], fields[1:]
		for len(f) < 17 {
			f = append(f, "")
		}
		switch kind {
		case "param":
			params[f[0]] = f[1]
		case "nokind":
			nokind = f[0]
		case "post":
			post, ok := dbPosts[f[3]]
			if !ok {
				return nil, fmt.Errorf("tally references missing post %q", f[3])
			}
			fm := postFrontmatter(post)
			missing := []string{}
			for _, key := range []string{"agent", "runner", "model", "session", "date", "branch"} {
				if str(fm[key]) == "" {
					missing = append(missing, key)
				}
			}
			row := object("id rel_path area thread thread_key kind agent runner model session date branch expires on_post vote text_hash has_read_line resolution summary links files frontmatter body provenance_ok provenance_missing in_area name project",
				f[3], f[0], f[1], f[2], f[1]+"/"+f[2], f[4], f[5], post.Runner, post.Model, f[12], f[6], post.Branch,
				f[7], f[8], f[9], f[13], flagInt(f[14] == "1"), f[10], f[11], post.Links, post.Files, fm,
				strings.Trim(post.Body, "\n"), flagInt(len(missing) == 0), missing, flagInt(area == "" || area == f[1]),
				truncate(post.Name, 40), truncate(post.OriginProject, 60))
			d.Posts = append(d.Posts, row)
			d.ByID[f[3]] = row
		case "claim":
			d.Claims = append(d.Claims, object("id thread_key agent expires status summary", f[0], f[1], f[2], f[3], f[4], f[5]))
		case "thread":
			area, thread, _ := strings.Cut(f[0], "/")
			d.Threads = append(d.Threads, object("key area thread posts last status resolution summary", f[0], area, thread, num(f[1]), f[2], f[3], f[4], f[5]))
		case "proposal":
			d.Proposals = append(d.Proposals, object("thread_key id agent date status decided_id decided_date summary", f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]))
		case "suggestion":
			d.Suggestions = append(d.Suggestions, object("score date id thread_key up down voters status agent summary", num(f[0]), f[1], f[2], f[3], num(f[4]), num(f[5]), num(f[6]), f[7], f[8], f[9]))
		case "svote":
			svotes[f[0]] = append(svotes[f[0]], object("agent post_id date vote classification", f[1], f[2], f[3], f[4], f[5]))
		case "amendment":
			d.Amendments = append(d.Amendments, object("date id thread_key status up down voters quorum pct_up identities uncounted_self uncounted_hash_mismatch uncounted_no_read_line text_hash section agent summary",
				f[0], f[1], f[2], f[3], num(f[4]), num(f[5]), num(f[6]), num(f[7]), num(f[8]), num(f[9]), num(f[10]), num(f[11]), num(f[12]), f[13], f[14], f[15], f[16]))
		case "avote":
			avotes[f[0]] = append(avotes[f[0]], object("session agent post_id date vote classification", f[1], f[2], f[3], f[4], f[5], f[6]))
		default:
			return nil, fmt.Errorf("unknown tally record kind %q", kind)
		}
	}
	d.Params = object("area now quorum expiry_days stale_days amend_quorum amend_majority protected_sections constitution_present stop_present",
		params["area"], params["now"], num(params["quorum"]), num(params["expiry_days"]), num(params["stale_days"]),
		num(params["amend_quorum"]), params["amend_majority"], params["protected"], params["constitution_present"] == "1", params["stop_present"] == "1")
	d.enrichThreads()
	for _, row := range d.Proposals {
		post, decided := d.ByID[str(row["id"])], d.ByID[str(row["decided_id"])]
		for _, key := range []string{"name", "session", "rel_path", "body", "kind"} {
			row[key] = str(post[key])
		}
		for _, key := range []string{"summary", "agent", "name"} {
			row["decided_"+key] = str(decided[key])
		}
		row["proposal_text"] = proposalText(str(row["body"]))
		row["can_decide"] = true
	}
	order(d.Proposals, "-date", "id")
	sort.SliceStable(d.Proposals, func(i, j int) bool {
		return d.Proposals[i]["status"] == "awaiting" && d.Proposals[j]["status"] != "awaiting"
	})
	statusOrder := map[string]int{"eligible": 0, "needs votes": 1, "expired": 2}
	priority := func(v any) int {
		if n, ok := statusOrder[str(v)]; ok {
			return n
		}
		return 3
	}
	sort.SliceStable(d.Suggestions, func(i, j int) bool {
		a, b := d.Suggestions[i], d.Suggestions[j]
		if priority(a["status"]) != priority(b["status"]) {
			return priority(a["status"]) < priority(b["status"])
		}
		if num(a["score"]) != num(b["score"]) {
			return num(a["score"]) > num(b["score"])
		}
		if a["date"] != b["date"] {
			return str(a["date"]) < str(b["date"])
		}
		return str(a["id"]) < str(b["id"])
	})
	rank := 0
	for _, row := range d.Suggestions {
		id := str(row["id"])
		row["name"], row["session"] = str(d.ByID[id]["name"]), str(d.ByID[id]["session"])
		job := openJobs[id]
		row["job"], row["job_status"] = job.ID, job.Status
		gate := ""
		if row["status"] == "eligible" {
			var post *tally.Post
			if p, ok := dbPosts[id]; ok {
				post = &p
			}
			gate = tally.PlanGate(h.Store, p.Path, tally.Suggestion{
				Score:     num(row["score"]),
				ID:        id,
				Key:       str(row["thread_key"]),
				Agent:     str(row["agent"]),
				Summary:   str(row["summary"]),
				Job:       str(row["job"]),
				JobStatus: str(row["job_status"]),
			}, post)
		}
		row["gate"] = gate
		row["plannable"] = row["status"] == "eligible" && gate == ""
		row["rank"] = nil
		if priority(row["status"]) < 3 {
			rank++
			row["rank"] = rank
		}
		votes := svotes[id]
		if votes == nil {
			votes = []map[string]any{}
		}
		for _, vote := range votes {
			post := d.ByID[str(vote["post_id"])]
			vote["reason"], vote["name"], vote["session"] = str(post["summary"]), str(post["name"]), str(post["session"])
		}
		order(votes, "date", "agent")
		row["votes"] = votes
	}
	order(d.Amendments, "date", "id")
	sort.SliceStable(d.Amendments, func(i, j int) bool {
		return !strings.HasPrefix(str(d.Amendments[i]["status"]), "closed:") && strings.HasPrefix(str(d.Amendments[j]["status"]), "closed:")
	})
	for _, row := range d.Amendments {
		id := str(row["id"])
		row["name"], row["session"] = str(d.ByID[id]["name"]), str(d.ByID[id]["session"])
		votes := avotes[id]
		if votes == nil {
			votes = []map[string]any{}
		}
		for _, vote := range votes {
			post := d.ByID[str(vote["post_id"])]
			vote["reason"], vote["name"], vote["read_line"] = str(post["summary"]), str(post["name"]), ""
			for _, line := range strings.Split(str(post["body"]), "\n") {
				if strings.HasPrefix(line, "Read:") {
					vote["read_line"] = strings.TrimSpace(strings.TrimPrefix(line, "Read:"))
					break
				}
			}
		}
		order(votes, "date", "session")
		row["votes"] = votes
	}
	d.Evolution, err = h.evolution(project)
	if err != nil {
		return nil, err
	}
	d.Meta = object("schema_version built_at now repo repo_name remote branch head dirty board_present evolution_present tally_script tally_mode board_params posts_without_kind last_error",
		"3", h.Store.Now(), params["now"], p.Path, filepath.Base(p.Path), gitValue(p.Path, "config", "--get", "remote.origin.url"),
		gitValue(p.Path, "rev-parse", "--abbrev-ref", "HEAD"), gitValue(p.Path, "rev-parse", "--short", "HEAD"),
		strconv.Itoa(flagInt(gitValue(p.Path, "status", "--porcelain", "--", "docs/board", "docs/evolution") != "")),
		params["board_present"], evolutionSummary(d.Evolution)["evolution_present"],
		"", "go", d.Params, nokind, "")
	metadata, err := h.metadata()
	if err != nil {
		return nil, err
	}
	for key, value := range metadata {
		d.Meta[key] = value
	}
	return d, nil
}

func truncate(text string, n int) string {
	r := []rune(text)
	if len(r) > n {
		return string(r[:n])
	}
	return text
}

func postFrontmatter(p tally.Post) map[string]any {
	fm := object("kind agent runner model session date branch summary", p.Kind, p.Agent, p.Runner, p.Model, p.Session, p.Date, p.Branch, p.Summary)
	for key, value := range map[string]string{"expires": p.Expires, "on": p.OnPost, "vote": p.Vote, "resolution": p.Resolution, "text_hash": p.TextHash, "links": p.Links, "files": p.Files, "name": p.Name, "project": p.OriginProject} {
		if value != "" {
			fm[key] = value
		}
	}
	if p.ForOwner {
		fm["for_owner"] = "true"
	}
	fm["id"], fm["area"], fm["thread"], fm["path"] = p.ID, p.Area, p.Thread, "store:"+p.ID
	return fm
}

var proposalHeading = regexp.MustCompile(`(?i)^\W*proposal for the human`)
var proposalPrefix = regexp.MustCompile(`(?i)^\W*proposal for the human[^:]*:\W*`)

func proposalText(body string) string {
	lines := strings.Split(body, "\n")
	start := -1
	for i, line := range lines {
		if proposalHeading.MatchString(strings.TrimSpace(line)) {
			start = i
			break
		}
	}
	para := []string{}
	if start >= 0 {
		first := proposalPrefix.ReplaceAllString(strings.TrimSpace(lines[start]), "")
		if first != "" {
			para = append(para, first)
		}
		lines = lines[start+1:]
	}
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if start < 0 && strings.HasPrefix(line, "#") {
			continue
		}
		if line == "" {
			if len(para) > 0 {
				break
			}
			continue
		}
		para = append(para, line)
	}
	text := strings.Join(para, " ")
	if start >= 0 {
		text = strings.TrimSpace(strings.Trim(text, "* "))
	}
	return truncate(text, 600)
}

func (d *boardData) thread(key string) map[string]any {
	for _, thread := range d.Threads {
		if thread["key"] != key {
			continue
		}
		posts := []map[string]any{}
		for _, p := range d.Posts {
			if p["thread_key"] == key {
				posts = append(posts, pick(p, "id rel_path kind agent name project runner model session date branch expires on_post vote text_hash has_read_line resolution summary provenance_ok provenance_missing"))
			}
		}
		order(posts, "date", "id")
		return object("thread posts", thread, posts)
	}
	return nil
}

func (d *boardData) enrichThreads() {
	threads := map[string]map[string]any{}
	for _, thread := range d.Threads {
		key := str(thread["key"])
		threads[key] = thread
		thread["participants"] = []string{}
		thread["last_by"] = ""
		thread["last_kind"] = ""
		thread["open_questions"] = 0
		thread["open_handoffs"] = 0
		thread["active_claims"] = []string{}
	}
	posts := append([]map[string]any{}, d.Posts...)
	order(posts, "date", "id")
	seen := map[string]map[string]bool{}
	byThread := map[string][]map[string]any{}
	for _, post := range posts {
		key := str(post["thread_key"])
		thread := threads[key]
		if thread == nil {
			continue
		}
		byThread[key] = append(byThread[key], post)
		if seen[key] == nil {
			seen[key] = map[string]bool{}
		}
		agent := str(post["agent"])
		if agent != "" && !seen[key][agent] {
			seen[key][agent] = true
			thread["participants"] = append(thread["participants"].([]string), agent)
		}
		if str(post["date"]) >= str(thread["last"]) {
			thread["last_by"] = agent
			thread["last_kind"] = str(post["kind"])
		}
	}
	for _, claim := range d.Claims {
		if claim["status"] != "active" {
			continue
		}
		key := str(claim["thread_key"])
		if thread := threads[key]; thread != nil {
			thread["active_claims"] = append(thread["active_claims"].([]string), str(claim["agent"]))
		}
	}
	answered := map[string]bool{}
	for _, post := range posts {
		if post["kind"] == "answer" && str(post["on_post"]) != "" {
			answered[str(post["on_post"])] = true
		}
	}
	// Keep these rules in parity with internal/board.ThreadSummaries.
	for key, threadPosts := range byThread {
		thread := threads[key]
		if thread == nil || thread["status"] == "closed" {
			continue
		}
		for i, post := range threadPosts {
			switch post["kind"] {
			case "question":
				if post["area"] != "jobs" && !answered[str(post["id"])] && !laterHubAnswerDate(threadPosts, i) {
					thread["open_questions"] = num(thread["open_questions"]) + 1
				}
			case "handoff":
				if !laterHubKind(threadPosts, i, "claim", "result") {
					thread["open_handoffs"] = num(thread["open_handoffs"]) + 1
				}
			}
		}
	}
}

func laterHubKind(posts []map[string]any, index int, kinds ...string) bool {
	want := map[string]bool{}
	for _, kind := range kinds {
		want[kind] = true
	}
	for _, post := range posts[index+1:] {
		if want[str(post["kind"])] {
			return true
		}
	}
	return false
}

func laterHubAnswerDate(posts []map[string]any, index int) bool {
	date := str(posts[index]["date"])
	for _, post := range posts[index+1:] {
		if post["kind"] == "answer" && str(post["date"]) > date {
			return true
		}
	}
	return false
}

func (d *boardData) post(id string) map[string]any {
	p, ok := d.ByID[id]
	if !ok {
		return nil
	}
	result := clone(p)
	responses := []map[string]any{}
	for _, row := range d.Posts {
		if row["on_post"] == id {
			responses = append(responses, pick(row, "id kind agent name session date vote summary"))
		}
	}
	order(responses, "date")
	result["responses"] = responses
	return result
}

func countStatus(rows []map[string]any, statuses ...string) int {
	count := 0
	for _, row := range rows {
		for _, status := range statuses {
			if row["status"] == status {
				count++
				break
			}
		}
	}
	return count
}

func (d *boardData) overview() map[string]any {
	active, stale := []map[string]any{}, []map[string]any{}
	for _, claim := range d.Claims {
		row := clone(claim)
		post := d.ByID[str(row["id"])]
		row["name"], row["session"] = str(post["name"]), str(post["session"])
		switch row["status"] {
		case "active":
			active = append(active, row)
		case "expired", "no expiry":
			stale = append(stale, row)
		}
	}
	order(active, "expires")
	order(stale, "-expires")
	open := []map[string]any{}
	for _, t := range d.Threads {
		if t["status"] == "open" || t["status"] == "stale" {
			open = append(open, t)
		}
	}
	order(open, "-last")
	recent := []map[string]any{}
	kinds, areas := map[string]int{}, map[string]bool{}
	agents := map[string]map[string]any{}
	agentRows := []map[string]any{}
	withoutProvenance := 0
	for _, p := range d.Posts {
		recent = append(recent, pick(p, "id thread_key kind agent name session date summary provenance_ok"))
		kind := str(p["kind"])
		if kind == "" {
			kind = "(none)"
		}
		kinds[kind]++
		areas[str(p["area"])] = true
		withoutProvenance += 1 - num(p["provenance_ok"])
		key := str(p["agent"]) + "\x00" + str(p["session"])
		row, exists := agents[key]
		if !exists {
			row = object("agent session name posts last", p["agent"], p["session"], nil, 0, "")
			agents[key] = row
		}
		row["posts"] = num(row["posts"]) + 1
		if str(p["date"]) > str(row["last"]) {
			row["last"] = p["date"]
		}
		if str(p["name"]) != "" && str(p["name"]) > str(row["name"]) {
			row["name"] = p["name"]
		}
	}
	order(recent, "-date")
	if len(recent) > 12 {
		recent = recent[:12]
	}
	for _, key := range sortedKeys(agents) {
		agentRows = append(agentRows, agents[key])
	}
	order(agentRows, "-last")
	if len(agentRows) > 12 {
		agentRows = agentRows[:12]
	}
	amendOpen, amendPassed := 0, 0
	for _, a := range d.Amendments {
		if !strings.HasPrefix(str(a["status"]), "closed:") {
			amendOpen++
		}
		if strings.HasPrefix(str(a["status"]), "passed") {
			amendPassed++
		}
	}
	evo := evolutionSummary(d.Evolution)
	missing, _ := json.Marshal(evo["missing_files"])
	evo["missing_files"] = string(missing)
	return object("board_present posts_without_kind quorum claims_active claims_expired threads_open threads_stale threads_closed posts posts_without_provenance suggestions_eligible suggestions_plannable suggestions_needing_votes suggestions_expired amendments_open amendments_passed proposals_awaiting proposals_decided active_claims stale_claims open_threads recent_posts evolution kinds areas agents",
		d.Meta["board_present"] == "1", num(d.Meta["posts_without_kind"]), d.Params["quorum"], len(active), len(stale), len(open),
		countStatus(d.Threads, "stale"), countStatus(d.Threads, "closed"), len(d.Posts), withoutProvenance, countStatus(d.Suggestions, "eligible"),
		countPlannable(d.Suggestions), countStatus(d.Suggestions, "needs votes"), countStatus(d.Suggestions, "expired"), amendOpen, amendPassed,
		countStatus(d.Proposals, "awaiting"), countStatus(d.Proposals, "accepted", "declined"), active, stale, open, recent, evo, kinds, sortedKeys(areas), agentRows)
}

func countPlannable(rows []map[string]any) int {
	count := 0
	for _, row := range rows {
		if row["plannable"] == true {
			count++
		}
	}
	return count
}

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for key := range m {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	return keys
}

func (d *boardData) threads() []map[string]any {
	rows := append([]map[string]any{}, d.Threads...)
	order(rows, "-last")
	status := map[string]int{"open": 0, "stale": 1, "closed": 2}
	sort.SliceStable(rows, func(i, j int) bool { return status[str(rows[i]["status"])] < status[str(rows[j]["status"])] })
	return rows
}

func (d *boardData) notices() map[string]any {
	suggestions := countStatus(d.Suggestions, "eligible", "needs votes")
	amendments := len(d.Amendments)
	for _, row := range d.Amendments {
		if strings.HasPrefix(str(row["status"]), "closed:") {
			amendments--
		}
	}
	proposals := countStatus(d.Proposals, "awaiting")
	name := str(d.Meta["repo_name"])
	return object("ballot votes_open proposals_awaiting brain brains commons_present commons_base commons_questions",
		[]map[string]any{object("slug base name brain suggestions amendments proposals", name, "", name, false, suggestions, amendments, proposals)},
		suggestions+amendments, proposals, "", []string{}, nil, "", nil)
}
