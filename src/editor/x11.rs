use std::os::raw::{c_char, c_int, c_long, c_ulong, c_void};

// ── X11 event type constants ─────────────────────────────────────
const KEY_PRESS: i32 = 2;
const KEY_RELEASE: i32 = 3;
const BUTTON_PRESS: i32 = 4;
const FOCUS_IN: i32 = 9;
const FOCUS_OUT: i32 = 10;

// ── X11 event masks for XSelectInput ─────────────────────────────
const KEY_PRESS_MASK: c_long = 1 << 0;
const KEY_RELEASE_MASK: c_long = 1 << 1;
const BUTTON_PRESS_MASK: c_long = 1 << 2;
const FOCUS_CHANGE_MASK: c_long = 1 << 21;

// ── X11 scroll button constants ───────────────────────────────────
const BUTTON_SCROLL_UP: u32 = 4;
const BUTTON_SCROLL_DOWN: u32 = 5;

/// Minimal XEvent union for the event types we handle.
/// The full XEvent is 192 bytes — we only read the type field and
/// the event-specific data we need.
#[repr(C)]
pub(super) union XEvent {
    pub event_type: i32,
    pub key: XKeyEvent,
    pub button: XButtonEvent,
    pub focus: XFocusChangeEvent,
    pub _pad: [u8; 192], // XEvent is 192 bytes
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    std::mem::size_of::<XEvent>() == 192,
    "XEvent must be 192 bytes on 64-bit"
);

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct XKeyEvent {
    pub event_type: i32,
    pub _serial: c_ulong,
    pub _send_event: i32,
    pub _display: *mut c_void,
    pub _window: c_ulong,
    pub _root: c_ulong,
    pub _subwindow: c_ulong,
    pub _time: c_ulong,
    pub _x: i32,
    pub _y: i32,
    pub _x_root: i32,
    pub _y_root: i32,
    pub state: u32, // modifier mask
    pub keycode: u32,
    pub _same_screen: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct XButtonEvent {
    pub event_type: i32,
    pub _serial: c_ulong,
    pub _send_event: i32,
    pub _display: *mut c_void,
    pub _window: c_ulong,
    pub _root: c_ulong,
    pub _subwindow: c_ulong,
    pub _time: c_ulong,
    pub _x: i32,
    pub _y: i32,
    pub _x_root: i32,
    pub _y_root: i32,
    pub state: u32,
    pub button: u32,
    pub _same_screen: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct XFocusChangeEvent {
    pub event_type: i32,
    pub _serial: c_ulong,
    pub _send_event: i32,
    pub _display: *mut c_void,
    pub _window: c_ulong,
    pub _mode: i32,
    pub _detail: i32,
}

/// Input event produced by X11 event polling.
pub(super) enum InputEvent {
    KeyDown {
        key_char: u16,
        key_code: i16,
        modifiers: i16,
    },
    KeyUp {
        key_char: u16,
        key_code: i16,
        modifiers: i16,
    },
    Scroll {
        distance: f32,
    },
    Focus {
        state: bool,
    },
}

/// Convert X11 modifier state to VST3 modifier flags.
/// VST3 modifiers (from ivstevents.h):
///   kShiftKey = 1 << 0, kAlternateKey = 1 << 1, kCommandKey = 1 << 2, kControlKey = 1 << 3
fn x11_state_to_vst3_modifiers(state: u32) -> i16 {
    let mut mods: i16 = 0;
    if state & 1 != 0 {
        mods |= 1;
    } // ShiftMask → kShiftKey
    if state & 8 != 0 {
        mods |= 1 << 1;
    } // Mod1Mask (Alt) → kAlternateKey
    if state & 128 != 0 {
        mods |= 1 << 2;
    } // Mod4Mask (1<<7, Super) → kCommandKey
    if state & 4 != 0 {
        mods |= 1 << 3;
    } // ControlMask → kControlKey
    mods
}

/// Dynamically loaded X11 functions.
struct X11Lib {
    _lib: libloading::Library,
    open_display: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    default_root_window: unsafe extern "C" fn(*mut c_void) -> c_ulong,
    create_simple_window: unsafe extern "C" fn(
        *mut c_void,
        c_ulong,
        c_int,
        c_int,
        u32,
        u32,
        u32,
        c_ulong,
        c_ulong,
    ) -> c_ulong,
    map_window: unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int,
    destroy_window: unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int,
    close_display: unsafe extern "C" fn(*mut c_void) -> c_int,
    flush: unsafe extern "C" fn(*mut c_void) -> c_int,
    x_select_input: unsafe extern "C" fn(*mut c_void, c_ulong, c_long) -> c_int,
    x_pending: unsafe extern "C" fn(*mut c_void) -> c_int,
    x_next_event: unsafe extern "C" fn(*mut c_void, *mut XEvent) -> c_int,
}

impl X11Lib {
    fn load() -> Result<Self, String> {
        let lib = unsafe { libloading::Library::new("libX11.so.6") }
            .or_else(|_| unsafe { libloading::Library::new("libX11.so") })
            .map_err(|e| format!("failed to load libX11: {e}"))?;

        unsafe {
            let open_display = *lib
                .get::<unsafe extern "C" fn(*const c_char) -> *mut c_void>(b"XOpenDisplay")
                .map_err(|e| format!("XOpenDisplay: {e}"))?;
            let default_root_window = *lib
                .get::<unsafe extern "C" fn(*mut c_void) -> c_ulong>(b"XDefaultRootWindow")
                .map_err(|e| format!("XDefaultRootWindow: {e}"))?;
            let create_simple_window = *lib
                .get::<unsafe extern "C" fn(
                    *mut c_void,
                    c_ulong,
                    c_int,
                    c_int,
                    u32,
                    u32,
                    u32,
                    c_ulong,
                    c_ulong,
                ) -> c_ulong>(b"XCreateSimpleWindow")
                .map_err(|e| format!("XCreateSimpleWindow: {e}"))?;
            let map_window = *lib
                .get::<unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int>(b"XMapWindow")
                .map_err(|e| format!("XMapWindow: {e}"))?;
            let destroy_window = *lib
                .get::<unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int>(b"XDestroyWindow")
                .map_err(|e| format!("XDestroyWindow: {e}"))?;
            let close_display = *lib
                .get::<unsafe extern "C" fn(*mut c_void) -> c_int>(b"XCloseDisplay")
                .map_err(|e| format!("XCloseDisplay: {e}"))?;
            let flush = *lib
                .get::<unsafe extern "C" fn(*mut c_void) -> c_int>(b"XFlush")
                .map_err(|e| format!("XFlush: {e}"))?;
            let x_select_input = *lib
                .get::<unsafe extern "C" fn(*mut c_void, c_ulong, c_long) -> c_int>(b"XSelectInput")
                .map_err(|e| format!("XSelectInput: {e}"))?;
            let x_pending = *lib
                .get::<unsafe extern "C" fn(*mut c_void) -> c_int>(b"XPending")
                .map_err(|e| format!("XPending: {e}"))?;
            let x_next_event = *lib
                .get::<unsafe extern "C" fn(*mut c_void, *mut XEvent) -> c_int>(b"XNextEvent")
                .map_err(|e| format!("XNextEvent: {e}"))?;

            Ok(Self {
                _lib: lib,
                open_display,
                default_root_window,
                create_simple_window,
                map_window,
                destroy_window,
                close_display,
                flush,
                x_select_input,
                x_pending,
                x_next_event,
            })
        }
    }
}

/// A native X11 window for hosting a VST3 editor.
pub(super) struct X11Window {
    x11: X11Lib,
    display: *mut c_void,
    window: c_ulong,
}

// Safety: X11Window is only used from one thread.
unsafe impl Send for X11Window {}

impl X11Window {
    /// Create and map a new X11 window.
    pub fn create(width: u32, height: u32) -> Result<Self, String> {
        let x11 = X11Lib::load()?;

        let display = unsafe { (x11.open_display)(std::ptr::null()) };
        if display.is_null() {
            return Err("XOpenDisplay failed (is DISPLAY set?)".into());
        }

        let root = unsafe { (x11.default_root_window)(display) };
        let window =
            unsafe { (x11.create_simple_window)(display, root, 0, 0, width, height, 0, 0, 0) };

        if window == 0 {
            unsafe { (x11.close_display)(display) };
            return Err("XCreateSimpleWindow failed".into());
        }

        unsafe {
            (x11.map_window)(display, window);
            // Register interest in keyboard, button-press (scroll), and focus events.
            (x11.x_select_input)(
                display,
                window,
                KEY_PRESS_MASK | KEY_RELEASE_MASK | BUTTON_PRESS_MASK | FOCUS_CHANGE_MASK,
            );
            (x11.flush)(display);
        }

        tracing::debug!(window_id = window, width, height, "X11 window created");

        Ok(Self {
            x11,
            display,
            window,
        })
    }

    /// Get the X11 window ID (for passing to IPlugView::attached).
    pub fn window_id(&self) -> c_ulong {
        self.window
    }

    /// Poll X11 events and return input events for IPlugView forwarding.
    /// Non-blocking: returns immediately if no events are pending.
    pub(super) fn poll_events(&self) -> Vec<InputEvent> {
        let mut events = Vec::new();
        unsafe {
            while (self.x11.x_pending)(self.display) > 0 {
                let mut event = std::mem::zeroed::<XEvent>();
                (self.x11.x_next_event)(self.display, &mut event);

                match event.event_type {
                    KEY_PRESS => {
                        let key = event.key;
                        let modifiers = x11_state_to_vst3_modifiers(key.state);
                        // Use keycode directly as key_code. The plugin interprets
                        // these as platform-specific virtual key codes.
                        events.push(InputEvent::KeyDown {
                            key_char: 0, // No Unicode lookup without Xkb
                            // X11 keycodes are 8-255, safe to truncate to i16.
                            key_code: key.keycode as i16,
                            modifiers,
                        });
                    }
                    KEY_RELEASE => {
                        let key = event.key;
                        let modifiers = x11_state_to_vst3_modifiers(key.state);
                        events.push(InputEvent::KeyUp {
                            key_char: 0,
                            key_code: key.keycode as i16,
                            modifiers,
                        });
                    }
                    BUTTON_PRESS => {
                        let button = event.button;
                        match button.button {
                            BUTTON_SCROLL_UP => {
                                events.push(InputEvent::Scroll { distance: 1.0 });
                            }
                            BUTTON_SCROLL_DOWN => {
                                events.push(InputEvent::Scroll { distance: -1.0 });
                            }
                            _ => {} // Ignore regular mouse buttons
                        }
                    }
                    FOCUS_IN => events.push(InputEvent::Focus { state: true }),
                    FOCUS_OUT => events.push(InputEvent::Focus { state: false }),
                    _ => {} // Ignore other events
                }
            }
        }
        events
    }
}

impl Drop for X11Window {
    fn drop(&mut self) {
        unsafe {
            (self.x11.destroy_window)(self.display, self.window);
            (self.x11.close_display)(self.display);
        }
        tracing::debug!(window_id = self.window, "X11 window destroyed");
    }
}
