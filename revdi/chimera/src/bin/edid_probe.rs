// SPDX-License-Identifier: GPL-2.0-or-later

use vino_chimera::evdi::Device;
use vino_chimera::session::ControlSession;

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let head = arguments
        .get(1)
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0);
    let arm_mode = arguments.iter().any(|argument| argument == "--mode");
    let mode_first = arguments.iter().any(|argument| argument == "--mode-first");
    let evdi_after_mode = arguments
        .iter()
        .any(|argument| argument == "--evdi-after-mode");

    match ControlSession::engage_head(head).and_then(|mut session| {
        if mode_first {
            session.activate_mode(head, 1920, 1080, 60)?;
        }
        let _evdi = if evdi_after_mode {
            let device = Device::open().map_err(|error| format!("open EVDI: {error}"))?;
            println!("edid-probe head={head} evdi_after_mode=true");
            Some(device)
        } else {
            None
        };
        let edid = session
            .fetch_edid(head)
            .map_err(|error| error.to_string())?;
        if arm_mode && !mode_first {
            session.activate_mode(head, 1920, 1080, 60)?;
        }
        Ok(edid)
    }) {
        Ok(edid) => {
            println!(
                "edid-probe head={head} bytes={} extensions={} checksums_ok={}",
                edid.len(),
                edid.get(126).copied().unwrap_or(0),
                edid.chunks_exact(128).all(|block| {
                    block.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) == 0
                })
            );
            if arm_mode || mode_first {
                println!("edid-probe head={head} mode=1920x1080@60 armed=true");
            }
        }
        Err(error) => {
            eprintln!("chimera-edid-probe: {error}");
            std::process::exit(1);
        }
    }
}
