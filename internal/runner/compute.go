package runner

import (
	"context"
	"encoding/json"
	"fmt"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

// Compute (D-D): the daemon turns an approved compute request into a machine through a Provider, registers
// the machine as an executor the way `pulse pair` does, and releases it when the request's hours are up or
// the executor sat idle for computeIdleMinutes - whichever comes first. Every step is a compute.<status>
// event through tasks.SetComputeStatus and a `finding` on the request's thread (runner/compute); a failure
// carries its reason with tokens redacted. The caps compute.max_vms / compute.max_hours_per_day are read
// through tasks.GetCaps and hold even for a request the owner approved by hand.

const (
	computeIdleMinutes     = 30
	computeProvisionBudget = 45 * time.Minute
	computeReleaseBudget   = 20 * time.Minute
	computeArea            = "runner"
	computeThread          = "compute"
)

// ProvisionRequest is everything a Provider needs to satisfy one approved compute request.
type ProvisionRequest struct {
	ID       string       // the compute request id (CR-NNN); the machine carries the tag pulse=<ID>
	Project  string       // the project that asked; the executor is limited to it
	Size     string       // the provider's machine size (compute.azure.vm_size when the request named none)
	Hours    int          // how long the machine is wanted
	Reason   string       // the agent's reason, for the machine's description
	Executor string       // the executor name the machine registers as
	Store    *store.Store // settings and the executor registration
	Actor    string       // whom the store writes are attributed to
	HubHome  string       // the hub user's home (ssh keys, config); "" = the process owner's
	Log      func(format string, a ...any)
}

// Machine is a provisioned machine as the daemon remembers it (compute_machines) so a later daemon can
// release it. Detail holds provider facts (resource group, VM name, ...), never a credential.
type Machine struct {
	Provider string            `json:"provider"`
	Name     string            `json:"name"`
	Executor string            `json:"executor"`
	Address  string            `json:"address"` // the admin ssh address (user@host) the machine answers on
	Tags     map[string]string `json:"tags,omitempty"`
	Detail   map[string]string `json:"detail,omitempty"`
}

// Provider is one place agents get machines from. Provision creates the machine, waits until it answers,
// registers it as an executor and returns it; when it returns an error together with a Machine, the
// provider left resources behind and the caller releases them. Release destroys the machine (the executor
// row is removed by the caller). Probe reports whether the machine still runs.
type Provider interface {
	Name() string
	Provision(ctx context.Context, req ProvisionRequest) (*Machine, error)
	Release(ctx context.Context, m *Machine) error
	Probe(ctx context.Context, m *Machine) error
}

var (
	providersMu sync.RWMutex
	providerSet = map[string]Provider{}
)

// RegisterProvider makes a provider available under its Name; registering the same name again replaces it
// (tests swap in a fake).
func RegisterProvider(p Provider) {
	providersMu.Lock()
	defer providersMu.Unlock()
	providerSet[p.Name()] = p
}

// providers lists the registered providers sorted by name.
func providers() []Provider {
	providersMu.RLock()
	defer providersMu.RUnlock()
	out := make([]Provider, 0, len(providerSet))
	for _, p := range providerSet {
		out = append(out, p)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Name() < out[j].Name() })
	return out
}

func providerNamed(name string) Provider {
	providersMu.RLock()
	defer providersMu.RUnlock()
	return providerSet[name]
}

// ProviderNames is what `pulse compute providers` prints.
func ProviderNames() []string {
	var names []string
	for _, p := range providers() {
		names = append(names, p.Name())
	}
	return names
}

// computeExecutorName: the executor a request's machine registers as, e.g. hands-cr-007.
func computeExecutorName(requestID string) string {
	return slugify("hands-"+strings.ToLower(requestID), 40)
}

var wordRe = regexp.MustCompile(`\S+`)

