// ponytail: main display only, every display of a machine will join the layout later
use serde::{Deserialize, Serialize};

use crate::store::Side;

const EDGE_INSET: f64 = 2.0;
const FALLBACK_DOTS_PER_MM: f64 = 96.0 / 25.4;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub width_px: f32,
    pub height_px: f32,
    pub width_mm: f32,
    pub height_mm: f32,
}

#[cfg(target_os = "macos")]
mod platform {
    use core_graphics::{
        display::{CGDisplay, CGRect},
        event::CGEvent,
        event_source::{CGEventSource, CGEventSourceStateID},
        geometry::CGPoint,
    };

    pub fn bounds() -> (f64, f64, f64, f64) {
        let CGRect { origin, size } = CGDisplay::main().bounds();
        (origin.x, origin.y, size.width, size.height)
    }

    pub fn physical_mm() -> (f64, f64) {
        let size = CGDisplay::main().screen_size();
        (size.width, size.height)
    }

    pub fn position() -> Option<(f64, f64)> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
        let point = CGEvent::new(source).ok()?.location();
        Some((point.x, point.y))
    }

    pub fn warp(x: f64, y: f64) {
        let _ = CGDisplay::warp_mouse_cursor_position(CGPoint::new(x, y));
    }

    pub fn move_by(_: f64, _: f64) -> bool {
        false
    }
}

#[cfg(windows)]
mod platform {
    use std::cell::Cell;

    use windows_sys::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{GetDC, GetDeviceCaps, HORZSIZE, ReleaseDC, VERTSIZE},
        UI::{
            Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput},
            WindowsAndMessaging::{GetCursorPos, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SetCursorPos},
        },
    };

    const ABSOLUTE_RANGE: f64 = 65535.0;

    thread_local! {
        static REMAINDER: Cell<(f64, f64)> = const { Cell::new((0.0, 0.0)) };
    }

    pub fn bounds() -> (f64, f64, f64, f64) {
        let (width, height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        (0.0, 0.0, width as f64, height as f64)
    }

    pub fn physical_mm() -> (f64, f64) {
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            let size = (GetDeviceCaps(screen, HORZSIZE as i32), GetDeviceCaps(screen, VERTSIZE as i32));
            ReleaseDC(std::ptr::null_mut(), screen);
            (size.0 as f64, size.1 as f64)
        }
    }

    pub fn position() -> Option<(f64, f64)> {
        let mut point = POINT { x: 0, y: 0 };
        (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x as f64, point.y as f64))
    }

    pub fn warp(x: f64, y: f64) {
        REMAINDER.set((0.0, 0.0));
        unsafe { SetCursorPos(x as i32, y as i32) };
    }

    /// Absolute moves skip Windows pointer acceleration, which would otherwise
    /// accelerate a second time what the sending machine already accelerated,
    /// and the remainder keeps sub-pixel motion that integer deltas would drop.
    pub fn move_by(dx: f64, dy: f64) -> bool {
        let Some((x, y)) = position() else {
            return false;
        };
        let (_, _, width, height) = bounds();
        let (rest_x, rest_y) = REMAINDER.get();
        let (target_x, target_y) = (x + dx + rest_x, y + dy + rest_y);
        let (pixel_x, pixel_y) = (target_x.round(), target_y.round());
        REMAINDER.set((target_x - pixel_x, target_y - pixel_y));
        let (pixel_x, pixel_y) = (pixel_x.clamp(0.0, width - 1.0), pixel_y.clamp(0.0, height - 1.0));
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: (pixel_x * ABSOLUTE_RANGE / (width - 1.0)).round() as i32,
                    dy: (pixel_y * ABSOLUTE_RANGE / (height - 1.0)).round() as i32,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
        true
    }
}

pub fn detect() -> Screen {
    let (_, _, width, height) = platform::bounds();
    let (mut width_mm, mut height_mm) = platform::physical_mm();
    if width_mm < 1.0 || height_mm < 1.0 {
        width_mm = width / FALLBACK_DOTS_PER_MM;
        height_mm = height / FALLBACK_DOTS_PER_MM;
    }
    Screen {
        width_px: width as f32,
        height_px: height as f32,
        width_mm: width_mm as f32,
        height_mm: height_mm as f32,
    }
}

pub fn move_by(dx: f64, dy: f64) -> bool {
    platform::move_by(dx, dy)
}

pub fn edge_fraction(side: Side) -> f32 {
    let (left, top, width, height) = platform::bounds();
    let Some((x, y)) = platform::position() else {
        return 0.5;
    };
    let fraction = match side {
        Side::Left | Side::Right => (y - top) / height,
        Side::Top | Side::Bottom => (x - left) / width,
    };
    fraction.clamp(0.0, 1.0) as f32
}

pub fn warp_to_edge(side: Side, fraction: f32) {
    let (left, top, width, height) = platform::bounds();
    let fraction = fraction.clamp(0.0, 1.0) as f64;
    let (x, y) = match side {
        Side::Left => (left + EDGE_INSET, top + fraction * height),
        Side::Right => (left + width - EDGE_INSET, top + fraction * height),
        Side::Top => (left + fraction * width, top + EDGE_INSET),
        Side::Bottom => (left + fraction * width, top + height - EDGE_INSET),
    };
    platform::warp(x, y);
}
