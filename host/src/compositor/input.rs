use smithay::{
    backend::{
        input::{
            Axis,
            Event,
            InputEvent,
            KeyboardKeyEvent,
            PointerMotionEvent,
            PointerButtonEvent,
            PointerAxisEvent,
        },
        libinput::LibinputInputBackend,
    },
    desktop::WindowSurfaceType,
    reexports::{
        input::LibinputInterface,
        nix::{fcntl, fcntl::OFlag, sys::stat, unistd::close},
        wayland_server::{
            DisplayHandle,
            protocol::wl_pointer,
        },
    },
    wayland::{
        SERIAL_COUNTER,
        Serial,
        seat::{
            FilterResult,
            MotionEvent,
            ButtonEvent,
            AxisFrame,
        },
    },
    utils::{Point, Logical},
};
use std::{
    path::Path,
    os::unix::io::RawFd,
};
use super::State;

pub struct NixInterface;

impl LibinputInterface for NixInterface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<RawFd, i32> {
        fcntl::open(path, OFlag::from_bits_truncate(flags), stat::Mode::empty()).map_err(|err| err as i32)
    }
    fn close_restricted(&mut self, fd: RawFd) {
        if let Err(err) = close(fd) {
            slog_scope::warn!("Failed to close fd: {}", err);
        }
    }
}

impl State {
    pub fn process_input_event(&mut self, dh: &DisplayHandle, event: InputEvent<LibinputInputBackend>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let keycode = event.key_code();
                let state = event.state();
                let serial = SERIAL_COUNTER.next_serial();
                let time = event.time();
                let keyboard = self.seat.get_keyboard().unwrap();

                keyboard.input::<(), _>(dh, keycode, state, serial, time, |_modifiers, _handle| {
                    FilterResult::Forward 
                });
            },
            InputEvent::PointerMotion { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                self.pointer_location += event.delta();
                self.pointer_location = self.clamp_coords(self.pointer_location);

                let pointer = self.seat.get_pointer().unwrap();
                let under = self.space.surface_under(self.pointer_location, WindowSurfaceType::ALL);
                pointer.motion(
                    self,
                    dh,
                    &MotionEvent {
                        location: self.pointer_location,
                        focus: under.map(|(w, _, pos)| (
                            w.toplevel().wl_surface().clone(),
                            pos,
                        )),
                        serial,
                        time: event.time(),
                    }
                );
            },
            InputEvent::PointerButton { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();

                let state = wl_pointer::ButtonState::from(event.state());
                if wl_pointer::ButtonState::Pressed == state {
                    self.update_keyboard_focus(dh, serial);
                };
                self.seat.get_pointer().unwrap().button(
                    self,
                    dh,
                    &ButtonEvent {
                        button,
                        state,
                        serial,
                        time: event.time(),
                    },
                );
            },
            InputEvent::PointerAxis { event, .. } => {
                let source = wl_pointer::AxisSource::from(event.source());

                let horizontal_amount = event
                    .amount(Axis::Horizontal)
                    .unwrap_or_else(|| event.amount_discrete(Axis::Horizontal).unwrap() * 2.0);
                let vertical_amount = event
                    .amount(Axis::Vertical)
                    .unwrap_or_else(|| event.amount_discrete(Axis::Vertical).unwrap() * 2.0);
                let horizontal_amount_discrete = event.amount_discrete(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_discrete(Axis::Vertical);

                {
                    let mut frame = AxisFrame::new(event.time()).source(source);
                    if horizontal_amount != 0.0 {
                        frame = frame.value(wl_pointer::Axis::HorizontalScroll, horizontal_amount);
                        if let Some(discrete) = horizontal_amount_discrete {
                            frame = frame.discrete(wl_pointer::Axis::HorizontalScroll, discrete as i32);
                        }
                    } else if source == wl_pointer::AxisSource::Finger {
                        frame = frame.stop(wl_pointer::Axis::HorizontalScroll);
                    }
                    if vertical_amount != 0.0 {
                        frame = frame.value(wl_pointer::Axis::VerticalScroll, vertical_amount);
                        if let Some(discrete) = vertical_amount_discrete {
                            frame = frame.discrete(wl_pointer::Axis::VerticalScroll, discrete as i32);
                        }
                    } else if source == wl_pointer::AxisSource::Finger {
                        frame = frame.stop(wl_pointer::Axis::VerticalScroll);
                    }
                    self.seat.get_pointer().unwrap().axis(self, dh, frame);
                }
            },
            _ => {},
        }
    }
    
    fn clamp_coords(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        if let Some(mode) = self.output.current_mode() {
            (
                pos.x.max(0.0).min(mode.size.w as f64),
                pos.y.max(0.0).min(mode.size.h as f64),
            ).into()
        } else {
            pos
        }
    }

    fn update_keyboard_focus(&mut self, dh: &DisplayHandle, serial: Serial) {
        let pointer = self.seat.get_pointer().unwrap();
        let keyboard = self.seat.get_keyboard().unwrap();
        // change the keyboard focus unless the pointer or keyboard is grabbed
        // We test for any matching surface type here but always use the root
        // (in case of a window the toplevel) surface for the focus.
        // So for example if a user clicks on a subsurface or popup the toplevel
        // will receive the keyboard focus. Directly assigning the focus to the
        // matching surface leads to issues with clients dismissing popups and
        // subsurface menus (for example firefox-wayland).
        // see here for a discussion about that issue:
        // https://gitlab.freedesktop.org/wayland/wayland/-/issues/294
        if !pointer.is_grabbed() && !keyboard.is_grabbed() {
            if let Some((window, _, _)) = self
                .space
                .surface_under(self.pointer_location, WindowSurfaceType::ALL)
            {
                self.space.raise_window(&window, true);
                keyboard.set_focus(dh, Some(window.toplevel().wl_surface()), serial);
                return;
            }
        }
    }
}
