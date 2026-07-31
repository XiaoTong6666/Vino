//! Live per-head EDID discovery using the same message builders and bounded transaction as Vino.

use crate::kvino;
use std::time::{Duration, Instant};
use vino_driver::{Dock, Error as UsbError};

const ACQUISITION_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSACTION_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_DELAY: Duration = Duration::from_millis(100);
const SETTLE_DELAY: Duration = Duration::from_millis(100);
const DRAIN_TIMEOUT: Duration = Duration::from_millis(5);
const PLACEHOLDER_REFETCH_POLLS: usize = 8;

fn trace(message: &str) {
    if std::env::var_os("VINO_TRACE_EDID").is_some() {
        eprintln!("[edid] {message}");
    }
}

#[derive(Debug)]
pub enum FetchError {
    Build(crate::kshim::Error),
    Transport(UsbError),
    NoEdid { head: u8 },
}

impl core::fmt::Display for FetchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Build(e) => write!(f, "control message construction failed: {e}"),
            Self::Transport(e) => write!(f, "USB transport failed: {e:?}"),
            Self::NoEdid { head } => write!(f, "head {head} did not return an EDID"),
        }
    }
}

impl std::error::Error for FetchError {}

/// Seal `content` under `id` at the running wire sequence, send it on the CP control endpoint,
/// and advance the wire sequence by the number of AES-CTR blocks consumed.
fn send(
    dock: &Dock,
    ks: &[u8; 16],
    riv: &[u8; 8],
    wire_seq: &mut u32,
    id: u16,
    content: &[u8],
) -> Result<(), FetchError> {
    let frame =
        kvino::seal_interactive(ks, riv, id, *wire_seq, content).map_err(FetchError::Build)?;
    dock.write_ctrl_raw(&frame).map_err(FetchError::Transport)?;
    *wire_seq = wire_seq.wrapping_add(content.len().div_ceil(16) as u32);
    Ok(())
}

fn send_message(
    dock: &Dock,
    ks: &[u8; 16],
    riv: &[u8; 8],
    wire_seq: &mut u32,
    inner_counter: &mut u16,
    id: u16,
    build: impl FnOnce(u16) -> crate::kshim::Result<Vec<u8>>,
) -> Result<(), FetchError> {
    let content = build(*inner_counter).map_err(FetchError::Build)?;
    send(dock, ks, riv, wire_seq, id, &content)?;
    *inner_counter = inner_counter.wrapping_add(1);
    Ok(())
}

fn receive_status(
    dock: &Dock,
    ks: &[u8; 16],
    riv: &[u8; 8],
    deadline: Instant,
) -> Result<Option<(u8, bool, u16)>, FetchError> {
    while Instant::now() < deadline {
        let timeout = deadline.saturating_duration_since(Instant::now());
        let reply = match dock.recv_frame_raw_timeout(4096, timeout) {
            Ok(reply) => reply,
            Err(UsbError::Timeout) => return Ok(None),
            Err(error) => return Err(FetchError::Transport(error)),
        };
        trace(&format!(
            "status rx bytes={} header={:02x?} decoded={:?} sample={:?}",
            reply.len(),
            &reply[..reply.len().min(16)],
            kvino::decode_in_lenient(ks, riv, &reply),
            kvino::decode_any(ks, riv, &reply)
        ));
        if let Some(status) = kvino::parse_edid_status(ks, riv, &reply) {
            trace(&format!(
                "status presence={} available={} length={}",
                status.0, status.1, status.2
            ));
            return Ok(Some(status));
        }
        if matches!(
            kvino::decode_in_lenient(ks, riv, &reply),
            Some((0x14, 0x20, _))
        ) {
            return Ok(None);
        }
    }
    Ok(None)
}

enum EdidReply {
    Real(Vec<u8>),
    Placeholder,
    Unavailable,
}

fn receive_edid(
    dock: &Dock,
    ks: &[u8; 16],
    riv: &[u8; 8],
    declared_length: u16,
    deadline: Instant,
) -> Result<EdidReply, FetchError> {
    while Instant::now() < deadline {
        let timeout = deadline.saturating_duration_since(Instant::now());
        let reply = match dock.recv_frame_raw_timeout(4096, timeout) {
            Ok(reply) => reply,
            Err(UsbError::Timeout) => return Ok(EdidReply::Unavailable),
            Err(error) => return Err(FetchError::Transport(error)),
        };
        trace(&format!(
            "fetch rx bytes={} decoded={:?} expected={declared_length}",
            reply.len(),
            kvino::decode_in_lenient(ks, riv, &reply)
        ));
        if let Some(edid) = kvino::parse_edid_from_reply(ks, riv, &reply, declared_length)
            .map_err(FetchError::Build)?
        {
            return Ok(EdidReply::Real(edid));
        }
        match kvino::decode_in_lenient(ks, riv, &reply) {
            Some((0x114, 0x21, _)) => return Ok(EdidReply::Placeholder),
            Some((0x194 | 0x14, 0x21, _)) => return Ok(EdidReply::Unavailable),
            _ => {}
        }
    }
    Ok(EdidReply::Unavailable)
}

