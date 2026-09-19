#!/usr/bin/env bash
# Build and install the BuildYourOwn toolchain: every tester that is present, the `byo`
# command and the journey site. Idempotent — re-run it after pulling changes.
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

# ---------------------------------------------------------------------------------------
# The track registry. Keep this table in step with `byo/src/track.rs`: adding a track is one
# row here and one TrackDef there — no other code changes anywhere.
#
#   id | repo dir | tester binary | cargo build flags | data files (src:dest,…) | catalogs
#
# `src` is relative to the repo root, `dest` to $BYO_HOME. Catalog candidates are tried in
# order; if none exists the tester is asked for one with `--list --json`.
# ---------------------------------------------------------------------------------------
TRACKS=(
  "shell|shelltest|shelltest|--bins|shelltest/tests:tests,shelltest/shells.yaml:shells.yaml|shelltest/catalog.json,site/src/lib/data/catalog.shell.json"
  "kafka|kafkatest|kafkatest|--bins --examples|kafkatest/brokers.yaml:brokers.yaml|kafkatest/catalog.json,site/src/lib/data/catalog.kafka.json"
  "wasm|wasmtest|wasmtest|--bins|wasmtest/runtimes.yaml:runtimes.yaml|wasmtest/catalog.json,site/src/lib/data/catalog.wasm.json"
  "tls|tlstest|tlstest|--bins|tlstest/servers.yaml:servers.yaml|tlstest/catalog.json,site/src/lib/data/catalog.tls.json"
  "link|linktest|linktest|--bins|linktest/linkers.yaml:linkers.yaml|linktest/catalog.json,site/src/lib/data/catalog.link.json"
  "dist|disttest|disttest|--bins|disttest/targets.yaml:targets.yaml|disttest/catalog.json,site/src/lib/data/catalog.dist.json"
)
field() { printf '%s' "$1" | cut -d'|' -f"$2"; }

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

# The journey owner's name lives in the git-ignored .env. The site build bakes
# PUBLIC_JOURNEY_OWNER in, and `byo` reads it at runtime — so export it here, and never copy
# the file itself into $BYO_HOME (it is personal configuration, not installable data).
step "Configuration"
if [ -f "$REPO/.env" ]; then
  set -a
  # shellcheck disable=SC1091
  . "$REPO/.env"
  set +a
  note ".env sourced — PUBLIC_JOURNEY_OWNER=${PUBLIC_JOURNEY_OWNER:-(unset)}"
else
  note "no $REPO/.env — everything will read \"The Journey\" (copy .env.example to .env)"
fi

step "Building the testers and byo (release)"
for row in "${TRACKS[@]}"; do
  id="$(field "$row" 1)"; dir="$(field "$row" 2)"; tester="$(field "$row" 3)"
  read -r -a build_flags <<< "$(field "$row" 4)"
  if [ ! -d "$REPO/$dir" ]; then
    note "$id: $dir/ (not present, skipped)"
    continue
  fi
  printf '  %s… ' "$tester"
  if (cd "$REPO/$dir" && cargo build --release "${build_flags[@]}" >"/tmp/byo-build-$tester.log" 2>&1); then
    printf '%sok%s\n' "$GREEN" "$OFF"
  else
    printf '%sfailed%s\n' "$YELLOW" "$OFF"
    tail -20 "/tmp/byo-build-$tester.log" >&2 || true
    warn "$tester did not build; the $id track will not work until it does"
    WARNINGS+=("$tester failed to build (see /tmp/byo-build-$tester.log)")
  fi
done

printf '  %s… ' byo
[ -d "$REPO/byo" ] || die "$REPO/byo is missing"
if (cd "$REPO/byo" && cargo build --release >/tmp/byo-build-byo.log 2>&1); then
  printf '%sok%s\n' "$GREEN" "$OFF"
else
  printf '%sfailed%s\n' "$RED" "$OFF"
  tail -30 /tmp/byo-build-byo.log >&2 || true
  die "byo failed to build"
fi

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
# Binaries land in the workspace target directory; the per-crate path is kept as a
# fallback so a crate built on its own still installs.
built_binary() { # built_binary <crate dir> <binary name>
  if [ -x "$REPO/target/release/$2" ]; then
    printf '%s\n' "$REPO/target/release/$2"
  else
    printf '%s\n' "$REPO/$1/target/release/$2"
  fi
}

