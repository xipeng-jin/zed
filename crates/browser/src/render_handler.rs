//! CEF render handler for software OSR, ported from
//! `Glass:crates/browser/src/render_handler.rs`. Glass only implemented the
//! macOS accelerated path; the `on_paint` software path here is new work
//! (migration plan §7 M1 step 5): CEF hands over a BGRA pixel buffer which is
//! copied into shared state and surfaced through the tab backend's paint
//! output for the frame presenter.

use crate::tab_backend::{EventSender, SoftwareFrame, TabBackendEvent, send_event};
use cef::{
    Browser, ImplRenderHandler, PaintElementType, Rect, RenderHandler, ScreenInfo,
    WrapRenderHandler, rc::Rc as _, wrap_render_handler,
};
use parking_lot::Mutex;
use std::sync::Arc;

/// Render state shared between the tab backend (foreground thread) and the
/// CEF render handler. Width and height are logical pixels; CEF derives the
/// physical paint size from them and `scale_factor`.
pub(crate) struct RenderState {
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
    /// The most recent unconsumed software frame. Overwritten by newer paints;
    /// taken by `TabBackend::take_paint_output`.
    pub frame: Option<SoftwareFrame>,
}

impl Default for RenderState {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            scale_factor: 1.0,
            frame: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct OsrRenderHandler {
    state: Arc<Mutex<RenderState>>,
    sender: EventSender,
}

impl OsrRenderHandler {
    pub fn new(state: Arc<Mutex<RenderState>>, sender: EventSender) -> Self {
        Self { state, sender }
    }
}

wrap_render_handler! {
    pub(crate) struct RenderHandlerBuilder {
        handler: OsrRenderHandler,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let state = self.handler.state.lock();
                rect.x = 0;
                rect.y = 0;
                // CEF misbehaves on zero-sized views; clamp to 1x1.
                rect.width = state.width.max(1) as i32;
                rect.height = state.height.max(1) as i32;
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> ::std::os::raw::c_int {
            if let Some(info) = screen_info {
                let state = self.handler.state.lock();
                info.device_scale_factor = state.scale_factor;
                info.rect.x = 0;
                info.rect.y = 0;
                info.rect.width = state.width.max(1) as i32;
                info.rect.height = state.height.max(1) as i32;
                info.available_rect = info.rect.clone();
                info.depth = 32;
                info.depth_per_component = 8;
                info.is_monochrome = 0;
                return 1;
            }
            0
        }

        fn screen_point(
            &self,
            _browser: Option<&mut Browser>,
            view_x: ::std::os::raw::c_int,
            view_y: ::std::os::raw::c_int,
            screen_x: Option<&mut ::std::os::raw::c_int>,
            screen_y: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            if let Some(screen_x) = screen_x {
                *screen_x = view_x;
            }
            if let Some(screen_y) = screen_y {
                *screen_y = view_y;
            }
            1
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            // Only the view itself; popup widgets (dropdowns) come in M2.
            if type_ != PaintElementType::default() {
                return;
            }
            if buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }

            let byte_len = width as usize * height as usize * 4;
            let bgra = unsafe { std::slice::from_raw_parts(buffer, byte_len) }.to_vec();
            self.handler.state.lock().frame = Some(SoftwareFrame {
                width: width as u32,
                height: height as u32,
                bgra,
            });
            send_event(&self.handler.sender, TabBackendEvent::FrameReady);
        }
    }
}
