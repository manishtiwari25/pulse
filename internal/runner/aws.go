package runner

// AWS compute provider (ADR-020 D-D, ADR-034): an EC2 instance made through the AWS CLI the daemon's user
// logged in with (`aws configure` / `aws sso login`), never a stored key. The machine is Ubuntu 24.04 from
// Canonical's SSM parameter (or an AMI id), booted with the shared cloud-init that creates the admin user
// with the hub key and prints the host key on the serial console; PULSE reads it back with `aws ec2
// get-console-output`, the channel that lets the first ssh connection be checked instead of trusted. One
// security group, pulse-hands, opens port 22 in the default VPC (or the configured subnet's VPC); instances
// carry the pulse=hands tag every provider uses so retire and delete recognise them.

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

type awsProvider struct{}

func init() { RegisterProvider(awsProvider{}) }

func (awsProvider) Name() string { return "aws" }
func (awsProvider) Kind() string { return "EC2 instance" }

const (
	awsInstanceType = "t3.medium"
	awsImage        = "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id"
	awsGroupName    = "pulse-hands" // the security group that opens port 22 for the hands
	awsShort        = 60 * time.Second
	awsLong         = 10 * time.Minute
)

// runAws runs the AWS CLI; tests swap it for a fake `aws`.
var runAws = cliCommand("aws", "the AWS CLI (aws) is not installed on this machine", "AWS_PAGER=", "NO_COLOR=1")

type awsConfig struct {
	Profile, Region, InstanceType, Image, Subnet, SecurityGroup string
}

func awsSettings(s *store.Store, size string) awsConfig {
	cfg := awsConfig{
		Profile:       strings.TrimSpace(s.Setting("compute.aws.profile", "")),
		Region:        strings.TrimSpace(s.Setting("compute.aws.region", "")),
		InstanceType:  strings.TrimSpace(size),
		Image:         strings.TrimSpace(s.Setting("compute.aws.image", awsImage)),
		Subnet:        strings.TrimSpace(s.Setting("compute.aws.subnet", "")),
		SecurityGroup: strings.TrimSpace(s.Setting("compute.aws.security_group", "")),
	}
	if cfg.InstanceType == "" {
		cfg.InstanceType = strings.TrimSpace(s.Setting("compute.aws.instance_type", awsInstanceType))
	}
	if cfg.InstanceType == "" {
		cfg.InstanceType = awsInstanceType
	}
	if cfg.Image == "" {
		cfg.Image = awsImage
	}
	return cfg
}

// scope adds the profile and the region when set.
func (c awsConfig) scope(a []string) []string {
	if c.Profile != "" {
		a = append(a, "--profile", c.Profile)
	}
	if c.Region != "" {
		a = append(a, "--region", c.Region)
	}
	return a
}

// args is `aws ec2 <a...>` in scope, answering JSON.
func (c awsConfig) args(a ...string) []string {
	return append(c.scope(append([]string{"ec2"}, a...)), "--output", "json")
}

// imageArg is the --image-id value: an SSM parameter path resolves at launch, an ami- id is used as is.
func (c awsConfig) imageArg() string {
	if strings.HasPrefix(c.Image, "/") {
		return "resolve:ssm:" + c.Image
	}
	return c.Image
}

func awsErr(what string, rc int, stdout, stderr string, err error) error {
	return cliErr(what, rc, stdout, stderr, err)
}

// awsJSON runs one call that must answer JSON into v.
func awsJSON(ctx context.Context, what string, args []string, timeout time.Duration, v any) error {
	rc, out, errText, err := runAws(ctx, args, timeout)
	if err != nil || rc != 0 {
		return awsErr(what, rc, out, errText, err)
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), v); err != nil {
		return fmt.Errorf("%s: unreadable answer: %s", what, squeeze(strings.TrimSpace(out), 200))
	}
	return nil
}

