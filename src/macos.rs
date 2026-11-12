//! The two bits of AppKit/CoreGraphics that winit/eframe don't expose.

#[cfg(target_os = "macos")]
mod imp {
    use objc2::{msg_send, runtime::AnyObject};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    /// Cursor position in global display points (top-left origin), the same space xcap's
    /// `Monitor::from_point` and winit's window positions use.
    pub fn cursor_position() -> Option<(f64, f64)> {
        unsafe {
            let event = CGEventCreate(std::ptr::null());
            if event.is_null() {
                return None;
            }
            let p = CGEventGetLocation(event);
            CFRelease(event);
            Some((p.x, p.y))
        }
    }

    const NS_SCREEN_SAVER_WINDOW_LEVEL: isize = 1000;
    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;

    /// winit's `AlwaysOnTop` is NSFloatingWindowLevel, which still sits under the menu bar.
    /// Raise the overlay above it and let it appear on top of full-screen Spaces too.
    pub fn float_above_everything(handle: &impl HasWindowHandle) {
        let Ok(handle) = handle.window_handle() else { return };
        let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
        unsafe {
            let view = h.ns_view.as_ptr() as *mut AnyObject;
            let window: *mut AnyObject = msg_send![view, window];
            if window.is_null() {
                return;
            }
            let _: () = msg_send![window, setLevel: NS_SCREEN_SAVER_WINDOW_LEVEL];
            let _: () = msg_send![window, setCollectionBehavior: CAN_JOIN_ALL_SPACES | FULL_SCREEN_AUXILIARY];
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn cursor_position() -> Option<(f64, f64)> {
        None
    }
    pub fn float_above_everything(_: &impl raw_window_handle::HasWindowHandle) {}
}

pub use imp::*;
