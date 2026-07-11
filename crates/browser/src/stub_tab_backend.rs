//! The scripted stub implementation of the tab-backend seam: the single test
//! seam for the browser crate (PRD issue #2, "Testing Decisions"; ticket #8).
//!
//! Tests hold a [`StubTabController`] while the [`StubTabBackend`] half lives
//! inside a browser view. The controller scripts engine events and paint
//! frames on demand; every command the view sends across the seam is recorded
//! in arrival order as a [`RecordedCommand`]. Scripted events reach the view
//! through the same post-pump drain the real engine uses — script, then call
//! [`crate::simulate_message_pump`].

use crate::tab_backend::{PaintOutput, SoftwareFrame, TabBackend, TabBackendEvent};
use anyhow::{Result, anyhow};
use gpui::{Keystroke, Modifiers, MouseButton, Pixels, Point, ScrollDelta};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;

/// A command a browser view sent across the tab-backend seam.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordedCommand {
    Start {
        url: String,
    },
    Navigate {
        url: String,
    },
    Reload,
    Stop,
    GoBack,
    GoForward,
    SetViewport {
        width: u32,
        height: u32,
        scale_factor: f32,
    },
    SetFocus {
        focused: bool,
    },
    MouseDown {
        position: Point<Pixels>,
        button: MouseButton,
        click_count: usize,
    },
    MouseUp {
        position: Point<Pixels>,
        button: MouseButton,
    },
    MouseMove {
        position: Point<Pixels>,
    },
    ScrollWheel {
        position: Point<Pixels>,
        /// x, y, and whether the delta was line-based (`ScrollDelta` has no
        /// `PartialEq`).
        delta: (f32, f32, bool),
    },
    KeyDown {
        key: String,
        is_held: bool,
    },
    KeyUp {
        key: String,
    },
    Close,
}

#[derive(Default)]
struct StubState {
    engine_ready: bool,
    started_with: Option<String>,
    next_start_error: Option<String>,
    commands: Vec<RecordedCommand>,
    events: VecDeque<TabBackendEvent>,
    paint_output: Option<PaintOutput>,
}

/// The engine half of the stub: a [`TabBackend`] that records commands and
/// replays scripted events.
pub struct StubTabBackend(Arc<Mutex<StubState>>);

/// The test half of the stub: scripts engine behavior and inspects recorded
/// commands.
#[derive(Clone)]
pub struct StubTabController(Arc<Mutex<StubState>>);

impl StubTabBackend {
    pub fn new(engine_ready: bool) -> (Self, StubTabController) {
        let state = Arc::new(Mutex::new(StubState {
            engine_ready,
            ..Default::default()
        }));
        (Self(state.clone()), StubTabController(state))
    }

    fn record(&self, command: RecordedCommand) {
        self.0.lock().commands.push(command);
    }
}

impl StubTabController {
    pub fn set_engine_ready(&self, ready: bool) {
        self.0.lock().engine_ready = ready;
    }

    /// Make the next `start` call fail with `message`.
    pub fn fail_next_start(&self, message: &str) {
        self.0.lock().next_start_error = Some(message.to_string());
    }

    /// Queue engine events for the next drain, preserving order.
    pub fn script_events(&self, events: impl IntoIterator<Item = TabBackendEvent>) {
        self.0.lock().events.extend(events);
    }

    /// Stage a paint frame and queue the `FrameReady` announcing it, matching
    /// how the engine's render handler publishes frames.
    pub fn script_frame(&self, frame: SoftwareFrame) {
        let mut state = self.0.lock();
        state.paint_output = Some(PaintOutput::Software(frame));
        state.events.push_back(TabBackendEvent::FrameReady);
    }

    /// URL the backend was started with, if `start` has succeeded.
    pub fn started_with(&self) -> Option<String> {
        self.0.lock().started_with.clone()
    }

    /// Whether a scripted frame is still staged (i.e. the view has not taken
    /// it yet).
    pub fn has_staged_frame(&self) -> bool {
        self.0.lock().paint_output.is_some()
    }

    /// All commands received so far, in order.
    pub fn commands(&self) -> Vec<RecordedCommand> {
        self.0.lock().commands.clone()
    }

    /// Drain the recorded commands, so a test can assert on exactly what one
    /// interaction sent.
    pub fn take_commands(&self) -> Vec<RecordedCommand> {
        std::mem::take(&mut self.0.lock().commands)
    }

    /// The most recent `SetViewport` command.
    pub fn last_viewport(&self) -> Option<(u32, u32, f32)> {
        self.0.lock().commands.iter().rev().find_map(|command| {
            if let RecordedCommand::SetViewport {
                width,
                height,
                scale_factor,
            } = command
            {
                Some((*width, *height, *scale_factor))
            } else {
                None
            }
        })
    }

    /// The `focused` values of all `SetFocus` commands, in order.
    pub fn focus_calls(&self) -> Vec<bool> {
        self.0
            .lock()
            .commands
            .iter()
            .filter_map(|command| {
                if let RecordedCommand::SetFocus { focused } = command {
                    Some(*focused)
                } else {
                    None
                }
            })
            .collect()
    }
}

impl TabBackend for StubTabBackend {
    fn engine_ready(&self) -> bool {
        self.0.lock().engine_ready
    }

    fn start(&mut self, url: &str) -> Result<()> {
        let mut state = self.0.lock();
        state.commands.push(RecordedCommand::Start {
            url: url.to_string(),
        });
        if let Some(message) = state.next_start_error.take() {
            return Err(anyhow!(message));
        }
        state.started_with = Some(url.to_string());
        Ok(())
    }