type awsInstance struct {
	InstanceID string `json:"InstanceId"`
	PublicIP   string `json:"PublicIpAddress"`
	Type       string `json:"InstanceType"`
	SubnetID   string `json:"SubnetId"`
	State      struct {
		Name string `json:"Name"`
	} `json:"State"`
	Tags []struct {
		Key   string `json:"Key"`
		Value string `json:"Value"`
	} `json:"Tags"`
	SecurityGroups []struct {
		GroupID string `json:"GroupId"`
	} `json:"SecurityGroups"`
}

func (i awsInstance) tag(key string) string {
	for _, t := range i.Tags {
		if t.Key == key {
			return t.Value
		}
	}
	return ""
}

// awsDescribe lists the instances a filter set matches, flattened over reservations.
func awsDescribe(ctx context.Context, cfg awsConfig, filters ...string) ([]awsInstance, error) {
	args := []string{"describe-instances"}
	if len(filters) > 0 {
		args = append(append(args, "--filters"), filters...)
	}
	var res struct {
		Reservations []struct {
			Instances []awsInstance `json:"Instances"`
		} `json:"Reservations"`
	}
	if err := awsJSON(ctx, "aws ec2 describe-instances", cfg.args(args...), awsShort, &res); err != nil {
		return nil, err
	}
	var all []awsInstance
	for _, r := range res.Reservations {
		all = append(all, r.Instances...)
	}
	return all, nil
}

// awsMachine is the Machine an instance PULSE made maps to.
func awsMachine(cfg awsConfig, i awsInstance) *Machine {
	name := i.tag("Name")
	if name == "" {
		name = i.InstanceID
	}
	m := &Machine{Provider: "aws", Name: name, Tags: map[string]string{tagHands: "hands"},
		Detail: map[string]string{"instance_id": i.InstanceID, "region": cfg.Region, "profile": cfg.Profile, "type": i.Type, "public_ip": i.PublicIP, "subnet": i.SubnetID}}
	for _, k := range []string{tagRequest, tagProject} {
		if v := i.tag(k); v != "" {
			m.Tags[k] = v
		}
	}
	if len(i.SecurityGroups) > 0 {
		m.Detail["security_group"] = i.SecurityGroups[0].GroupID
	}
	return m
}

