#!/usr/bin/env bash
# Provision one PULSE remote executor ("hands") in Azure: a resource group, a network rule that admits ssh from
# one address, and an Ubuntu 24.04 VM prepared by cloud-init.yaml (ADR-020, docs/workflows/executors.md).
#
# Human-run only. It creates infrastructure and spends money, both hard stops for agents; it asks before it
# creates anything unless --yes is given. Idempotent: a second run with the same names changes nothing that
# exists and prints the same next steps. Nothing here holds or places a repository credential; the copilot
# login on the VM is a separate, interactive step the human takes once.
#
# Usage:
#   bash docs/scripts/hosted/azure-provision.sh --group pulse-hands --location eastus --name pulse-vm1 \
#        --pubkey ~/.ssh/id_ed25519.pub [--size Standard_B2ms] [--disk-gb 64] [--allow-ip 203.0.113.7] \
#        [--admin pulseadmin] [--yes]
#
# Teardown (yours, never an agent's):  az group delete --name pulse-hands
# Pause the meter:                    az vm deallocate --resource-group pulse-hands --name pulse-vm1
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GROUP="" LOCATION="" NAME="" PUBKEY="" SIZE="Standard_B2ms" DISK_GB="64" ALLOW_IP="" ADMIN="pulseadmin" YES=0
IMAGE="Canonical:ubuntu-24_04-lts:server:latest"

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
    --yes) YES=1; shift ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
done

[ -n "$GROUP" ] && [ -n "$LOCATION" ] && [ -n "$NAME" ] && [ -n "$PUBKEY" ] || die "--group, --location, --name, and --pubkey are required"
[ -f "$PUBKEY" ] || die "no public key at $PUBKEY"
[ -f "$HERE/cloud-init.yaml" ] || die "cloud-init.yaml missing next to this script"
[ -f "$HERE/pulse-hands.sh" ] || die "pulse-hands.sh missing next to this script"
command -v az >/dev/null 2>&1 || die "the Azure CLI (az) is not installed"
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

cat <<EOF
Subscription : $SUBSCRIPTION   (the one that pays)
Group        : $GROUP in $LOCATION
VM           : $NAME, $SIZE, ${DISK_GB} GiB StandardSSD, $IMAGE
Admin user   : $ADMIN (your key); hands user: pulse (same key, bound to pulse-hands + restrict)
SSH allowed  : from $ALLOW_IP/32 only
Cost         : the VM meter runs while it is on (a Standard_B2ms is roughly 60 USD a month) plus the
               Copilot premium requests the hands' login spends inside max_ai_credits per run.
EOF
if [ "$YES" -ne 1 ]; then
  read -r -p "Create or update these resources? [y/N] " answer
  [[ "$answer" =~ ^[Yy]$ ]] || { echo "nothing created"; exit 0; }
fi

echo "== resource group"
az group create --name "$GROUP" --location "$LOCATION" --tags pulse=hands -o none

NSG="$NAME-nsg"
echo "== network security group $NSG (ssh from $ALLOW_IP only)"
if ! az network nsg show --resource-group "$GROUP" --name "$NSG" -o none 2>/dev/null; then
  az network nsg create --resource-group "$GROUP" --name "$NSG" --location "$LOCATION" -o none
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
    --tags pulse=hands -o none
fi

IP="$(az vm show --resource-group "$GROUP" --name "$NAME" -d --query publicIps -o tsv)"
[ -n "$IP" ] || die "the VM has no public address yet; run again in a minute"

cat <<EOF

Done. The VM finishes cloud-init in a few minutes (watch: ssh $ADMIN@$IP 'cloud-init status --wait').

Next, in this order (each is yours, not an agent's):
  1. ssh -t $ADMIN@$IP sudo -iu pulse copilot login          # once; the hands' own Copilot login
  2. python3 docs/scripts/pulse.py executor add $NAME --ssh pulse@$IP --workdir /srv/pulse --workers 1 --projects '*'
  3. python3 docs/scripts/pulse.py executor probe $NAME       # activates it when the checklist passes
  4. python3 docs/scripts/pulse.py work --dry-run --executor $NAME
Pause the meter: az vm deallocate --resource-group $GROUP --name $NAME
Remove it all:   pulse executor drain $NAME; pulse executor remove $NAME; az group delete --name $GROUP
EOF
