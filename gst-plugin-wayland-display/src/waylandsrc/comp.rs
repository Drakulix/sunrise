use std::{
    os::unix::prelude::{AsRawFd, OwnedFd},
    path::PathBuf,
    sync::mpsc::SyncSender,
};

use super::imp::Command;
use slog::Drain;
use smithay::{
    backend::{
        allocator::{
            dmabuf::{AsDmabuf, Dmabuf},
            gbm::GbmDevice,
            Fourcc, Swapchain,
        },
        drm::{DrmDeviceFd, DrmNode, NodeType},
        egl::{EGLContext, EGLDisplay},
        libinput::LibinputInputBackend,
        renderer::{
            damage::{DamageTrackedRenderer, DamageTrackedRendererError as DTRError},
            element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            gles2::Gles2Renderer,
            utils::{import_surface_tree, on_commit_buffer_handler},
            Bind, ExportMem, ImportDma, ImportMemWl, Unbind,
        },
    },
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_output, delegate_seat,
    delegate_shm, delegate_viewporter, delegate_xdg_shell,
    desktop::{
        find_popup_root_surface, space::render_output, PopupKeyboardGrab, PopupKind, PopupManager,
        PopupPointerGrab, PopupUngrabStrategy, Space,
    },
    input::{keyboard::XkbConfig, pointer::Focus, Seat, SeatHandler, SeatState},
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            channel::{Channel, Event},
            generic::Generic,
            EventLoop, Interest, LoopHandle, Mode, PostAction,
        },
        input::Libinput,
        wayland_protocols::xdg::shell::server::xdg_toplevel::State as XdgState,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer::WlBuffer, wl_seat::WlSeat, wl_surface::WlSurface},
            Display, DisplayHandle, Resource,
        },
    },
    utils::{DeviceFd, Logical, Physical, Point, Rectangle, Serial, Size, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{with_states, CompositorHandler, CompositorState},
        data_device::{
            set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
            ServerDndGrabHandler,
        },
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportError},
        output::OutputManagerState,
        seat::WaylandFocus,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgPopupSurfaceData, XdgShellHandler,
            XdgShellState, XdgToplevelSurfaceData,
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
    },
    xwayland::{
        xwm::{WmWindowType, XwmId},
        X11Surface, XWayland, XWaylandEvent, XwmHandler, X11WM,
    },
};

mod focus;
mod input;
mod window;

use self::focus::*;
use self::input::*;
use self::window::*;

const CURSOR_DATA_BYTES: &[u8] = include_bytes!("./comp/cursor.rgba");

struct ClientState;
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

struct Data {
    display: Display<State>,
    state: State,
}

#[allow(dead_code)]
struct State {
    handle: LoopHandle<'static, Data>,
    should_quit: bool,
    start_time: std::time::Instant,
    log: slog::Logger,

    // render
    egl: EGLDisplay,
    dtr: DamageTrackedRenderer,
    renderer: Gles2Renderer,
    dmabuf_global: DmabufGlobal,
    swapchain: Swapchain<GbmDevice<DrmDeviceFd>>,
    direct_scanout: bool,

    // management
    output: Output,
    seat: Seat<Self>,
    space: Space<Window>,
    popups: PopupManager,
    pointer_location: Point<f64, Logical>,
    cursor_element: MemoryRenderBuffer,
    pending_windows: Vec<Window>,

    // wayland state
    dh: DisplayHandle,
    compositor_state: CompositorState,
    data_device_state: DataDeviceState,
    dmabuf_state: DmabufState,
    output_state: OutputManagerState,
    seat_state: SeatState<Self>,
    shell_state: XdgShellState,
    shm_state: ShmState,
    viewporter_state: ViewporterState,
    xwm: Option<X11WM>,
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        X11WM::commit_hook(surface);
        on_commit_buffer_handler(surface);
        if let Err(err) = import_surface_tree(&mut self.renderer, surface, &self.log) {
            slog::warn!(self.log, "Failed to load client buffer: {}", err);
        }

        if let Some(window) = self
            .space
            .elements()
            .find(|w| w.wl_surface().as_ref() == Some(surface))
        {
            window.on_commit();
        }
        self.popups.commit(surface);

