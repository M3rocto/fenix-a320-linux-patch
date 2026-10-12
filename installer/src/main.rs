// SPDX-License-Identifier: MIT
use fenix_patch::cli::{self, Cli};
use std::sync::{Arc, atomic::AtomicBool};

fn main() {
    let interrupt = Arc::new(AtomicBool::new(false));
    // Ctrl+C is noted and acted upon at the next safe point, so that helper
    // programs are stopped and temporary files removed first.
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, interrupt.clone());
    let code = cli::run(&mut Cli::host(interrupt), std::env::args_os());
    std::process::exit(code);
}
