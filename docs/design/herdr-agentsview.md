<!--
SPDX-FileCopyrightText: 2026 Phillip Cloud

SPDX-License-Identifier: Apache-2.0
-->

# Herdr AgentsView Design

## Purpose

`herdr-agentsview` is a terminal-native Herdr plugin for the AgentsView
Activity dashboard. It preserves the useful information hierarchy of the web
dashboard while using terminal-native navigation, layout, and status feedback.

The repository is private. A possible future public release does not authorize
changing its visibility.

## Product Boundary

Only the Activity view is included. The plugin does not expose or reserve
navigation for Sessions, Usage, Trends, Pinned, Insights, Trash, Recent Edits,
Data, import, settings, help, or global search.

The plugin consumes the existing AgentsView REST API. It does not change
AgentsView, read its database, launch its server, shell out to its binary, or
add a compatibility service. Authentication remains runtime-only through an
environment variable or token file.

## Activity Surface

The top of the screen contains the date, supported Activity filters, a compact
summary, and refresh state. The summary groups facts by meaning rather than
presenting a row of unrelated cards:

- concurrency: peak value and time
- time: active and idle duration
- work: agent-minutes and cost
- sessions: total plus interactive, automated, and untimed counts
- scope: project and model counts

The concurrency chart keeps the full selected day visible. A separate bucket
cursor can inspect time slices without shifting the chart. Bucket slicing is
off by default, begins at the first non-zero bucket when first enabled, resumes
its prior bucket when re-enabled, and filters the sessions table only while
active.

The sessions table is sortable and scrollable. It displays session, model,
project, agent, agent-minutes, cost, and time window. Stable categorical colors
differentiate project, model, and agent values while preserving monochrome
fallbacks.

Project, model, and agent breakdowns switch between agent time and cost. Large
values use compact decimal suffixes implemented locally, without a dependency
whose only purpose is number formatting.

## Interaction And Layout

The interface is keyboard-first. Focus uses background highlighting instead of
text chevrons. Selector popups open next to their controls, and the project
selector supports fuzzy search. Key hints use compact light-background pills;
the help overlay describes user actions rather than internal regions or panes.

Loading uses a single animated Braille spinner beside the `AgentsView` title.
Individual panels use quiet pending copy without repeating the spinner or the
word Activity. A new selection cancels the prior in-flight report task, and
generation checks prevent stale results from replacing the new selection.

Layouts are intentionally verified at 80x24, 120x40, and 200x50. Narrow mode
keeps errors and recovery actions visible whichever lower panel is selected.
Colors adapt to terminal capability and all meaning remains available in
monochrome.

## Repository Shape

The Rust crate lives at the repository root. `flake.nix` exposes:

- the packaged plugin and default application
- `nix run .#demo` for an isolated synthetic demonstration
- a Rust development shell
- named checks for x86_64 Linux, aarch64 Linux, and aarch64 Darwin

The demo starts an ephemeral fake HTTP boundary, writes temporary plugin
configuration, launches the same TUI binary used by Herdr, and removes the
process and temporary files on exit. It never contacts a live AgentsView
deployment. `nix run . -- tui` is the direct standalone path for a configured
AgentsView API.

The source configuration repository no longer packages or installs the plugin.
It does not gain private Git credentials or a private flake input. Its existing
trusted-repository declaration is retained for later use.

## Automation And Security

Prek runs repository-native Rust, Nix, workflow, Markdown, shell, and secret
checks. CI covers x86_64 Linux, aarch64 Linux, and aarch64 Darwin with pinned
actions and least permissions. StepSecurity harden-runner is used only where
officially supported, and StepSecurity's maintained TruffleHog action scans
pushes and pull requests.

No test mirrors workflow or Nix literals. Behavioral tests run the real artifact
with only external boundaries faked. HTTP fixtures bind ephemeral loopback
ports and never reach a live deployment.

## Privacy

Tests, examples, documentation, screenshots, and recordings use coherent
synthetic names, reserved domains, and invented usage figures. Before each
push, the complete worktree, commit history, README, fixtures, goldens, VHS
tape, and image metadata are scanned against the configured private-term list
and structural checks for home paths, identities, hostnames, private URLs, and
real financial data.

The raster icon is a flat terminal chevron with three activity bars. It has no
text, glow, gradient, shadow, photorealism, or faux depth and remains legible at
small sizes. The README screenshot is recorded by VHS from the synthetic demo.
Both PNG files contain no private metadata.

## Delivery

Work is committed in small logical changes. The private repository is pushed
only after GitHub visibility is freshly verified as `PRIVATE` and the complete
publish surface passes the privacy audit. The corresponding removal from the
source configuration repository is committed separately and left unpushed.
