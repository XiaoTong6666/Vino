// SPDX-License-Identifier: GPL-2.0-or-later

//! DisplayLinkManager-compatible EVDI-to-Vino service.

use crate::evdi::{self, Cursor, DamageRect, Device, DeviceEvent, Mode};
use crate::session::ControlSession;
use std::io::{self, Write};
use std::time::{Duration, Instant};

const ACTIVE_HEADS: [u8; 1] = [0];
const FRAME_WAIT: Duration = Duration::from_millis(8);
const KEEPALIVE_PERIOD: Duration = Duration::from_millis(13);
const HEARTBEAT_PERIOD: Duration = Duration::from_secs(3);
const RESTART_DELAY: Duration = Duration::from_secs(1);
const PREARM_MODE: Mode = Mode {
    width: 1920,
    height: 1080,
    refresh_hz: 60,
    bits_per_pixel: 32,
    pixel_format: u32::from_le_bytes(*b"AR24"),
};

struct Output {
    head: u8,
    card: Device,
    mode: Option<Mode>,
    active: bool,
    keyframe_pending: bool,
    previous_rgb: Option<Vec<u8>>,
    cursor_hotspot: (i32, i32),
}

/// Run the service, rebuilding the complete owned session after a transport failure.
pub fn run() -> Result<(), String> {
    loop {
        if let Err(error) = run_session() {
            eprintln!("chimera: session stopped: {error}; retrying");
            std::thread::sleep(RESTART_DELAY);
        }
    }
}

/// Engage the dock, expose connected sinks through EVDI, and forward compositor frames.
fn run_session() -> Result<(), String> {
    let head = ACTIVE_HEADS[0];
    let trace_frames = std::env::var_os("VINO_TRACE_FRAMES").is_some();
    println!("head {head}: opening EVDI output");
    flush_standard_streams(head, "before Device::open");
    let mut card = Device::open().map_err(|error| format!("open EVDI output: {error}"))?;
    println!("head {head}: EVDI output opened");
    flush_standard_streams(head, "after Device::open");

    let mut session = ControlSession::engage_head(head)?;
    session.activate_mode(
        head,
        PREARM_MODE.width,
        PREARM_MODE.height,
        PREARM_MODE.refresh_hz,
    )?;
    println!(
        "head {head}: dock pre-armed at {}x{}@{}",
        PREARM_MODE.width, PREARM_MODE.height, PREARM_MODE.refresh_hz
    );
    let edid = session
        .fetch_edid(head)
        .map_err(|error| format!("fetch head-{head} EDID from dock: {error}"))?;
    println!("head {head}: fetched {} EDID bytes from dock", edid.len());
    println!("head {head}: connecting EVDI monitor");
    flush_standard_streams(head, "before connect_monitor");
    evdi::connect_monitor(&mut card, &edid)
        .map_err(|error| format!("connect EVDI output: {error}"))?;
    println!("head {head}: EVDI monitor connected");
    flush_standard_streams(head, "after connect_monitor");
    let mut outputs = [Some(Output {
        head,
        card,
        mode: Some(PREARM_MODE),
        active: true,
        keyframe_pending: true,
        previous_rgb: None,
        cursor_hotspot: (0, 0),
    })];

    let mut next_keepalive = Instant::now() + KEEPALIVE_PERIOD;
    let mut next_heartbeat = Instant::now() + HEARTBEAT_PERIOD;
    loop {
        for output in outputs.iter_mut().flatten() {
            for event in output.card.events(Duration::ZERO) {
                handle_event(&mut session, output, event)?;
            }
            if !output.active {
                continue;
            }
            let Some(frame) = output.card.next_frame(FRAME_WAIT) else {
                continue;
            };
            let width = frame.width;
            let height = frame.height;
            let raw_damage_rects = frame.damage.len();
            let raw_clips = damage_clips(&frame.damage, width, height);
            if !output.keyframe_pending && raw_clips.is_empty() {
                continue;
            }
            let rgb = evdi::to_rgb888(&frame);
            drop(frame);

            let Some(mode) = output.card.mode() else {
                continue;
            };
            if Some(mode) != output.mode {
                return Err(format!(
                    "head {} compositor mode {}x{}@{} differs from pre-armed {}x{}@{}",
                    output.head,
                    mode.width,
                    mode.height,
                    mode.refresh_hz,
                    PREARM_MODE.width,
                    PREARM_MODE.height,
                    PREARM_MODE.refresh_hz,
                ));
            }
            let (padded, padded_width, padded_height) = pad_rgb(&rgb, width, height);
            let keyframe = output.keyframe_pending
                || output
                    .previous_rgb
                    .as_ref()
                    .is_none_or(|previous| previous.len() != padded.len());
            let clips = if keyframe {
                raw_clips
            } else {
                changed_strip_clips(
                    output.previous_rgb.as_deref().expect("checked above"),
                    &padded,
                    padded_width,
                    padded_height,
                    &raw_clips,
                )
            };
            if !keyframe && clips.is_empty() {
                continue;
            }
            let encoded_bytes = if keyframe {
                session.present_rgb(output.head, padded_width, padded_height, &padded)?
            } else {
                session.present_rgb_damage(
                    output.head,
                    padded_width,
                    padded_height,
                    &padded,
                    &clips,
                )?
            };
            if keyframe {
                output.keyframe_pending = false;
            }
            output.previous_rgb = Some(padded);
            if trace_frames && encoded_bytes != 0 {
                let damage_bounds = clip_bounds(&clips);
                println!(
                    "head {}: frame kind={} raw_damage_rects={} effective_strips={} encoded_bytes={} damage_bounds={damage_bounds:?}",
                    output.head,
                    if keyframe { "keyframe" } else { "damage" },
                    raw_damage_rects,
                    clips.len(),
                    encoded_bytes,
                );
            }
        }
        if Instant::now() >= next_keepalive {
            if outputs.iter().flatten().any(|output| output.active) {
                session.keepalive_poll()?;
            }
            next_keepalive = Instant::now() + KEEPALIVE_PERIOD;
        }
        if Instant::now() >= next_heartbeat {
            if trace_frames {
                for output in outputs.iter().flatten() {
                    let diagnostics = output.card.diagnostics();
                    println!(
                        "head {}: evdi state mode={:?} active={} poll_wakeups={} mode_events={} dpms_events={} crtc_events={} update_requests={} immediate_updates={} update_ready_events={} update_timeouts={} grabs={}",
                        output.head,
                        output.card.mode(),
                        output.active,
                        diagnostics.poll_wakeups,
                        diagnostics.mode_events,
                        diagnostics.dpms_events,
                        diagnostics.crtc_events,
                        diagnostics.update_requests,
                        diagnostics.immediate_updates,
                        diagnostics.update_ready_events,
                        diagnostics.update_timeouts,
                        diagnostics.grabs,
                    );
                }
            }
            session.heartbeat()?;
            next_heartbeat = Instant::now() + HEARTBEAT_PERIOD;
        }
        if !outputs.iter().flatten().any(|output| output.active) {
            std::thread::sleep(FRAME_WAIT);
        }
    }
}