    fn is_started(&self) -> bool {
        self.0.lock().started_with.is_some()
    }

    fn navigate(&mut self, url: &str) {
        self.record(RecordedCommand::Navigate {
            url: url.to_string(),
        });
    }

    fn reload(&mut self) {
        self.record(RecordedCommand::Reload);
    }

    fn stop(&mut self) {
        self.record(RecordedCommand::Stop);
    }

    fn go_back(&mut self) {
        self.record(RecordedCommand::GoBack);
    }

    fn go_forward(&mut self) {
        self.record(RecordedCommand::GoForward);
    }

    fn set_viewport(&mut self, width: u32, height: u32, scale_factor: f32) {
        self.record(RecordedCommand::SetViewport {
            width,
            height,
            scale_factor,
        });
    }

    fn set_focus(&mut self, focused: bool) {
        self.record(RecordedCommand::SetFocus { focused });
    }

    fn send_mouse_down(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        click_count: usize,
        _modifiers: Modifiers,
    ) {
        self.record(RecordedCommand::MouseDown {
            position,
            button,
            click_count,
        });
    }

    fn send_mouse_up(&mut self, position: Point<Pixels>, button: MouseButton, _: Modifiers) {
        self.record(RecordedCommand::MouseUp { position, button });
    }

    fn send_mouse_move(
        &mut self,
        position: Point<Pixels>,
        _pressed_button: Option<MouseButton>,
        _modifiers: Modifiers,
    ) {
        self.record(RecordedCommand::MouseMove { position });
    }

    fn send_scroll_wheel(&mut self, position: Point<Pixels>, delta: ScrollDelta, _: Modifiers) {
        let delta = match delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y), false),
            ScrollDelta::Lines(delta) => (delta.x, delta.y, true),
        };
        self.record(RecordedCommand::ScrollWheel { position, delta });
    }

    fn send_key_down(&mut self, keystroke: &Keystroke, is_held: bool) {
        self.record(RecordedCommand::KeyDown {
            key: keystroke.key.clone(),
            is_held,
        });
    }

    fn send_key_up(&mut self, keystroke: &Keystroke) {
        self.record(RecordedCommand::KeyUp {
            key: keystroke.key.clone(),
        });
    }

    fn close(&mut self) {
        self.record(RecordedCommand::Close);
    }

    fn try_recv_event(&mut self) -> Option<TabBackendEvent> {
        self.0.lock().events.pop_front()
    }

    fn take_paint_output(&mut self) -> Option<PaintOutput> {
        self.0.lock().paint_output.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_events_replay_in_order() {
        let (mut backend, controller) = StubTabBackend::new(true);
        controller.script_events([
            TabBackendEvent::AddressChanged("https://a.example".into()),
            TabBackendEvent::TitleChanged("A".into()),
        ]);
        controller.script_events([TabBackendEvent::LoadingProgress(0.5)]);

        assert_eq!(
            backend.try_recv_event(),
            Some(TabBackendEvent::AddressChanged("https://a.example".into()))
        );
        assert_eq!(
            backend.try_recv_event(),
            Some(TabBackendEvent::TitleChanged("A".into()))
        );
        assert_eq!(
            backend.try_recv_event(),
            Some(TabBackendEvent::LoadingProgress(0.5))
        );
        assert_eq!(backend.try_recv_event(), None);
    }

    #[test]
    fn commands_record_in_arrival_order() {
        let (mut backend, controller) = StubTabBackend::new(true);
        backend.set_viewport(800, 600, 2.0);
        backend.start("https://example.com").unwrap();
        backend.set_focus(true);
        backend.navigate("https://example.com/docs");
        backend.go_back();

        assert!(backend.is_started());
        assert_eq!(
            controller.take_commands(),
            vec![
                RecordedCommand::SetViewport {
                    width: 800,
                    height: 600,
                    scale_factor: 2.0,
                },
                RecordedCommand::Start {
                    url: "https://example.com".into(),
                },
                RecordedCommand::SetFocus { focused: true },
                RecordedCommand::Navigate {
                    url: "https://example.com/docs".into(),
                },
                RecordedCommand::GoBack,
            ],
        );
        assert_eq!(controller.commands(), vec![], "take_commands drains");
        assert_eq!(controller.started_with().as_deref(), Some("https://example.com"));
    }

    #[test]
    fn scripted_start_failure_is_consumed_once() {
        let (mut backend, controller) = StubTabBackend::new(true);
        controller.fail_next_start("engine exploded");

        let error = backend.start("https://example.com").unwrap_err();
        assert_eq!(error.to_string(), "engine exploded");
        assert!(!backend.is_started());

        backend.start("https://example.com").unwrap();
        assert!(backend.is_started());
    }

    #[test]
    fn scripted_frame_stages_paint_output_and_announces_it() {
        let (mut backend, controller) = StubTabBackend::new(true);
        controller.script_frame(SoftwareFrame {
            width: 2,
            height: 2,
            bgra: vec![0xff; 16],
        });

        assert_eq!(backend.try_recv_event(), Some(TabBackendEvent::FrameReady));
        assert!(controller.has_staged_frame());
        let output = backend.take_paint_output();
        assert!(matches!(
            output,
            Some(PaintOutput::Software(SoftwareFrame { width: 2, height: 2, .. }))
        ));
        assert!(!controller.has_staged_frame());
        assert!(
            backend.take_paint_output().is_none(),
            "only the latest unpresented frame is handed out"
        );
    }
}
