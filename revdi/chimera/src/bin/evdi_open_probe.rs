// SPDX-License-Identifier: GPL-2.0-or-later

use std::time::Instant;
use vino_chimera::evdi::Device;
use vino_chimera::session::ControlSession;

fn engage_first() -> bool {
    std::env::args().any(|arg| arg == "--engage-head0")
}

fn main() {
    let mut _session = None;
    if engage_first() {
        let started = Instant::now();
        match ControlSession::engage_head(0) {
            Ok(session) => {
                _session = Some(session);
                println!(
                    "evdi-open-probe: engage_head(0) succeeded in {:?}",
                    started.elapsed()
                );
            }
            Err(error) => {
                eprintln!(
                    "evdi-open-probe: engage_head(0) failed after {:?}: {}",
                    started.elapsed(),
                    error
                );
                std::process::exit(1);
            }
        }
    }
    let started = Instant::now();
    match Device::open() {
        Ok(_device) => {
            println!("evdi-open-probe: open succeeded in {:?}", started.elapsed());
        }
        Err(error) => {
            eprintln!(
                "evdi-open-probe: open failed after {:?}: {:?}",
                started.elapsed(),
                error
            );
            std::process::exit(1);
        }
    }
}
