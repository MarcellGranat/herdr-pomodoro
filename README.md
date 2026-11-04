# Herdr Pomodoro

Herdr Pomodoro is a private, local-first Herdr workflow plugin for focus sessions and
workspace-aware activity reporting.

The activity spike does not require a custom Herdr build. It discovers the same-user Herdr client
and local pane PTYs, samples only their access and modification timestamps, and counts input only
when one outer-client access change uniquely matches one inner-pane access change. It never reads
PTY contents, keystrokes, commands, or clipboard data.

This also works when the Mac is reached through SSH or mosh because the input reaches both PTYs on
the Mac. Herdr-native remote panes need a future metadata-only sampler on the remote host; capture
fails closed when a pane cannot be resolved safely.

Run `uv run herdr-pomodoro status` to inspect topology support. During development,
`uv run herdr-pomodoro watch --duration 10` prints content-free correlated observations as JSON
lines; silence means that no unambiguous human pane input was observed.

