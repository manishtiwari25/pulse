#!/usr/bin/env bash
# Provision one PULSE remote executor ("hands") in Azure: a resource group, a network rule that admits ssh from
# one address, and an Ubuntu 24.04 VM prepared by cloud-init.yaml (ADR-020, docs/workflows/executors.md).
#
# It creates infrastructure and spends money - hard stops for an agent unless the owner has recorded, for one
# subscription, that agents may create compute there (then the owner's naming and tagging rules apply). It asks
# before it creates or deletes anything unless --yes is given. Idempotent: a second run with the same names
# changes nothing that exists and prints the same next steps. An existing resource group is used as it is - never
# retagged, never deleted - so the VM can live in a group shared with other, disposable work. Nothing here holds
# or places a repository credential; the copilot login on the VM is a separate, interactive step a human takes once.
#
# Usage:
#   bash docs/scripts/hosted/azure-provision.sh --group pulse-hands --location eastus --name pulse-vm1 \
#        --pubkey ~/.ssh/pulse_hands_ed25519.pub [--size Standard_B2ms] [--disk-gb 64] [--allow-ip 203.0.113.7] \
#        [--admin pulseadmin] [--tag key=value ...] [--yes]
#   bash docs/scripts/hosted/azure-provision.sh --group G --name N --host-key      # known_hosts line, via Azure, no TOFU
#   bash docs/scripts/hosted/azure-provision.sh --group G --name N --delete [--yes] # the VM, its disk, NIC, IP, NSG, VNET
#
# Teardown:        --delete (above) removes exactly what this script created and leaves the group;
#                  az group delete --name G only when the group holds nothing else
# Pause the meter: az vm deallocate --resource-group G --name N
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GROUP="" LOCATION="" NAME="" PUBKEY="" SIZE="Standard_B2ms" DISK_GB="64" ALLOW_IP="" ADMIN="pulseadmin" YES=0
MODE="create"
IMAGE="Canonical:ubuntu-24_04-lts:server:latest"
TAGS=("pulse=hands")

die() { echo "azure-provision: $*" >&2; exit 2; }

while [ $# -gt 0 ]; do
  case "$1" in
    --group) GROUP="$2"; shift 2 ;;
    --location) LOCATION="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --pubkey) PUBKEY="$2"; shift 2 ;;
    --size) SIZE="$2"; shift 2 ;;
    --disk-gb) DISK_GB="$2"; shift 2 ;;
    --allow-ip) ALLOW_IP="$2"; shift 2 ;;
    --admin) ADMIN="$2"; shift 2 ;;
    --tag) [[ "$2" =~ ^[A-Za-z0-9_.-]+=[A-Za-z0-9_.:/-]*$ ]] || die "--tag wants key=value with plain characters"; TAGS+=("$2"); shift 2 ;;
    --delete) MODE="delete"; shift ;;
    --host-key) MODE="host-key"; shift ;;
    --yes) YES=1; shift ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
done

command -v az >/dev/null 2>&1 || die "the Azure CLI (az) is not installed"
[[ "$NAME" =~ ^[a-z][a-z0-9-]{1,30}$ ]] || die "--name must be lowercase letters, digits, and dashes"
SUBSCRIPTION="$(az account show --query name -o tsv 2>/dev/null || true)"
[ -n "$SUBSCRIPTION" ] || die "not logged in: run az login first"
NSG="$NAME-nsg"

if [ "$MODE" = "host-key" ]; then
  # The VM's own host key, read through the Azure control plane (already authenticated), so the hub can trust it
  # without a first blind connection. Prints a known_hosts line for the VM's public address.
  [ -n "$GROUP" ] || die "--group and --name are required"
  IP="$(az vm show --resource-group "$GROUP" --name "$NAME" -d --query publicIps -o tsv)"
  [ -n "$IP" ] || die "the VM has no public address"
  KEY="$(az vm run-command invoke --resource-group "$GROUP" --name "$NAME" --command-id RunShellScript \
          --scripts 'cat /etc/ssh/ssh_host_ed25519_key.pub' --query 'value[0].message' -o tsv | grep -E '^ssh-ed25519 ' | head -1)"
  [ -n "$KEY" ] || die "could not read the host key (is the VM running and its agent up?)"
  echo "$IP $(echo "$KEY" | awk '{print $1, $2}')"
  exit 0
fi

