package runner

// The aws, gcp and multipass compute providers against fake CLIs: the provision flow up to a ready executor
// and its release, the retire and delete passes (ADR-034) through the Provider interface, and the shared
// pieces (cloud-init, host key parsing, sizes).

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

// fakeCLI records every argv and answers through handle; a secret-shaped argument fails the call, the way
// the Azure fake does. A key=value argument is judged by its value (gcloud's user-data=PATH), and an SSM
// parameter reference is a path, not a secret.
type fakeCLI struct {
	mu     sync.Mutex
	calls  [][]string
	handle func(cmd string, args []string) (int, string, string)
}

func (f *fakeCLI) run(_ context.Context, args []string, _ time.Duration) (int, string, string, error) {
	f.mu.Lock()
	f.calls = append(f.calls, args)
	f.mu.Unlock()
	for _, a := range args {
		value := strings.TrimPrefix(a, "resolve:ssm:")
		if k, v, ok := strings.Cut(a, "="); ok && !strings.ContainsAny(k, " ,{[") {
			value = v
		}
		if store.SecretShaped(value) {
			return 1, "", "", errors.New("secret-shaped argument reached the CLI: " + a)
		}
	}
	rc, out, errText := f.handle(strings.Join(args, " "), args)
	return rc, out, errText, nil
}

func (f *fakeCLI) joined() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	var out []string
	for _, c := range f.calls {
		out = append(out, strings.Join(c, " "))
	}
	return out
}

func (f *fakeCLI) has(prefix string) bool { return f.count(prefix) > 0 }

func (f *fakeCLI) count(prefix string) int {
	n := 0
	for _, c := range f.joined() {
		if strings.HasPrefix(c, prefix) {
			n++
		}
	}
	return n
}

func (f *fakeCLI) first(prefix string) string {
	for _, c := range f.joined() {
		if strings.HasPrefix(c, prefix) {
			return c
		}
	}
	return ""
}

func (f *fakeCLI) reset() {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = nil
}

const testHostKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE root@vm"

// computeHarness is what every provision test shares: a hub key, the settings, a fast poll, no real ssh, a
// pairing that registers the executor, a login source and a login placement that succeeds.
type computeHarness struct {
	hubHome string
	placed  *placeLoginStub
	paired  struct {
		sync.Mutex
		admin, name string
		opts        pairOpts
	}
	ssh []string
}

func newComputeHarness(t *testing.T, s *store.Store, settings map[string]string) *computeHarness {
	t.Helper()
	bus.ResetDefault()
	newRepo(t, s)
	join(t, s, "aurora")
	h := &computeHarness{hubHome: t.TempDir()}
	if err := os.MkdirAll(filepath.Join(h.hubHome, ".ssh"), 0o700); err != nil {
		t.Fatal(err)
	}
	os.WriteFile(filepath.Join(h.hubHome, ".ssh", "pulse_hands_ed25519"), []byte("fake private key\n"), 0o600)
	os.WriteFile(filepath.Join(h.hubHome, ".ssh", "pulse_hands_ed25519.pub"), []byte("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUB pulse-hub\n"), 0o644)
	for k, v := range map[string]string{"compute.max_vms": "2", "compute.max_hours_per_day": "24", "minutes_per_job": "1"} {
		setSetting(t, s, k, v)
	}
	for k, v := range settings {
		setSetting(t, s, k, v)
	}
	oldWait, oldSSH, oldPair := provisionWait, runSSH, pairHands
	t.Cleanup(func() { provisionWait, runSSH, pairHands = oldWait, oldSSH, oldPair })
	provisionWait = 5 * time.Millisecond
	writeExecutorToken(t, s)
	h.placed = stubPlaceLogin(t, true)
	var mu sync.Mutex
	runSSH = func(_ context.Context, argv []string, stdin []byte, _ time.Duration) (int, string, string, error) {
		mu.Lock()
		h.ssh = append(h.ssh, strings.Join(argv, " "))
		mu.Unlock()
		return 0, "pulse-ready\n", "", nil
	}
	pairHands = func(st *store.Store, admin, name, now string, o pairOpts) (*pairReport, error) {
		h.paired.Lock()
		defer h.paired.Unlock()
		h.paired.admin, h.paired.name, h.paired.opts = admin, name, o
		if _, err := addExecutor(st, now, name, admin, o.Workdir, o.Workers, o.Projects, humanAgent); err != nil {
			return nil, err
		}
		exec1(st.DB, "UPDATE executors SET status = 'active' WHERE name = ?", name)
		return &pairReport{OK: true, Lines: []string{"paired " + name}}, nil
	}
	return h
}

