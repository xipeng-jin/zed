//! The frame-presenter seam: turns engine paint output into a GPUI element
//! (ADR-0002). `SoftwarePresenter` is the cross-platform baseline; an
//! IOSurface-backed zero-copy presenter joins in M3 behind the same trait.

use crate::tab_backend::{PaintOutput, SoftwareFrame};
use gpui::{
    AnyElement, IntoElement as _, ObjectFit, RenderImage, Styled as _, StyledImage as _, Window,
    img,
};
use image::{Frame, ImageBuffer};
use smallvec::SmallVec;
use std::sync::Arc;
use util::ResultExt as _;

/// Presents engine frames as GPUI elements. All frame presentation in the
/// browser view flows through this trait.
pub trait FramePresenter: 'static {
    /// Accept the latest paint output. Cheap; may be called ahead of render.
    fn present(&mut self, output: PaintOutput);

    /// Build the element painting the most recent frame, or `None` before the
    /// first frame arrives. Called during render; also retires GPU resources
    /// of frames that are no longer displayed.
    fn render_frame(&mut self, window: &mut Window) -> Option<AnyElement>;

    /// Release all retained GPU resources. Call when the owning view is
    /// released.
    fn release(&mut self, window: &mut Window);

    fn has_frame(&self) -> bool;
}

/// Software OSR presentation: each BGRA frame is wrapped in a
/// [`RenderImage`] (natively BGRA — no pixel conversion) and painted with a
/// stock `img()` element stretched over the content bounds.
///
/// Every frame gets a fresh image id, so each upload allocates a new sprite
/// atlas tile. To keep the atlas from accumulating dead tiles we retain the
/// two most recently rendered images and explicitly drop the older one once a
/// newer frame has been handed to the renderer, following
/// `RemoteVideoTrackView`'s pattern (the previous frame may still be
/// referenced by the frame in flight, so it cannot be dropped immediately).
pub struct SoftwarePresenter {
    latest: Option<Arc<RenderImage>>,
    current_rendered: Option<Arc<RenderImage>>,
    previous_rendered: Option<Arc<RenderImage>>,
}

impl SoftwarePresenter {
    pub fn new() -> Self {
        Self {
            latest: None,
            current_rendered: None,
            previous_rendered: None,
        }
    }

    fn image_from_frame(frame: SoftwareFrame) -> Option<Arc<RenderImage>> {
        // The buffer is BGRA, but `ImageBuffer` is only used as a container
        // here: gpui treats `RenderImage` bytes as BGRA when uploading.
        let buffer = ImageBuffer::from_raw(frame.width, frame.height, frame.bgra)?;
        Some(Arc::new(RenderImage::new(SmallVec::from_elem(
            Frame::new(buffer),
            1,
        ))))
    }
}

impl FramePresenter for SoftwarePresenter {
    fn present(&mut self, output: PaintOutput) {
        match output {
            PaintOutput::Software(frame) => {
                if frame.bgra.len() != frame.width as usize * frame.height as usize * 4 {
                    log::error!(
                        "[browser] dropping malformed software frame: {}x{} with {} bytes",
                        frame.width,
                        frame.height,
                        frame.bgra.len()
                    );
                    return;
                }
                self.latest = Self::image_from_frame(frame);
            }
        }
    }

    fn render_frame(&mut self, window: &mut Window) -> Option<AnyElement> {
        let latest = self.latest.clone()?;
        if let Some(current) = self.current_rendered.take() {
            if let Some(previous) = self.previous_rendered.take()
                && previous.id != current.id
                && previous.id != latest.id
            {
                window.drop_image(previous).log_err();
            }
            self.previous_rendered = Some(current);
        }
        self.current_rendered = Some(latest.clone());
        Some(
            img(latest)
                .size_full()
                .object_fit(ObjectFit::Fill)
                .into_any_element(),
        )
    }

    fn release(&mut self, window: &mut Window) {
        let mut dropped = Vec::new();
        for image in [
            self.previous_rendered.take(),
            self.current_rendered.take(),
            self.latest.take(),
        ]
        .into_iter()
        .flatten()
        {
            if !dropped.contains(&image.id) {
                dropped.push(image.id);
                window.drop_image(image).log_err();
            }
        }
    }

    fn has_frame(&self) -> bool {
        self.latest.is_some()
    }
}
