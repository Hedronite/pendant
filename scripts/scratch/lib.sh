#!/usr/bin/env bash
# Shared guard for the scratch-session scripts. Sourced, not executed.
#
# Prints the session name to use, so a caller can capture it:
#   SESSION="$(scratch_session_name "$@")" || exit 1
#
# The name must start with `pendant-scratch` or `pendant-shell`. Anything else,
# including an empty name, is refused: these scripts stop and delete sessions,
# so they must never be able to reach a live one.
scratch_session_name() {
    local name="${1-${PENDANT_SCRATCH_SESSION-pendant-shell-g1}}"
    case "$name" in
        "")
            printf 'refusing an empty session name\n' >&2
            return 1
            ;;
        pendant-scratch* | pendant-shell*)
            printf '%s\n' "$name"
            ;;
        *)
            printf 'refusing session name outside pendant-scratch*/pendant-shell*: %s\n' "$name" >&2
            return 1
            ;;
    esac
}