// provisionAndRelease drives one request from approval to ready and, three hours later, to released, and
// checks everything the providers share; the caller checks the CLI calls.
func (h *computeHarness) provisionAndRelease(t *testing.T, s *store.Store, provider, ip string) {
	t.Helper()
	req, err := tasks.RequestCompute(s, "proj", "aurora", "", 2, "the fixtures take an hour here")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := tasks.Approve(s, req.ID, "go ahead", humanAgent); err != nil {
		t.Fatal(err)
	}
	log := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'", HubHome: h.hubHome})
	got, err := tasks.GetCompute(s, req.ID)
	if err != nil || got.Status != "ready" || got.Executor != "hands-cr-001" {
		t.Fatalf("after provisioning: %+v %v\n%s", got, err, log)
	}
	h.paired.Lock()
	if h.paired.name != "hands-cr-001" || h.paired.admin != "pulseadmin@pulse-cr-001-admin" || h.paired.opts.Projects != "proj" || h.paired.opts.InstallCopilot || h.paired.opts.Workdir != handsWorkdir {
		t.Fatalf("pairing: %s %s %+v", h.paired.admin, h.paired.name, h.paired.opts)
	}
	h.paired.Unlock()
	ex := executorRow(s, "hands-cr-001")
	if ex == nil || ex.str("status") != "active" || ex.str("admin") != "pulseadmin@"+ip {
		t.Fatalf("executor: %v", ex)
	}
	if calls := h.placed.calls(); len(calls) != 1 || calls[0].admin != "pulseadmin@"+ip {
		t.Fatalf("login placement: %+v", calls)
	}
	known, _ := os.ReadFile(filepath.Join(h.hubHome, ".ssh", "known_hosts"))
	if !strings.Contains(string(known), ip+" "+strings.Join(strings.Fields(testHostKey)[:2], " ")) {
		t.Fatalf("known_hosts: %q", known)
	}
	sshCfg, _ := os.ReadFile(filepath.Join(h.hubHome, ".ssh", "config"))
	if !strings.Contains(string(sshCfg), "Host pulse-cr-001-admin") || !strings.Contains(string(sshCfg), "HostName "+ip) {
		t.Fatalf("ssh config: %q", sshCfg)
	}
	m, mrow := loadMachine(s, req.ID)
	if m == nil || m.Provider != provider || m.Name != "pulse-cr-001" || m.Tags["pulse"] != "hands" || m.Tags["pulse-request"] != req.ID ||
		m.Detail["public_ip"] != ip || m.Address != "pulseadmin@"+ip || mrow.str("released_at") != "" {
		t.Fatalf("machine memory: %+v %v", m, mrow)
	}
	if len(h.ssh) == 0 || !strings.Contains(h.ssh[0], "cloud-init status --wait") {
		t.Fatalf("ssh: %v", h.ssh)
	}
	later, err := store.Open(store.Options{Home: s.Home, Now: "2026-09-06T13:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	defer later.Close()
	log3 := onceDaemon(t, later, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'", HubHome: h.hubHome})
	if got, _ := tasks.GetCompute(later, req.ID); got.Status != "released" {
		t.Fatalf("after the hours: %+v\n%s", got, log3)
	}
	if executorRow(later, "hands-cr-001") != nil {
		t.Fatal("executor still registered")
	}
	if _, mrow := loadMachine(later, req.ID); mrow.str("released_at") == "" {
		t.Fatalf("machine not marked released: %v", mrow)
	}
}

// -- the shared pieces ---------------------------------------------------------------------------------------

func TestProvidersRegistered(t *testing.T) {
	names := ProviderNames()
	want := append([]string{}, store.ComputeProviders...)
	sort.Strings(want)
	if strings.Join(names, ",") != strings.Join(want, ",") {
		t.Fatalf("providers %v, settings know %v", names, want)
	}
	kinds := map[string]bool{}
	for _, n := range names {
		p := providerNamed(n)
		if p == nil || p.Name() != n || p.Kind() == "" || kinds[p.Kind()] {
			t.Fatalf("provider %s: %v", n, p)
		}
		kinds[p.Kind()] = true
		if providerAuth[n] == "" || store.ComputeSizeSetting(n) == "" || store.ComputeSettings[store.ComputeSizeSetting(n)] == "" {
			t.Fatalf("provider %s: no auth line or size setting default", n)
		}
	}
}

func TestHostKeyLines(t *testing.T) {
	console := "[   12.345] cloud-init[812]: -----BEGIN SSH HOST KEY KEYS-----\n" +
		"ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBFAKE root@pulse\n" +
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKB root@pulse\n" +
		"-----END SSH HOST KEY KEYS-----\n" +
		"some other noise\n" +
		"pulse-hostkey ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKB root@pulse\n"
	keys := hostKeyLines(console)
	if len(keys) != 1 || !strings.HasPrefix(keys[0], "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBLOCK") {
		t.Fatalf("keys: %q", keys)
	}
	if keys := hostKeyLines("Enable succeeded: \n[stdout]\n" + testHostKey + "\n[stderr]\n"); len(keys) != 1 || keys[0] != strings.Join(strings.Fields(testHostKey)[:2], " ") {
		t.Fatalf("run-command shape: %q", keys)
	}
	if keys := hostKeyLines("nothing yet\n"); len(keys) != 0 {
		t.Fatalf("no key: %q", keys)
	}
}

func TestCloudInitConfigAdminKey(t *testing.T) {
	key := "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUB pulse-hub"
	with := cloudInitConfig(key, true)
	for _, want := range []string{"#cloud-config", "name: " + adminUser, "- " + key, "ALL=(ALL) NOPASSWD:ALL", hostkeyMarker, "/dev/console", "mkdir -p " + handsWorkdir, "@github/copilot"} {
		if !strings.Contains(with, want) {
			t.Fatalf("cloud-init lacks %q:\n%s", want, with)
		}
	}
	without := cloudInitConfig("", true)
	if strings.Contains(without, "name: "+adminUser) || strings.Contains(without, "ssh_authorized_keys") {
		t.Fatalf("cloud-init without a key still adds a user:\n%s", without)
	}
	if !strings.Contains(without, hostkeyMarker) {
		t.Fatal("host key line missing without a key")
	}
	native := cloudInitConfig(key, false)
	for _, absent := range []string{"deb.nodesource.com", "apt-get install -y nodejs", "npm install -g @github/copilot"} {
		if strings.Contains(native, absent) {
			t.Fatalf("native cloud-init has legacy install line %q:\n%s", absent, native)
		}
	}
	if !strings.Contains(native, "mkdir -p "+handsWorkdir) || !strings.Contains(native, hostkeyMarker) {
		t.Fatalf("native cloud-init lacks base setup:\n%s", native)
	}
	path, err := writeCloudInit(key, true)
	if err != nil {
		t.Fatal(err)
	}
	defer os.Remove(path)
	data, _ := os.ReadFile(path)
	if string(data) != with {
		t.Fatal("written cloud-init differs from the rendered one")
	}
}

func TestMultipassSizeParsing(t *testing.T) {
	s := newStore(t)
	cfg, err := multipassSettings(s, "")
	if err != nil || cfg.CPUs != "2" || cfg.Memory != "4G" || cfg.Disk != "32G" || cfg.Image != "24.04" {
		t.Fatalf("defaults: %+v %v", cfg, err)
	}
	cfg, err = multipassSettings(s, "cpus=4, memory=8GiB")
	if err != nil || cfg.CPUs != "4" || cfg.Memory != "8GiB" || cfg.Disk != "" {
		t.Fatalf("subset: %+v %v", cfg, err)
	}
	for _, bad := range []string{"cpus=0", "mem=lots", "disk=32X", "size=big", "cpus", "cpus=2,,mem="} {
		if _, err := multipassSettings(s, bad); err == nil {
			t.Fatalf("%q accepted", bad)
		}
	}
	if !multipassBytes("512M") || !multipassBytes("1.5G") || !multipassBytes("20GB") || multipassBytes("G") || multipassBytes("4TB") {
		t.Fatal("multipassBytes mismatch")
	}
}

func TestGcpLabel(t *testing.T) {
	if got := gcpLabel("CR-001"); got != "cr-001" {
		t.Fatalf("label: %q", got)
	}
	if got := gcpLabel("Proj.Name/x"); got != "proj-name-x" {
		t.Fatalf("label: %q", got)
	}
	if got := gcpLabel(strings.Repeat("a", 70)); len(got) != 63 {
		t.Fatalf("label length: %d", len(got))
	}
}

// -- aws ------------------------------------------------------------------------------------------------------

// fakeAws is an EC2 account with a default VPC and no pulse-hands group yet; the instance it runs comes up on
// the second describe, and its console shows the host key on the second read.
type fakeAws struct {
	fakeCLI
	instances map[string]*awsFake // by instance id
	describes int
	consoles  int
	terminate []string
}

type awsFake struct {
	id, name, request, project, state, ip string
}

func (f *fakeAws) instanceJSON(i *awsFake) string {
	ip := ""
	if i.ip != "" {
		ip = `"PublicIpAddress": "` + i.ip + `",`
	}
	return fmt.Sprintf(`{"InstanceId": %q, %s "InstanceType": "t3.medium", "SubnetId": "subnet-1", "State": {"Name": %q},
		"Tags": [{"Key": "Name", "Value": %q}, {"Key": "pulse", "Value": "hands"}, {"Key": "pulse-request", "Value": %q}, {"Key": "pulse-project", "Value": %q}],
		"SecurityGroups": [{"GroupId": "sg-pulse"}]}`, i.id, ip, i.state, i.name, i.request, i.project)
}

func newFakeAws() *fakeAws {
	f := &fakeAws{instances: map[string]*awsFake{}}
	f.handle = func(cmd string, args []string) (int, string, string) {
		switch {
		case strings.HasPrefix(cmd, "sts get-caller-identity"):
			return 0, `{"Account": "123456789012", "Arn": "arn:aws:iam::123456789012:user/pulse"}`, ""
		case strings.HasPrefix(cmd, "ec2 describe-vpcs"):
			return 0, `{"Vpcs": [{"VpcId": "vpc-1"}]}`, ""
		case strings.HasPrefix(cmd, "ec2 describe-subnets"):
			return 0, `{"Subnets": [{"SubnetId": "subnet-1", "VpcId": "vpc-1"}]}`, ""
		case strings.HasPrefix(cmd, "ec2 describe-security-groups"):
			return 0, `{"SecurityGroups": []}`, ""
		case strings.HasPrefix(cmd, "ec2 create-security-group"):
			return 0, `{"GroupId": "sg-pulse"}`, ""
		case strings.HasPrefix(cmd, "ec2 authorize-security-group-ingress"):
			return 0, `{"Return": true}`, ""
		case strings.HasPrefix(cmd, "ec2 run-instances"):
			tags := argAfter(args, "--tag-specifications")
			pick := func(key string) string {
				_, after, _ := strings.Cut(tags, "{Key="+key+",Value=")
				v, _, _ := strings.Cut(after, "}")
				return v
			}
			i := &awsFake{id: "i-0abc", name: pick("Name"), request: pick("pulse-request"), project: pick("pulse-project"), state: "pending"}
			f.instances[i.id] = i
			return 0, `{"Instances": [` + f.instanceJSON(i) + `]}`, ""
		case strings.HasPrefix(cmd, "ec2 describe-instances"):
			f.describes++
			var out []string
			for _, i := range f.instances {
				if !awsFilterMatch(args, i) {
					continue
				}
				if i.state == "pending" && f.describes >= 2 {
					i.state, i.ip = "running", "203.0.113.9"
				}
				out = append(out, f.instanceJSON(i))
			}
			return 0, `{"Reservations": [{"Instances": [` + strings.Join(out, ",") + `]}]}`, ""
		case strings.HasPrefix(cmd, "ec2 get-console-output"):
			f.consoles++
			if f.consoles < 2 {
				return 0, "None\n", ""
			}
			return 0, "[    0.000000] Linux version 6.8\n" + hostkeyMarker + testHostKey + "\ncloud-init done\n", ""
		case strings.HasPrefix(cmd, "ec2 stop-instances"):
			if i := f.instances[argAfter(args, "--instance-ids")]; i != nil {
				i.state = "stopped"
			}
			return 0, `{"StoppingInstances": []}`, ""
		case strings.HasPrefix(cmd, "ec2 terminate-instances"):
			id := argAfter(args, "--instance-ids")
			f.terminate = append(f.terminate, id)
			if i := f.instances[id]; i != nil {
				i.state = "terminated"
			}
			return 0, `{"TerminatingInstances": []}`, ""
		}
		return 1, "", "unexpected aws " + cmd
	}
	return f
}

// awsFilterMatch applies the describe-instances filters the provider uses.
func awsFilterMatch(args []string, i *awsFake) bool {
	for _, a := range args {
		name, values, ok := strings.Cut(strings.TrimPrefix(a, "Name="), ",Values=")
		if !ok || !strings.HasPrefix(a, "Name=") {
			continue
		}
		have := ""
		switch name {
		case "instance-id":
			have = i.id
		case "instance-state-name":
			have = i.state
		case "tag:pulse":
			have = "hands"
		case "tag:pulse-request":
			have = i.request
		case "tag:Name":
			have = i.name
		default:
			return false
		}
		if !contains(strings.Split(values, ","), have) {
			return false
		}
	}
	return true
}

func installFake(t *testing.T, target *cliRunner, f cliRunner) {
	t.Helper()
	old := *target
	t.Cleanup(func() { *target = old })
	*target = f
}

func TestDaemonOnceComputeAWS(t *testing.T) {
	s := newStore(t)
	h := newComputeHarness(t, s, map[string]string{"compute.provider": "aws", "compute.aws.region": "eu-west-1", "compute.aws.profile": "pulse"})
	aws := newFakeAws()
	installFake(t, &runAws, aws.run)
	h.provisionAndRelease(t, s, "aws", "203.0.113.9")

	run := aws.first("ec2 run-instances")
	for _, want := range []string{
		"--image-id resolve:ssm:" + awsImage, "--instance-type t3.medium", "--subnet-id subnet-1", "--security-group-ids sg-pulse",
		"--associate-public-ip-address", "--user-data file://", "VolumeSize=" + handsDiskGB, "HttpTokens=required",
		"Tags=[{Key=Name,Value=pulse-cr-001},{Key=pulse,Value=hands},{Key=pulse-request,Value=CR-001},{Key=pulse-project,Value=proj}]",
		"--profile pulse --region eu-west-1 --output json",
	} {
		if !strings.Contains(run, want) {
			t.Fatalf("run-instances lacks %q: %s", want, run)
		}
	}
	if !aws.has("ec2 create-security-group --group-name pulse-hands") || !aws.has("ec2 authorize-security-group-ingress --group-id sg-pulse --protocol tcp --port 22 --cidr 0.0.0.0/0") {
		t.Fatalf("security group: %q", aws.joined())
	}
	if aws.consoles != 2 || !aws.has("ec2 get-console-output --instance-id i-0abc --latest --query Output --profile pulse --region eu-west-1 --output text") {
		t.Fatalf("console reads %d: %q", aws.consoles, aws.joined())
	}
	if len(aws.terminate) != 1 || aws.terminate[0] != "i-0abc" {
		t.Fatalf("terminated: %v", aws.terminate)
	}
	m, _ := loadMachine(s, "CR-001")
	if m.Detail["instance_id"] != "i-0abc" || m.Detail["region"] != "eu-west-1" || m.Detail["security_group"] != "sg-pulse" || m.Detail["subnet"] != "subnet-1" {
		t.Fatalf("detail: %v", m.Detail)
	}
}

func TestAWSProvisionNeedsLoginAndRegion(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	f := &fakeCLI{handle: func(cmd string, _ []string) (int, string, string) {
		if strings.HasPrefix(cmd, "sts get-caller-identity") {
			return 255, "", "Unable to locate credentials. You can configure credentials by running \"aws configure\"."
		}
		return 0, "", ""
	}}
	installFake(t, &runAws, f.run)
	_, err := awsProvider{}.Provision(context.Background(), ProvisionRequest{Store: s, ID: "CR-001", Executor: "hands-cr-001", Project: "proj", HubHome: t.TempDir()})
	if err == nil || !strings.Contains(err.Error(), "not logged in to AWS") || !strings.Contains(err.Error(), "aws configure") {
		t.Fatalf("err: %v", err)
	}
	f.handle = func(cmd string, _ []string) (int, string, string) {
		switch {
		case strings.HasPrefix(cmd, "sts get-caller-identity"):
			return 0, `{"Account": "1", "Arn": "arn"}`, ""
		case strings.HasPrefix(cmd, "configure get region"):
			return 1, "", ""
		}
		return 1, "", "unexpected"
	}
	_, err = awsProvider{}.Provision(context.Background(), ProvisionRequest{Store: s, ID: "CR-001", Executor: "hands-cr-001", Project: "proj", HubHome: t.TempDir()})
	if err == nil || !strings.Contains(err.Error(), "compute.aws.region") {
		t.Fatalf("err: %v", err)
	}
}

func TestTendRetiresAndDeletesOurEC2Instance(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "compute.provider", "aws")
	setSetting(t, s, "compute.aws.region", "eu-west-1")
	drainedByLogin(t, s, "vm-seven", "pulse@vm-seven")
	aws := newFakeAws()
	aws.instances["i-0777"] = &awsFake{id: "i-0777", name: "pulse-cr-007", request: "CR-007", project: "proj", state: "running", ip: "203.0.113.7"}
	aws.describes = 5
	installFake(t, &runAws, aws.run)
	oldHost := sshHostname
	t.Cleanup(func() { sshHostname = oldHost })
	sshHostname = func(host string) string { return "203.0.113.7" }
	stubGh(t, "")

	lines := tendExecutors(s, testNow, testDaemon)
	calls := aws.joined()
	if len(calls) != 2 || calls[0] != "ec2 describe-instances --filters Name=tag:pulse,Values=hands Name=instance-state-name,Values=pending,running,stopping,stopped --region eu-west-1 --output json" ||
		calls[1] != "ec2 stop-instances --instance-ids i-0777 --region eu-west-1 --output json" {
		t.Fatalf("aws calls: %q", calls)
	}
	r := mustOne(s.DB, "SELECT * FROM retired_vms WHERE executor = 'vm-seven'")
	if r == nil || r.str("provider") != "aws" || r.str("name") != "pulse-cr-007" || r.str("delete_after") != plusHours(testNow, 24) || !strings.Contains(r.str("detail"), `"instance_id":"i-0777"`) {
		t.Fatalf("retired_vms: %v", r)
	}
	ev := mustOne(s.DB, "SELECT * FROM events WHERE action = 'executor.retired' AND subject = 'vm-seven'")
	if d := jsonMap(ev.str("detail")); d["provider"] != "aws" || d["instance_id"] != "i-0777" || d["place"] != "eu-west-1" {
		t.Fatalf("event: %v", ev)
	}
	p := mustOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute'")
	if p.str("summary") != "retired vm-seven: login refused; EC2 instance pulse-cr-007 (eu-west-1) stopped now, deleted at "+plusHours(testNow, 24)+" unless kept" {
		t.Fatalf("summary: %q", p.str("summary"))
	}
	if !strings.Contains(p.str("body"), "Undo: `aws ec2 start-instances --instance-ids i-0777 --region eu-west-1` then `pulse executor activate vm-seven`") {
		t.Fatalf("body:\n%s", p.str("body"))
	}
	if !strings.Contains(strings.Join(lines, "\n"), "retired vm-seven: stopped pulse-cr-007 (eu-west-1)") {
		t.Fatalf("lines: %q", lines)
	}

	// At the grace the instance is terminated through the row's provider, whatever compute.provider says now.
	setSetting(t, s, "compute.provider", "azure")
	aws.reset()
	later := plusHours(testNow, 24)
	lines = tendExecutors(s, later, testDaemon)
	calls = aws.joined()
	if len(calls) != 2 || !strings.HasPrefix(calls[0], "ec2 describe-instances --filters Name=instance-id,Values=i-0777 --region eu-west-1") ||
		calls[1] != "ec2 terminate-instances --instance-ids i-0777 --region eu-west-1 --output json" {
		t.Fatalf("delete calls: %q", calls)
	}
	if r := mustOne(s.DB, "SELECT * FROM retired_vms WHERE executor = 'vm-seven'"); r.str("deleted_at") != later || r.str("note") != "" {
		t.Fatalf("row: %v", r)
	}
	if mustOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute' AND summary = 'deleted pulse-cr-007 (eu-west-1)'") == nil {
		t.Fatalf("delete finding missing: %v", mustRows(s.DB, "SELECT summary FROM posts"))
	}
	if !strings.Contains(strings.Join(lines, "\n"), "deleted pulse-cr-007 (eu-west-1): instance i-0777 with its root volume") {
		t.Fatalf("lines: %q", lines)
	}
	// Already gone: closed without a terminate.
	aws.reset()
	exec1(s.DB, "INSERT INTO retired_vms (executor, provider, resource_group, name, deallocated_at, delete_after, detail) VALUES ('vm-gone', 'aws', '', 'pulse-cr-gone', ?, ?, ?)",
		testNow, testNow, `{"detail":{"instance_id":"i-gone","region":"eu-west-1"}}`)
	lines = tendExecutors(s, later, testDaemon)
	if calls := aws.joined(); len(calls) != 1 || !strings.HasPrefix(calls[0], "ec2 describe-instances --filters Name=instance-id,Values=i-gone") {
		t.Fatalf("missing instance calls: %q", calls)
	}
	if r := mustOne(s.DB, "SELECT * FROM retired_vms WHERE executor = 'vm-gone'"); r.str("note") != "vm not found at delete time" {
		t.Fatalf("row: %v", r)
	}
}

// -- gcp ------------------------------------------------------------------------------------------------------

type fakeGcp struct {
	fakeCLI
	instances map[string]*gcpFake
	describes int
	serials   int
	deleted   []string
	firewall  bool
}

type gcpFake struct {
	name, status, ip, request, project string
}

func (f *fakeGcp) instanceJSON(i *gcpFake) string {
	access := ""
	if i.ip != "" {
		access = `{"natIP": "` + i.ip + `"}`
	}
	return fmt.Sprintf(`{"name": %q, "status": %q, "zone": "https://www.googleapis.com/compute/v1/projects/pulse-proj/zones/europe-west1-b",
		"machineType": "https://www.googleapis.com/compute/v1/projects/pulse-proj/zones/europe-west1-b/machineTypes/e2-medium",
		"labels": {"pulse": "hands", "pulse-request": %q, "pulse-project": %q}, "networkInterfaces": [{"accessConfigs": [%s]}]}`,
		i.name, i.status, i.request, i.project, access)
}

func newFakeGcp() *fakeGcp {
	f := &fakeGcp{instances: map[string]*gcpFake{}}
	f.handle = func(cmd string, args []string) (int, string, string) {
		switch {
		case strings.HasPrefix(cmd, "auth list"):
			return 0, "pulse@example.com\n", ""
		case strings.HasPrefix(cmd, "config get-value project"):
			return 0, "pulse-proj\n", ""
		case strings.HasPrefix(cmd, "compute firewall-rules describe"):
			if f.firewall {
				return 0, `{"name": "pulse-hands-ssh"}`, ""
			}
			return 1, "", "ERROR: (gcloud.compute.firewall-rules.describe) Could not fetch resource:\n - The resource 'projects/pulse-proj/global/firewalls/pulse-hands-ssh' was not found"
		case strings.HasPrefix(cmd, "compute firewall-rules create"):
			f.firewall = true
			return 0, `[{"name": "pulse-hands-ssh"}]`, ""
		case strings.HasPrefix(cmd, "compute instances describe"):
			f.describes++
			i := f.instances[args[3]]
			if i == nil {
				return 1, "", "ERROR: (gcloud.compute.instances.describe) Could not fetch resource:\n - The resource 'projects/pulse-proj/zones/europe-west1-b/instances/" + args[3] + "' was not found"
			}
			if i.status == "STAGING" && f.describes >= 2 {
				i.status, i.ip = "RUNNING", "203.0.113.11"
			}
			return 0, f.instanceJSON(i), ""
		case strings.HasPrefix(cmd, "compute instances create"):
			labels := map[string]string{}
			for _, kv := range strings.Split(argAfter(args, "--labels"), ",") {
				k, v, _ := strings.Cut(kv, "=")
				labels[k] = v
			}
			i := &gcpFake{name: args[3], status: "STAGING", request: labels["pulse-request"], project: labels["pulse-project"]}
			f.instances[i.name] = i
			f.describes = 0
			return 0, `[` + f.instanceJSON(i) + `]`, ""
		case strings.HasPrefix(cmd, "compute instances get-serial-port-output"):
			f.serials++
			if f.serials < 2 {
				return 0, "[    0.000000] Linux version 6.8\n", "Specify --start=1200 in the next get-serial-port-output invocation to get only the new output starting from here."
			}
			return 0, "[    0.000000] Linux version 6.8\n" + hostkeyMarker + testHostKey + "\n", ""
		case strings.HasPrefix(cmd, "compute instances list"):
			var out []string
			for _, i := range f.instances {
				out = append(out, f.instanceJSON(i))
			}
			return 0, "[" + strings.Join(out, ",") + "]", ""
		case strings.HasPrefix(cmd, "compute instances stop"):
			if i := f.instances[args[3]]; i != nil {
				i.status = "TERMINATED"
			}
			return 0, "", ""
		case strings.HasPrefix(cmd, "compute instances delete"):
			f.deleted = append(f.deleted, args[3])
			if f.instances[args[3]] == nil {
				return 1, "", "ERROR: (gcloud.compute.instances.delete) Could not fetch resource: - The resource was not found"
			}
			delete(f.instances, args[3])
			return 0, "", ""
		}
		return 1, "", "unexpected gcloud " + cmd
	}
	return f
}

func TestDaemonOnceComputeGCP(t *testing.T) {
	s := newStore(t)
	h := newComputeHarness(t, s, map[string]string{"compute.provider": "gcp", "compute.gcp.zone": "europe-west1-b"})
	gcp := newFakeGcp()
	installFake(t, &runGcloud, gcp.run)
	h.provisionAndRelease(t, s, "gcp", "203.0.113.11")

	create := gcp.first("compute instances create")
	for _, want := range []string{
		"compute instances create pulse-cr-001 --zone europe-west1-b --machine-type e2-medium --image-family ubuntu-2404-lts-amd64 --image-project ubuntu-os-cloud",
		"--boot-disk-size " + handsDiskGB + "GB", "--boot-disk-auto-delete", "--network default",
		"--labels pulse=hands,pulse-request=cr-001,pulse-project=proj", "--tags pulse-hands", "--metadata-from-file user-data=", "--project pulse-proj --format json",
	} {
		if !strings.Contains(create, want) {
			t.Fatalf("create lacks %q: %s", want, create)
		}
	}
	if !gcp.has("compute firewall-rules create pulse-hands-ssh --network default --direction INGRESS --allow tcp:22 --source-ranges 0.0.0.0/0 --target-tags pulse-hands") {
		t.Fatalf("firewall: %q", gcp.joined())
	}
	if gcp.serials != 2 || !gcp.has("compute instances get-serial-port-output pulse-cr-001 --zone europe-west1-b --project pulse-proj") {
		t.Fatalf("serial reads %d: %q", gcp.serials, gcp.joined())
	}
	if len(gcp.deleted) != 1 || gcp.deleted[0] != "pulse-cr-001" || !gcp.has("compute instances delete pulse-cr-001 --zone europe-west1-b --quiet --project pulse-proj") {
		t.Fatalf("deleted: %v %q", gcp.deleted, gcp.joined())
	}
	m, _ := loadMachine(s, "CR-001")
	if m.Detail["project"] != "pulse-proj" || m.Detail["zone"] != "europe-west1-b" || m.Detail["type"] != "e2-medium" {
		t.Fatalf("detail: %v", m.Detail)
	}
}

func TestTendRetiresAndDeletesOurGCEInstance(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "compute.provider", "gcp")
	setSetting(t, s, "compute.gcp.project", "pulse-proj")
	drainedByLogin(t, s, "vm-seven", "pulse@vm-seven")
	gcp := newFakeGcp()
	gcp.instances["pulse-cr-007"] = &gcpFake{name: "pulse-cr-007", status: "RUNNING", ip: "203.0.113.7", request: "cr-007", project: "proj"}
	installFake(t, &runGcloud, gcp.run)
	oldHost := sshHostname
	t.Cleanup(func() { sshHostname = oldHost })
	sshHostname = func(host string) string { return "203.0.113.7" }
	stubGh(t, "")

	lines := tendExecutors(s, testNow, testDaemon)
	calls := gcp.joined()
	if len(calls) != 2 || calls[0] != "compute instances list --filter=labels.pulse=hands --project pulse-proj --format json" ||
		calls[1] != "compute instances stop pulse-cr-007 --zone europe-west1-b --quiet --project pulse-proj" {
		t.Fatalf("gcloud calls: %q", calls)
	}
	p := mustOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute'")
	if p.str("summary") != "retired vm-seven: login refused; Compute Engine instance pulse-cr-007 (europe-west1-b) stopped now, deleted at "+plusHours(testNow, 24)+" unless kept" {
		t.Fatalf("summary: %q", p.str("summary"))
	}
	if !strings.Contains(p.str("body"), "Undo: `gcloud compute instances start pulse-cr-007 --zone europe-west1-b --project pulse-proj` then") {
		t.Fatalf("body:\n%s", p.str("body"))
	}
	if !strings.Contains(strings.Join(lines, "\n"), "retired vm-seven: stopped pulse-cr-007 (europe-west1-b)") {
		t.Fatalf("lines: %q", lines)
	}
	gcp.reset()
	later := plusHours(testNow, 24)
	lines = tendExecutors(s, later, testDaemon)
	calls = gcp.joined()
	if len(calls) != 2 || calls[0] != "compute instances describe pulse-cr-007 --zone europe-west1-b --project pulse-proj --format json" ||
		calls[1] != "compute instances delete pulse-cr-007 --zone europe-west1-b --quiet --project pulse-proj" {
		t.Fatalf("delete calls: %q", calls)
	}
	if !strings.Contains(strings.Join(lines, "\n"), "deleted pulse-cr-007 (europe-west1-b): instance pulse-cr-007 with its boot disk") {
		t.Fatalf("lines: %q", lines)
	}
	if mustOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute' AND summary = 'deleted pulse-cr-007 (europe-west1-b)'") == nil {
		t.Fatalf("delete finding missing: %v", mustRows(s.DB, "SELECT summary FROM posts"))
	}
}