fn flush_standard_streams(head: u8, stage: &str) {
    let stdout_started = Instant::now();
    let stdout_result = io::stdout().lock().flush();
    let stdout_elapsed = stdout_started.elapsed();

    let mut stderr = io::stderr().lock();
    let _ = writeln!(
        stderr,
        "head {head}: stdout.flush() {stage} returned in {stdout_elapsed:?}: {}",
        if stdout_result.is_ok() { "ok" } else { "error" }
    );
    let _ = stderr.flush();

    let stderr_started = Instant::now();
    let stderr_result = stderr.flush();
    let stderr_elapsed = stderr_started.elapsed();
    let _ = writeln!(
        stderr,
        "head {head}: stderr.flush() {stage} returned in {stderr_elapsed:?}: {}",
        if stderr_result.is_ok() { "ok" } else { "error" }
    );
    let _ = stderr.flush();
}

fn handle_event(
    session: &mut ControlSession,
    output: &mut Output,
    event: DeviceEvent,
) -> Result<(), String> {
    match event {
        DeviceEvent::Mode(mode) if Some(mode) != output.mode => {
            return Err(format!(
                "head {} compositor mode {}x{}@{} differs from pre-armed {}x{}@{}",
                output.head,
                mode.width,
                mode.height,
                mode.refresh_hz,
                PREARM_MODE.width,
                PREARM_MODE.height,
                PREARM_MODE.refresh_hz,
            ));
        }
        DeviceEvent::Mode(mode) => {
            println!(
                "head {}: active at {}x{}@{}",
                output.head, mode.width, mode.height, mode.refresh_hz
            );
        }
        DeviceEvent::CrtcState(_) => {}
        DeviceEvent::Dpms(0) if !output.active => {
            if let Some(mode) = output.mode {
                session.activate_mode(output.head, mode.width, mode.height, mode.refresh_hz)?;
                output.active = true;
                output.keyframe_pending = true;
                output.previous_rgb = None;
            }
        }
        DeviceEvent::Dpms(0) => {}
        DeviceEvent::Dpms(_) if output.active => {
            let mode = output
                .mode
                .ok_or_else(|| format!("head {} has no active mode", output.head))?;
            session.deactivate_mode(output.head, mode.width, mode.height)?;
            output.active = false;
            output.previous_rgb = None;
        }
        DeviceEvent::Dpms(_) => {}
        DeviceEvent::CursorSet(cursor) => {
            output.cursor_hotspot = (cursor.hot_x, cursor.hot_y);
            if cursor.enabled {
                let bgra = cursor_bgra(&cursor)?;
                session.set_cursor(output.head, cursor.width, cursor.height, &bgra)?;
            } else {
                session.move_cursor(output.head, i32::from(u16::MAX), i32::from(u16::MAX))?;
            }
        }
        DeviceEvent::CursorMove(position) if output.active => {
            session.move_cursor(
                output.head,
                position.x.saturating_sub(output.cursor_hotspot.0),
                position.y.saturating_sub(output.cursor_hotspot.1),
            )?;
        }
        DeviceEvent::CursorMove(_) => {}
        DeviceEvent::Ddcci(request) => {
            match session.ddcci(output.head, request.address, request.flags, &request.buffer) {
                Ok(response) => output.card.ddcci_response(&response, true),
                Err(error) => {
                    eprintln!("head {}: DDC/CI transaction failed: {error}", output.head);
                    output.card.ddcci_response(&[], false);
                }
            }
        }
    }
    Ok(())
}