// awsNetwork settles the subnet and the security group: the configured subnet or the default VPC's first
// default subnet, and the configured group or pulse-hands in that VPC, created with port 22 open when missing.
func awsNetwork(ctx context.Context, cfg awsConfig, log func(string, ...any)) (subnet, group string, err error) {
	vpc := ""
	subnet = cfg.Subnet
	if subnet != "" {
		var res struct {
			Subnets []struct {
				VpcID string `json:"VpcId"`
			} `json:"Subnets"`
		}
		if err := awsJSON(ctx, "aws ec2 describe-subnets "+subnet, cfg.args("describe-subnets", "--subnet-ids", subnet), awsShort, &res); err != nil {
			return "", "", err
		}
		if len(res.Subnets) == 0 {
			return "", "", fmt.Errorf("compute.aws.subnet %s does not exist in %s", subnet, cfg.Region)
		}
		vpc = res.Subnets[0].VpcID
	} else {
		var vpcs struct {
			Vpcs []struct {
				VpcID string `json:"VpcId"`
			} `json:"Vpcs"`
		}
		if err := awsJSON(ctx, "aws ec2 describe-vpcs", cfg.args("describe-vpcs", "--filters", "Name=is-default,Values=true"), awsShort, &vpcs); err != nil {
			return "", "", err
		}
		if len(vpcs.Vpcs) == 0 {
			return "", "", errors.New("the account has no default VPC in " + cfg.Region + ": set compute.aws.subnet to a public subnet")
		}
		vpc = vpcs.Vpcs[0].VpcID
		var subs struct {
			Subnets []struct {
				SubnetID string `json:"SubnetId"`
			} `json:"Subnets"`
		}
		if err := awsJSON(ctx, "aws ec2 describe-subnets", cfg.args("describe-subnets", "--filters", "Name=vpc-id,Values="+vpc, "Name=default-for-az,Values=true"), awsShort, &subs); err != nil {
			return "", "", err
		}
		if len(subs.Subnets) == 0 {
			return "", "", fmt.Errorf("the default VPC %s has no default subnet: set compute.aws.subnet", vpc)
		}
		subnet = subs.Subnets[0].SubnetID
	}
	if group = cfg.SecurityGroup; group != "" {
		return subnet, group, nil
	}
	var groups struct {
		SecurityGroups []struct {
			GroupID string `json:"GroupId"`
		} `json:"SecurityGroups"`
	}
	if err := awsJSON(ctx, "aws ec2 describe-security-groups", cfg.args("describe-security-groups", "--filters", "Name=group-name,Values="+awsGroupName, "Name=vpc-id,Values="+vpc), awsShort, &groups); err != nil {
		return "", "", err
	}
	if len(groups.SecurityGroups) > 0 {
		return subnet, groups.SecurityGroups[0].GroupID, nil
	}
	var created struct {
		GroupID string `json:"GroupId"`
	}
	if err := awsJSON(ctx, "aws ec2 create-security-group "+awsGroupName, cfg.args("create-security-group", "--group-name", awsGroupName,
		"--description", "PULSE compute machines: ssh in", "--vpc-id", vpc,
		"--tag-specifications", "ResourceType=security-group,Tags=[{Key="+tagHands+",Value=hands}]"), awsShort, &created); err != nil {
		return "", "", err
	}
	rc, out, errText, err := runAws(ctx, cfg.args("authorize-security-group-ingress", "--group-id", created.GroupID, "--protocol", "tcp", "--port", "22", "--cidr", "0.0.0.0/0"), awsShort)
	if err != nil || rc != 0 {
		return "", "", awsErr("aws ec2 authorize-security-group-ingress "+created.GroupID, rc, out, errText, err)
	}
	log("created security group %s (%s) in %s with port 22 open", awsGroupName, created.GroupID, vpc)
	return subnet, created.GroupID, nil
}

