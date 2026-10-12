// SPDX-License-Identifier: MIT
use fenix_patch::system::Host;
use fenix_tools::{Context, cli, repository};
use std::sync::{Arc, atomic::AtomicBool};

fn main() {
    let interrupt = Arc::new(AtomicBool::new(false));
    // Ctrl+C is noted and acted upon at the next step, so that Wine is stopped
    // and the programs that were started are waited for first.
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, interrupt.clone());
    let root = match repository() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("fenix-tools: {error}");
            std::process::exit(1);
        }
    };
    let host = Host::new(root).with_interrupt(interrupt);
    let code = cli::run(&Context::host(&host), std::env::args_os());
    std::process::exit(code);
}