if [ "$MODE" = "delete" ]; then
  [ -n "$GROUP" ] || die "--group and --name are required"
  cat <<EOF
Subscription : $SUBSCRIPTION
Will remove  : VM $NAME (with its OS disk and NIC), public IP ${NAME}PublicIP, NSG $NSG, VNET ${NAME}VNET
Keeps        : resource group $GROUP and everything else in it
EOF
  if [ "$YES" -ne 1 ]; then
    read -r -p "Delete these resources? [y/N] " answer
    [[ "$answer" =~ ^[Yy]$ ]] || { echo "nothing deleted"; exit 0; }
  fi
  if az vm show --resource-group "$GROUP" --name "$NAME" -o none 2>/dev/null; then
    echo "== vm $NAME"; az vm delete --resource-group "$GROUP" --name "$NAME" --yes -o none
  fi
  # The NIC is detached by the VM deletion (created with --nic-delete-option Delete); give it a moment to go.
  if az network nic show --resource-group "$GROUP" --name "${NAME}VMNic" -o none 2>/dev/null; then
    echo "== nic ${NAME}VMNic"; az network nic delete --resource-group "$GROUP" --name "${NAME}VMNic" -o none
  fi
  for attempt in 1 2 3 4 5 6; do
    if ! az network public-ip show --resource-group "$GROUP" --name "${NAME}PublicIP" -o none 2>/dev/null; then break; fi
    echo "== public ip ${NAME}PublicIP (try $attempt)"
    az network public-ip delete --resource-group "$GROUP" --name "${NAME}PublicIP" -o none 2>/dev/null && break
    sleep 10
  done
  if az network vnet show --resource-group "$GROUP" --name "${NAME}VNET" -o none 2>/dev/null; then
    echo "== vnet ${NAME}VNET"; az network vnet delete --resource-group "$GROUP" --name "${NAME}VNET" -o none
  fi
  if az network nsg show --resource-group "$GROUP" --name "$NSG" -o none 2>/dev/null; then
    echo "== nsg $NSG"; az network nsg delete --resource-group "$GROUP" --name "$NSG" -o none
  fi
  echo "left in $GROUP:"; az resource list --resource-group "$GROUP" --query "[].{name:name,type:type}" -o table
  exit 0
fi

[ -n "$GROUP" ] && [ -n "$LOCATION" ] && [ -n "$PUBKEY" ] || die "--group, --location, --name, and --pubkey are required"
[ -f "$PUBKEY" ] || die "no public key at $PUBKEY"
[ -f "$HERE/cloud-init.yaml" ] || die "cloud-init.yaml missing next to this script"
[ -f "$HERE/pulse-hands.sh" ] || die "pulse-hands.sh missing next to this script"
[[ "$ADMIN" =~ ^[a-z][a-z0-9_]{1,31}$ ]] || die "--admin must be a plain lowercase name"

KEY_LINE="$(grep -vE '^\s*(#|$)' "$PUBKEY" | head -1)"
[[ "$KEY_LINE" =~ ^(ssh-ed25519|ssh-rsa|ecdsa-sha2-nistp256|sk-ssh-ed25519@openssh.com)\  ]] || die "$PUBKEY does not look like an OpenSSH public key"

SUBSCRIPTION="$(az account show --query name -o tsv 2>/dev/null || true)"
[ -n "$SUBSCRIPTION" ] || die "not logged in: run az login first"
if [ -z "$ALLOW_IP" ]; then
  ALLOW_IP="$(curl -fsS --max-time 10 https://api.ipify.org 2>/dev/null || true)"
  [ -n "$ALLOW_IP" ] || die "could not learn this machine's public address; pass --allow-ip"
fi
[[ "$ALLOW_IP" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]] || die "--allow-ip must be an IPv4 address"
GROUP_EXISTS=0
az group show --name "$GROUP" -o none 2>/dev/null && GROUP_EXISTS=1

cat <<EOF
Subscription : $SUBSCRIPTION   (the one that pays)
Group        : $GROUP in $LOCATION $([ "$GROUP_EXISTS" -eq 1 ] && echo '(exists: used as it is, tags untouched)' || echo '(new)')
VM           : $NAME, $SIZE, ${DISK_GB} GiB StandardSSD, $IMAGE
Admin user   : $ADMIN (your key); hands user: pulse (same key, bound to pulse-hands + restrict)
SSH allowed  : from $ALLOW_IP/32 only
Tags         : ${TAGS[*]}
Cost         : the VM meter runs while it is on (a Standard_B2ms is roughly 60 USD a month) plus the
               Copilot premium requests the hands' login spends inside max_ai_credits per run.
