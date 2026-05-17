//! Binary entrypoint.

mod cli;
mod error;
mod fs;
mod launch;
mod mise;
mod spec;
mod tasks;
mod workspace;

use clap::Parser;
use cli::{App, Cli};

fn main() {
    App::execute(Cli::parse())
}
