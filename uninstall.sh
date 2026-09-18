#!/usr/bin/env bash
# Remove everything ./install.sh installed: the `byo` command and every tester binary.
# With --purge, also delete the data directory (stage progress, run history, the installed
# site, the copied catalogs and data files).
#
#   ./uninstall.sh
#   ./uninstall.sh --purge
set -euo pipefail

BYO_BIN_DIR="${BYO_BIN_DIR:-$HOME/.local/bin}"
BYO_HOME="${BYO_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/byo}"
PURGE=0
YES=0

# Keep in step with install.sh's TRACKS table and byo/src/track.rs.
BINARIES=(byo shelltest kafkatest wasmtest tlstest linktest)

for arg in "$@"; do
  case "$arg" in
    --purge) PURGE=1 ;;
    --yes|-y) YES=1 ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) echo "uninstall.sh: unknown option '$arg'" >&2; exit 2 ;;
  esac
done

for b in "${BINARIES[@]}"; do
  if [ -e "$BYO_BIN_DIR/$b" ]; then
    rm -f "$BYO_BIN_DIR/$b"
    echo "removed $BYO_BIN_DIR/$b"
  fi
done

if [ "$PURGE" = 1 ]; then
  if [ -d "$BYO_HOME" ]; then
    if [ "$YES" != 1 ]; then
      printf 'Delete %s (progress, run history, catalogs, installed site)? Type yes: ' "$BYO_HOME"
      read -r answer
      [ "$answer" = yes ] || { echo "kept $BYO_HOME"; exit 0; }
    fi
    rm -rf "$BYO_HOME"
    echo "removed $BYO_HOME"
  else
    echo "$BYO_HOME does not exist"
  fi
else
  echo "kept $BYO_HOME (pass --purge to delete it)"
fi