install_bin() { # install_bin <crate dir> <binary name> <required: yes|no>
  local src
  src="$(built_binary "$1" "$2")"
  if [ -x "$src" ]; then
    install -m 0755 "$src" "$BYO_BIN_DIR/$2"
    note "$BYO_BIN_DIR/$2"
  elif [ "$3" = yes ]; then
    die "$src does not exist; byo was not installed"
  else
    warn "$src does not exist; $2 was not installed"
    WARNINGS+=("$2 binary missing")
  fi
}
install_bin byo byo yes
for row in "${TRACKS[@]}"; do
  dir="$(field "$row" 2)"; tester="$(field "$row" 3)"
  [ -d "$REPO/$dir" ] || continue
  install_bin "$dir" "$tester" no
done

step "Installing data into $BYO_HOME"
mkdir -p "$BYO_HOME"
# An early version of this script had no such rule; make sure no personal .env lingers.
if [ -f "$BYO_HOME/.env" ]; then
  rm -f "$BYO_HOME/.env"
  note "removed a stale $BYO_HOME/.env (personal config never belongs here)"
fi

copy_tree() { # copy_tree <src dir> <dest dir>
  rm -rf "$2"
  mkdir -p "$(dirname "$2")"
  cp -R "$1" "$2"
  note "$2"
}
copy_file() { # copy_file <src> <dest>
  install -m 0644 "$1" "$2"
  note "$2"
}

for row in "${TRACKS[@]}"; do
  id="$(field "$row" 1)"; dir="$(field "$row" 2)"; tester="$(field "$row" 3)"
  if [ ! -d "$REPO/$dir" ]; then
    note "$id: no data (not present, skipped)"
    continue
  fi

  IFS=',' read -r -a pairs <<< "$(field "$row" 5)"
  for pair in "${pairs[@]}"; do
    [ -n "$pair" ] || continue
    src="$REPO/${pair%%:*}"
    dest="$BYO_HOME/${pair##*:}"
    if [ -d "$src" ]; then
      copy_tree "$src" "$dest"
    elif [ -f "$src" ]; then
      copy_file "$src" "$dest"
    else
      warn "$src is missing; the $id track will run without it"
      WARNINGS+=("missing ${pair%%:*}")
    fi
  done

  # The stage catalog the site and `byo status` read: a committed one if there is one,
  # otherwise whatever the freshly built tester says.
  catalog_dest="$BYO_HOME/catalog.$id.json"
  IFS=',' read -r -a candidates <<< "$(field "$row" 6)"
  copied=0
  for cand in "${candidates[@]}"; do
    if [ -f "$REPO/$cand" ]; then
      copy_file "$REPO/$cand" "$catalog_dest"
      copied=1
      break
    fi
  done
  if [ "$copied" = 0 ]; then
    tmp="$(mktemp)"
    tester_bin="$(built_binary "$dir" "$tester")"
    if [ -x "$tester_bin" ] \
       && "$tester_bin" --list --json >"$tmp" 2>/dev/null \
       && head -c1 "$tmp" | grep -q '{'; then
      install -m 0644 "$tmp" "$catalog_dest"
      note "$catalog_dest (generated by \`$tester --list --json\`)"
    else
      warn "no catalog for the $id track; /api/catalog/$id will 404"
      WARNINGS+=("no catalog.$id.json — commit $dir/catalog.json or teach $tester --list --json")
    fi
    rm -f "$tmp"
  fi
done

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

# `byo` reads the owner from the environment (never from a copied file), so the name only
# shows up in `byo status` once your shell exports it too.
if [ -n "${PUBLIC_JOURNEY_OWNER:-}" ]; then
  printf '\n%sFor "%s'"'"'s Journey" in `byo status`, export it in your shell rc file too:%s\n\n' \
    "$DIM" "$PUBLIC_JOURNEY_OWNER" "$OFF"
  printf '    export PUBLIC_JOURNEY_OWNER="%s"\n' "$PUBLIC_JOURNEY_OWNER"
fi

if [ "${#WARNINGS[@]}" -gt 0 ]; then
  printf '\n%sInstalled with %d warning(s):%s\n' "$YELLOW" "${#WARNINGS[@]}" "$OFF"
  for w in "${WARNINGS[@]}"; do printf '  - %s\n' "$w"; done
fi

cat <<EOF

${BOLD}Next steps${OFF}
  byo tracks                                      what you can build
  cd ~/code/my-shell && byo init shell --command ./your_program.sh
  byo test --stage 1
  byo status
  byo site
EOF
