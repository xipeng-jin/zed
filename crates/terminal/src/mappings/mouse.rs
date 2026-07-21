use std::cmp::{self, min};
use std::iter::repeat;

/// Most of the code, and specifically the constants, in this are copied from Alacritty,
/// with modifications for our circumstances
use gpui::{Modifiers, MouseButton, Pixels, Point as GpuiPoint, ScrollWheelEvent, px};

use crate::{Modes, Point, SelectionSide, TerminalBounds};

// Alacritty-era wire formats: production on macOS/Windows, contract-test
// oracle on Linux. Deleted at P10.
#[cfg_attr(target_os = "linux", allow(dead_code))]
enum MouseFormat {
    Sgr,
    Normal(bool),
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
impl MouseFormat {
    fn from_mode(mode: Modes) -> Self {
        if mode.contains(Modes::SGR_MOUSE) {
            MouseFormat::Sgr
        } else if mode.contains(Modes::UTF8_MOUSE) {
            MouseFormat::Normal(true)
        } else {
            MouseFormat::Normal(false)
        }
    }
}

#[derive(Debug)]
enum MouseButtonCode {
    LeftButton = 0,
    MiddleButton = 1,
    RightButton = 2,
    LeftMove = 32,
    MiddleMove = 33,
    RightMove = 34,
    NoneMove = 35,
    ScrollUp = 64,
    ScrollDown = 65,
    Other = 99,
}

impl MouseButtonCode {
    fn from_move_button(e: Option<MouseButton>) -> Self {
        match e {
            Some(gpui::MouseButton::Left) => MouseButtonCode::LeftMove,
            Some(gpui::MouseButton::Middle) => MouseButtonCode::MiddleMove,
            Some(gpui::MouseButton::Right) => MouseButtonCode::RightMove,
            Some(gpui::MouseButton::Navigate(_)) => MouseButtonCode::Other,
            None => MouseButtonCode::NoneMove,
        }
    }

    fn from_button(e: MouseButton) -> Self {
        match e {
            gpui::MouseButton::Left => MouseButtonCode::LeftButton,
            gpui::MouseButton::Middle => MouseButtonCode::MiddleButton,
            gpui::MouseButton::Right => MouseButtonCode::RightButton,
            gpui::MouseButton::Navigate(_) => MouseButtonCode::Other,
        }
    }

    fn from_scroll(e: &ScrollWheelEvent) -> Self {
        let is_positive = match e.delta {
            gpui::ScrollDelta::Pixels(pixels) => pixels.y > px(0.),
            gpui::ScrollDelta::Lines(lines) => lines.y > 0.,
        };

        if is_positive {
            MouseButtonCode::ScrollUp
        } else {
            MouseButtonCode::ScrollDown
        }
    }