fn cursor_bgra(cursor: &Cursor) -> Result<Vec<u8>, String> {
    const DRM_FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
    if cursor.pixel_format != DRM_FORMAT_ARGB8888 {
        return Err(format!(
            "unsupported cursor format {:#010x}",
            cursor.pixel_format
        ));
    }
    let width = usize::try_from(cursor.width).map_err(|_| "cursor width overflow")?;
    let height = usize::try_from(cursor.height).map_err(|_| "cursor height overflow")?;
    let stride = usize::try_from(cursor.stride).map_err(|_| "cursor stride overflow")?;
    let row = width
        .checked_mul(4)
        .ok_or_else(|| "cursor row size overflow".to_string())?;
    let required = stride
        .checked_mul(height)
        .ok_or_else(|| "cursor buffer size overflow".to_string())?;
    if stride < row || cursor.pixels.len() < required {
        return Err("cursor buffer is shorter than its declared geometry".into());
    }
    let mut packed = Vec::with_capacity(
        row.checked_mul(height)
            .ok_or_else(|| "cursor image size overflow".to_string())?,
    );
    for source in cursor.pixels[..required].chunks_exact(stride) {
        packed.extend_from_slice(&source[..row]);
    }
    Ok(packed)
}

fn pad_rgb(rgb: &[u8], width: usize, height: usize) -> (Vec<u8>, usize, usize) {
    let padded_width = width.div_ceil(64) * 64;
    let padded_height = height.div_ceil(16) * 16;
    if padded_width == width && padded_height == height {
        return (rgb.to_vec(), width, height);
    }
    let mut padded = vec![0; padded_width * padded_height * 3];
    let source_stride = width * 3;
    let target_stride = padded_width * 3;
    for row in 0..height {
        let source = row * source_stride;
        let target = row * target_stride;
        padded[target..target + source_stride]
            .copy_from_slice(&rgb[source..source + source_stride]);
    }
    (padded, padded_width, padded_height)
}

fn damage_clips(
    damage: &[DamageRect],
    width: usize,
    height: usize,
) -> Vec<(usize, usize, usize, usize)> {
    damage
        .iter()
        .filter_map(|rect| {
            let x0 = usize::try_from(rect.x1.max(0)).ok()?.min(width);
            let y0 = usize::try_from(rect.y1.max(0)).ok()?.min(height);
            let x1 = usize::try_from(rect.x2.max(0)).ok()?.min(width);
            let y1 = usize::try_from(rect.y2.max(0)).ok()?.min(height);
            (x0 < x1 && y0 < y1).then_some((x0, y0, x1, y1))
        })
        .collect()
}