// -- multipass ------------------------------------------------------------------------------------------------

type fakeMultipass struct {
	fakeCLI
	instances map[string]*mpFake
	infos     int
	execs     int
	deleted   []string
}

type mpFake struct{ name, state, ip string }

func (f *fakeMultipass) infoJSON(i *mpFake) string {
	ip := "[]"
	if i.ip != "" {
		ip = `["` + i.ip + `"]`
	}
	return fmt.Sprintf(`{"name": %q, "state": %q, "ipv4": %s, "image_release": "24.04 LTS", "release": "Ubuntu 24.04 LTS"}`, i.name, i.state, ip)
}

func newFakeMultipass() *fakeMultipass {
	f := &fakeMultipass{instances: map[string]*mpFake{}}
	f.handle = func(cmd string, args []string) (int, string, string) {
		switch {
		case cmd == "version":
			return 0, "multipass   1.15.0\nmultipassd  1.15.0\n", ""
		case strings.HasPrefix(cmd, "info "):
			f.infos++
			i := f.instances[args[1]]
			if i == nil {
				return 2, "", "info failed: instance \"" + args[1] + "\" does not exist"
			}
			if i.state == "Starting" && f.infos >= 2 {
				i.state, i.ip = "Running", "192.168.64.7"
			}
			return 0, `{"errors": [], "info": {"` + i.name + `": ` + f.infoJSON(i) + `}}`, ""
		case strings.HasPrefix(cmd, "launch "):
			i := &mpFake{name: argAfter(args, "--name"), state: "Starting"}
			f.instances[i.name] = i
			f.infos = 0
			return 0, "Launched: " + i.name + "\n", ""
		case strings.HasPrefix(cmd, "exec "):
			f.execs++
			if f.execs < 2 {
				return 1, "", "cat: /etc/ssh/ssh_host_ed25519_key.pub: No such file or directory"
			}
			return 0, testHostKey + "\n", ""
		case cmd == "list --format json":
			var out []string
			for _, i := range f.instances {
				out = append(out, f.infoJSON(i))
			}
			return 0, `{"list": [` + strings.Join(out, ",") + `]}`, ""
		case strings.HasPrefix(cmd, "stop "):
			if i := f.instances[args[1]]; i != nil {
				i.state, i.ip = "Stopped", ""
			}
			return 0, "", ""
		case strings.HasPrefix(cmd, "delete --purge "):
			f.deleted = append(f.deleted, args[2])
			if f.instances[args[2]] == nil {
				return 2, "", "delete failed: instance \"" + args[2] + "\" does not exist"
			}
			delete(f.instances, args[2])
			return 0, "", ""
		}
		return 1, "", "unexpected multipass " + cmd
	}
	return f
}

