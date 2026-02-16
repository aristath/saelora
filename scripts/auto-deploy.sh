#!/bin/sh
set -eu

usage() {
  cat <<'EOF'
Usage:
  auto-deploy.sh [options]

Options:
  --repo <path>        Repo directory (default: $HOME/saelora)
  --branch <name>      Branch to track (default: main)
  --service <name>     systemd service name to restart (default: saelora)
  --bin <path>         Installed binary path (default: $HOME/saelora-bin/saelora)
  --no-restart         Deploy code/build/binary only; do not restart service
  --check-only         Only check whether remote commit differs; exit 0 either way
  --force              Force deploy even when local HEAD == origin/<branch>
  -h, --help           Show this help
EOF
}

log() {
  printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*"
}

REPO_DIR="${HOME}/saelora"
BRANCH="main"
SERVICE_NAME="saelora"
BIN_PATH="${HOME}/saelora-bin/saelora"
CARGO_BIN="${HOME}/.cargo/bin/cargo"
CHECK_ONLY=0
FORCE=0
DO_RESTART=1

while [ "$#" -gt 0 ]; do
  case "$1" in
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
    --no-restart)
      DO_RESTART=0
      shift
      ;;
    --check-only)
      CHECK_ONLY=1
      shift
      ;;
    --force)
      FORCE=1
      shift
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

if [ ! -d "$REPO_DIR/.git" ]; then
  echo "Repo not found or not a git checkout: $REPO_DIR" >&2
  exit 1
fi

if [ ! -x "$CARGO_BIN" ] && ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found at $CARGO_BIN and not in PATH" >&2
  exit 1
fi

cd "$REPO_DIR"

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Working tree is dirty in $REPO_DIR; refusing to auto-deploy." >&2
  exit 1
fi

log "Fetching origin/$BRANCH"
git fetch --prune origin "$BRANCH"

REMOTE_COMMIT="$(git rev-parse "origin/$BRANCH")"
LOCAL_COMMIT="$(git rev-parse HEAD)"

if [ "$CHECK_ONLY" -eq 1 ]; then
  if [ "$LOCAL_COMMIT" = "$REMOTE_COMMIT" ]; then
    log "No deploy needed (local == origin/$BRANCH at $LOCAL_COMMIT)"
  else
    log "Deploy needed (local $LOCAL_COMMIT -> remote $REMOTE_COMMIT)"
  fi
  exit 0
fi

if [ "$FORCE" -ne 1 ] && [ "$LOCAL_COMMIT" = "$REMOTE_COMMIT" ]; then
  log "No new commits on origin/$BRANCH"
  exit 0
fi

CURRENT_BRANCH="$(git symbolic-ref --short -q HEAD || true)"
if [ "$CURRENT_BRANCH" != "$BRANCH" ]; then
  if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
    log "Checking out local branch $BRANCH"
    git checkout "$BRANCH"
  else
    log "Creating tracking branch $BRANCH -> origin/$BRANCH"
    git checkout -b "$BRANCH" --track "origin/$BRANCH"
  fi
fi

log "Pulling latest commit from origin/$BRANCH"
git pull --ff-only origin "$BRANCH"

if [ -x "$CARGO_BIN" ]; then
  BUILD_CMD="$CARGO_BIN"
else
  BUILD_CMD="$(command -v cargo)"
fi

log "Building release binary"
"$BUILD_CMD" build --release --locked

TARGET_BIN="$REPO_DIR/target/release/saelora"
if [ ! -f "$TARGET_BIN" ]; then
  echo "Build finished but binary not found: $TARGET_BIN" >&2
  exit 1
fi

INSTALL_DIR="$(dirname "$BIN_PATH")"
mkdir -p "$INSTALL_DIR"
install -m 755 "$TARGET_BIN" "$BIN_PATH"
log "Installed binary to $BIN_PATH"

if [ "$DO_RESTART" -eq 1 ]; then
  log "Restarting service: $SERVICE_NAME"
  if ! sudo systemctl restart "$SERVICE_NAME"; then
    echo "Failed to restart $SERVICE_NAME. Check sudo/systemd permissions." >&2
    exit 1
  fi
  log "Service restarted: $SERVICE_NAME"
else
  log "Skipping restart (--no-restart)"
fi

exit 0