fn valid_status_length(length: u16) -> bool {
    let length = length as usize;
    (128..=3968).contains(&length) && length % 128 == 0
}

fn drain_optional_reply(dock: &Dock) -> Result<(), FetchError> {
    match dock.recv_frame_raw_timeout(4096, DRAIN_TIMEOUT) {
        Ok(_) | Err(UsbError::Timeout) => Ok(()),
        Err(error) => Err(FetchError::Transport(error)),
    }
}

fn finalize_sink_engagement(
    dock: &Dock,
    ks: &[u8; 16],
    riv: &[u8; 8],
    wire_seq: &mut u32,
    inner_counter: &mut u16,
    head: u8,
) -> Result<(), FetchError> {
    for engage_index in 0..2 {
        trace(&format!("sink engage head={head} index={engage_index}"));
        send_message(dock, ks, riv, wire_seq, inner_counter, 0x16, |counter| {
            kvino::edid_engage_req(counter, head)
        })?;
        drain_optional_reply(dock)?;
        std::thread::sleep(SETTLE_DELAY);
    }
    trace(&format!("post-EDID capability query head={head}"));
    send_message(dock, ks, riv, wire_seq, inner_counter, 0x15, |counter| {
        kvino::post_edid_query(counter, head)
    })?;
    drain_optional_reply(dock)
}

/// Discover `head`'s EDID in an already-engaged session.
///
/// `wire_seq` and `inner_counter` are shared with every other control operation in the session and
/// are advanced only after a successful transfer.
pub fn fetch_edid(
    dock: &Dock,
    ks: &[u8; 16],
    riv: &[u8; 8],
    wire_seq: &mut u32,
    inner_counter: &mut u16,
    head: u8,
) -> Result<Vec<u8>, FetchError> {
    let acquisition_deadline = Instant::now() + ACQUISITION_TIMEOUT;
    let mut placeholder_length = None;
    let mut status_polls_since_fetch = PLACEHOLDER_REFETCH_POLLS;
    let mut attempt = 0usize;

    while Instant::now() < acquisition_deadline {
        attempt += 1;
        trace(&format!("attempt={attempt} send status head={head}"));
        send_message(dock, ks, riv, wire_seq, inner_counter, 0x15, |counter| {
            kvino::get_edid_req_sub(counter, 0x20, head)
        })?;
        let transaction_deadline = (Instant::now() + TRANSACTION_TIMEOUT).min(acquisition_deadline);
        let Some((presence, available, length)) =
            receive_status(dock, ks, riv, transaction_deadline)?
        else {
            std::thread::sleep(RETRY_DELAY);
            continue;
        };
        if presence == 0 && !available {
            return Err(FetchError::NoEdid { head });
        }
        if !available || !valid_status_length(length) {
            std::thread::sleep(RETRY_DELAY);
            continue;
        }

        status_polls_since_fetch = status_polls_since_fetch.saturating_add(1);
        if placeholder_length == Some(length)
            && status_polls_since_fetch < PLACEHOLDER_REFETCH_POLLS
        {
            std::thread::sleep(RETRY_DELAY);
            continue;
        }

        trace(&format!(
            "attempt={attempt} send fetch head={head} declared_length={length}"
        ));
        send_message(dock, ks, riv, wire_seq, inner_counter, 0x15, |counter| {
            kvino::get_edid_req(counter, head)
        })?;
        status_polls_since_fetch = 0;
        let transaction_deadline = (Instant::now() + TRANSACTION_TIMEOUT).min(acquisition_deadline);
        match receive_edid(dock, ks, riv, length, transaction_deadline)? {
            EdidReply::Real(edid) => {
                finalize_sink_engagement(dock, ks, riv, wire_seq, inner_counter, head)?;
                return Ok(edid);
            }
            EdidReply::Placeholder => {
                trace(&format!("attempt={attempt} placeholder length={length}"));
                placeholder_length = Some(length);
            }
            EdidReply::Unavailable => {
                trace(&format!("attempt={attempt} fetch unavailable"));
                placeholder_length = None;
            }
        }
        std::thread::sleep(RETRY_DELAY);
    }

    Err(FetchError::NoEdid { head })
}
