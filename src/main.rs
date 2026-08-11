// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use clap::Parser;
use herdr_agentsview::config::PluginConfig;
use herdr_agentsview::{herdr, tui};

#[derive(Parser)]
#[command(about = "AgentsView Activity dashboard for Herdr")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    Open,
    Tui,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Open => herdr::open(),
        Command::Tui => tui::run(PluginConfig::load()?),
    }
}
