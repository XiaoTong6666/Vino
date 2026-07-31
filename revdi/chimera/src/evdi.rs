// SPDX-License-Identifier: LGPL-2.1-or-later

//! Safe Rust ownership around the stock EVDI C ABI.
//!
//! The unsafe code in this module is limited to the `libevdi` boundary. The
//! daemon receives owned events and borrowed frame views, never raw handles or
//! callback pointers.

use core::ffi::{c_int, c_uint, c_void};
use core::ptr::NonNull;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const BUFFER_ID: c_int = 1;
const MAX_DEVICE_INDEX: c_int = 64;
const MAX_DAMAGE_RECTS: usize = 16;
const DDCCI_BUFFER_SIZE: usize = 64;

fn trace(message: &str) {
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "{message}");
    let _ = stderr.flush();
}

#[repr(C)]
struct EvdiDeviceContext {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EvdiRect {
    x1: c_int,
    y1: c_int,
    x2: c_int,
    y2: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EvdiMode {
    width: c_int,
    height: c_int,
    refresh_rate: c_int,
    bits_per_pixel: c_int,
    pixel_format: u32,
}

#[repr(C)]
struct EvdiBuffer {
    id: c_int,
    buffer: *mut c_void,
    width: c_int,
    height: c_int,
    stride: c_int,
    rects: *mut EvdiRect,
    rect_count: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EvdiCursorSet {
    hot_x: i32,
    hot_y: i32,
    width: u32,
    height: u32,
    enabled: u8,
    buffer_length: u32,
    buffer: *mut u32,
    pixel_format: u32,
    stride: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EvdiCursorMove {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EvdiDdcciData {
    address: u16,
    flags: u16,
    buffer_length: u32,
    buffer: *mut u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EvdiEventContext {
    dpms_handler: Option<extern "C" fn(c_int, *mut c_void)>,
    mode_changed_handler: Option<extern "C" fn(EvdiMode, *mut c_void)>,
    update_ready_handler: Option<extern "C" fn(c_int, *mut c_void)>,
    crtc_state_handler: Option<extern "C" fn(c_int, *mut c_void)>,
    cursor_set_handler: Option<extern "C" fn(EvdiCursorSet, *mut c_void)>,
    cursor_move_handler: Option<extern "C" fn(EvdiCursorMove, *mut c_void)>,
    ddcci_data_handler: Option<extern "C" fn(EvdiDdcciData, *mut c_void)>,
    user_data: *mut c_void,
}

#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum EvdiDeviceStatus {
    Available = 0,
    _Unrecognized = 1,
    _NotPresent = 2,
}

unsafe extern "C" {
    fn evdi_check_device(device: c_int) -> EvdiDeviceStatus;
    fn evdi_add_device() -> c_int;
    fn evdi_open(device: c_int) -> *mut EvdiDeviceContext;
    fn evdi_close(handle: *mut EvdiDeviceContext);
    fn evdi_connect2(
        handle: *mut EvdiDeviceContext,
        edid: *const u8,
        edid_length: c_uint,
        pixel_area_limit: u32,
        pixel_per_second_limit: u32,
    );
    fn evdi_disconnect(handle: *mut EvdiDeviceContext);
    fn evdi_enable_cursor_events(handle: *mut EvdiDeviceContext, enable: bool);
    fn evdi_grab_pixels(
        handle: *mut EvdiDeviceContext,
        rects: *mut EvdiRect,
        num_rects: *mut c_int,
    );
    fn evdi_register_buffer(handle: *mut EvdiDeviceContext, buffer: EvdiBuffer);
    fn evdi_unregister_buffer(handle: *mut EvdiDeviceContext, buffer_id: c_int);
    fn evdi_request_update(handle: *mut EvdiDeviceContext, buffer_id: c_int) -> bool;
    fn evdi_ddcci_response(
        handle: *mut EvdiDeviceContext,
        buffer: *const u8,
        buffer_length: u32,
        result: bool,
    );
    fn evdi_handle_events(handle: *mut EvdiDeviceContext, context: *mut EvdiEventContext);
    fn evdi_get_event_ready(handle: *mut EvdiDeviceContext) -> c_int;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    AddDevice,
    NoAvailableDevice,
    OpenDevice(c_int),
    InvalidEdidLength,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AddDevice => write!(f, "failed to add an EVDI device"),
            Self::NoAvailableDevice => write!(f, "no available EVDI device"),
            Self::OpenDevice(index) => write!(f, "failed to open EVDI device {index}"),
            Self::InvalidEdidLength => write!(f, "EDID is too large for the EVDI ABI"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mode {
    pub width: usize,
    pub height: usize,
    pub refresh_hz: u32,
    pub bits_per_pixel: u32,
    pub pixel_format: u32,
}

impl Mode {
    fn from_abi(mode: EvdiMode) -> Option<Self> {
        Some(Self {
            width: usize::try_from(mode.width).ok().filter(|&v| v != 0)?,
            height: usize::try_from(mode.height).ok().filter(|&v| v != 0)?,
            refresh_hz: u32::try_from(mode.refresh_rate).ok()?,
            bits_per_pixel: u32::try_from(mode.bits_per_pixel).ok()?,
            pixel_format: mode.pixel_format,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DamageRect {
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cursor {
    pub hot_x: i32,
    pub hot_y: i32,
    pub width: u32,
    pub height: u32,
    pub enabled: bool,
    pub pixels: Vec<u8>,
    pub pixel_format: u32,
    pub stride: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorPosition {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DdcciRequest {
    pub address: u16,
    pub flags: u16,
    pub buffer: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceEvent {
    Dpms(i32),
    Mode(Mode),
    CrtcState(i32),
    CursorSet(Cursor),
    CursorMove(CursorPosition),
    Ddcci(DdcciRequest),
}

pub struct Frame<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub damage: Vec<DamageRect>,
}

#[derive(Default)]
struct EventState {
    mode: Mutex<Option<EvdiMode>>,
    ready_buffer: Mutex<Option<c_int>>,
    events: Mutex<Vec<DeviceEvent>>,
    mode_events: AtomicU64,
    dpms_events: AtomicU64,
    update_ready_events: AtomicU64,
    crtc_events: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Diagnostics {
    pub poll_wakeups: u64,
    pub mode_events: u64,
    pub dpms_events: u64,
    pub crtc_events: u64,
    pub update_requests: u64,
    pub immediate_updates: u64,
    pub update_ready_events: u64,
    pub update_timeouts: u64,
    pub grabs: u64,
}

fn with_event_state(user_data: *mut c_void, f: impl FnOnce(&EventState)) {
    let Some(state) = NonNull::new(user_data.cast::<EventState>()) else {
        return;
    };
    // SAFETY: Device::event_context stores this Box pointer and libevdi invokes
    // callbacks synchronously from evdi_handle_events.
    f(unsafe { state.as_ref() });
}

extern "C" fn dpms_changed(mode: c_int, user_data: *mut c_void) {
    with_event_state(user_data, |state| {
        state.dpms_events.fetch_add(1, Ordering::Relaxed);
        trace(&format!("[evdi] event dpms mode={mode}"));
        state
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(DeviceEvent::Dpms(mode));
    });
}

extern "C" fn mode_changed(mode: EvdiMode, user_data: *mut c_void) {
    with_event_state(user_data, |state| {
        state.mode_events.fetch_add(1, Ordering::Relaxed);
        trace(&format!(
            "[evdi] event mode_changed {}x{}@{} bpp={} format={:#010x}",
            mode.width, mode.height, mode.refresh_rate, mode.bits_per_pixel, mode.pixel_format
        ));
        *state.mode.lock().unwrap_or_else(|e| e.into_inner()) = Some(mode);
        if let Some(mode) = Mode::from_abi(mode) {
            state
                .events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(DeviceEvent::Mode(mode));
        }
    });
}

extern "C" fn update_ready(buffer_id: c_int, user_data: *mut c_void) {
    with_event_state(user_data, |state| {
        state.update_ready_events.fetch_add(1, Ordering::Relaxed);
        *state.ready_buffer.lock().unwrap_or_else(|e| e.into_inner()) = Some(buffer_id);
    });
}

extern "C" fn crtc_state_changed(value: c_int, user_data: *mut c_void) {
    with_event_state(user_data, |state| {
        state.crtc_events.fetch_add(1, Ordering::Relaxed);
        trace(&format!("[evdi] event crtc_state value={value}"));
        state
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(DeviceEvent::CrtcState(value));
    });
}

extern "C" fn cursor_set(cursor: EvdiCursorSet, user_data: *mut c_void) {
    let length = usize::try_from(cursor.buffer_length).unwrap_or(0);
    let pixels = if cursor.buffer.is_null() || length == 0 {
        Vec::new()
    } else {
        // SAFETY: stock libevdi allocates exactly buffer_length bytes for this
        // callback and transfers ownership to the client callback.
        let bytes =
            unsafe { core::slice::from_raw_parts(cursor.buffer.cast::<u8>(), length).to_vec() };
        // SAFETY: the pointer is owned by this callback after libevdi returns it.
        unsafe { libc::free(cursor.buffer.cast::<c_void>()) };
        bytes
    };
    with_event_state(user_data, |state| {
        state
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(DeviceEvent::CursorSet(Cursor {
                hot_x: cursor.hot_x,
                hot_y: cursor.hot_y,
                width: cursor.width,
                height: cursor.height,
                enabled: cursor.enabled != 0,
                pixels,
                pixel_format: cursor.pixel_format,
                stride: cursor.stride,
            }));
    });
}

extern "C" fn cursor_move(cursor: EvdiCursorMove, user_data: *mut c_void) {
    with_event_state(user_data, |state| {
        state
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(DeviceEvent::CursorMove(CursorPosition {
                x: cursor.x,
                y: cursor.y,
            }));
    });
}

extern "C" fn ddcci_data(request: EvdiDdcciData, user_data: *mut c_void) {
    let length = usize::try_from(request.buffer_length)
        .unwrap_or(0)
        .min(DDCCI_BUFFER_SIZE);
    let buffer = if request.buffer.is_null() || length == 0 {
        Vec::new()
    } else {
        // SAFETY: the event owns a complete initialized buffer for this
        // synchronous callback.
        unsafe { core::slice::from_raw_parts(request.buffer, length).to_vec() }
    };
    with_event_state(user_data, |state| {
        state
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(DeviceEvent::Ddcci(DdcciRequest {
                address: request.address,
                flags: request.flags,
                buffer,
            }));
    });
}

struct RegisteredBuffer {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    stride: usize,
}

pub struct Device {
    handle: NonNull<EvdiDeviceContext>,
    state: Box<EventState>,
    buffer: Option<RegisteredBuffer>,
    mode: Option<Mode>,
    diagnostics: Diagnostics,
}

impl Device {
    pub fn open() -> Result<Self, Error> {
        let open_started = Instant::now();
        let find_available = || {
            (0..MAX_DEVICE_INDEX).find(|&index| {
                // SAFETY: evdi_check_device only reads platform/DRM state.
                unsafe { evdi_check_device(index) == EvdiDeviceStatus::Available }
            })
        };
        trace("[evdi] scanning for available card");
        let mut index = find_available();
        trace(&format!("[evdi] initial available card = {index:?}"));
        if index.is_none() {
            let add_started = Instant::now();
            trace("[evdi] no available card; calling evdi_add_device()");
            // SAFETY: creates an EVDI platform device through libevdi.
            if unsafe { evdi_add_device() } < 0 {
                return Err(Error::AddDevice);
            }
            trace(&format!(
                "[evdi] evdi_add_device() returned in {:?}",
                add_started.elapsed()
            ));
            index = find_available();
            trace(&format!("[evdi] available card after add = {index:?}"));
        }
        let index = index.ok_or(Error::NoAvailableDevice)?;
        let ffi_open_started = Instant::now();
        trace(&format!("[evdi] calling evdi_open({index})"));
        // SAFETY: the returned pointer is owned by this Device after success.
        let handle = NonNull::new(unsafe { evdi_open(index) }).ok_or(Error::OpenDevice(index))?;
        trace(&format!(
            "[evdi] evdi_open({index}) returned in {:?}; total open {:?}",
            ffi_open_started.elapsed(),
            open_started.elapsed()
        ));
        let device = Self {
            handle,
            state: Box::default(),
            buffer: None,
            mode: None,
            diagnostics: Diagnostics::default(),
        };
        let cursor_started = Instant::now();
        trace("[evdi] calling evdi_enable_cursor_events(true)");
        // SAFETY: handle is live and owned until Drop.
        unsafe { evdi_enable_cursor_events(device.handle.as_ptr(), true) };
        trace(&format!(
            "[evdi] evdi_enable_cursor_events(true) returned in {:?}",
            cursor_started.elapsed()
        ));
        Ok(device)
    }

    pub fn connect(
        &mut self,
        edid: &[u8],
        pixel_area_limit: u32,
        pixel_per_second_limit: u32,
    ) -> Result<(), Error> {
        let edid_length = u32::try_from(edid.len()).map_err(|_| Error::InvalidEdidLength)?;
        // SAFETY: handle is live and edid remains valid for this synchronous call.
        unsafe {
            evdi_connect2(
                self.handle.as_ptr(),
                if edid.is_empty() {
                    core::ptr::null()
                } else {
                    edid.as_ptr()
                },
                edid_length,
                pixel_area_limit,
                pixel_per_second_limit,
            );
        }
        Ok(())
    }

    pub fn mode(&self) -> Option<Mode> {
        self.mode
    }

    pub fn diagnostics(&self) -> Diagnostics {
        Diagnostics {
            mode_events: self.state.mode_events.load(Ordering::Relaxed),
            dpms_events: self.state.dpms_events.load(Ordering::Relaxed),
            update_ready_events: self.state.update_ready_events.load(Ordering::Relaxed),
            crtc_events: self.state.crtc_events.load(Ordering::Relaxed),
            ..self.diagnostics
        }
    }

    pub fn next_frame(&mut self, timeout: Duration) -> Option<Frame<'_>> {
        self.poll_events(Duration::ZERO);
        self.buffer.as_ref()?;
        self.state
            .ready_buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        // SAFETY: BUFFER_ID is registered while buffer is Some.
        let immediately_ready = unsafe { evdi_request_update(self.handle.as_ptr(), BUFFER_ID) };
        self.diagnostics.update_requests += 1;
        if !immediately_ready {
            let deadline = Instant::now() + timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    self.diagnostics.update_timeouts += 1;
                    return None;
                }
                self.poll_events(remaining);
                if self
                    .state
                    .ready_buffer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                    == Some(BUFFER_ID)
                {
                    break;
                }
            }
        } else {
            self.diagnostics.immediate_updates += 1;
        }
        self.diagnostics.grabs += 1;
        self.grab_frame()
    }

    pub fn events(&mut self, timeout: Duration) -> Vec<DeviceEvent> {
        self.poll_events(timeout);
        core::mem::take(&mut *self.state.events.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn ddcci_response(&mut self, response: &[u8], success: bool) {
        let length = response.len().min(DDCCI_BUFFER_SIZE);
        // SAFETY: handle is live and response remains valid for this synchronous call.
        unsafe {
            evdi_ddcci_response(
                self.handle.as_ptr(),
                if length == 0 {
                    core::ptr::null()
                } else {
                    response.as_ptr()
                },
                length as u32,
                success,
            );
        }
    }

    fn grab_frame(&mut self) -> Option<Frame<'_>> {
        let mut rects = [EvdiRect {
            x1: 0,
            y1: 0,
            x2: 0,
            y2: 0,
        }; MAX_DAMAGE_RECTS];
        let mut count = rects.len() as c_int;
        // SAFETY: rects has MAX_DAMAGE_RECTS slots and BUFFER_ID is registered.
        unsafe { evdi_grab_pixels(self.handle.as_ptr(), rects.as_mut_ptr(), &mut count) };
        let count = usize::try_from(count).ok()?.min(rects.len());
        let damage = rects[..count]
            .iter()
            .map(|r| DamageRect {
                x1: r.x1,
                y1: r.y1,
                x2: r.x2,
                y2: r.y2,
            })
            .collect();
        let buffer = self.buffer.as_ref()?;
        Some(Frame {
            pixels: &buffer.pixels,
            width: buffer.width,
            height: buffer.height,
            stride: buffer.stride,
            damage,
        })
    }

    fn poll_events(&mut self, timeout: Duration) {
        // SAFETY: handle is live and owned.
        let fd = unsafe { evdi_get_event_ready(self.handle.as_ptr()) };
        if fd < 0 {
            return;
        }
        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = timeout.as_millis().min(c_int::MAX as u128) as c_int;
        // SAFETY: poll_fd points to one initialized entry for the duration of the call.
        let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
        if ready <= 0 || poll_fd.revents & libc::POLLIN == 0 {
            return;
        }
        self.diagnostics.poll_wakeups += 1;
        let mut context = self.event_context();
        // SAFETY: context and the boxed state remain live during synchronous dispatch.
        unsafe { evdi_handle_events(self.handle.as_ptr(), &mut context) };
        let selected = self
            .state
            .mode
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(mode) = selected.and_then(Mode::from_abi) {
            if self.mode != Some(mode) {
                self.register_mode(mode);
            }
        }
    }

    fn event_context(&self) -> EvdiEventContext {
        EvdiEventContext {
            dpms_handler: Some(dpms_changed),
            mode_changed_handler: Some(mode_changed),
            update_ready_handler: Some(update_ready),
            crtc_state_handler: Some(crtc_state_changed),
            cursor_set_handler: Some(cursor_set),
            cursor_move_handler: Some(cursor_move),
            ddcci_data_handler: Some(ddcci_data),
            user_data: (&*self.state as *const EventState).cast_mut().cast(),
        }
    }

    fn register_mode(&mut self, mode: Mode) {
        trace(&format!(
            "[evdi] registering capture buffer {}x{}@{}",
            mode.width, mode.height, mode.refresh_hz
        ));
        self.unregister_buffer();
        self.mode = None;
        let Some(stride) = mode.width.checked_mul(4) else {
            return;
        };
        let Some(length) = stride.checked_mul(mode.height) else {
            return;
        };
        let (Some(width), Some(height), Some(stride32)) = (
            c_int::try_from(mode.width).ok(),
            c_int::try_from(mode.height).ok(),
            c_int::try_from(stride).ok(),
        ) else {
            return;
        };
        self.buffer = Some(RegisteredBuffer {
            pixels: vec![0; length],
            width: mode.width,
            height: mode.height,
            stride,
        });
        let buffer = self.buffer.as_mut().expect("buffer was just stored");
        let abi_buffer = EvdiBuffer {
            id: BUFFER_ID,
            buffer: buffer.pixels.as_mut_ptr().cast(),
            width,
            height,
            stride: stride32,
            rects: core::ptr::null_mut(),
            rect_count: 0,
        };
        // SAFETY: the vector is stored before registration and unregistered before replacement.
        unsafe { evdi_register_buffer(self.handle.as_ptr(), abi_buffer) };
        self.mode = Some(mode);
    }

    fn unregister_buffer(&mut self) {
        if self.buffer.take().is_some() {
            // SAFETY: BUFFER_ID is registered exactly while buffer is Some.
            unsafe { evdi_unregister_buffer(self.handle.as_ptr(), BUFFER_ID) };
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.unregister_buffer();
        // SAFETY: this object owns the handle and closes it exactly once.
        unsafe {
            evdi_disconnect(self.handle.as_ptr());
            evdi_close(self.handle.as_ptr());
        }
    }
}

pub fn connect_monitor(card: &mut Device, edid: &[u8]) -> Result<(), Error> {
    const MAX_PIXEL_AREA: u32 = 3840 * 2160;
    let started = Instant::now();
    trace(&format!(
        "[evdi] calling connect_monitor(edid_len={})",
        edid.len()
    ));
    let result = card.connect(edid, MAX_PIXEL_AREA, MAX_PIXEL_AREA * 60);
    trace(&format!(
        "[evdi] connect_monitor returned in {:?}: {}",
        started.elapsed(),
        if result.is_ok() { "ok" } else { "error" }
    ));
    result
}

pub fn to_rgb888(frame: &Frame<'_>) -> Vec<u8> {
    let mut out = vec![0u8; frame.width * frame.height * 3];
    for y in 0..frame.height {
        for x in 0..frame.width {
            let source = y * frame.stride + x * 4;
            let target = (y * frame.width + x) * 3;
            let Some(pixel) = frame.pixels.get(source..source + 4) else {
                continue;
            };
            let pixel = u32::from_le_bytes(pixel.try_into().expect("four-byte pixel"));
            out[target] = ((pixel >> 16) & 0xff) as u8;
            out[target + 1] = ((pixel >> 8) & 0xff) as u8;
            out[target + 2] = (pixel & 0xff) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_xrgb8888_with_stride() {
        let pixels = [
            0x33, 0x22, 0x11, 0xff, 0x66, 0x55, 0x44, 0xff, 0xaa, 0xaa, 0xaa, 0xaa,
        ];
        let frame = Frame {
            pixels: &pixels,
            width: 2,
            height: 1,
            stride: 12,
            damage: Vec::new(),
        };
        assert_eq!(to_rgb888(&frame), [0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
    }

    #[test]
    fn rejects_invalid_mode() {
        assert_eq!(
            Mode::from_abi(EvdiMode {
                width: 0,
                height: 1080,
                refresh_rate: 60,
                bits_per_pixel: 32,
                pixel_format: 0,
            }),
            None
        );
    }
}
