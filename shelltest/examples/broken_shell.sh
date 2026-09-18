#!/bin/sh
# A deliberately wrong shell, so you can see what shelltest failure output looks like:
#   shelltest --shell examples/broken_shell.sh --until 5
# Bugs on purpose: echo drops its last argument, `exit N` ignores N, the
# command-not-found message goes to stdout with the wrong wording, and `type` is missing.
while :; do
    printf '$ '
    if ! IFS= read -r line; then
        exit 0
    fi
    set -- $line
    [ $# -eq 0 ] && continue
    cmd=$1
    shift
    case $cmd in
        exit) exit 0 ;;
        echo)
            if [ $# -gt 1 ]; then
                n=$(( $# - 1 )); i=0; out=""
                for a in "$@"; do
                    i=$((i + 1)); [ $i -gt $n ] && break
                    out="$out${out:+ }$a"
                done
                printf '%s\n' "$out"
            else
                printf '%s\n' "$*"
            fi ;;
        *) printf '%s: not found\n' "$cmd" ;;
    esac
done
