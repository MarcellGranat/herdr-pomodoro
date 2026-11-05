#!/bin/sh
# Remote shim for Herdr Pomodoro. The watcher on the main machine pushes the
# label into bar.txt and picks up requests dropped into requests/ over SSH.
dir="${XDG_STATE_HOME:-$HOME/.local/state}/herdr-pomodoro"
case "$1" in
bar)
    # A stale file means the watcher is gone.
    if [ -n "$(find "$dir/bar.txt" -mmin -1 2>/dev/null)" ]; then
        cat "$dir/bar.txt"
    else
        printf '\360\237\246\246\n' # otter, kept ASCII in this file
    fi
    ;;
menu)
    # Remember where the key was pressed, then open the popup menu pane.
    mkdir -p "$dir"
    printf '%s' "${HERDR_WORKSPACE_ID:-}" >"$dir/workspace"
    exec "${HERDR_BIN_PATH:-herdr}" plugin pane open --plugin chmarax.herdr-pomodoro --entrypoint menu
    ;;
start | end | stop | away | workspace-grace | return | break)
    mkdir -p "$dir/requests"
    tmp="$dir/requests/.$$.tmp"
    printf '{"action":"%s","workspace_id":"%s"}\n' "$1" "${HERDR_WORKSPACE_ID:-}" >"$tmp"
    mv "$tmp" "$dir/requests/$$-$(date +%s).json"
    ;;
*)
    echo "usage: herdr-pomodoro bar|menu|start|end|stop|away|workspace-grace|return|break" >&2
    exit 2
    ;;
esac