// Provision: login check, region, network, `aws ec2 run-instances` with the cloud-init as user data, the
// public address once running, the host key from the console output, then the shared finish.
func (p awsProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error) {
	s, hubHome, log, err := provisionStart("aws", req)
	if err != nil {
		return nil, err
	}
	cfg := awsSettings(s, req.Size)
	var who struct {
		Account string `json:"Account"`
		Arn     string `json:"Arn"`
	}
	if err := awsJSON(ctx, "not logged in to AWS (run `aws configure` or `aws sso login` on this machine as the daemon's user)",
		append(cfg.scope([]string{"sts", "get-caller-identity"}), "--output", "json"), awsShort, &who); err != nil {
		return nil, err
	}
	if cfg.Region == "" {
		if rc, out, _, err := runAws(ctx, cfg.scope([]string{"configure", "get", "region"}), awsShort); err == nil && rc == 0 {
			cfg.Region = strings.TrimSpace(out)
		}
	}
	if cfg.Region == "" {
		return nil, errors.New("aws is not configured: set compute.aws.region (pulse settings set compute.aws.region <region>) or a default region for the profile")
	}
	log("aws account %s as %s in %s", who.Account, who.Arn, cfg.Region)
	var lines []string
	_, pubkey, err := pairEnsureHubKey(hubHome, &lines)
	if err != nil {
		return nil, err
	}
	name := machineName(req.ID)
	m := &Machine{Provider: "aws", Name: name, Executor: req.Executor, Tags: machineTags(req),
		Detail: map[string]string{"region": cfg.Region, "profile": cfg.Profile, "type": cfg.InstanceType, "image": cfg.Image}}
	have, err := awsDescribe(ctx, cfg, "Name=tag:"+tagRequest+",Values="+req.ID, "Name=tag:"+tagHands+",Values=hands", "Name=instance-state-name,Values=pending,running")
	if err != nil {
		return nil, err
	}
	var inst awsInstance
	if len(have) > 0 {
		inst = have[0]
		log("instance %s (%s) exists; not recreated", name, inst.InstanceID)
	} else {
		subnet, group, err := awsNetwork(ctx, cfg, log)
		if err != nil {
			return nil, err
		}
		rendered, err := writeCloudInit(pubkey)
		if err != nil {
			return nil, err
		}
		defer os.Remove(rendered)
		log("aws ec2 run-instances %s (%s, %s, %s GiB)", name, cfg.InstanceType, cfg.Image, handsDiskGB)
		var res struct {
			Instances []awsInstance `json:"Instances"`
		}
		tags := fmt.Sprintf("ResourceType=instance,Tags=[{Key=Name,Value=%s},{Key=%s,Value=hands},{Key=%s,Value=%s},{Key=%s,Value=%s}]",
			name, tagHands, tagRequest, req.ID, tagProject, req.Project)
		if err := awsJSON(ctx, "aws ec2 run-instances "+name, cfg.args("run-instances", "--image-id", cfg.imageArg(), "--instance-type", cfg.InstanceType,
			"--count", "1", "--subnet-id", subnet, "--security-group-ids", group, "--associate-public-ip-address",
			"--user-data", "file://"+rendered, "--block-device-mappings",
			"DeviceName=/dev/sda1,Ebs={VolumeSize="+handsDiskGB+",VolumeType=gp3,DeleteOnTermination=true}",
			"--metadata-options", "HttpTokens=required", "--tag-specifications", tags), 5*time.Minute, &res); err != nil {
			return m, err
		}
		if len(res.Instances) == 0 {
			return m, fmt.Errorf("aws ec2 run-instances %s made no instance", name)
		}
		inst = res.Instances[0]
		if inst.SubnetID == "" {
			inst.SubnetID = subnet
		}
	}
	m.Detail["instance_id"] = inst.InstanceID
	m.Detail["subnet"] = inst.SubnetID
	if len(inst.SecurityGroups) > 0 {
		m.Detail["security_group"] = inst.SecurityGroups[0].GroupID
	}
	ip := inst.PublicIP
	if err := pollUntil(ctx, "the address of "+name, func() (bool, error) {
		if ip != "" && inst.State.Name == "running" {
			return true, nil
		}
		got, err := awsDescribe(ctx, cfg, "Name=instance-id,Values="+inst.InstanceID)
		if err != nil {
			return false, err
		}
		if len(got) == 0 {
			return false, fmt.Errorf("instance %s vanished while booting", inst.InstanceID)
		}
		inst, ip = got[0], got[0].PublicIP
		switch inst.State.Name {
		case "pending", "running":
			return ip != "" && inst.State.Name == "running", nil
		}
		return false, fmt.Errorf("instance %s is %s", inst.InstanceID, inst.State.Name)
	}); err != nil {
		return m, err
	}
	m.Detail["public_ip"] = ip
	var hostkeys []string
	if err := pollUntil(ctx, "the host key of "+name+" on its console", func() (bool, error) {
		rc, out, errText, err := runAws(ctx, append(cfg.scope([]string{"ec2", "get-console-output", "--instance-id", inst.InstanceID, "--latest", "--query", "Output"}), "--output", "text"), awsShort)
		if err != nil || rc != 0 {
			return false, awsErr("aws ec2 get-console-output "+inst.InstanceID, rc, out, errText, err)
		}
		hostkeys = hostKeyLines(out)
		return len(hostkeys) > 0, nil
	}); err != nil {
		return m, err
	}
	return m, provisionFinish(ctx, req, m, hubHome, ip, hostkeys, log)
}

func (p awsProvider) config(m *Machine) awsConfig {
	return awsConfig{Profile: m.Detail["profile"], Region: m.Detail["region"]}
}

