#!/usr/bin/env bash
# Build and install the BuildYourOwn toolchain: the two testers, the `byo` command and the
# journey site. Idempotent — re-run it after pulling changes.
#
#   ./install.sh                 build everything and install
#   ./install.sh --skip-site     skip the (slow) npm build; keep whatever site is installed
#   BYO_BIN_DIR=~/bin ./install.sh
#   BYO_HOME=~/.byo ./install.sh
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BYO_BIN_DIR="${BYO_BIN_DIR:-$HOME/.local/bin}"
BYO_HOME="${BYO_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/byo}"
SKIP_SITE=0

for arg in "$@"; do
  case "$arg" in
    --skip-site) SKIP_SITE=1 ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "install.sh: unknown option '$arg'" >&2; exit 2 ;;
  esac
done

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  BOLD=$'\033[1m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; RED=$'\033[31m'; DIM=$'\033[2m'; OFF=$'\033[0m'
else
  BOLD=''; GREEN=''; YELLOW=''; RED=''; DIM=''; OFF=''
fi
step() { printf '\n%s==>%s %s%s%s\n' "$GREEN" "$OFF" "$BOLD" "$*" "$OFF"; }
warn() { printf '%s!!%s  %s\n' "$YELLOW" "$OFF" "$*" >&2; }
die()  { printf '%sxx%s  %s\n' "$RED" "$OFF" "$*" >&2; exit 1; }
note() { printf '%s    %s%s\n' "$DIM" "$*" "$OFF"; }

WARNINGS=()

command -v cargo >/dev/null 2>&1 || die "cargo is not on PATH — install Rust from https://rustup.rs"

step "Building the testers and byo (release)"
for crate in shelltest kafkatest byo; do
  [ -d "$REPO/$crate" ] || die "$REPO/$crate is missing"
  printf '  %s… ' "$crate"
  if [ "$crate" = kafkatest ]; then
    # kafkatest also ships an intentionally broken example broker used by the docs.
    if (cd "$REPO/$crate" && cargo build --release --bins --examples >/tmp/byo-build-$crate.log 2>&1); then
      printf '%sok%s\n' "$GREEN" "$OFF"
    else
      printf '%sfailed%s\n' "$YELLOW" "$OFF"
      tail -20 "/tmp/byo-build-$crate.log" >&2 || true
      warn "kafkatest did not build; the kafka track will not work until it does"
      WARNINGS+=("kafkatest failed to build (see /tmp/byo-build-kafkatest.log)")
      continue
    fi
  else
    if (cd "$REPO/$crate" && cargo build --release >/tmp/byo-build-$crate.log 2>&1); then
      printf '%sok%s\n' "$GREEN" "$OFF"
    else
      printf '%sfailed%s\n' "$RED" "$OFF"
      tail -30 "/tmp/byo-build-$crate.log" >&2 || true
      die "$crate failed to build"
    fi
  fi
done

step "Building the site"
if [ "$SKIP_SITE" = 1 ]; then
  note "--skip-site given; leaving $BYO_HOME/site alone"
elif [ ! -d "$REPO/site" ]; then
  warn "$REPO/site is missing; skipping the site build"
  WARNINGS+=("no site/ directory in the repo")
elif ! command -v npm >/dev/null 2>&1; then
  warn "npm is not on PATH; skipping the site build"
  WARNINGS+=("npm missing — run \`byo site --rebuild\` once node is installed")
else
  (
    cd "$REPO/site"
    if [ -f package-lock.json ]; then npm ci; else npm install; fi
    npm run build
  ) || {
    warn "the site build failed; \`byo site\` will serve a placeholder page"
    WARNINGS+=("site build failed — run \`byo site --rebuild\` after fixing it")
  }
fi

step "Installing binaries into $BYO_BIN_DIR"
mkdir -p "$BYO_BIN_DIR"
for crate in shelltest kafkatest byo; do
  src="$REPO/$crate/target/release/$crate"
  if [ -x "$src" ]; then
    install -m 0755 "$src" "$BYO_BIN_DIR/$crate"
    note "$BYO_BIN_DIR/$crate"
  else
    warn "$src does not exist; $crate was not installed"
    WARNINGS+=("$crate binary missing")
  fi
done

step "Installing data into $BYO_HOME"
mkdir -p "$BYO_HOME"

copy_tree() { # copy_tree <src dir> <dest dir>
  if [ -d "$1" ]; then
    rm -rf "$2"
    mkdir -p "$(dirname "$2")"
    cp -R "$1" "$2"
    note "$2"
  else
    warn "$1 is missing"
    WARNINGS+=("missing $1")
  fi
}
copy_file() { # copy_file <src> <dest> [optional]
  if [ -f "$1" ]; then
    install -m 0644 "$1" "$2"
    note "$2"
  elif [ "${3:-}" = optional ]; then
    note "(no $1 — skipped)"
  else
    warn "$1 is missing"
    WARNINGS+=("missing $1")
  fi
}

copy_tree "$REPO/shelltest/tests" "$BYO_HOME/tests"
copy_file "$REPO/shelltest/shells.yaml" "$BYO_HOME/shells.yaml"
copy_file "$REPO/kafkatest/brokers.yaml" "$BYO_HOME/brokers.yaml"
copy_file "$REPO/kafkatest/catalog.json" "$BYO_HOME/catalog.kafka.json"
copy_file "$REPO/site/src/lib/data/catalog.shell.json" "$BYO_HOME/catalog.shell.json" optional

if [ -d "$REPO/site/build" ]; then
  copy_tree "$REPO/site/build" "$BYO_HOME/site"
else
  warn "no $REPO/site/build — \`byo site\` will serve a placeholder until you run \`byo site --rebuild\`"
  WARNINGS+=("site/build missing — run \`byo site --rebuild\`")
fi

step "Creating the database"
BYO="$BYO_BIN_DIR/byo"
[ -x "$BYO" ] || die "byo was not installed"
export BYO_HOME
if [ -d "$REPO/site" ]; then
  "$BYO" db set-site-source "$REPO/site" >/dev/null
fi
note "$("$BYO" db path)  (schema created/migrated)"

step "Done"
"$BYO" doctor || true

case ":${PATH}:" in
  *":$BYO_BIN_DIR:"*) ;;
  *)
    printf '\n%s%s is not on your PATH.%s Add this to your shell rc file:\n\n' "$YELLOW" "$BYO_BIN_DIR" "$OFF"
    printf '    export PATH="%s:$PATH"\n' "$BYO_BIN_DIR"
    ;;
esac

if [ "${#WARNINGS[@]}" -gt 0 ]; then
  printf '\n%sInstalled with %d warning(s):%s\n' "$YELLOW" "${#WARNINGS[@]}" "$OFF"
  for w in "${WARNINGS[@]}"; do printf '  - %s\n' "$w"; done
fi

cat <<EOF

${BOLD}Next steps${OFF}
  cd ~/code/my-shell && byo init shell --command ./your_program.sh
  byo test --stage 1
  byo status
  byo site
EOF
