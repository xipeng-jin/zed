# Software OSR first, behind a presenter seam, with zero GPUI changes

Browser frames are presented via CEF's software off-screen rendering path (`on_paint`
pixel buffer → `gpui::RenderImage` → stock `img()` element), implemented entirely
inside the browser crate behind an internal `FramePresenter` trait. GPUI is not
modified: no new surface types, no external-texture import, no renderer changes.

This inverts Glass's approach — Glass only ever implemented the macOS accelerated
path (IOSurface → CVPixelBuffer → gpui `surface()`), leaving every other platform
blank — because a CPU-copy path is the only one that works on Linux, macOS, and
Windows today without forking GPUI, and this project ranks upstream trackability and
cross-platform reach above peak rendering throughput.

## Consequences

- Full-pane 60fps content (video, heavy animation) pays a per-frame CPU copy and
  texture upload; acceptable for browsing, mediocre for fullscreen video.
- The presenter seam is the designated extension point: the macOS zero-copy
  presenter is re-added behind it (M3), and a Linux dmabuf/Vulkan presenter may be
  added later **only if measurement shows the software path is inadequate** — never
  pre-emptively. Any such accelerated Linux path is the one place a GPUI extension
  would be considered, and should be designed for upstreaming.
