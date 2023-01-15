use std::time::Duration;

use smithay::{
    backend::{
        input::KeyState,
        renderer::{
            element::{surface::WaylandSurfaceRenderElement, AsRenderElements},
            ImportAll, Renderer,
        },
    },
    desktop::{
        utils::{send_frames_surface_tree, under_from_surface_tree},
        Window as WaylandWindow, WindowSurfaceType,
    },
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerTarget},
        Seat,
    },
    output::Output,
    reexports::wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface},
    space_elements,
    utils::{Logical, Physical, Point, Serial},
    wayland::{compositor::SurfaceData, seat::WaylandFocus, shell::xdg::ToplevelSurface},
    xwayland::X11Surface,
};

space_elements! {
    #[derive(Debug, Clone, PartialEq)]
    pub Window;
    Wayland=WaylandWindow,
    X11=X11Surface,
}

impl From<ToplevelSurface> for Window {
    fn from(s: ToplevelSurface) -> Self {
        Window::Wayland(WaylandWindow::new(s))
    }
}

impl KeyboardTarget<super::State> for Window {
    fn enter(
        &self,
        seat: &Seat<super::State>,
        data: &mut super::State,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            Window::Wayland(w) => KeyboardTarget::enter(w, seat, data, keys, serial),
            Window::X11(w) => KeyboardTarget::enter(w, seat, data, keys, serial),
            _ => unreachable!(),
        }
    }

    fn leave(&self, seat: &Seat<super::State>, data: &mut super::State, serial: Serial) {
        match self {
            Window::Wayland(w) => KeyboardTarget::leave(w, seat, data, serial),
            Window::X11(w) => KeyboardTarget::leave(w, seat, data, serial),
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
            Window::Wayland(w) => w.key(seat, data, key, state, serial, time),
            Window::X11(w) => w.key(seat, data, key, state, serial, time),
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
            Window::Wayland(w) => w.modifiers(seat, data, modifiers, serial),
            Window::X11(w) => w.modifiers(seat, data, modifiers, serial),
            _ => unreachable!(),
        }
    }
}

impl PointerTarget<super::State> for Window {
    fn enter(&self, seat: &Seat<super::State>, data: &mut super::State, event: &MotionEvent) {
        match self {
            Window::Wayland(w) => PointerTarget::enter(w, seat, data, event),
            Window::X11(w) => PointerTarget::enter(w, seat, data, event),
            _ => unreachable!(),
        }
    }

    fn motion(&self, seat: &Seat<super::State>, data: &mut super::State, event: &MotionEvent) {
        match self {
            Window::Wayland(w) => w.motion(seat, data, event),
            Window::X11(w) => w.motion(seat, data, event),
            _ => unreachable!(),
        }
    }

    fn button(&self, seat: &Seat<super::State>, data: &mut super::State, event: &ButtonEvent) {
        match self {
            Window::Wayland(w) => w.button(seat, data, event),
            Window::X11(w) => w.button(seat, data, event),
            _ => unreachable!(),
        }
    }

    fn axis(&self, seat: &Seat<super::State>, data: &mut super::State, frame: AxisFrame) {
        match self {
            Window::Wayland(w) => w.axis(seat, data, frame),
            Window::X11(w) => w.axis(seat, data, frame),
            _ => unreachable!(),
        }
    }

    fn leave(&self, seat: &Seat<super::State>, data: &mut super::State, serial: Serial, time: u32) {
        match self {
            Window::Wayland(w) => PointerTarget::leave(w, seat, data, serial, time),
            Window::X11(w) => PointerTarget::leave(w, seat, data, serial, time),
            _ => unreachable!(),
        }
    }
}

impl WaylandFocus for Window {
    fn wl_surface(&self) -> Option<WlSurface> {
        match self {
            Window::Wayland(w) => w.wl_surface(),
            Window::X11(w) => w.wl_surface(),
            _ => unreachable!(),
        }
    }

    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            Window::Wayland(w) => w.same_client_as(object_id),
            Window::X11(w) => w.same_client_as(object_id),
            _ => unreachable!(),
        }
    }
}

impl<R> AsRenderElements<R> for Window
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    type RenderElement = WaylandSurfaceRenderElement<R>;
    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: smithay::utils::Scale<f64>,
    ) -> Vec<C> {
        match self {
            Window::Wayland(w) => w.render_elements(renderer, location, scale),
            Window::X11(s) => s.render_elements(renderer, location, scale),
            _ => unreachable!(),
        }
    }
}

impl Window {
    pub fn on_commit(&self) {
        match self {
            Window::Wayland(w) => w.on_commit(),
            _ => {}
        }
    }

    pub fn send_frame<T, F>(
        &self,
        output: &Output,
        time: T,
        throttle: Option<Duration>,
        primary_scan_out_output: F,
    ) where
        T: Into<Duration>,
        F: FnMut(&WlSurface, &SurfaceData) -> Option<Output> + Copy,
    {
        match self {
            Window::Wayland(w) => w.send_frame(output, time, throttle, primary_scan_out_output),
            Window::X11(s) => {
                if let Some(surface) = s.wl_surface() {
                    send_frames_surface_tree(
                        &surface,
                        output,
                        time,
                        throttle,
                        primary_scan_out_output,
                    )
                }
            }
            _ => unreachable!(),
        }
    }

    pub fn surface_under<P: Into<Point<f64, Logical>>>(
        &self,
        point: P,
        surface_type: WindowSurfaceType,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        match self {
            Window::Wayland(w) => w.surface_under(point, surface_type),
            Window::X11(w) => {
                if let Some(wl_surface) = w.wl_surface() {
                    under_from_surface_tree(&wl_surface, point.into(), (0, 0), surface_type)
                } else {
                    None
                }
            }
            _ => unreachable!(),
        }
    }
}