func TestDaemonOnceComputeMultipass(t *testing.T) {
	s := newStore(t)
	h := newComputeHarness(t, s, map[string]string{"compute.provider": "multipass"})
	mp := newFakeMultipass()
	installFake(t, &runMultipass, mp.run)
	h.provisionAndRelease(t, s, "multipass", "192.168.64.7")

	launch := mp.first("launch ")
	if !strings.HasPrefix(launch, "launch 24.04 --name pulse-cr-001 --cloud-init ") || !strings.HasSuffix(launch, "--timeout 1200 --cpus 2 --memory 4G --disk 32G") {
		t.Fatalf("launch: %s", launch)
	}
	if mp.execs != 2 || !mp.has("exec pulse-cr-001 -- cat /etc/ssh/ssh_host_ed25519_key.pub") {
		t.Fatalf("exec reads %d: %q", mp.execs, mp.joined())
	}
	if len(mp.deleted) != 1 || mp.deleted[0] != "pulse-cr-001" {
		t.Fatalf("deleted: %v", mp.deleted)
	}
	m, _ := loadMachine(s, "CR-001")
	if m.Detail["image"] != "24.04" || m.Detail["cpus"] != "2" || m.Detail["memory"] != "4G" || m.Detail["disk"] != "32G" {
		t.Fatalf("detail: %v", m.Detail)
	}
}