    fn is_other(&self) -> bool {
        matches!(self, MouseButtonCode::Other)
    }
}

pub(crate) fn scroll_report(
    point: Point,
    scroll_lines: i32,
    e: &ScrollWheelEvent,
    mode: Modes,
    backend: &crate::TerminalBackend,
) -> Option<impl Iterator<Item = Vec<u8>>> {
    if mode.intersects(Modes::MOUSE_MODE) {
        mouse_report(
            point,
            MouseButtonCode::from_scroll(e),
            true,
            e.modifiers,
            mode,
            backend,
        )
        .map(|report| repeat(report).take(scroll_lines.unsigned_abs() as usize))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mappings::test_support::backend_with_modes;
    use gpui::{ScrollDelta, TouchPhase, point};

    fn collect_scroll_reports(
        point: Point,
        scroll_lines: i32,
        e: &ScrollWheelEvent,
        mode: Modes,
    ) -> Option<Vec<Vec<u8>>> {
        let (backend, _events_rx) = backend_with_modes(mode);
        scroll_report(point, scroll_lines, e, mode, &backend).map(|reports| reports.collect())
    }

    #[test]
    fn scroll_report_repeats_for_negative_scroll_lines() {
        let grid_point = Point::new(0, 0);

        let scroll_event = ScrollWheelEvent {
            delta: ScrollDelta::Lines(point(0., -1.)),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        };

        let mode = Modes::MOUSE_MODE;
        let reports = collect_scroll_reports(grid_point, -3, &scroll_event, mode)
            .expect("mouse mode should produce a scroll report");

        assert_eq!(reports.len(), 3);
    }

    /// Permanent byte-level contract suite on the mouse wire formats
    /// (SPEC.md §4.3): expectations are the alacritty-era bytes. On Linux
    /// this exercises the ghostty `mouse::Encoder`; elsewhere the legacy
    /// formats, so the expectations hold everywhere by construction.
    mod contract {
        use super::*;
        use gpui::MouseButton;

        const CLICK: Modes = Modes::MOUSE_REPORT_CLICK;

        fn sgr(mode: Modes) -> Modes {
            mode | Modes::SGR_MOUSE
        }

        fn utf8(mode: Modes) -> Modes {
            mode | Modes::UTF8_MOUSE
        }

        fn button_report(
            point: Point,
            button: MouseButton,
            modifiers: Modifiers,
            pressed: bool,
            mode: Modes,
        ) -> Option<Vec<u8>> {
            let (backend, _events_rx) = backend_with_modes(mode);
            mouse_button_report(point, button, modifiers, pressed, mode, &backend)
        }

        fn moved_report(
            point: Point,
            button: Option<MouseButton>,
            modifiers: Modifiers,
            mode: Modes,
        ) -> Option<Vec<u8>> {
            let (backend, _events_rx) = backend_with_modes(mode);
            mouse_moved_report(point, button, modifiers, mode, &backend)
        }

        #[track_caller]
        fn assert_button_report(
            point: Point,
            button: MouseButton,
            modifiers: Modifiers,
            pressed: bool,
            mode: Modes,
            expected: Option<&[u8]>,
        ) {
            let actual = button_report(point, button, modifiers, pressed, mode);
            assert_eq!(
                actual.as_deref(),
                expected,
                "button {button:?} pressed {pressed} at {point:?} in {mode:?}"
            );
        }

        fn shift() -> Modifiers {
            Modifiers {
                shift: true,
                ..Default::default()
            }
        }

        #[test]
        fn normal_format_press_release() {
            let point = Point::new(3, 5);
            // 32+button, 32+1+column, 32+1+line
            assert_button_report(
                point,
                MouseButton::Left,
                Modifiers::default(),
                true,
                CLICK,
                Some(b"\x1b[M &$"),
            );
            assert_button_report(
                point,
                MouseButton::Left,
                Modifiers::default(),
                false,
                CLICK,
                Some(b"\x1b[M#&$"),
            );
            assert_button_report(
                point,
                MouseButton::Middle,
                Modifiers::default(),
                true,
                CLICK,
                Some(b"\x1b[M!&$"),
            );
            assert_button_report(
                point,
                MouseButton::Right,
                shift(),
                true,
                CLICK,
                Some(b"\x1b[M&&$"),
            );
            let ctrl_alt = Modifiers {
                control: true,
                alt: true,
                ..Default::default()
            };
            // middle (1) + alt (8) + ctrl (16) = 25; 32+25 = '9'
            assert_button_report(
                point,
                MouseButton::Middle,
                ctrl_alt,
                true,
                CLICK,
                Some(b"\x1b[M9&$"),
            );
        }

        #[test]
        fn no_report_without_mouse_mode() {
            assert_button_report(
                Point::new(3, 5),
                MouseButton::Left,
                Modifiers::default(),
                true,
                Modes::NONE,
                None,
            );
        }

        #[test]
        fn no_report_above_viewport() {
            assert_button_report(
                Point::new(-1, 5),
                MouseButton::Left,
                Modifiers::default(),
                true,
                CLICK,
                None,
            );
        }

        #[test]
        fn normal_format_coordinate_cap() {
            // Columns/lines at 222 encode as the last single byte (255);
            // beyond the cap no report is produced.
            let at_cap = Point::new(3, 222);
            let report = button_report(
                at_cap,
                MouseButton::Left,
                Modifiers::default(),
                true,
                CLICK,
            )
            .unwrap();
            assert_eq!(report, vec![0x1b, b'[', b'M', 32, 255, 36]);

            let beyond = Point::new(3, 223);
            assert_button_report(
                beyond,
                MouseButton::Left,
                Modifiers::default(),
                true,
                CLICK,
                None,
            );
        }

        #[test]
        fn utf8_format_wide_coordinates() {
            // Below 95 stays single-byte...
            let report = button_report(
                Point::new(3, 94),
                MouseButton::Left,
                Modifiers::default(),
                true,
                utf8(CLICK),
            )
            .unwrap();
            assert_eq!(report, vec![0x1b, b'[', b'M', 32, 0x7f, 36]);

            // ...from 95 the coordinate is two-byte encoded.
            let report = button_report(
                Point::new(3, 100),
                MouseButton::Left,
                Modifiers::default(),
                true,
                utf8(CLICK),
            )
            .unwrap();
            assert_eq!(report, vec![0x1b, b'[', b'M', 32, 0xc2, 0x85, 36]);

            // The two-byte range tops out at 2014...
            let report = button_report(
                Point::new(3, 2014),
                MouseButton::Left,
                Modifiers::default(),
                true,
                utf8(CLICK),
            );
            assert!(report.is_some());

            // ...beyond it the alacritty-era format dropped the report, while
            // ghostty continues with three-byte UTF-8 (adjudicated divergence
            // P3-009 in the divergence ledger).
            let beyond = button_report(
                Point::new(3, 2015),
                MouseButton::Left,
                Modifiers::default(),
                true,
                utf8(CLICK),
            );
            #[cfg(target_os = "linux")]
            assert_eq!(
                beyond.as_deref(),
                Some([0x1b, b'[', b'M', 32, 0xe0, 0xa0, 0x80, 36].as_slice())
            );
            #[cfg(not(target_os = "linux"))]
            assert_eq!(beyond, None);
        }

        #[test]
        fn sgr_format() {
            let point = Point::new(3, 5);
            assert_button_report(
                point,
                MouseButton::Left,
                Modifiers::default(),
                true,
                sgr(CLICK),
                Some(b"\x1b[<0;6;4M"),
            );
            assert_button_report(
                point,
                MouseButton::Left,
                Modifiers::default(),
                false,
                sgr(CLICK),
                Some(b"\x1b[<0;6;4m"),
            );
            assert_button_report(
                point,
                MouseButton::Right,
                shift(),
                true,
                sgr(CLICK),
                Some(b"\x1b[<6;6;4M"),
            );
            // SGR has no coordinate cap.
            assert_button_report(
                Point::new(4000, 5000),
                MouseButton::Left,
                Modifiers::default(),
                true,
                sgr(CLICK),
                Some(b"\x1b[<0;5001;4001M"),
            );
        }

        #[test]
        fn motion_reports() {
            let point = Point::new(3, 5);
            // Motion with a held button: 32 + 32.
            assert_eq!(
                moved_report(
                    point,
                    Some(MouseButton::Left),
                    Modifiers::default(),
                    Modes::MOUSE_MOTION,
                )
                .as_deref(),
                Some(b"\x1b[M@&$".as_slice()),
            );
            // Motion without a button: code 35.
            assert_eq!(
                moved_report(point, None, Modifiers::default(), Modes::MOUSE_MOTION)
                    .as_deref(),
                Some(b"\x1b[MC&$".as_slice()),
            );
            assert_eq!(
                moved_report(
                    point,
                    Some(MouseButton::Left),
                    Modifiers::default(),
                    sgr(Modes::MOUSE_MOTION),
                )
                .as_deref(),
                Some(b"\x1b[<32;6;4M".as_slice()),
            );
            // Drag mode only reports motion while a button is held.
            assert_eq!(
                moved_report(point, None, Modifiers::default(), Modes::MOUSE_DRAG),
                None,
            );
            assert_eq!(
                moved_report(
                    point,
                    Some(MouseButton::Left),
                    Modifiers::default(),
                    Modes::MOUSE_DRAG,
                )
                .as_deref(),
                Some(b"\x1b[M@&$".as_slice()),
            );
        }

        #[test]
        fn scroll_wire_format() {
            use gpui::{ScrollDelta, ScrollWheelEvent, TouchPhase, point};

            let up_event = ScrollWheelEvent {
                delta: ScrollDelta::Lines(point(0., 1.)),
                touch_phase: TouchPhase::Moved,
                ..Default::default()
            };
            let down_event = ScrollWheelEvent {
                delta: ScrollDelta::Lines(point(0., -1.)),
                touch_phase: TouchPhase::Moved,
                ..Default::default()
            };
            let grid_point = Point::new(3, 5);

            // Wheel buttons are 64/65.
            let reports = collect_scroll_reports(grid_point, 2, &up_event, CLICK).unwrap();
            assert_eq!(reports, vec![b"\x1b[M`&$".to_vec(), b"\x1b[M`&$".to_vec()]);

            let reports = collect_scroll_reports(grid_point, -1, &down_event, sgr(CLICK)).unwrap();
            assert_eq!(reports, vec![b"\x1b[<65;6;4M".to_vec()]);
        }

        #[test]
        fn alt_scroll_arrows() {
            assert_eq!(alt_scroll(3), b"\x1bOA\x1bOA\x1bOA".to_vec());
            assert_eq!(alt_scroll(-2), b"\x1bOB\x1bOB".to_vec());
            assert_eq!(alt_scroll(0), Vec::<u8>::new());
        }
    }

    #[test]
    fn scroll_report_repeats_for_positive_scroll_lines() {
        let grid_point = Point::new(0, 0);

        let scroll_event = ScrollWheelEvent {
            delta: ScrollDelta::Lines(point(0., 1.)),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        };

        let mode = Modes::MOUSE_MODE;
        let reports = collect_scroll_reports(grid_point, 3, &scroll_event, mode)
            .expect("mouse mode should produce a scroll report");

        assert_eq!(reports.len(), 3);
    }
}

pub(crate) fn alt_scroll(scroll_lines: i32) -> Vec<u8> {
    #[cfg(target_os = "linux")]
    if let Some(arrow) = super::keys::ghostty_scroll_arrow_bytes(scroll_lines > 0) {
        return arrow.repeat(scroll_lines.unsigned_abs() as usize);
    }

    legacy_alt_scroll(scroll_lines)
}

// Alacritty-era path; on Linux only the fallback for encoder-construction
// failure. Deleted at P10.
fn legacy_alt_scroll(scroll_lines: i32) -> Vec<u8> {
    let cmd = if scroll_lines > 0 { b'A' } else { b'B' };

    let mut content = Vec::with_capacity(scroll_lines.unsigned_abs() as usize * 3);
    for _ in 0..scroll_lines.abs() {
        content.push(0x1b);
        content.push(b'O');
        content.push(cmd);
    }
    content
}

pub(crate) fn mouse_button_report(
    point: Point,
    button: gpui::MouseButton,
    modifiers: Modifiers,
    pressed: bool,
    mode: Modes,
    backend: &crate::TerminalBackend,
) -> Option<Vec<u8>> {
    let button = MouseButtonCode::from_button(button);
    if !button.is_other() && mode.intersects(Modes::MOUSE_MODE) {
        mouse_report(point, button, pressed, modifiers, mode, backend)
    } else {
        None
    }
}

pub(crate) fn mouse_moved_report(
    point: Point,
    button: Option<MouseButton>,
    modifiers: Modifiers,
    mode: Modes,
    backend: &crate::TerminalBackend,
) -> Option<Vec<u8>> {
    let button = MouseButtonCode::from_move_button(button);

    if !button.is_other() && mode.intersects(Modes::MOUSE_MOTION | Modes::MOUSE_DRAG) {
        //Only drags are reported in drag mode, so block NoneMove.
        if mode.contains(Modes::MOUSE_DRAG) && matches!(button, MouseButtonCode::NoneMove) {
            None
        } else {
            mouse_report(point, button, true, modifiers, mode, backend)
        }
    } else {
        None
    }
}

pub(crate) fn grid_point(
    pos: GpuiPoint<Pixels>,
    cur_size: TerminalBounds,
    display_offset: usize,
) -> Point {
    grid_point_and_side(pos, cur_size, display_offset).0
}

pub(crate) fn grid_point_and_side(
    pos: GpuiPoint<Pixels>,
    cur_size: TerminalBounds,
    display_offset: usize,
) -> (Point, SelectionSide) {
    let mut column = (pos.x / cur_size.cell_width) as usize;
    let cell_x = cmp::max(px(0.), pos.x) % cur_size.cell_width;
    let half_cell_width = cur_size.cell_width / 2.0;
    let mut side = if cell_x > half_cell_width {
        SelectionSide::Right
    } else {
        SelectionSide::Left
    };

    let last_column = cur_size.num_columns().saturating_sub(1);
    if column > last_column {
        column = last_column;
        side = SelectionSide::Right;
    }
    let column = min(column, last_column);
    let mut line = (pos.y / cur_size.line_height) as i32;
    let bottommost_line = i32::try_from(cur_size.num_lines().saturating_sub(1)).unwrap_or(i32::MAX);
    if line > bottommost_line {
        line = bottommost_line;
        side = SelectionSide::Right;
    } else if line < 0 {
        side = SelectionSide::Left;
    }

    let display_offset = i32::try_from(display_offset).unwrap_or(i32::MAX);
    (
        Point::new(line.saturating_sub(display_offset), column),
        side,
    )
}

///Generate the bytes to send to the terminal, from the cell location, a mouse event, and the terminal mode
///
/// Zed keeps the policy layer (mode gating, grid math, jitter/dedup); on
/// Linux the wire bytes come from ghostty's `mouse::Encoder` with its options
/// read off the live terminal at encode time (SPEC.md §4.3), elsewhere from
/// the alacritty-era formats until the §8 platform gates open.
#[cfg(target_os = "linux")]
fn mouse_report(
    point: Point,
    button: MouseButtonCode,
    pressed: bool,
    modifiers: Modifiers,
    _mode: Modes,
    backend: &crate::TerminalBackend,
) -> Option<Vec<u8>> {
    if point.line < 0 {
        return None;
    }

    ghostty_mouse_report(point, button, pressed, modifiers, backend.vt_terminal())
}

#[cfg(not(target_os = "linux"))]
fn mouse_report(
    point: Point,
    button: MouseButtonCode,
    pressed: bool,
    modifiers: Modifiers,
    mode: Modes,
    _backend: &crate::TerminalBackend,
) -> Option<Vec<u8>> {
    if point.line < 0 {
        return None;
    }

    legacy_mouse_report(point, button, pressed, modifiers, MouseFormat::from_mode(mode))
}

#[cfg(target_os = "linux")]
fn ghostty_mouse_report(
    point: Point,
    button: MouseButtonCode,
    pressed: bool,
    modifiers: Modifiers,
    terminal: &ghostty_vt::Terminal<'_, '_>,
) -> Option<Vec<u8>> {
    use ghostty_vt::{key, mouse};
    use util::ResultExt as _;

    let (action, mouse_button) = match button {
        MouseButtonCode::LeftButton if pressed => (mouse::Action::Press, Some(mouse::Button::Left)),
        MouseButtonCode::LeftButton => (mouse::Action::Release, Some(mouse::Button::Left)),
        MouseButtonCode::MiddleButton if pressed => {
            (mouse::Action::Press, Some(mouse::Button::Middle))
        }
        MouseButtonCode::MiddleButton => (mouse::Action::Release, Some(mouse::Button::Middle)),
        MouseButtonCode::RightButton if pressed => {
            (mouse::Action::Press, Some(mouse::Button::Right))
        }
        MouseButtonCode::RightButton => (mouse::Action::Release, Some(mouse::Button::Right)),
        MouseButtonCode::LeftMove => (mouse::Action::Motion, Some(mouse::Button::Left)),
        MouseButtonCode::MiddleMove => (mouse::Action::Motion, Some(mouse::Button::Middle)),
        MouseButtonCode::RightMove => (mouse::Action::Motion, Some(mouse::Button::Right)),
        MouseButtonCode::NoneMove => (mouse::Action::Motion, None),
        MouseButtonCode::ScrollUp => (mouse::Action::Press, Some(mouse::Button::Four)),
        MouseButtonCode::ScrollDown => (mouse::Action::Press, Some(mouse::Button::Five)),
        MouseButtonCode::Other => return None,
    };

    let mut mods = key::Mods::empty();
    mods.set(key::Mods::SHIFT, modifiers.shift);
    mods.set(key::Mods::ALT, modifiers.alt);
    mods.set(key::Mods::CTRL, modifiers.control);

    // Zed's grid math is authoritative: 1×1-pixel cells make the encoder's
    // pixel→cell conversion the identity, so `point` passes through as-is.
    // (Under SGR-Pixels — mode 1016, now live via terminal state — the same
    // identity map means reports carry cell-granular coordinates; divergence
    // ledger P8-002.)
    let mut encoder = mouse::Encoder::new().log_err()?;
    encoder
        // Encode-time mode sync from the live terminal (SPEC.md §4.3):
        // tracking mode and output format come from terminal state; size and
        // any-button state are Zed-set below.
        .set_options_from_terminal(terminal)
        // Screen dimensions far above any real grid, so the encoder never
        // clamps a coordinate Zed's own math produced.
        .set_size(mouse::EncoderSize {
            screen_width: 100_000,
            screen_height: 100_000,
            cell_width: 1,
            cell_height: 1,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        })
        .set_any_button_pressed(mouse_button.is_some())
        .set_track_last_cell(false);

    let mut event = mouse::Event::new().log_err()?;
    event
        .set_action(action)
        .set_button(mouse_button)
        .set_mods(mods)
        .set_position(mouse::Position {
            x: point.column as f32,
            y: point.line as f32,
        });

    let mut bytes = Vec::new();
    encoder.encode_to_vec(&event, &mut bytes).log_err()?;
    if bytes.is_empty() { None } else { Some(bytes) }
}

// Alacritty-era wire formats: production on macOS/Windows, contract-test
// oracle on Linux. Deleted at P10.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn legacy_mouse_report(
    point: Point,
    button: MouseButtonCode,
    pressed: bool,
    modifiers: Modifiers,
    format: MouseFormat,
) -> Option<Vec<u8>> {
    let mut mods = 0;
    if modifiers.shift {
        mods += 4;
    }
    if modifiers.alt {
        mods += 8;
    }
    if modifiers.control {
        mods += 16;
    }

    match format {
        MouseFormat::Sgr => {
            Some(sgr_mouse_report(point, button as u8 + mods, pressed).into_bytes())
        }
        MouseFormat::Normal(utf8) => {
            if pressed {
                normal_mouse_report(point, button as u8 + mods, utf8)
            } else {
                normal_mouse_report(point, 3 + mods, utf8)
            }
        }
    }
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn normal_mouse_report(point: Point, button: u8, utf8: bool) -> Option<Vec<u8>> {
    let max_point = if utf8 { 2015 } else { 223 };

    if point.line >= max_point || point.column >= max_point as usize {
        return None;
    }

    let mut msg = vec![b'\x1b', b'[', b'M', 32 + button];

    let mouse_pos_encode = |pos: usize| -> Vec<u8> {
        let pos = 32 + 1 + pos;
        let first = 0xC0 + pos / 64;
        let second = 0x80 + (pos & 63);
        vec![first as u8, second as u8]
    };

    if utf8 && point.column >= 95 {
        msg.append(&mut mouse_pos_encode(point.column));
    } else {
        msg.push(32 + 1 + point.column as u8);
    }

    if utf8 && point.line >= 95 {
        msg.append(&mut mouse_pos_encode(point.line as usize));
    } else {
        msg.push(32 + 1 + point.line as u8);
    }

    Some(msg)
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn sgr_mouse_report(point: Point, button: u8, pressed: bool) -> String {
    let c = if pressed { 'M' } else { 'm' };

    let msg = format!(
        "\x1b[<{};{};{}{}",
        button,
        point.column + 1,
        point.line + 1,
        c
    );

    msg
}
