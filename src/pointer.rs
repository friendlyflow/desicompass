//! The mouse, the touchpad and the touch screen: libinput's events (or the nested
//! window's) handed to the client under them.
//!
//! Both backends call [`handle`] with every input event that is not a key. The
//! pointer moves over the whole output and goes to the surface under it (a
//! window, the superkey, the bar). A press on a window that does not have the
//! keyboard gives it the keyboard, as Super+J and Super+K do. A touch goes to the
//! surface under the finger that started it, and focuses its window the same way.
//! No cursor is drawn for a touch.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        PointerAxisEvent, PointerButtonEvent, PointerMotionEvent, TouchEvent,
    },
    desktop::WindowSurfaceType,
    input::{
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
        touch::{DownEvent, MotionEvent as TouchMotionEvent, UpEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::state::State;

impl State {
    /// The surface at `pos` on the output, and where its origin is.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        let (window, loc) = self.space.element_under(pos)?;
        window
            .surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL)
            .map(|(surface, at)| (surface, (at + loc).to_f64()))
    }

    /// `pos`, kept on the output.
    fn clamp_to_output(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let size = self.output_size();
        (
            pos.x.clamp(0.0, f64::from(size.w - 1)),
            pos.y.clamp(0.0, f64::from(size.h - 1)),
        )
            .into()
    }

    /// A press at `pos`: the window there gets the keyboard, if it is one the
    /// layout holds and not the one that has it. The superkey and the bar are not
    /// focused this way: the superkey has the keyboard while it shows, and the bar
    /// never takes it.
    fn focus_window_at(&mut self, pos: Point<f64, Logical>) {
        let Some((window, _)) = self.space.element_under(pos) else {
            return;
        };
        let Some(surface) = window.toplevel().map(|t| t.wl_surface().clone()) else {
            return;
        };
        let Some(id) = self.id_for_surface(&surface) else {
            return;
        };
        if self.focus.focused() != Some(id) {
            self.focus_id(id);
        }
    }

    fn pointer_moved(&mut self, pos: Point<f64, Logical>, time: smithay::backend::input::InputTime) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let pos = self.clamp_to_output(pos);
        self.cursor_shown = true;
        let under = self.surface_under(pos);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }
}

/// Handle `event` if it is the pointer's or a touch. `false` for anything else
/// (a key, a device coming or going), which the caller handles.
pub fn handle<B: InputBackend>(state: &mut State, event: InputEvent<B>) -> bool {
    match event {
        InputEvent::PointerMotion { event } => {
            let Some(pointer) = state.seat.get_pointer() else {
                return true;
            };
            let pos = pointer.current_location() + event.delta();
            state.pointer_moved(pos, event.time());
        }
        InputEvent::PointerMotionAbsolute { event } => {
            let pos = event.position_transformed(state.output_size());
            state.pointer_moved(pos, event.time());
        }
        InputEvent::PointerButton { event } => {
            let Some(pointer) = state.seat.get_pointer() else {
                return true;
            };
            let pressed = event.state() == ButtonState::Pressed;
            if pressed {
                state.cursor_shown = true;
                state.focus_window_at(pointer.current_location());
            }
            pointer.button(
                state,
                &ButtonEvent {
                    button: event.button_code(),
                    state: event.state(),
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time(),
                },
            );
            pointer.frame(state);
        }
        InputEvent::PointerAxis { event } => {
            let Some(pointer) = state.seat.get_pointer() else {
                return true;
            };
            let mut frame = AxisFrame::new(event.time()).source(event.source());
            for axis in [Axis::Horizontal, Axis::Vertical] {
                let v120 = event.amount_v120(axis);
                let amount = event
                    .amount(axis)
                    .unwrap_or_else(|| v120.unwrap_or(0.0) * 15.0 / 120.0);
                if amount != 0.0 {
                    frame = frame
                        .relative_direction(axis, event.relative_direction(axis))
                        .value(axis, amount);
                    if let Some(discrete) = v120 {
                        frame = frame.v120(axis, discrete as i32);
                    }
                }
                // A touchpad's fingers lifting: the scroll stops.
                if event.source() == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                    frame = frame.stop(axis);
                }
            }
            pointer.axis(state, frame);
            pointer.frame(state);
        }
        InputEvent::TouchDown { event } => {
            let Some(touch) = state.seat.get_touch() else {
                return true;
            };
            let pos = event.position_transformed(state.output_size());
            state.focus_window_at(pos);
            let under = state.surface_under(pos);
            touch.down(
                state,
                under,
                &DownEvent {
                    slot: event.slot(),
                    location: pos,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time(),
                },
            );
        }
        InputEvent::TouchMotion { event } => {
            let Some(touch) = state.seat.get_touch() else {
                return true;
            };
            let pos = event.position_transformed(state.output_size());
            let under = state.surface_under(pos);
            touch.motion(
                state,
                under,
                &TouchMotionEvent {
                    slot: event.slot(),
                    location: pos,
                    time: event.time(),
                },
            );
        }
        InputEvent::TouchUp { event } => {
            let Some(touch) = state.seat.get_touch() else {
                return true;
            };
            touch.up(
                state,
                &UpEvent {
                    slot: event.slot(),
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time(),
                },
            );
        }
        InputEvent::TouchFrame { .. } => {
            if let Some(touch) = state.seat.get_touch() {
                touch.frame(state);
            }
        }
        InputEvent::TouchCancel { .. } => {
            if let Some(touch) = state.seat.get_touch() {
                touch.cancel(state);
            }
        }
        _ => return false,
    }
    true
}
