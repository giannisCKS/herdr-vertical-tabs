# Herdr with vertical tabs

This is an unofficial customization of [Herdr](https://github.com/herdrdev/herdr),
not an upstream release or an upstream-approved feature.
The upstream license and attribution remain unchanged.

## Features

- Choose horizontal tabs (the default) or vertical tabs in Settings.
- Show the active space's tabs in a separate sidebar section in vertical mode.
- Scroll Spaces, Tabs, and Agents independently.
- Drag any divider between two neighbouring sidebar sections to resize just those two.
- Choose any of the six orders of Spaces, Tabs, and Agents in Settings.
- Save the orientation and section order across restarts.

Section ordering uses Settings, not a drag-and-drop panel layout editor.
When the sidebar cannot show vertical tabs, the horizontal tab strip remains
available as a fallback. Existing tab actions and keyboard shortcuts remain.

## Build and try

Follow the upstream build prerequisites, then run:

```sh
cargo build --release --locked
./target/release/herdr
```

A release build uses the normal Herdr configuration and state directories.
A debug build uses separate `herdr-dev` directories. Back up the installed
executable, configuration, and saved state before replacing an existing install.
No personal configuration or session data is required to build this fork.

Replacing the executable does not replace a running Herdr server. Finish important
work before restarting it: stopping the server terminates pane processes and
running agents. Saved layouts do not preserve live processes. To roll back,
safely stop the custom server and restore the backed-up executable. Do not restore
old state over newer state without first backing up the newer state too.

An official Herdr update may replace a locally installed custom executable.

## Validation limits

Focused sidebar and settings tests, formatting, Clippy, and maintenance checks
passed during local validation. Full `just ci` has not been certified:
`cargo-nextest` was unavailable. The alternative full Cargo test run encountered
a SIGPIPE failure in remote-bridge tests, also observed on the upstream baseline.
Windows cross-compilation has not been verified for this customization.

## Upstream discussion

The feature use case is discussed in
[Herdr Discussion #4367](https://github.com/herdrdev/herdr/discussions/4367).
This fork does not imply permission to submit an upstream implementation PR.