        // send the initial configure if relevant
        if let Some(idx) = self
            .pending_windows
            .iter_mut()
            .position(|w| w.wl_surface().as_ref() == Some(surface))
        {
            let Window::Wayland(window) = self.pending_windows.swap_remove(idx) else {
                return;
            };

            let toplevel = window.toplevel();
            let (initial_configure_sent, max_size) = with_states(surface, |states| {
                let attributes = states.data_map.get::<XdgToplevelSurfaceData>().unwrap();
                let attributes_guard = attributes.lock().unwrap();

                (
                    attributes_guard.initial_configure_sent,
                    attributes_guard.max_size,
                )
            });
            if !initial_configure_sent {
                if max_size.w == 0 && max_size.h == 0 {
                    toplevel.with_pending_state(|state| {
                        state.size = Some(
                            self.output
                                .current_mode()
                                .unwrap()
                                .size
                                .to_f64()
                                .to_logical(self.output.current_scale().fractional_scale())
                                .to_i32_round(),
                        );
                        state.states.set(XdgState::Fullscreen);
                    });
                }
                toplevel.with_pending_state(|state| {
                    state.states.set(XdgState::Activated);
                });
                toplevel.send_configure();
                self.pending_windows.push(Window::Wayland(window));
            } else {
                let window_size = toplevel.current_state().size.unwrap_or((0, 0).into());
                let output_size: Size<i32, _> = self
                    .output
                    .current_mode()
                    .unwrap()
                    .size
                    .to_f64()
                    .to_logical(self.output.current_scale().fractional_scale())
                    .to_i32_round();
                let loc = (
                    (output_size.w / 2) - (window_size.w / 2),
                    (output_size.h / 2) - (window_size.h / 2),
                );
                self.space.map_element(Window::Wayland(window), loc, false);
            }

            return;
        }

        if let Some(popup) = self.popups.find_popup(surface) {
            let PopupKind::Xdg(ref popup) = popup;
            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<XdgPopupSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .initial_configure_sent
            });
            if !initial_configure_sent {
                // NOTE: This should never fail as the initial configure is always
                // allowed.
                popup.send_configure().expect("initial configure failed");
            }

            return;
        };
    }
}

impl ServerDndGrabHandler for State {}
impl ClientDndGrabHandler for State {}
impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
    ) -> Result<(), ImportError> {
        self.renderer
            .import_dmabuf(&dmabuf, None)
            .map(|_| ())
            .map_err(|_| ImportError::Failed)
    }
}