// Release terminates the instance; its root volume goes with it (DeleteOnTermination). The security group
// stays: it is shared by every hand in the VPC.
func (p awsProvider) Release(ctx context.Context, m *Machine) error {
	if m == nil || m.Detail["instance_id"] == "" {
		return nil
	}
	cfg := p.config(m)
	rc, out, errText, err := runAws(ctx, cfg.args("terminate-instances", "--instance-ids", m.Detail["instance_id"]), awsLong)
	if err != nil || rc != 0 {
		if err == nil && notFoundAnswer(out, errText) {
			return nil
		}
		return awsErr("aws ec2 terminate-instances "+m.Detail["instance_id"], rc, out, errText, err)
	}
	return nil
}

// Probe: the instance still runs.
func (p awsProvider) Probe(ctx context.Context, m *Machine) error {
	if m == nil || m.Detail["instance_id"] == "" {
		return errors.New("aws: no machine")
	}
	got, err := awsDescribe(ctx, p.config(m), "Name=instance-id,Values="+m.Detail["instance_id"])
	if err != nil {
		return err
	}
	if len(got) == 0 {
		return fmt.Errorf("%s (%s) is gone", m.Name, m.Detail["instance_id"])
	}
	if got[0].State.Name != "running" {
		return fmt.Errorf("%s is %s", m.Name, pyRepr(got[0].State.Name))
	}
	return nil
}

// Find: the instance tagged pulse=hands whose Name tag is the executor's or whose public address is host,
// in the region and profile the compute.aws.* settings name.
func (p awsProvider) Find(ctx context.Context, s *store.Store, executor, host string) (*Machine, error) {
	cfg := awsSettings(s, "")
	got, err := awsDescribe(ctx, cfg, "Name=tag:"+tagHands+",Values=hands", "Name=instance-state-name,Values=pending,running,stopping,stopped")
	if err != nil {
		return nil, fmt.Errorf("the AWS CLI could not answer: %s", errMessage(err))
	}
	for _, i := range got {
		if i.tag("Name") == executor || (host != "" && i.PublicIP == host) {
			return awsMachine(cfg, i), nil
		}
	}
	return nil, nil
}

// Stop stops the instance: compute stops costing, its volume stays.
func (p awsProvider) Stop(ctx context.Context, m *Machine) error {
	rc, out, errText, err := runAws(ctx, p.config(m).args("stop-instances", "--instance-ids", m.Detail["instance_id"]), awsLong)
	if err != nil || rc != 0 {
		return awsErr("aws ec2 stop-instances "+m.Detail["instance_id"], rc, out, errText, err)
	}
	return nil
}

func (p awsProvider) Place(m *Machine) string { return m.Detail["region"] }

func (p awsProvider) StartHint(m *Machine) string {
	hint := "aws ec2 start-instances --instance-ids " + m.Detail["instance_id"]
	if m.Detail["region"] != "" {
		hint += " --region " + m.Detail["region"]
	}
	if m.Detail["profile"] != "" {
		hint += " --profile " + m.Detail["profile"]
	}
	return hint
}

// Delete terminates the instance; found is false when it is already gone. The root volume is deleted with it.
func (p awsProvider) Delete(ctx context.Context, m *Machine) (bool, []string, error) {
	id := m.Detail["instance_id"]
	if id == "" {
		return false, nil, nil
	}
	cfg := p.config(m)
	got, err := awsDescribe(ctx, cfg, "Name=instance-id,Values="+id)
	if err != nil {
		if notFoundAnswer(errMessage(err), "") {
			return false, nil, nil
		}
		return false, nil, err
	}
	if len(got) == 0 || got[0].State.Name == "terminated" || got[0].State.Name == "shutting-down" {
		return false, nil, nil
	}
	rc, out, errText, err := runAws(ctx, cfg.args("terminate-instances", "--instance-ids", id), awsLong)
	if err != nil || rc != 0 {
		return true, nil, awsErr("aws ec2 terminate-instances "+id, rc, out, errText, err)
	}
	return true, []string{"instance " + id + " with its root volume"}, nil
}
