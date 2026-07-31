// SPDX-License-Identifier: GPL-2.0-or-later

fn main() {
    match vino_chimera::session::ControlSession::probe_shared_ake() {
        Ok(report) => {
            println!(
                "shared-ake repeater={} receiver-id={:02x}{:02x}{:02x}{:02x}{:02x} receiver-list={} bytes",
                if report.repeater { "yes" } else { "no" },
                report.receiver_id[0],
                report.receiver_id[1],
                report.receiver_id[2],
                report.receiver_id[3],
                report.receiver_id[4],
                report.receiver_id_list_len,
            );
        }
        Err(error) => {
            eprintln!("chimera-shared-ake-probe: {error}");
            std::process::exit(1);
        }
    }
}