// redactSecrets strips token-shaped words (store.SecretShaped, the Python secret_shaped rule) and GitHub
// tokens before a provider's error reaches the board or the log.
func redactSecrets(text string) string {
	text = redactTokens(text)
	return wordRe.ReplaceAllStringFunc(text, func(word string) string {
		bare := strings.Trim(word, `"'(),;:`)
		if bare != "" && store.SecretShaped(bare) {
			return "[redacted]"
		}
		return word
	})
}

// ---- machine memory ---------------------------------------------------------------------------------

func saveMachine(s *store.Store, requestID string, m *Machine, now string) {
	detail, _ := json.Marshal(map[string]any{"tags": m.Tags, "detail": m.Detail})
	exec1(s.DB, "INSERT INTO compute_machines (request_id, provider, name, executor, address, detail, created_at) VALUES (?, ?, ?, ?, ?, ?, ?) "+
		"ON CONFLICT(request_id) DO UPDATE SET provider = excluded.provider, name = excluded.name, executor = excluded.executor, "+
		"address = excluded.address, detail = excluded.detail, created_at = excluded.created_at, released_at = '', attempts = 0, note = ''",
		requestID, m.Provider, m.Name, m.Executor, m.Address, string(detail), now)
}

// loadMachine returns the machine a request produced and its row (created_at is when it became ready).
func loadMachine(s *store.Store, requestID string) (*Machine, row) {
	r := mustOne(s.DB, "SELECT * FROM compute_machines WHERE request_id = ?", requestID)
	if r == nil {
		return nil, nil
	}
	m := &Machine{Provider: r.str("provider"), Name: r.str("name"), Executor: r.str("executor"), Address: r.str("address")}
	var detail struct {
		Tags   map[string]string `json:"tags"`
		Detail map[string]string `json:"detail"`
	}
	if err := json.Unmarshal([]byte(r.str("detail")), &detail); err == nil {
		m.Tags, m.Detail = detail.Tags, detail.Detail
	}
	return m, r
}

func machineReleased(s *store.Store, requestID, now string) {
	exec1(s.DB, "UPDATE compute_machines SET released_at = ? WHERE request_id = ?", now, requestID)
}

func machineReleaseFailed(s *store.Store, requestID, note string) int {
	exec1(s.DB, "UPDATE compute_machines SET attempts = attempts + 1, note = ? WHERE request_id = ?", trunc(note, 500), requestID)
	return mustOne(s.DB, "SELECT attempts FROM compute_machines WHERE request_id = ?", requestID).int("attempts")
}

// ---- the daemon's compute worker ---------------------------------------------------------------------

type computeOpts struct {
	Actor   string
	HubHome string
}

// computeWorker is the daemon's compute goroutine set: a subscriber on compute.approved that nudges the
// loop, one goroutine per provisioning or release in flight, and tick() which the loop calls to start them
// from the durable rows (so a request approved while no daemon ran is picked up at the next start).
type computeWorker struct {
	s        *store.Store
	o        computeOpts
	mu       sync.Mutex
	inflight map[string]bool
	wg       sync.WaitGroup
	wake     chan struct{}
	lines    chan string
}

func newComputeWorker(ctx context.Context, s *store.Store, o computeOpts) *computeWorker {
	if o.Actor == "" {
		o.Actor = "daemon"
	}
	w := &computeWorker{s: s, o: o, inflight: map[string]bool{}, wake: make(chan struct{}, 1), lines: make(chan string, 64)}
	events, cancel := bus.Default().Subscribe(bus.Filter{Actions: []string{"compute.approved"}}, 16)
	go func() {
		defer cancel()
		for {
			select {
			case <-ctx.Done():
				return
			case _, ok := <-events:
				if !ok {
					return
				}
				w.nudge()
			}
		}
	}()
	return w
}

func (w *computeWorker) nudge() {
	select {
	case w.wake <- struct{}{}:
	default:
	}
}

func (w *computeWorker) say(format string, a ...any) {
	select {
	case w.lines <- fmt.Sprintf(format, a...):
	default:
	}
}

// drain returns the lines the goroutines reported since the last call.
func (w *computeWorker) drain() []string {
	var out []string
	for {
		select {
		case l := <-w.lines:
			out = append(out, l)
		default:
			return out
		}
	}
}

