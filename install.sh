#!/usr/bin/env sh
# Divan installer (F5.2). POSIX sh; macOS + Linux only (v1, P0.4).
#
# Installs all four Divan binaries into ~/.cargo/bin so the `divan` CLI can
# find its siblings (`divan-daemon`, `divan-mcp`, `divan-tui`) next to itself.
# Then optionally installs the Claude turn-boundary/activity hooks — which
# MERGE into your existing config and take a backup, never overwrite (K-safe).
#
# Usage:
#   ./install.sh                 # build + install the 4 binaries
#   ./install.sh --with-hooks    # also run `divan install-hooks --tool claude`
#   ./install.sh --help
#
# Requirements: a Rust toolchain (cargo) >= 1.88. Get one at https://rustup.rs.

set -eu

REPO_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
WITH_HOOKS=0

for arg in "$@"; do
  case "$arg" in
    --with-hooks) WITH_HOOKS=1 ;;
    -h|--help)
      sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "install.sh: unknown argument '$arg' (try --help)" >&2
      exit 2
      ;;
  esac
done

# --- preflight ---------------------------------------------------------------
case "$(uname -s)" in
  Darwin|Linux) ;;
  *)
    echo "Divan v1 supports macOS and Linux only (got $(uname -s))." >&2
    exit 1
    ;;
esac

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install a Rust toolchain (>= 1.88): https://rustup.rs" >&2
  exit 1
fi

echo "==> Installing Divan from $REPO_DIR"
echo "    cargo: $(cargo --version)"

# --- install the four binaries ----------------------------------------------
# Each binary lives in its own crate, so install them one at a time; --locked
# keeps the pinned Cargo.lock. They all land in the same cargo bin dir, which
# is exactly what the CLI's sibling-lookup expects.
for crate in divan-cli divan-daemon divan-mcp divan-tui; do
  echo "==> cargo install --path crates/$crate"
  cargo install --path "$REPO_DIR/crates/$crate" --locked --force
done

BIN_DIR="${CARGO_HOME:-$HOME/.cargo}/bin"
echo "==> Installed: divan, divan-daemon, divan-mcp, divan-tui -> $BIN_DIR"

# PATH hint (don't mutate the user's shell config for them).
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo
    echo "NOTE: $BIN_DIR is not on your PATH. Add this to your shell profile:"
    echo "      export PATH=\"$BIN_DIR:\$PATH\""
    ;;
esac

# --- optional: hooks ---------------------------------------------------------
if [ "$WITH_HOOKS" -eq 1 ]; then
  echo "==> Installing Claude hooks (merge + backup, never overwrite)"
  "$BIN_DIR/divan" install-hooks --tool claude
fi

cat <<'EOF'

Done. Next steps:
  divan up                       # start the hub daemon
  divan run "add a CHANGELOG" --repo /path/to/your/repo
  divan tui                      # watch agents/tasks/messages/trace live
  divan down                     # stop the daemon

Hooks (if you skipped --with-hooks):
  divan install-hooks --tool claude     # merges into ~/.claude config, takes a backup
  divan uninstall-hooks --tool claude   # restores

Uninstall the binaries:
  cargo uninstall divan-cli divan-daemon divan-mcp divan-tui
EOF
