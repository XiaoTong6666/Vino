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

fn parse_rgb() -> [u8; 3] {
    let text = std::env::args()
        .nth(5)
        .unwrap_or_else(|| "202020".to_string());
    let hex = text.strip_prefix('#').unwrap_or(&text);
    if hex.len() != 6 {
        return [0x20, 0x20, 0x20];
    }
    let parse = |offset| u8::from_str_radix(&hex[offset..offset + 2], 16).ok();
    match (parse(0), parse(2), parse(4)) {
        (Some(r), Some(g), Some(b)) => [r, g, b],
        _ => [0x20, 0x20, 0x20],
    }
}

fn main() {
    let head = parse_head();
    let width = parse_arg(2, 1920);
    let height = parse_arg(3, 1080);
    let refresh = parse_refresh(60);
    let rgb = parse_rgb();
    let frame = vec![rgb[0], rgb[1], rgb[2]].repeat(width * height);

    match ControlSession::engage_head(head)
        .and_then(|mut session| session.probe_first_frame(head, width, height, refresh, &frame))
    {
        Ok(report) => {
            println!(
                "first-frame-probe head={} {}x{}@{} padded={}x{} encoded_bytes={}",
                report.head,
                report.width,
                report.height,
                report.refresh_hz,
                report.padded_width,
                report.padded_height,
                report.encoded_bytes,
            );
        }
        Err(error) => {
            eprintln!("chimera-first-frame-probe: {error}");
            std::process::exit(1);
        }
    }
}
