#!/bin/sh
set -eu

usage() {
  cat <<'EOF'
Usage:
  sudo ./scripts/install-autodeploy.sh [options]

Options:
  --user <name>       Linux user running deploy/build (default: aristath)
  --repo <path>       Repo directory (default: /home/aristath/saelora)
  --branch <name>     Branch to track (default: main)
  --service <name>    App service to restart (default: saelora)
  --bin <path>        Installed binary path (default: /home/aristath/saelora-bin/saelora)
  --interval <dur>    Poll interval for updates (default: 120s)
  -h, --help          Show this help

Example:
  sudo ./scripts/install-autodeploy.sh --interval 120s
EOF
}

RUN_USER="aristath"
REPO_DIR="/home/aristath/saelora"
BRANCH="main"
SERVICE_NAME="saelora"
BIN_PATH="/home/aristath/saelora-bin/saelora"
INTERVAL="120s"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --user)
      RUN_USER="${2:-}"
      shift 2
      ;;
    --repo)
      REPO_DIR="${2:-}"
      shift 2
      ;;
    --branch)
      BRANCH="${2:-}"
      shift 2
      ;;
    --service)
      SERVICE_NAME="${2:-}"
      shift 2
      ;;
    --bin)
      BIN_PATH="${2:-}"
      shift 2
      ;;
    --interval)
      INTERVAL="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ "$(id -u)" -ne 0 ]; then
  echo "Run this script with sudo/root." >&2
  exit 1
fi

if ! id "$RUN_USER" >/dev/null 2>&1; then
  echo "User not found: $RUN_USER" >&2
  exit 1
fi

if [ ! -d "$REPO_DIR/.git" ]; then
  echo "Not a git checkout: $REPO_DIR" >&2
  exit 1
fi

DEPLOY_SCRIPT="$REPO_DIR/scripts/auto-deploy.sh"
if [ ! -f "$DEPLOY_SCRIPT" ]; then
  echo "Deploy script not found: $DEPLOY_SCRIPT" >&2
  exit 1
fi

chmod +x "$DEPLOY_SCRIPT"

SUDOERS_FILE="/etc/sudoers.d/saelora-autodeploy"
cat > "$SUDOERS_FILE" <<EOF
$RUN_USER ALL=(root) NOPASSWD:/bin/systemctl restart $SERVICE_NAME
EOF
chmod 0440 "$SUDOERS_FILE"
visudo -cf "$SUDOERS_FILE" >/dev/null

SERVICE_UNIT="/etc/systemd/system/saelora-autodeploy.service"
cat > "$SERVICE_UNIT" <<EOF
[Unit]
Description=Saelora auto deploy (git pull/build/restart)
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
User=$RUN_USER
Group=$RUN_USER
WorkingDirectory=$REPO_DIR
Environment=HOME=/home/$RUN_USER
Environment=PATH=/home/$RUN_USER/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ExecStart=$DEPLOY_SCRIPT --repo $REPO_DIR --branch $BRANCH --service $SERVICE_NAME --bin $BIN_PATH
EOF

TIMER_UNIT="/etc/systemd/system/saelora-autodeploy.timer"
cat > "$TIMER_UNIT" <<EOF
[Unit]
Description=Check git updates for Saelora every $INTERVAL

[Timer]
OnBootSec=45s
OnUnitActiveSec=$INTERVAL
Unit=saelora-autodeploy.service
Persistent=true

[Install]
WantedBy=timers.target
EOF

systemctl daemon-reload
systemctl enable --now saelora-autodeploy.timer
systemctl start saelora-autodeploy.service

echo "Installed:"
echo "  - $SERVICE_UNIT"
echo "  - $TIMER_UNIT"
echo "  - $SUDOERS_FILE"
echo
systemctl status saelora-autodeploy.timer --no-pager -l | head -n 20
