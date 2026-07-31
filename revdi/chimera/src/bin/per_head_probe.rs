// SPDX-License-Identifier: GPL-2.0-or-later

fn main() {
    let head = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "0".to_string())
        .parse::<u8>()
        .unwrap_or(0);

    match vino_chimera::session::probe_per_head(head) {
        Ok(report) => {
            println!(
                "per-head head={} rrx={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} ack={} ctr={} wire={}",
                report.head,
                report.rrx[0],
                report.rrx[1],
                report.rrx[2],
                report.rrx[3],
                report.rrx[4],
                report.rrx[5],
                report.rrx[6],
                report.rrx[7],
                report.acknowledgements,
                report.final_inner_counter,
                report.final_wire_sequence,
            );
        }
        Err(error) => {
            eprintln!("chimera-per-head-probe: {error}");
            std::process::exit(1);
        }
    }
}