func TestTendRetiresAndDeletesOurMultipassInstance(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "compute.provider", "multipass")
	drainedByLogin(t, s, "vm-seven", "pulse@vm-seven")
	// A laptop VM PULSE did not make shares the network: never touched.
	drainedByLogin(t, s, "vm-mine", "pulse@192.168.64.3")
	mp := newFakeMultipass()
	mp.instances["pulse-cr-007"] = &mpFake{name: "pulse-cr-007", state: "Running", ip: "192.168.64.7"}
	mp.instances["primary"] = &mpFake{name: "primary", state: "Running", ip: "192.168.64.3"}
	installFake(t, &runMultipass, mp.run)
	oldHost := sshHostname
	t.Cleanup(func() { sshHostname = oldHost })
	sshHostname = func(host string) string {
		if host == "vm-seven" {
			return "192.168.64.7"
		}
		return host
	}
	stubGh(t, "")

	lines := tendExecutors(s, testNow, testDaemon)
	if calls := mp.joined(); len(calls) != 3 || calls[0] != "list --format json" || calls[1] != "list --format json" || calls[2] != "stop pulse-cr-007" {
		t.Fatalf("multipass calls: %q", calls)
	}
	text := strings.Join(lines, "\n")
	if !strings.Contains(text, "retired vm-seven: stopped pulse-cr-007 (this machine)") ||
		!strings.Contains(text, "left running because PULSE did not create it (no Multipass instance tagged pulse=hands is named vm-mine or answers at 192.168.64.3)") {
		t.Fatalf("lines: %q", lines)
	}
	p := mustOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute' AND summary LIKE 'retired vm-seven%'")
	if p == nil || !strings.Contains(p.str("body"), "Undo: `multipass start pulse-cr-007` then") {
		t.Fatalf("finding: %v", p)
	}
	mp.reset()
	later := plusHours(testNow, 24)
	lines = tendExecutors(s, later, testDaemon)
	if calls := mp.joined(); len(calls) != 2 || calls[0] != "info pulse-cr-007 --format json" || calls[1] != "delete --purge pulse-cr-007" {
		t.Fatalf("delete calls: %q", calls)
	}
	if !strings.Contains(strings.Join(lines, "\n"), "deleted pulse-cr-007 (this machine): instance pulse-cr-007 with its disk") {
		t.Fatalf("lines: %q", lines)
	}
	if _, ok := mp.instances["primary"]; !ok {
		t.Fatal("the owner's own VM was deleted")
	}
}

