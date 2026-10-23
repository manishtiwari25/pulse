#![allow(dead_code)]

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const AWS_INSTANCE_TYPE: &str = "t3.medium";
pub(crate) const AWS_IMAGE: &str =
    "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id";
pub(crate) const AWS_GROUP_NAME: &str = "pulse-hands";
pub(crate) const AWS_SHORT: Duration = Duration::from_secs(60);
pub(crate) const AWS_LONG: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AwsProvider {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AwsConfig {
    pub profile: String,
    pub region: String,
    pub instance_type: String,
    pub image: String,
    pub subnet: String,
    pub security_group: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AwsInstance {
    #[serde(rename = "InstanceId")]
    pub instance_id: String,
    #[serde(rename = "PublicIpAddress", default)]
    pub public_ip: String,
    #[serde(rename = "InstanceType")]
    pub r#type: String,
    #[serde(rename = "SubnetId", default)]
    pub subnet_id: String,
    #[serde(rename = "State", default)]
    pub state: AwsState,
    #[serde(rename = "Tags", default)]
    pub tags: Vec<AwsTag>,
    #[serde(rename = "SecurityGroups", default)]
    pub security_groups: Vec<AwsSecurityGroup>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AwsState {
    #[serde(rename = "Name", default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AwsTag {
    #[serde(rename = "Key", default)]
    pub key: String,
    #[serde(rename = "Value", default)]
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AwsSecurityGroup {
    #[serde(rename = "GroupId", default)]
    pub group_id: String,
}

pub(crate) static RUN_AWS: LazyLock<RwLock<Box<crate::provider::CliRunner>>> =
    LazyLock::new(|| {
        RwLock::new(crate::provider::cli_command(
            "aws".into(),
            "the AWS CLI (aws) is not installed on this machine".into(),
            &["AWS_PAGER=".into(), "NO_COLOR=1".into()],
        ))
    });

pub(crate) fn set_run_aws(
    runner: Box<crate::provider::CliRunner>,
) -> Box<crate::provider::CliRunner> {
    std::mem::replace(&mut *RUN_AWS.write().expect("run_aws poisoned"), runner)
}

pub(crate) fn init() {
    crate::provider::register_provider(AwsProvider {});
}

pub(crate) fn aws_settings(s: &pulse_store::Store, size: String) -> AwsConfig {
    let mut cfg = AwsConfig {
        profile: s.setting("compute.aws.profile", "").trim().into(),
        region: s.setting("compute.aws.region", "").trim().into(),
        instance_type: size.trim().into(),
        image: s.setting("compute.aws.image", AWS_IMAGE).trim().into(),
        subnet: s.setting("compute.aws.subnet", "").trim().into(),
        security_group: s.setting("compute.aws.security_group", "").trim().into(),
    };
    if cfg.instance_type.is_empty() {
        cfg.instance_type = s
            .setting("compute.aws.instance_type", AWS_INSTANCE_TYPE)
            .trim()
            .into();
    }
    if cfg.instance_type.is_empty() {
        cfg.instance_type = AWS_INSTANCE_TYPE.into();
    }
    if cfg.image.is_empty() {
        cfg.image = AWS_IMAGE.into();
    }
    cfg
}

pub(crate) fn aws_err(
    what: String,
    rc: i32,
    stdout: String,
    stderr: String,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    crate::provider::cli_err(what, rc, stdout, stderr, err)
}

pub(crate) fn aws_json(
    ctx: &pulse_bus::CancellationToken,
    what: String,
    args: Vec<String>,
    timeout: std::time::Duration,
    _v: serde_json::Value,
) -> anyhow::Result<()> {
    let (rc, out, err_text) = (RUN_AWS.read().unwrap())(ctx, args, timeout)?;
    if rc != 0 {
        return aws_err(what, rc, out, err_text, anyhow!(""));
    }
    let _parsed: serde_json::Value = serde_json::from_str(out.trim()).map_err(|_| {
        anyhow!(
            "{}: unreadable answer: {}",
            what,
            crate::run::squeeze(out.trim(), 200)
        )
    })?;
    Ok(())
}

pub(crate) fn aws_describe(
    ctx: &pulse_bus::CancellationToken,
    cfg: AwsConfig,
    filters: &[String],
) -> anyhow::Result<Vec<AwsInstance>> {
    let mut args = vec!["describe-instances".to_string()];
    if !filters.is_empty() {
        args.push("--filters".into());
        args.extend_from_slice(filters);
    }
    #[derive(Deserialize)]
    struct Res {
        #[serde(rename = "Reservations", default)]
        reservations: Vec<Reservation>,
    }
    #[derive(Deserialize)]
    struct Reservation {
        #[serde(rename = "Instances", default)]
        instances: Vec<AwsInstance>,
    }
    let full = cfg.args(&args);
    let (rc, out, err_text) = (RUN_AWS.read().unwrap())(ctx, full, AWS_SHORT)?;
    if rc != 0 {
        return aws_err(
            "aws ec2 describe-instances".into(),
            rc,
            out,
            err_text,
            anyhow!(""),
        )
        .map(|_| Vec::new());
    }
    let res: Res = serde_json::from_str(out.trim()).map_err(|_| {
        anyhow!(
            "aws ec2 describe-instances: unreadable answer: {}",
            crate::run::squeeze(out.trim(), 200)
        )
    })?;
    Ok(res
        .reservations
        .into_iter()
        .flat_map(|r| r.instances)
        .collect())
}

pub(crate) fn aws_machine(cfg: AwsConfig, i: AwsInstance) -> Option<crate::provider::Machine> {
    let mut name = i.tag("Name".into());
    if name.is_empty() {
        name = i.instance_id.clone();
    }
    let mut tags = BTreeMap::from([(crate::provider::TAG_HANDS.into(), "hands".into())]);
    for k in [crate::provider::TAG_REQUEST, crate::provider::TAG_PROJECT] {
        let v = i.tag(k.into());
        if !v.is_empty() {
            tags.insert(k.into(), v);
        }
    }
    let mut detail = BTreeMap::from([
        ("instance_id".into(), i.instance_id.clone()),
        ("region".into(), cfg.region),
        ("profile".into(), cfg.profile),
        ("type".into(), i.r#type),
        ("public_ip".into(), i.public_ip),
        ("subnet".into(), i.subnet_id),
    ]);
    if let Some(g) = i.security_groups.first() {
        detail.insert("security_group".into(), g.group_id.clone());
    }
    Some(crate::provider::Machine {
        provider: "aws".into(),
        name,
        tags,
        detail,
        ..Default::default()
    })
}

pub(crate) fn aws_network(
    ctx: &pulse_bus::CancellationToken,
    cfg: AwsConfig,
    log: &(dyn Fn(&str, &[serde_json::Value]) + Send + Sync),
) -> anyhow::Result<(String, String)> {
    let vpc;
    let mut subnet = cfg.subnet.clone();
    if !subnet.is_empty() {
        #[derive(Deserialize)]
        struct Res {
            #[serde(rename = "Subnets", default)]
            subnets: Vec<Sub>,
        }
        #[derive(Deserialize)]
        struct Sub {
            #[serde(rename = "VpcId", default)]
            vpc_id: String,
        }
        let (rc, out, err_text) = (RUN_AWS.read().unwrap())(
            ctx,
            cfg.args(&[
                "describe-subnets".into(),
                "--subnet-ids".into(),
                subnet.clone(),
            ]),
            AWS_SHORT,
        )?;
        if rc != 0 {
            return aws_err(
                format!("aws ec2 describe-subnets {subnet}"),
                rc,
                out,
                err_text,
                anyhow!(""),
            )
            .map(|_| unreachable!());
        }
        let res: Res = serde_json::from_str(out.trim()).unwrap_or(Res { subnets: vec![] });
        if res.subnets.is_empty() {
            return Err(anyhow!(
                "compute.aws.subnet {} does not exist in {}",
                subnet,
                cfg.region
            ));
        }
        vpc = res.subnets[0].vpc_id.clone();
    } else {
        #[derive(Deserialize)]
        struct Vpcs {
            #[serde(rename = "Vpcs", default)]
            vpcs: Vec<Vpc>,
        }
        #[derive(Deserialize)]
        struct Vpc {
            #[serde(rename = "VpcId", default)]
            vpc_id: String,
        }
        let (rc, out, err_text) = (RUN_AWS.read().unwrap())(
            ctx,
            cfg.args(&[
                "describe-vpcs".into(),
                "--filters".into(),
                "Name=is-default,Values=true".into(),
            ]),
            AWS_SHORT,
        )?;
        if rc != 0 {
            return aws_err(
                "aws ec2 describe-vpcs".into(),
                rc,
                out,
                err_text,
                anyhow!(""),
            )
            .map(|_| unreachable!());
        }
        let vpcs: Vpcs = serde_json::from_str(out.trim()).unwrap_or(Vpcs { vpcs: vec![] });
        if vpcs.vpcs.is_empty() {
            return Err(anyhow!(
                "the account has no default VPC in {}: set compute.aws.subnet to a public subnet",
                cfg.region
            ));
        }
        vpc = vpcs.vpcs[0].vpc_id.clone();
        #[derive(Deserialize)]
        struct Subs {
            #[serde(rename = "Subnets", default)]
            subnets: Vec<Sub>,
        }
        #[derive(Deserialize)]
        struct Sub {
            #[serde(rename = "SubnetId", default)]
            subnet_id: String,
        }
        let (rc, out, err_text) = (RUN_AWS.read().unwrap())(
            ctx,
            cfg.args(&[
                "describe-subnets".into(),
                "--filters".into(),
                format!("Name=vpc-id,Values={vpc}"),
                "Name=default-for-az,Values=true".into(),
            ]),
            AWS_SHORT,
        )?;
        if rc != 0 {
            return aws_err(
                "aws ec2 describe-subnets".into(),
                rc,
                out,
                err_text,
                anyhow!(""),
            )
            .map(|_| unreachable!());
        }
        let subs: Subs = serde_json::from_str(out.trim()).unwrap_or(Subs { subnets: vec![] });
        if subs.subnets.is_empty() {
            return Err(anyhow!(
                "the default VPC {} has no default subnet: set compute.aws.subnet",
                vpc
            ));
        }
        subnet = subs.subnets[0].subnet_id.clone();
    }
    if !cfg.security_group.is_empty() {
        return Ok((subnet, cfg.security_group));
    }
    #[derive(Deserialize)]
    struct Groups {
        #[serde(rename = "SecurityGroups", default)]
        security_groups: Vec<Group>,
    }
    #[derive(Deserialize)]
    struct Group {
        #[serde(rename = "GroupId", default)]
        group_id: String,
    }
    let (rc, out, err_text) = (RUN_AWS.read().unwrap())(
        ctx,
        cfg.args(&[
            "describe-security-groups".into(),
            "--filters".into(),
            format!("Name=group-name,Values={AWS_GROUP_NAME}"),
            format!("Name=vpc-id,Values={vpc}"),
        ]),
        AWS_SHORT,
    )?;
    if rc != 0 {
        return aws_err(
            "aws ec2 describe-security-groups".into(),
            rc,
            out,
            err_text,
            anyhow!(""),
        )
        .map(|_| unreachable!());
    }
    let groups: Groups = serde_json::from_str(out.trim()).unwrap_or(Groups {
        security_groups: vec![],
    });
    if let Some(g) = groups.security_groups.first() {
        return Ok((subnet, g.group_id.clone()));
    }
    #[derive(Deserialize, Default)]
    struct Created {
        #[serde(rename = "GroupId", default)]
        group_id: String,
    }
    let (rc, out, err_text) = (RUN_AWS.read().unwrap())(
        ctx,
        cfg.args(&[
            "create-security-group".into(),
            "--group-name".into(),
            AWS_GROUP_NAME.into(),
            "--description".into(),
            "PULSE compute machines: ssh in".into(),
            "--vpc-id".into(),
            vpc.clone(),
            "--tag-specifications".into(),
            format!(
                "ResourceType=security-group,Tags=[{{Key={},Value=hands}}]",
                crate::provider::TAG_HANDS
            ),
        ]),
        AWS_SHORT,
    )?;
    if rc != 0 {
        return aws_err(
            format!("aws ec2 create-security-group {AWS_GROUP_NAME}"),
            rc,
            out,
            err_text,
            anyhow!(""),
        )
        .map(|_| unreachable!());
    }
    let created: Created = serde_json::from_str(out.trim()).unwrap_or_default();
    let (rc, out, err_text) = (RUN_AWS.read().unwrap())(
        ctx,
        cfg.args(&[
            "authorize-security-group-ingress".into(),
            "--group-id".into(),
            created.group_id.clone(),
            "--protocol".into(),
            "tcp".into(),
            "--port".into(),
            "22".into(),
            "--cidr".into(),
            "0.0.0.0/0".into(),
        ]),
        AWS_SHORT,
    )?;
    if rc != 0 {
        return aws_err(
            format!(
                "aws ec2 authorize-security-group-ingress {}",
                created.group_id
            ),
            rc,
            out,
            err_text,
            anyhow!(""),
        )
        .map(|_| unreachable!());
    }
    log(
        "created security group %s (%s) in %s with port 22 open",
        &[json!(AWS_GROUP_NAME), json!(created.group_id), json!(vpc)],
    );
    Ok((subnet, created.group_id))
}

impl AwsProvider {
    pub(crate) fn name(&self) -> String {
        "aws".into()
    }
    pub(crate) fn kind(&self) -> String {
        "EC2 instance".into()
    }
    pub(crate) fn provision(
        &self,
        ctx: &pulse_bus::CancellationToken,
        req: crate::provider::ProvisionRequest,
    ) -> anyhow::Result<crate::provider::Machine> {
        let (s, hub_home, log) = crate::provider::provision_start("aws".into(), req.clone())?;
        let mut cfg = aws_settings(&s, req.size.clone());
        #[derive(Deserialize, Default)]
        struct Who {
            #[serde(rename = "Account", default)]
            account: String,
            #[serde(rename = "Arn", default)]
            arn: String,
        }
        let (rc, out, err_text) = (RUN_AWS.read().unwrap())(
            ctx,
            cfg.scope(vec![
                "sts".into(),
                "get-caller-identity".into(),
                "--output".into(),
                "json".into(),
            ]),
            AWS_SHORT,
        )?;
        if rc != 0 {
            return aws_err("not logged in to AWS (run `aws configure` or `aws sso login` on this machine as the daemon's user)".into(), rc, out, err_text, anyhow!("")).map(|_| unreachable!());
        }
        let who: Who = serde_json::from_str(out.trim()).unwrap_or_default();
        if cfg.region.is_empty()
            && let Ok((0, out, _)) = (RUN_AWS.read().unwrap())(
                ctx,
                cfg.scope(vec!["configure".into(), "get".into(), "region".into()]),
                AWS_SHORT,
            )
        {
            cfg.region = out.trim().into();
        }
        if cfg.region.is_empty() {
            return Err(anyhow!(
                "aws is not configured: set compute.aws.region (pulse settings set compute.aws.region <region>) or a default region for the profile"
            ));
        }
        log(
            "aws account %s as %s in %s",
            &[json!(who.account), json!(who.arn), json!(cfg.region)],
        );
        let mut lines = Vec::new();
        let (_, pubkey) = crate::pair::pair_ensure_hub_key(hub_home.clone(), &mut lines)?;
        let name = crate::provider::machine_name(req.id.clone());
        let mut m = crate::provider::Machine {
            provider: "aws".into(),
            name: name.clone(),
            executor: req.executor.clone(),
            tags: crate::provider::machine_tags(req.clone()),
            detail: BTreeMap::from([
                ("region".into(), cfg.region.clone()),
                ("profile".into(), cfg.profile.clone()),
                ("type".into(), cfg.instance_type.clone()),
                ("image".into(), cfg.image.clone()),
            ]),
            ..Default::default()
        };
        let have = aws_describe(
            ctx,
            cfg.clone(),
            &[
                format!(
                    "Name=tag:{},Values={}",
                    crate::provider::TAG_REQUEST,
                    req.id
                ),
                format!("Name=tag:{},Values=hands", crate::provider::TAG_HANDS),
                "Name=instance-state-name,Values=pending,running".into(),
            ],
        )?;
        let mut inst;
        if let Some(first) = have.into_iter().next() {
            inst = first;
            log(
                "instance %s (%s) exists; not recreated",
                &[json!(name), json!(inst.instance_id)],
            );
        } else {
            let (subnet, group) = aws_network(ctx, cfg.clone(), &*log)?;
            let rendered = crate::provider::write_cloud_init(pubkey)?;
            log(
                "aws ec2 run-instances %s (%s, %s, %s GiB)",
                &[
                    json!(name),
                    json!(cfg.instance_type),
                    json!(cfg.image),
                    json!(crate::provider::HANDS_DISK_GB),
                ],
            );
            let tags = format!(
                "ResourceType=instance,Tags=[{{Key=Name,Value={name}}},{{Key={},Value=hands}},{{Key={},Value={}}},{{Key={},Value={}}}]",
                crate::provider::TAG_HANDS,
                crate::provider::TAG_REQUEST,
                req.id,
                crate::provider::TAG_PROJECT,
                req.project
            );
            #[derive(Deserialize, Default)]
            struct Res {
                #[serde(rename = "Instances", default)]
                instances: Vec<AwsInstance>,
            }
            let args = cfg.args(&["run-instances".into(), "--image-id".into(), cfg.image_arg(), "--instance-type".into(), cfg.instance_type.clone(), "--count".into(), "1".into(), "--subnet-id".into(), subnet.clone(), "--security-group-ids".into(), group, "--associate-public-ip-address".into(), "--user-data".into(), format!("file://{rendered}"), "--block-device-mappings".into(), format!("DeviceName=/dev/sda1,Ebs={{VolumeSize={},VolumeType=gp3,DeleteOnTermination=true}}", crate::provider::HANDS_DISK_GB), "--metadata-options".into(), "HttpTokens=required".into(), "--tag-specifications".into(), tags]);
            let (rc, out, err_text) =
                (RUN_AWS.read().unwrap())(ctx, args, Duration::from_secs(5 * 60))?;
            let _ = std::fs::remove_file(rendered);
            if rc != 0 {
                return aws_err(
                    format!("aws ec2 run-instances {name}"),
                    rc,
                    out,
                    err_text,
                    anyhow!(""),
                )
                .map(|_| m);
            }
            let res: Res = serde_json::from_str(out.trim()).unwrap_or_default();
            if res.instances.is_empty() {
                return Err(anyhow!("aws ec2 run-instances {} made no instance", name));
            }
            inst = res.instances[0].clone();
            if inst.subnet_id.is_empty() {
                inst.subnet_id = subnet;
            }
        }
        m.detail
            .insert("instance_id".into(), inst.instance_id.clone());
        m.detail.insert("subnet".into(), inst.subnet_id.clone());
        if let Some(g) = inst.security_groups.first() {
            m.detail.insert("security_group".into(), g.group_id.clone());
        }
        crate::provider::machine_created(&req, &m);
        let mut ip = inst.public_ip.clone();
        crate::provider::poll_until(ctx, format!("the address of {name}"), || {
            if !ip.is_empty() && inst.state.name == "running" {
                return Ok(true);
            }
            let got = aws_describe(
                ctx,
                cfg.clone(),
                &[format!("Name=instance-id,Values={}", inst.instance_id)],
            )?;
            if got.is_empty() {
                return Err(anyhow!(
                    "instance {} vanished while booting",
                    inst.instance_id
                ));
            }
            inst = got[0].clone();
            ip = inst.public_ip.clone();
            match inst.state.name.as_str() {
                "pending" | "running" => Ok(!ip.is_empty() && inst.state.name == "running"),
                _ => Err(anyhow!(
                    "instance {} is {}",
                    inst.instance_id,
                    inst.state.name
                )),
            }
        })?;
        m.detail.insert("public_ip".into(), ip.clone());
        let mut hostkeys = Vec::new();
        crate::provider::poll_until(
            ctx,
            format!("the host key of {name} on its console"),
            || {
                let mut args = cfg.scope(vec![
                    "ec2".into(),
                    "get-console-output".into(),
                    "--instance-id".into(),
                    inst.instance_id.clone(),
                    "--latest".into(),
                    "--query".into(),
                    "Output".into(),
                ]);
                args.extend(["--output".into(), "text".into()]);
                let (rc, out, err_text) = (RUN_AWS.read().unwrap())(ctx, args, AWS_SHORT)?;
                if rc != 0 {
                    return aws_err(
                        format!("aws ec2 get-console-output {}", inst.instance_id),
                        rc,
                        out,
                        err_text,
                        anyhow!(""),
                    )
                    .map(|_| false);
                }
                hostkeys = crate::provider::host_key_lines(out);
                Ok(!hostkeys.is_empty())
            },
        )?;
        crate::provider::provision_finish(ctx, req, &mut m, hub_home, ip, hostkeys, log)?;
        Ok(m)
    }
    pub(crate) fn config(&self, m: &crate::provider::Machine) -> AwsConfig {
        AwsConfig {
            profile: m.detail.get("profile").cloned().unwrap_or_default(),
            region: m.detail.get("region").cloned().unwrap_or_default(),
            ..Default::default()
        }
    }
    pub(crate) fn release(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        let id = m.detail.get("instance_id").cloned().unwrap_or_default();
        if id.is_empty() {
            return Ok(());
        }
        let (rc, out, err_text) = (RUN_AWS.read().unwrap())(
            ctx,
            self.config(m).args(&[
                "terminate-instances".into(),
                "--instance-ids".into(),
                id.clone(),
            ]),
            AWS_LONG,
        )?;
        if rc != 0 {
            if crate::provider::not_found_answer(out.clone(), err_text.clone()) {
                return Ok(());
            }
            return aws_err(
                format!("aws ec2 terminate-instances {id}"),
                rc,
                out,
                err_text,
                anyhow!(""),
            );
        }
        Ok(())
    }
    pub(crate) fn probe(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        let id = m.detail.get("instance_id").cloned().unwrap_or_default();
        if id.is_empty() {
            return Err(anyhow!("aws: no machine"));
        }
        let got = aws_describe(
            ctx,
            self.config(m),
            &[format!("Name=instance-id,Values={id}")],
        )?;
        if got.is_empty() {
            return Err(anyhow!("{} ({}) is gone", m.name, id));
        }
        if got[0].state.name != "running" {
            return Err(anyhow!(
                "{} is {}",
                m.name,
                crate::board::py_repr(got[0].state.name.clone())
            ));
        }
        Ok(())
    }
    pub(crate) fn find(
        &self,
        ctx: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        executor: String,
        host: String,
    ) -> anyhow::Result<Option<crate::provider::Machine>> {
        let cfg = aws_settings(s, String::new());
        let got = aws_describe(
            ctx,
            cfg.clone(),
            &[
                format!("Name=tag:{},Values=hands", crate::provider::TAG_HANDS),
                "Name=instance-state-name,Values=pending,running,stopping,stopped".into(),
            ],
        )
        .map_err(|e| {
            anyhow!(
                "the AWS CLI could not answer: {}",
                crate::run::err_message(&e)
            )
        })?;
        for i in got {
            if i.tag("Name".into()) == executor || (!host.is_empty() && i.public_ip == host) {
                return Ok(aws_machine(cfg, i));
            }
        }
        Ok(None)
    }
    pub(crate) fn stop(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        let id = m.detail.get("instance_id").cloned().unwrap_or_default();
        let (rc, out, err_text) = (RUN_AWS.read().unwrap())(
            ctx,
            self.config(m)
                .args(&["stop-instances".into(), "--instance-ids".into(), id.clone()]),
            AWS_LONG,
        )?;
        if rc != 0 {
            return aws_err(
                format!("aws ec2 stop-instances {id}"),
                rc,
                out,
                err_text,
                anyhow!(""),
            );
        }
        Ok(())
    }
    pub(crate) fn place(&self, m: &crate::provider::Machine) -> String {
        m.detail.get("region").cloned().unwrap_or_default()
    }
    pub(crate) fn start_hint(&self, m: &crate::provider::Machine) -> String {
        let mut hint = format!(
            "aws ec2 start-instances --instance-ids {}",
            m.detail.get("instance_id").cloned().unwrap_or_default()
        );
        if let Some(region) = m.detail.get("region").filter(|s| !s.is_empty()) {
            hint.push_str(&format!(" --region {region}"));
        }
        if let Some(profile) = m.detail.get("profile").filter(|s| !s.is_empty()) {
            hint.push_str(&format!(" --profile {profile}"));
        }
        hint
    }
    pub(crate) fn delete(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<(bool, Vec<String>)> {
        let id = m.detail.get("instance_id").cloned().unwrap_or_default();
        if id.is_empty() {
            return Ok((false, vec![]));
        }
        let cfg = self.config(m);
        let got = aws_describe(ctx, cfg.clone(), &[format!("Name=instance-id,Values={id}")])?;
        if got.is_empty()
            || got[0].state.name == "terminated"
            || got[0].state.name == "shutting-down"
        {
            return Ok((false, vec![]));
        }
        let (rc, out, err_text) = (RUN_AWS.read().unwrap())(
            ctx,
            cfg.args(&[
                "terminate-instances".into(),
                "--instance-ids".into(),
                id.clone(),
            ]),
            AWS_LONG,
        )?;
        if rc != 0 {
            return aws_err(
                format!("aws ec2 terminate-instances {id}"),
                rc,
                out,
                err_text,
                anyhow!(""),
            )
            .map(|_| (true, vec![]));
        }
        Ok((true, vec![format!("instance {id} with its root volume")]))
    }
}

impl AwsConfig {
    pub(crate) fn scope(&self, a: Vec<String>) -> Vec<String> {
        let mut out = a;
        if !self.profile.is_empty() {
            out.extend(["--profile".into(), self.profile.clone()]);
        }
        if !self.region.is_empty() {
            out.extend(["--region".into(), self.region.clone()]);
        }
        out
    }
    pub(crate) fn args(&self, a: &[String]) -> Vec<String> {
        let mut out = vec!["ec2".into()];
        out.extend_from_slice(a);
        out = self.scope(out);
        out.extend(["--output".into(), "json".into()]);
        out
    }
    pub(crate) fn image_arg(&self) -> String {
        if self.image.starts_with('/') {
            format!("resolve:ssm:{}", self.image)
        } else {
            self.image.clone()
        }
    }
}

impl AwsInstance {
    pub(crate) fn tag(&self, key: String) -> String {
        self.tags
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.value.clone())
            .unwrap_or_default()
    }
}

impl crate::provider::Provider for AwsProvider {
    fn name(&self) -> String {
        "aws".to_string()
    }
    fn kind(&self) -> String {
        self.kind()
    }
    fn provision(
        &self,
        cancel: &pulse_bus::CancellationToken,
        req: crate::provider::ProvisionRequest,
    ) -> anyhow::Result<crate::provider::Machine> {
        self.provision(cancel, req)
    }
    fn release(
        &self,
        cancel: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        self.release(cancel, m)
    }
    fn probe(
        &self,
        cancel: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        self.probe(cancel, m)
    }
    fn find(
        &self,
        cancel: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        executor: &str,
        host: &str,
    ) -> anyhow::Result<Option<crate::provider::Machine>> {
        self.find(cancel, s, executor.to_string(), host.to_string())
    }
    fn stop(
        &self,
        cancel: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        self.stop(cancel, m)
    }
    fn delete(
        &self,
        cancel: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<(bool, Vec<String>)> {
        self.delete(cancel, m)
    }
    fn place(&self, m: &crate::provider::Machine) -> String {
        self.place(m)
    }
    fn start_hint(&self, m: &crate::provider::Machine) -> String {
        self.start_hint(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::new_store;

    #[test]
    fn aws_provision_needs_login_and_region() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        crate::test_helpers::new_bare_repo(&s);
        let old = set_run_aws(Box::new(|_, args, _| {
            let cmd = args.join(" ");
            if cmd.starts_with("sts get-caller-identity") {
                return Ok((
                    255,
                    String::new(),
                    "Unable to locate credentials. You can configure credentials by running \"aws configure\".".into(),
                ));
            }
            Ok((0, String::new(), String::new()))
        }));
        let err = AwsProvider {}
            .provision(
                &pulse_bus::CancellationToken::new(),
                crate::provider::ProvisionRequest {
                    store: Some((*s).clone()),
                    id: "CR-001".into(),
                    executor: "hands-cr-001".into(),
                    project: "proj".into(),
                    hub_home: tempfile::tempdir()
                        .expect("hub")
                        .path()
                        .display()
                        .to_string(),
                    ..Default::default()
                },
            )
            .expect_err("missing AWS login");
        assert!(
            err.to_string().contains("not logged in to AWS")
                && err.to_string().contains("aws configure"),
            "err: {err}"
        );
        let _ = set_run_aws(Box::new(|_, args, _| {
            let cmd = args.join(" ");
            if cmd.starts_with("sts get-caller-identity") {
                return Ok((0, r#"{"Account":"1","Arn":"arn"}"#.into(), String::new()));
            }
            if cmd.starts_with("configure get region") {
                return Ok((1, String::new(), String::new()));
            }
            Ok((1, String::new(), "unexpected".into()))
        }));
        let err = AwsProvider {}
            .provision(
                &pulse_bus::CancellationToken::new(),
                crate::provider::ProvisionRequest {
                    store: Some((*s).clone()),
                    id: "CR-001".into(),
                    executor: "hands-cr-001".into(),
                    project: "proj".into(),
                    hub_home: tempfile::tempdir()
                        .expect("hub")
                        .path()
                        .display()
                        .to_string(),
                    ..Default::default()
                },
            )
            .expect_err("missing AWS region");
        assert!(err.to_string().contains("compute.aws.region"), "err: {err}");
        let _ = set_run_aws(old);
    }
}