func (w *computeWorker) claim(id string) bool {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.inflight[id] {
		return false
	}
	w.inflight[id] = true
	return true
}

func (w *computeWorker) done(id string) {
	w.mu.Lock()
	defer w.mu.Unlock()
	delete(w.inflight, id)
}

func (w *computeWorker) busy() int {
	w.mu.Lock()
	defer w.mu.Unlock()
	return len(w.inflight)
}

func (w *computeWorker) start(id string, fn func()) {
	if !w.claim(id) {
		return
	}
	w.wg.Add(1)
	go func() {
		defer w.wg.Done()
		defer w.done(id)
		defer func() {
			if r := recover(); r != nil {
				w.say("compute %s: daemon error: %v", id, r)
			}
		}()
		fn()
	}()
}

// wait blocks until the goroutines in flight are done or the grace is over (0 = no limit).
func (w *computeWorker) wait(grace time.Duration) {
	finished := make(chan struct{})
	go func() {
		w.wg.Wait()
		close(finished)
	}()
	if grace <= 0 {
		<-finished
		return
	}
	select {
	case <-finished:
	case <-time.After(grace):
	}
}

// tick is one pass over the durable rows: approved -> provision, ready -> release when due, provisioning
// rows no goroutine here owns -> failed (a previous daemon died mid-way).
func (w *computeWorker) tick(ctx context.Context, now string) []string {
	if ctx.Err() != nil {
		return w.drain()
	}
	s := w.s
	approved, err := tasks.ListCompute(s, "", "approved")
	if err != nil {
		w.say("compute: %s", errMessage(err))
	}
	for _, req := range approved {
		req := req
		w.start(req.ID, func() { w.provision(ctx, req) })
	}
	stale, _ := tasks.ListCompute(s, "", "provisioning")
	for _, req := range stale {
		w.mu.Lock()
		owned := w.inflight[req.ID]
		w.mu.Unlock()
		if owned {
			continue
		}
		req := req
		w.start(req.ID, func() {
			why := "the daemon stopped while this machine was being provisioned"
			if m, _ := loadMachine(s, req.ID); m != nil {
				if p := providerNamed(m.Provider); p != nil {
					rctx, cancel := context.WithTimeout(context.Background(), computeReleaseBudget)
					if err := p.Release(rctx, m); err == nil {
						why += "; the machine was released"
					} else {
						why += "; releasing it failed: " + redactSecrets(errMessage(err))
					}
					cancel()
				}
			} else {
				why += fmt.Sprintf("; look for a machine tagged pulse=%s at the provider", req.ID)
			}
			w.fail(req, why)
		})
	}
	ready, _ := tasks.ListCompute(s, "", "ready")
	for _, req := range ready {
		req := req
		why := w.releaseDue(req, now)
		if why == "" {
			continue
		}
		w.start(req.ID, func() { w.release(ctx, req, why) })
	}
	return w.drain()
}

func (w *computeWorker) finding(req tasks.ComputeRequest, summary, body string) {
	_, err := createPost(w.s, w.s.Now(), postOpts{Kind: "finding", Project: req.Project, Area: computeArea, Thread: computeThread,
		Summary: trunc(summary, 230), Body: body, Agent: actorAgent(w.s, w.o.Actor), On: req.ProposalPost})
	if err != nil {
		w.say("compute %s: finding not posted: %s", req.ID, errMessage(err))
	}
}

func (w *computeWorker) fail(req tasks.ComputeRequest, why string) {
	why = redactSecrets(why)
	if _, err := tasks.SetComputeStatus(w.s, req.ID, "failed", "", w.o.Actor); err != nil {
		w.say("compute %s: %s", req.ID, errMessage(err))
	}
	w.finding(req, fmt.Sprintf("compute %s failed: %s", req.ID, why),
		fmt.Sprintf("The request by %s for one %s machine for %d hour(s) (%s) could not be served.\n\nReason: %s\n\n"+
			"Nothing was registered. A new request needs `pulse compute request`; the owner may adjust the caps with "+
			"`pulse settings set compute.max_vms N` / `compute.max_hours_per_day N`.\n", req.RequestedBy, req.Size, req.Hours, req.Reason, why))
	w.say("compute %s failed: %s", req.ID, why)
}