fn changed_strip_clips(
    previous: &[u8],
    current: &[u8],
    width: usize,
    height: usize,
    coarse_clips: &[(usize, usize, usize, usize)],
) -> Vec<(usize, usize, usize, usize)> {
    const STRIP_W: usize = 64;
    const STRIP_H: usize = 16;
    if previous.len() != current.len() || current.len() != width * height * 3 {
        return vec![(0, 0, width, height)];
    }

    let mut changed = Vec::new();
    for sy in (0..height).step_by(STRIP_H) {
        for sx in (0..width).step_by(STRIP_W) {
            let x1 = (sx + STRIP_W).min(width);
            let y1 = (sy + STRIP_H).min(height);
            if !coarse_clips
                .iter()
                .any(|&(x0, y0, x2, y2)| sx < x2 && x0 < x1 && sy < y2 && y0 < y1)
            {
                continue;
            }
            let differs = (sy..y1).any(|y| {
                let start = (y * width + sx) * 3;
                let end = (y * width + x1) * 3;
                previous[start..end] != current[start..end]
            });
            if differs {
                changed.push((sx, sy, x1, y1));
            }
        }
    }
    changed
}

fn clip_bounds(clips: &[(usize, usize, usize, usize)]) -> Option<(usize, usize, usize, usize)> {
    let first = *clips.first()?;
    Some(clips[1..].iter().fold(first, |bounds, clip| {
        (
            bounds.0.min(clip.0),
            bounds.1.min(clip.1),
            bounds.2.max(clip.2),
            bounds.3.max(clip.3),
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_to_vino_strip_geometry() {
        let rgb = vec![0x55; 65 * 17 * 3];
        let (padded, width, height) = pad_rgb(&rgb, 65, 17);
        assert_eq!((width, height), (128, 32));
        assert_eq!(&padded[..65 * 3], &rgb[..65 * 3]);
        assert!(padded[65 * 3..128 * 3].iter().all(|&byte| byte == 0));
        assert_eq!(padded.len(), 128 * 32 * 3);
    }

    #[test]
    fn repacks_cursor_stride() {
        let cursor = Cursor {
            hot_x: 1,
            hot_y: 2,
            width: 2,
            height: 2,
            enabled: true,
            pixels: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 9, 10, 11, 12, 13, 14, 15, 16, 0, 0, 0, 0,
            ],
            pixel_format: u32::from_le_bytes(*b"AR24"),
            stride: 12,
        };
        assert_eq!(
            cursor_bgra(&cursor).unwrap(),
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
    }

    #[test]
    fn clamps_and_discards_invalid_damage() {
        let clips = damage_clips(
            &[
                DamageRect {
                    x1: -10,
                    y1: -20,
                    x2: 40,
                    y2: 50,
                },
                DamageRect {
                    x1: 1900,
                    y1: 1000,
                    x2: 2100,
                    y2: 1200,
                },
                DamageRect {
                    x1: 80,
                    y1: 80,
                    x2: 40,
                    y2: 40,
                },
            ],
            1920,
            1080,
        );
        assert_eq!(clips, [(0, 0, 40, 50), (1900, 1000, 1920, 1080)]);
    }

    #[test]
    fn damage_frame_is_smaller_than_a_keyframe() {
        let (width, height) = (512, 128);
        let rgb = vec![0u8; width * height * 3];
        let (keyframe, _) =
            crate::kvino::colour_frame_ep08_head(width, height, &rgb, 0, 0).expect("keyframe");
        let (damage, _) =
            crate::kvino::colour_frame_ep08_damage_head(width, height, &rgb, 1, 0, &[(1, 1, 2, 2)])
                .expect("damage frame");
        let keyframe_bytes: usize = keyframe.iter().map(Vec::len).sum();
        let damage_bytes: usize = damage.iter().map(Vec::len).sum();
        assert!(!damage.is_empty());
        assert!(damage_bytes < keyframe_bytes);
    }

    #[test]
    fn pixel_diff_refines_full_damage_to_changed_strips() {
        let (width, height) = (128, 32);
        let previous = vec![0u8; width * height * 3];
        let mut current = previous.clone();
        let changed_pixel = (20 * width + 70) * 3;
        current[changed_pixel..changed_pixel + 3].copy_from_slice(&[1, 2, 3]);

        assert_eq!(
            changed_strip_clips(&previous, &current, width, height, &[(0, 0, width, height)]),
            [(64, 16, 128, 32)]
        );
        assert!(
            changed_strip_clips(&current, &current, width, height, &[(0, 0, width, height)])
                .is_empty()
        );
    }
}
