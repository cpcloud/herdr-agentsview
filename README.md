<!--
SPDX-FileCopyrightText: 2026 Phillip Cloud

SPDX-License-Identifier: Apache-2.0
-->

# herdr-agentsview

<!-- HTML is required because GitHub Markdown has no image-sizing syntax. -->
<!-- markdownlint-disable-next-line MD033 -->
<img src="assets/icon.png" width="96" alt="Terminal chevron and activity bars">

[AgentsView](https://github.com/kenn-io/agentsview) activity, compressed into one
very busy terminal.

![Synthetic AgentsView Activity dashboard with compact colored summary metrics](assets/dashboard.png)

## Install

Install the plugin from GitHub. Herdr previews the source and build command,
then builds it with Cargo before registering it:

```console
herdr plugin install cpcloud/herdr-agentsview
herdr plugin config-dir local.agentsview
```

Create `config.toml` in the printed directory:

```toml
api_base_url = "https://agentsview.example.com/"
request_timeout_seconds = 10
refresh_interval_seconds = 60
timezone = "Etc/UTC"
```

The default loopback AgentsView server does not require a token. If the server
has authentication enabled, expose its bearer token to Herdr through exactly
one of `AGENTSVIEW_TOKEN` or `AGENTSVIEW_TOKEN_FILE`. Keep the credential out
of the config file and the repository. From a Herdr pane, open the dashboard
with:

```console
herdr plugin action invoke open --plugin local.agentsview
```

For a standalone terminal from a local checkout, set `HERDR_PLUGIN_CONFIG_DIR`
to the directory containing `config.toml`. If the server requires a token,
expose it through the same environment variable, then run
`cargo run --release -- tui`.

## Demo

Nix users can run an isolated synthetic dashboard without an AgentsView or
Herdr setup:

```console
nix run .#demo
```

## Platforms

x86_64 Linux, aarch64 Linux, and aarch64 macOS.

## Dependency Updates

Dependency updates use the Mend-hosted Renovate service. To activate it, a
repository owner must install the Mend Renovate App for only
`cpcloud/herdr-agentsview`. The hosted service manages authentication,
scheduling, and Renovate version updates; this repository does not need a
Renovate workflow or token.

## Keys

- `Tab` / `Shift-Tab`: move focus
- arrows: move through dates, rows, choices, and timeline buckets
- `Backspace`: clear the focused filter or return the date to today
- `Enter`: apply a choice or toggle the focused timeline session slice
- `s` / `b`: show sessions or breakdowns in a narrow terminal
- `p` / `m` / `a`: choose project, model, or agent breakdowns
- `r`: refresh; `?`: help; `q`: quit