// capsAllow: the request must fit both caps counting itself (an approved row is already in VMsNow; its
// hours are in HoursToday when it was made today).
func capsAllow(caps tasks.Caps, req tasks.ComputeRequest, now string) string {
	if caps.MaxVMs <= 0 || caps.VMsNow > caps.MaxVMs {
		return fmt.Sprintf("cap: %d machine(s) approved, provisioning or ready against compute.max_vms=%d", caps.VMsNow, caps.MaxVMs)
	}
	used := caps.HoursToday
	if len(req.At) >= 10 && len(now) >= 10 && req.At[:10] != now[:10] {
		used += req.Hours
	}
	if caps.MaxHoursPerDay <= 0 || used > caps.MaxHoursPerDay {
		return fmt.Sprintf("cap: %d machine-hour(s) today against compute.max_hours_per_day=%d", used, caps.MaxHoursPerDay)
	}
	return ""
}

func (w *computeWorker) provision(ctx context.Context, req tasks.ComputeRequest) {
	s := w.s
	if cur, err := tasks.GetCompute(s, req.ID); err != nil || cur.Status != "approved" {
		return
	}
	caps, err := tasks.GetCaps(s)
	if err != nil {
		w.fail(req, errMessage(err))
		return
	}
	if why := capsAllow(caps, req, s.Now()); why != "" {
		w.fail(req, why)
		return
	}
	p := providerNamed(caps.Provider)
	if p == nil {
		w.fail(req, fmt.Sprintf("no compute provider %s is built in (have: %s)", pyRepr(caps.Provider), strings.Join(ProviderNames(), ", ")))
		return
	}
	if _, err := tasks.SetComputeStatus(s, req.ID, "provisioning", "", w.o.Actor); err != nil {
		w.say("compute %s: %s", req.ID, errMessage(err))
		return
	}
	w.say("compute %s: provisioning one %s machine through %s", req.ID, req.Size, p.Name())
	pctx, cancel := context.WithTimeout(ctx, computeProvisionBudget)
	defer cancel()
	m, err := p.Provision(pctx, ProvisionRequest{ID: req.ID, Project: req.Project, Size: req.Size, Hours: req.Hours, Reason: req.Reason,
		Executor: computeExecutorName(req.ID), Store: s, Actor: w.o.Actor, HubHome: w.o.HubHome,
		Log: func(format string, a ...any) { w.say("compute "+req.ID+": "+format, a...) }})
	if err != nil {
		why := errMessage(err)
		if pctx.Err() != nil {
			why = fmt.Sprintf("gave up after %s: %s", computeProvisionBudget, why)
		}
		if m != nil {
			rctx, rcancel := context.WithTimeout(context.Background(), computeReleaseBudget)
			if rerr := p.Release(rctx, m); rerr != nil {
				why += "; releasing what was created failed too: " + errMessage(rerr)
			} else {
				why += "; what was created is released"
			}
			rcancel()
		}
		w.fail(req, why)
		return
	}
	if m.Provider == "" {
		m.Provider = p.Name()
	}
	now := s.Now()
	saveMachine(s, req.ID, m, now)
	if _, err := tasks.SetComputeStatus(s, req.ID, "ready", m.Executor, w.o.Actor); err != nil {
		w.say("compute %s: %s", req.ID, errMessage(err))
		return
	}
	w.finding(req, fmt.Sprintf("compute %s ready: executor %s (%s %s) for %d hour(s)", req.ID, m.Executor, m.Provider, m.Name, req.Hours),
		fmt.Sprintf("The machine %s asked for is up: `%s` on %s, registered as executor `%s` for project `%s` "+
			"(approved by %s). The daemon releases it when its %d hour(s) are over or after %d idle minutes, whichever comes first; "+
			"`pulse executor list` shows it meanwhile.\n\nReason given: %s\n", req.RequestedBy, m.Name, m.Provider, m.Executor, req.Project,
			req.DecidedBy, req.Hours, computeIdleMinutes, req.Reason))
	w.say("compute %s ready: executor %s", req.ID, m.Executor)
}

