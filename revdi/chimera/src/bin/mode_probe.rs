// SPDX-License-Identifier: GPL-2.0-or-later

use vino_chimera::session::ControlSession;

fn parse_arg(index: usize, default: usize) -> usize {
    std::env::args()
        .nth(index)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
}

fn parse_head() -> u8 {
    std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0)
}

fn parse_refresh(default: u32) -> u32 {
    std::env::args()
        .nth(4)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(default)
}

fn main() {
    let head = parse_head();
    let width = parse_arg(2, 1920);
    let height = parse_arg(3, 1080);
    let refresh = parse_refresh(60);

    match ControlSession::engage_head(head)
        .and_then(|mut session| session.probe_mode(head, width, height, refresh))
    {
        Ok(report) => {
            println!(
                "mode-probe head={} {}x{}@{} padded={}x{}",
                report.head,
                report.width,
                report.height,
                report.refresh_hz,
                report.padded_width,
                report.padded_height,
            );
        }
        Err(error) => {
            eprintln!("chimera-mode-probe: {error}");
            std::process::exit(1);
        }
    }
}