impl SeatHandler for State {
    type KeyboardFocus = FocusTarget;
    type PointerFocus = FocusTarget;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focus: Option<&Self::KeyboardFocus>) {
        if let Some(surface) = focus {
            let client = match surface {
                FocusTarget::Wayland(w) => w.toplevel().wl_surface().client(),
                FocusTarget::Popup(p) => p.wl_surface().client(),
                FocusTarget::X11(s) => s.wl_surface().and_then(|s| s.client()),
            };
            set_data_device_focus(&self.dh, seat, client);
        } else {
            set_data_device_focus(&self.dh, seat, None);
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::from(surface);
        self.pending_windows.push(window);
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        // TODO: properly recompute the geometry with the whole of positioner state
        surface.with_pending_state(|state| {
            // NOTE: This is not really necessary as the default geometry
            // is already set the same way, but for demonstrating how
            // to set the initial popup geometry this code is left as
            // an example
            state.geometry = positioner.get_geometry();
        });
        if let Err(err) = self.popups.track_popup(PopupKind::from(surface)) {
            slog::warn!(self.log, "Failed to track popup: {}", err);
        }
    }

    fn grab(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        let seat: Seat<State> = Seat::from_resource(&seat).unwrap();
        let kind = PopupKind::Xdg(surface.clone());
        if let Some(root) = find_popup_root_surface(&kind).ok().and_then(|root| {
            self.space
                .elements()
                .find(|w| w.wl_surface().map(|s| s == root).unwrap_or(false))
                .cloned()
                .map(FocusTarget::from)
        }) {
            let ret = self.popups.grab_popup(root, surface.into(), &seat, serial);
            if let Ok(mut grab) = ret {
                if let Some(keyboard) = seat.get_keyboard() {
                    if keyboard.is_grabbed()
                        && !(keyboard.has_grab(serial)
                            || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    keyboard.set_focus(self, grab.current_grab(), serial);
                    keyboard.set_grab(PopupKeyboardGrab::new(&grab), serial);
                }
                if let Some(pointer) = seat.get_pointer() {
                    if pointer.is_grabbed()
                        && !(pointer.has_grab(serial)
                            || pointer
                                .has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Clear);
                }
            }
        }
    }
}

delegate_compositor!(State);
delegate_data_device!(State);
delegate_dmabuf!(State);
delegate_output!(State);
delegate_seat!(State);
delegate_shm!(State);
delegate_xdg_shell!(State);
delegate_viewporter!(State);

impl State {
    fn create_frame(&mut self) -> Result<Dmabuf, DTRError<Gles2Renderer>> {
        /*
        if let Some(window) = self.space.windows().next() {
            let SurfaceKind::Xdg(xdg) = window.toplevel();
            let surface = xdg.wl_surface();
            if get_children(surface).is_empty() && self.popups.find_popup(surface).is_none() {
                let output_size: Size<i32, _> = self.output.current_mode().unwrap().size
                    .to_f64()
                    .to_logical(self.output.current_scale().fractional_scale())
                    .to_i32_round();
                if let Some(dmabuf) = with_renderer_surface_state(xdg.wl_surface(), |state| {
                    if state.surface_size().map(|size| size == output_size).unwrap_or(false) {
                        state.wl_buffer().and_then(|buf| get_dmabuf(buf).ok())
                    } else {
                        None
                    }
                }) {
                    if !self.direct_scanout {
                        self.swapchain.reset_buffers();
                        self.direct_scanout = true;
                    };
                    return Ok(Capture {
                        dmabuf,
                        presentation_time: std::time::Instant::now(),
                    });
                }
            }
        }
        self.direct_scanout = false;
        */

        let elements = vec![MemoryRenderBufferRenderElement::from_buffer(
            &mut self.renderer,
            self.pointer_location.to_physical_precise_round(1),
            &self.cursor_element,
            None,
            None,
            None,
            None,
        )
        .map_err(DTRError::Rendering)?];

        let offscreen = self
            .swapchain
            .acquire()
            .unwrap()
            .expect("Failed to acquire buffer");
        let age = offscreen.age();

        let mut dmabuf = offscreen.userdata().get::<Dmabuf>().cloned();
        if dmabuf.is_none() {
            let new_dmabuf = offscreen.export().unwrap();
            offscreen
                .userdata()
                .insert_if_missing(|| new_dmabuf.clone());
            dmabuf = Some(new_dmabuf);
        }

        let dmabuf = dmabuf.unwrap();
        self.renderer
            .bind(dmabuf.clone())
            .map_err(DTRError::Rendering)?;
        render_output(
            &self.output,
            &mut self.renderer,
            age as usize,
            [&self.space],
            &*elements,
            &mut self.dtr,
            [0.0, 0.0, 0.0, 1.0],
            self.log.clone(),
        )?;
        self.renderer.unbind().map_err(DTRError::Rendering)?;
        Ok(dmabuf)
    }
}

impl XwmHandler for Data {
    fn xwm_state(&mut self, _: XwmId) -> &mut X11WM {
        self.state.xwm.as_mut().unwrap()
    }

    fn new_window(&mut self, _: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, id: XwmId, window: X11Surface) {
        if !matches!(
            window.window_type(),
            None | Some(WmWindowType::Normal)
                | Some(WmWindowType::Utility)
                | Some(WmWindowType::Splash)
        ) {
            let geo = window.geometry();
            let _ = window.set_mapped(true);
            let _ = window.set_activated(true);
            let _ = window.configure(geo);
            let _ = self.xwm_state(id).raise_window(&window);
            self.state
                .space
                .map_element(Window::from(window), geo.loc, true);
            return;
        }

        let output_geo = Rectangle::from_loc_and_size(
            (0, 0),
            self.state
                .output
                .current_mode()
                .unwrap()
                .size
                .to_f64()
                .to_logical(self.state.output.current_scale().fractional_scale())
                .to_i32_round(),
        );

        let window_size = if window.window_type() == Some(WmWindowType::Splash) {
            // don't resize splashes
            window.geometry().size
        } else {
            // if max_size doesn't prohibit it, give it the full output by default
            window
                .max_size()
                .map(|size| Rectangle::from_loc_and_size((0, 0), size))
                .unwrap_or(output_geo)
                .intersection(output_geo)
                .unwrap()
                .size
        };
        // center it
        let window_loc = (
            (output_geo.size.w / 2) - (window_size.w / 2),
            (output_geo.size.h / 2) - (window_size.h / 2),
        );

        let _ = window.set_mapped(true);
        if window.window_type() != Some(WmWindowType::Splash) {
            let _ = window.set_fullscreen(true);
        }
        let _ = window.set_activated(true);
        let _ = window.configure(Rectangle::from_loc_and_size(window_loc, window_size));
        let _ = self.xwm_state(id).raise_window(&window);
        self.state
            .space
            .map_element(Window::X11(window), window_loc, true);
    }

    fn mapped_override_redirect_window(&mut self, _: XwmId, window: X11Surface) {
        let geo = window.geometry();
        self.state
            .space
            .map_element(Window::from(window), geo.loc, true);
    }

    fn unmapped_window(&mut self, _: XwmId, window: X11Surface) {
        let maybe = self
            .state
            .space
            .elements()
            .find(|e| matches!(e, Window::X11(w) if w == &window))
            .cloned();
        if let Some(elem) = maybe {
            self.state.space.unmap_elem(&elem)
        }
        if !window.is_override_redirect() {
            window.set_mapped(false).unwrap();
        }
    }

    fn destroyed_window(&mut self, _: XwmId, _window: X11Surface) {}

    fn configure_request(
        &mut self,
        _: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
    ) {
        let mut geo = window.geometry();
        /*
        if let Some(x) = x {
            geo.loc.x = x;
        }
        if let Some(y) = y {
            geo.loc.y = y;
        }
        */
        if let Some(w) = w {
            geo.size.w = w as i32;
        }
        if let Some(h) = h {
            geo.size.h = h as i32;
        }
        let _ = window.configure(geo);

        /*
        let Some(elem) = self
            .state
            .space
            .elements()
            .find(|e| matches!(e, Window::X11(w) if w == &window))
            .cloned()
        else { return };
        self.state.space.map_element(elem, geo.loc, false);
        */
    }

    fn configure_notify(&mut self, _: XwmId, window: X11Surface, x: i32, y: i32, _w: u32, _h: u32) {
        if window.is_override_redirect() {
            let Some(elem) = self
                .state
                .space
                .elements()
                .find(|e| matches!(e, Window::X11(w) if w == &window))
                .cloned()
            else { return };
            self.state.space.map_element(elem, (x, y), false);
        }
    }

    fn resize_request(
        &mut self,
        _: XwmId,
        _window: X11Surface,
        _button: u32,
        _resize_edge: smithay::xwayland::xwm::ResizeEdge,
    ) {
    }
    fn move_request(&mut self, _: XwmId, _window: X11Surface, _button: u32) {}

    fn fullscreen_request(&mut self, id: XwmId, window: X11Surface) {
        let maybe = self
            .state
            .space
            .elements()
            .find(|e| matches!(e, Window::X11(w) if w == &window))
            .cloned();
        if let Some(elem) = maybe {
            let _ = window.set_fullscreen(true);

            let output_geo = Rectangle::from_loc_and_size(
                (0, 0),
                self.state
                    .output
                    .current_mode()
                    .unwrap()
                    .size
                    .to_f64()
                    .to_logical(self.state.output.current_scale().fractional_scale())
                    .to_i32_round(),
            );
            let window_geo = window.geometry();
            if window_geo != output_geo {
                let _ = window.configure(output_geo);
                let _ = self.xwm_state(id).raise_window(&window);
                self.state.space.map_element(elem, (0, 0), true);
            }
        }
    }
    fn unfullscreen_request(&mut self, _: XwmId, window: X11Surface) {
        let _ = window.set_fullscreen(false);
    }
}

pub fn init(
    buffer_tx: SyncSender<gst::Buffer>,
    command_src: Channel<Command>,
    node: impl Into<PathBuf>,
    seat: impl AsRef<str>,
    info: gst_video::VideoInfo,
) {
    let log = ::slog::Logger::root(super::imp::SlogGstDrain.fuse(), slog::o!());

    let mut display = Display::<State>::new().unwrap();
    let dh = display.handle();

    let node = node.into();
    let size: Size<i32, Physical> = (info.width() as i32, info.height() as i32).into();
    let framerate = info.fps().numer();

    // init state
    let compositor_state = CompositorState::new::<State, _>(&dh, log.clone());
    let data_device_state = DataDeviceState::new::<State, _>(&dh, log.clone());
    let mut dmabuf_state = DmabufState::new();
    let output_state = OutputManagerState::new_with_xdg_output::<State>(&dh);
    let mut seat_state = SeatState::new();
    let shell_state = XdgShellState::new::<State, _>(&dh, log.clone());
    let viewporter_state = ViewporterState::new::<State, _>(&dh, log.clone());

    // init render backend
    let drm_node = DrmNode::from_path(&node).expect("Invalid render node path");
    let drm_file = std::fs::File::open(
        drm_node
            .dev_path_with_type(NodeType::Render)
            .or_else(|| drm_node.dev_path())
            .unwrap_or_else(|| node),
    )
    .expect("Failed to open drm device");

    // GBM device code path
    let drm_fd = DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(drm_file)), None);
    let gbm_device = GbmDevice::new(drm_fd).expect("Failed to open gbm device");

    let egl =
        EGLDisplay::new(gbm_device.clone(), log.clone()).expect("Failed to create EGLDisplay");
    let context = EGLContext::new(&egl, log.clone()).expect("Failed to create EGLContext");

    let modifiers = context
        .dmabuf_texture_formats()
        .into_iter()
        .filter(|x| x.code == Fourcc::Xrgb8888)
        .map(|x| x.modifier)
        .collect();
    let swapchain = Swapchain::new(
        gbm_device,
        size.w as u32,
        size.h as u32,
        Fourcc::Xrgb8888,
        modifiers,
    );
    let renderer =
        unsafe { Gles2Renderer::new(context, log.clone()) }.expect("Failed to initialize renderer");
    let formats = Bind::<Dmabuf>::supported_formats(&renderer)
        .expect("Failed to query formats")
        .into_iter()
        .collect::<Vec<_>>();

    // shm buffer
    let shm_state = ShmState::new::<State, _>(&dh, Vec::from(renderer.shm_formats()), log.clone());
    // egl buffer
    let _egl_guard = egl.bind_wl_display(&dh).expect("Failed to bind EGLDisplay");
    // dma buffer
    let dmabuf_global = dmabuf_state.create_global::<State, _>(&dh, formats.clone(), log.clone());

    let cursor_element =
        MemoryRenderBuffer::from_memory(CURSOR_DATA_BYTES, (64, 64), 1, Transform::Normal, None);

    // init input backend
    let mut libinput_context = Libinput::new_with_udev(NixInterface::new(log.clone()));
    libinput_context
        .udev_assign_seat(seat.as_ref())
        .expect("Failed to assign libinput seat");
    let libinput_backend = LibinputInputBackend::new(libinput_context, log.clone());

    // init wayland objects
    let output = Output::new(
        "HEADLESS-1".into(),
        PhysicalProperties {
            make: "Virtual".into(),
            model: "Sunrise".into(),
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
        },
        log.clone(),
    );
    let mode = OutputMode {
        size: size.into(),
        refresh: (framerate * 1000) as i32,
    };
    output.change_current_state(Some(mode), None, None, None);
    output.set_preferred(mode);
    let dtr = DamageTrackedRenderer::from_output(&output);

    let mut space = Space::new(log.clone());
    space.map_output(&output, (0, 0));

    let mut seat = seat_state.new_wl_seat(&dh, "seat-0", log.clone());
    seat.add_keyboard(XkbConfig::default(), 200, 25)
        .expect("Failed to add keyboard to seat");
    seat.add_pointer();

    let mut event_loop = EventLoop::<Data>::try_new().expect("Unable to create event_loop");
    let state = State {
        handle: event_loop.handle(),
        should_quit: false,
        start_time: std::time::Instant::now(),
        log: log.clone(),

        egl,
        renderer,
        dtr,
        dmabuf_global,
        swapchain,
        direct_scanout: false,

        space,
        popups: PopupManager::new(log.clone()),
        output,
        seat,
        pointer_location: (size.w as f64 / 2.0, size.h as f64 / 2.0).into(),
        cursor_element,
        pending_windows: Vec::new(),

        dh: display.handle(),
        compositor_state,
        data_device_state,
        dmabuf_state,
        output_state,
        seat_state,
        shell_state,
        shm_state,
        viewporter_state,
        xwm: None,
    };

    // init event loop
    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, data| {
            data.state.process_input_event(event)
        })
        .unwrap();
    event_loop
        .handle()
        .insert_source(command_src, move |event, _, data| {
            match event {
                Event::Msg(Command::Quit) | Event::Closed => {
                    data.state.should_quit = true;
                }
            };
        })
        .unwrap();

    let source = ListeningSocketSource::new_auto(log.clone()).unwrap();
    slog::info!(
        log,
        "Listening on wayland socket: {}",
        source.socket_name().to_string_lossy()
    );
    event_loop
        .handle()
        .insert_source(source, |client_stream, _, data| {
            if let Err(err) = data
                .display
                .handle()
                .insert_client(client_stream, std::sync::Arc::new(ClientState))
            {
                slog::error!(data.state.log, "Error adding wayland client: {}", err);
            };
        })
        .expect("Failed to init wayland socket source");

    event_loop
        .handle()
        .insert_source(
            Generic::new(
                display.backend().poll_fd().as_raw_fd(),
                Interest::READ,
                Mode::Level,
            ),
            |_, _, data| {
                data.display.dispatch_clients(&mut data.state).unwrap();
                Ok(PostAction::Continue)
            },
        )
        .expect("Failed to init wayland server source");

    // startup xwayland
    let _xwayland = {
        let (xwayland, channel) = XWayland::new(log.clone(), &dh);
        let log2 = log.clone();
        let ret = event_loop
            .handle()
            .insert_source(channel, move |event, _, data| match event {
                XWaylandEvent::Ready {
                    connection,
                    client,
                    client_fd: _,
                    display,
                } => {
                    let mut wm = X11WM::start_wm(
                        data.state.handle.clone(),
                        dh.clone(),
                        connection,
                        client,
                        log2.clone(),
                    )
                    .expect("Failed to attach X11 Window Manager");
                    wm.set_cursor(CURSOR_DATA_BYTES, Size::from((64, 64)), Point::from((0, 0)))
                        .expect("Failed to set xwayland default cursor");
                    data.state.xwm = Some(wm);
                    slog::info!(log2, "Started Xwayland on display :{}", display);
                }
                XWaylandEvent::Exited => {
                    let _ = data.state.xwm.take();
                }
            });
        if let Err(e) = ret {
            slog::error!(
                log,
                "Failed to insert the XWaylandSource into the event loop: {}",
                e
            );
        }
        xwayland
            .start(event_loop.handle())
            .expect("Failed to start Xwayland");
        xwayland
    };

    let mut data = Data { display, state };
    while !data.state.should_quit {
        event_loop
            .dispatch(std::time::Duration::ZERO, &mut data)
            .expect("Failed to dispatch event loop");
        let next_buffer = data.state.create_frame().expect("Failed to render buffer");
        for window in data.state.space.elements() {
            window.send_frame(
                &data.state.output,
                data.state.start_time.elapsed(),
                None,
                |_, _| Some(data.state.output.clone()),
            )
        }
        data.display
            .flush_clients()
            .expect("Failed to flush clients");

        let gst_buffer = {
            data.state
                .renderer
                .bind(next_buffer)
                .expect("Failed to bind dmabuf");
            let mapping = data
                .state
                .renderer
                .copy_framebuffer(Rectangle::from_loc_and_size(
                    (0, 0),
                    size.to_logical(1).to_buffer(1, Transform::Normal),
                ))
                .expect("Failed to copy");
            let slice = data
                .state
                .renderer
                .map_texture(&mapping)
                .expect("Failed to map copy");
            let mut buffer =
                gst::Buffer::with_size(info.size()).expect("failed to create gst buffer");

            {
                let buffer = buffer.get_mut().unwrap();

                let mut vframe =
                    gst_video::VideoFrameRef::from_buffer_ref_writable(buffer, &info).unwrap();

                let plane_data = vframe.plane_data_mut(0).unwrap();
                plane_data.clone_from_slice(slice);
            }

            buffer
        };
        if buffer_tx.send(gst_buffer).is_err() {
            break;
        }

        data.state.space.refresh();
        data.state.popups.cleanup();
    }
}