EOF
if [ "$YES" -ne 1 ]; then
  read -r -p "Create or update these resources? [y/N] " answer
  [[ "$answer" =~ ^[Yy]$ ]] || { echo "nothing created"; exit 0; }
fi

echo "== resource group"
if [ "$GROUP_EXISTS" -eq 1 ]; then
  echo "exists; kept as it is"
else
  az group create --name "$GROUP" --location "$LOCATION" --tags "${TAGS[@]}" -o none
fi

echo "== network security group $NSG (ssh from $ALLOW_IP only)"
if ! az network nsg show --resource-group "$GROUP" --name "$NSG" -o none 2>/dev/null; then
  az network nsg create --resource-group "$GROUP" --name "$NSG" --location "$LOCATION" --tags "${TAGS[@]}" -o none
fi
if az network nsg rule show --resource-group "$GROUP" --nsg-name "$NSG" --name allow-ssh-hub -o none 2>/dev/null; then
  az network nsg rule update --resource-group "$GROUP" --nsg-name "$NSG" --name allow-ssh-hub \
    --source-address-prefixes "$ALLOW_IP/32" -o none
else
  az network nsg rule create --resource-group "$GROUP" --nsg-name "$NSG" --name allow-ssh-hub --priority 100 \
    --direction Inbound --access Allow --protocol Tcp --source-address-prefixes "$ALLOW_IP/32" \
    --destination-port-ranges 22 -o none
fi

echo "== cloud-init"
RENDERED="$(mktemp -t pulse-cloud-init.XXXXXX)"
trap 'rm -f "$RENDERED"' EXIT
HANDS_B64="$(base64 < "$HERE/pulse-hands.sh" | tr -d '\n')"
# Both placeholders are single tokens; the key line has no slash-free delimiter risk with |, the base64 none.
sed -e "s|__HUB_PUBKEY__|$KEY_LINE|" -e "s|__PULSE_HANDS__|$HANDS_B64|" "$HERE/cloud-init.yaml" > "$RENDERED"
grep -q "__HUB_PUBKEY__\|__PULSE_HANDS__" "$RENDERED" && die "placeholder left in the rendered cloud-init"

echo "== virtual machine $NAME"
if az vm show --resource-group "$GROUP" --name "$NAME" -o none 2>/dev/null; then
  echo "exists; not recreated (cloud-init runs on first boot only - re-place the key by hand if it changed)"
else
  az vm create --resource-group "$GROUP" --name "$NAME" --location "$LOCATION" --image "$IMAGE" --size "$SIZE" \
    --admin-username "$ADMIN" --ssh-key-values "$PUBKEY" --custom-data "$RENDERED" --nsg "$NSG" \
    --public-ip-sku Standard --os-disk-size-gb "$DISK_GB" --storage-sku StandardSSD_LRS \
    --os-disk-delete-option Delete --nic-delete-option Delete \
    --tags "${TAGS[@]}" -o none
fi

IP="$(az vm show --resource-group "$GROUP" --name "$NAME" -d --query publicIps -o tsv)"
[ -n "$IP" ] || die "the VM has no public address yet; run again in a minute"

cat <<EOF

Done. The VM finishes cloud-init in a few minutes (watch: ssh $ADMIN@$IP 'cloud-init status --wait').

Trust its host key through Azure, not a first blind connection:
  bash $0 --group $GROUP --name $NAME --host-key >> ~/.ssh/known_hosts

Next, in this order:
  1. ssh -t $ADMIN@$IP sudo -iu pulse copilot login          # a human, once: the hands' own Copilot login
  2. python3 docs/scripts/pulse.py executor add $NAME --ssh pulse@$IP --workdir /srv/pulse --workers 1 --projects '*'
  3. python3 docs/scripts/pulse.py executor probe $NAME       # activates it when the checklist passes
  4. python3 docs/scripts/pulse.py work --dry-run --executor $NAME
If this hub's public address changes, run this script again: it rewrites the ssh rule and touches nothing else.
Pause the meter: az vm deallocate --resource-group $GROUP --name $NAME
Remove it:       pulse executor drain $NAME; pulse executor remove $NAME; bash $0 --group $GROUP --name $NAME --delete
EOF
