use clap::Parser;
use std::{io::Write, process::ExitCode};

fn main() -> ExitCode {
    match bg_key_dbclk_checker::cli::run(bg_key_dbclk_checker::cli::Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "Error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