// releaseDue says why a ready request's machine should go now, or "" to keep it: its hours are up, or the
// executor sat idle computeIdleMinutes since it became ready or since its last run ended. A run in progress
// always keeps it.
func (w *computeWorker) releaseDue(req tasks.ComputeRequest, now string) string {
	s := w.s
	m, mrow := loadMachine(s, req.ID)
	readyAt := req.UpdatedAt
	if mrow != nil && mrow.str("created_at") != "" {
		readyAt = mrow.str("created_at")
	}
	executor := req.Executor
	if m != nil && m.Executor != "" {
		executor = m.Executor
	}
	if executor != "" && executorRunning(s, executor, now) > 0 {
		return ""
	}
	if readyAt == "" {
		return ""
	}
	age := mustISO(now).Sub(mustISO(readyAt))
	if req.Hours > 0 && age >= time.Duration(req.Hours)*time.Hour {
		return fmt.Sprintf("its %d hour(s) are up", req.Hours)
	}
	last := readyAt
	if executor != "" {
		if r := mustOne(s.DB, "SELECT MAX(finished_at) AS at FROM runs WHERE executor = ? AND finished_at IS NOT NULL AND finished_at != ''", executor); r != nil && r.str("at") > last {
			last = r.str("at")
		}
	}
	if mustISO(now).Sub(mustISO(last)) >= computeIdleMinutes*time.Minute {
		return fmt.Sprintf("the executor sat idle for %d minutes", computeIdleMinutes)
	}
	return ""
}

func (w *computeWorker) release(ctx context.Context, req tasks.ComputeRequest, why string) {
	s := w.s
	m, _ := loadMachine(s, req.ID)
	if m == nil {
		m = &Machine{Provider: s.Setting("compute.provider", ""), Executor: req.Executor}
	}
	if m.Name != "" {
		if p := providerNamed(m.Provider); p != nil {
			rctx, cancel := context.WithTimeout(context.Background(), computeReleaseBudget)
			err := p.Release(rctx, m)
			cancel()
			if err != nil {
				attempts := machineReleaseFailed(s, req.ID, errMessage(err))
				w.say("compute %s: release attempt %d failed: %s", req.ID, attempts, redactSecrets(errMessage(err)))
				if attempts == 1 {
					w.finding(req, fmt.Sprintf("compute %s: releasing %s failed (%s); the daemon retries", req.ID, m.Name, why),
						fmt.Sprintf("`%s` on %s should be released because %s, but the provider refused: %s\n\nThe daemon retries every loop; "+
							"a human may delete it by hand (its tag is pulse=%s).\n", m.Name, m.Provider, why, redactSecrets(errMessage(err)), req.ID))
				}
				return
			}
		}
	}
	if m.Executor != "" && executorRow(s, m.Executor) != nil {
		if _, err := board.RemoveExecutor(s, m.Executor, w.o.Actor, false); err != nil {
			w.say("compute %s: executor %s not removed: %s", req.ID, m.Executor, errMessage(err))
		}
	}
	now := s.Now()
	machineReleased(s, req.ID, now)
	if _, err := tasks.SetComputeStatus(s, req.ID, "released", "", w.o.Actor); err != nil {
		w.say("compute %s: %s", req.ID, errMessage(err))
		return
	}
	w.finding(req, fmt.Sprintf("compute %s released: %s", req.ID, why),
		fmt.Sprintf("The machine `%s` (%s) that served %s's request is gone because %s; executor `%s` is removed. "+
			"Runs it made stay in the log.\n", m.Name, m.Provider, req.RequestedBy, why, m.Executor))
	w.say("compute %s released: %s", req.ID, why)
	_ = ctx
}
