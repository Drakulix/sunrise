use smithay::{
    backend::input::KeyState,
    desktop::{PopupKind, Window as WaylandWindow},
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerTarget},
        Seat,
    },
    reexports::wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface},
    utils::{IsAlive, Serial},
    wayland::seat::WaylandFocus,
    xwayland::X11Surface,
};

#[derive(Debug, Clone, PartialEq)]
pub enum FocusTarget {
    Wayland(WaylandWindow),
    X11(X11Surface),
    Popup(PopupKind),
}

impl IsAlive for FocusTarget {
    fn alive(&self) -> bool {
        match self {
            FocusTarget::Wayland(w) => w.alive(),
            FocusTarget::X11(w) => w.alive(),
            FocusTarget::Popup(p) => p.alive(),
        }
    }
}

impl From<super::Window> for FocusTarget {
    fn from(w: super::Window) -> Self {
        match w {
            super::Window::Wayland(w) => FocusTarget::Wayland(w),
            super::Window::X11(s) => FocusTarget::X11(s),
            _ => unreachable!(),
        }
    }
}

impl From<PopupKind> for FocusTarget {
    fn from(p: PopupKind) -> Self {
        FocusTarget::Popup(p)
    }
}

impl KeyboardTarget<super::State> for FocusTarget {
    fn enter(
        &self,
        seat: &Seat<super::State>,
        data: &mut super::State,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            FocusTarget::Wayland(w) => KeyboardTarget::enter(w, seat, data, keys, serial),
            FocusTarget::X11(w) => KeyboardTarget::enter(w, seat, data, keys, serial),
            _ => unreachable!(),
        }
    }

    fn leave(&self, seat: &Seat<super::State>, data: &mut super::State, serial: Serial) {
        match self {
            FocusTarget::Wayland(w) => KeyboardTarget::leave(w, seat, data, serial),
            FocusTarget::X11(w) => KeyboardTarget::leave(w, seat, data, serial),
            _ => unreachable!(),
        }
    }

    fn key(
        &self,
        seat: &Seat<super::State>,
        data: &mut super::State,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match self {
            FocusTarget::Wayland(w) => w.key(seat, data, key, state, serial, time),
            FocusTarget::X11(w) => w.key(seat, data, key, state, serial, time),
            _ => unreachable!(),
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<super::State>,
        data: &mut super::State,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            FocusTarget::Wayland(w) => w.modifiers(seat, data, modifiers, serial),
            FocusTarget::X11(w) => w.modifiers(seat, data, modifiers, serial),
            _ => unreachable!(),
        }
    }
}

impl PointerTarget<super::State> for FocusTarget {
    fn enter(&self, seat: &Seat<super::State>, data: &mut super::State, event: &MotionEvent) {
        match self {
            FocusTarget::Wayland(w) => PointerTarget::enter(w, seat, data, event),
            FocusTarget::X11(w) => PointerTarget::enter(w, seat, data, event),
            _ => unreachable!(),
        }
    }

    fn motion(&self, seat: &Seat<super::State>, data: &mut super::State, event: &MotionEvent) {
        match self {
            FocusTarget::Wayland(w) => w.motion(seat, data, event),
            FocusTarget::X11(w) => w.motion(seat, data, event),
            _ => unreachable!(),
        }
    }

    fn button(&self, seat: &Seat<super::State>, data: &mut super::State, event: &ButtonEvent) {
        match self {
            FocusTarget::Wayland(w) => w.button(seat, data, event),
            FocusTarget::X11(w) => w.button(seat, data, event),
            _ => unreachable!(),
        }
    }

    fn axis(&self, seat: &Seat<super::State>, data: &mut super::State, frame: AxisFrame) {
        match self {
            FocusTarget::Wayland(w) => w.axis(seat, data, frame),
            FocusTarget::X11(w) => w.axis(seat, data, frame),
            _ => unreachable!(),
        }
    }

    fn leave(&self, seat: &Seat<super::State>, data: &mut super::State, serial: Serial, time: u32) {
        match self {
            FocusTarget::Wayland(w) => PointerTarget::leave(w, seat, data, serial, time),
            FocusTarget::X11(w) => PointerTarget::leave(w, seat, data, serial, time),
            _ => unreachable!(),
        }
    }
}

impl WaylandFocus for FocusTarget {
    fn wl_surface(&self) -> Option<WlSurface> {
        match self {
            FocusTarget::Wayland(w) => w.wl_surface(),
            FocusTarget::X11(w) => w.wl_surface(),
            _ => unreachable!(),
        }
    }

    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            FocusTarget::Wayland(w) => w.same_client_as(object_id),
            FocusTarget::X11(w) => w.same_client_as(object_id),
            _ => unreachable!(),
        }
    }
}