// -- the daemon reports a provider whose CLI is missing without touching anything ---------------------------

func TestComputeFailsWhenTheProviderCLIIsMissing(t *testing.T) {
	s := newStore(t)
	h := newComputeHarness(t, s, map[string]string{"compute.provider": "gcp", "compute.gcp.zone": "europe-west1-b"})
	installFake(t, &runGcloud, func(context.Context, []string, time.Duration) (int, string, string, error) {
		return -1, "", "", errors.New("the Google Cloud CLI (gcloud) is not installed on this machine")
	})
	req, err := tasks.RequestCompute(s, "proj", "aurora", "", 2, "the fixtures take an hour here")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := tasks.Approve(s, req.ID, "go ahead", humanAgent); err != nil {
		t.Fatal(err)
	}
	log := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'", HubHome: h.hubHome})
	got, _ := tasks.GetCompute(s, req.ID)
	if got.Status != "failed" || !strings.Contains(log, "gcloud) is not installed") {
		t.Fatalf("request: %+v\n%s", got, log)
	}
	if executorRow(s, "hands-cr-001") != nil {
		t.Fatal("an executor was registered without a machine")
	}
}

// TestDaemonOnceComputeSlowProvision guards the loopCtx/computeCtx split in daemon.go: a provision whose
// polling pause is longer than one daemon loop tick must still reach ready when the once-daemon leaves,
// because compute outlives the loop by the grace instead of dying with loopCtx.
func TestDaemonOnceComputeSlowProvision(t *testing.T) {
	skipSlow(t)
	s := newStore(t)
	h := newComputeHarness(t, s, map[string]string{"compute.provider": "multipass"})
	// newComputeHarness shortens provisionWait to 5ms; make it longer than the 1s loop interval onceDaemon uses.
	provisionWait = 1500 * time.Millisecond
	mp := newFakeMultipass()
	installFake(t, &runMultipass, mp.run)
	req, err := tasks.RequestCompute(s, "proj", "aurora", "", 2, "the fixtures take an hour here")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := tasks.Approve(s, req.ID, "go ahead", humanAgent); err != nil {
		t.Fatal(err)
	}
	log := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'", HubHome: h.hubHome})
	got, err := tasks.GetCompute(s, req.ID)
	if err != nil || got.Status != "ready" || got.Executor != "hands-cr-001" {
		t.Fatalf("slow provision after once-daemon: %+v %v\n%s", got, err, log)
	}
}
