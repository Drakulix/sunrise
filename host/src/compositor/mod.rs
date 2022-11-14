use smithay::{
    backend::{
        allocator::{
            dmabuf::{AsDmabuf, Dmabuf},
            gbm::{GbmBuffer, GbmDevice},
            Fourcc, Modifier, Swapchain,
        },
        drm::{DrmNode, NodeType},
        egl::{EGLContext, EGLDevice, EGLDisplay},
        libinput::LibinputInputBackend,
        renderer::{
            gles2::Gles2Renderer,
            utils::{import_surface_tree, on_commit_buffer_handler, with_renderer_surface_state},
            Bind, ExportDma, ImportDma, ImportMemWl, Unbind,
        },
    },
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_output, delegate_seat,
    delegate_shm, delegate_viewporter, delegate_xdg_shell,
    desktop::{
        Kind as SurfaceKind, PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab,
        PopupUngrabStrategy, Space, Window,
    },
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, Mode, PostAction},
        input::Libinput,
        wayland_protocols::xdg::shell::server::xdg_toplevel::State as XdgState,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{
                wl_buffer::WlBuffer,
                wl_output::{Subpixel, WlOutput},
                wl_seat::WlSeat,
                wl_surface::WlSurface,
            },
            Display, DisplayHandle, Resource,
        },
    },
    utils::{Logical, Point, Size, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{get_children, with_states, CompositorHandler, CompositorState},
        data_device::{
            set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
            ServerDndGrabHandler,
        },
        dmabuf::{get_dmabuf, DmabufGlobal, DmabufHandler, DmabufState, ImportError},
        output::{Mode as OutputMode, Output, OutputManagerState, PhysicalProperties, Scale},
        seat::{Seat, SeatHandler, SeatState, XkbConfig},
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgPopupSurfaceRoleAttributes,
            XdgShellHandler, XdgShellState, XdgToplevelSurfaceRoleAttributes,
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
        Serial,
    },
};
use std::sync::Mutex;

mod cursor;
#[macro_use]
mod drm;
mod input;
use self::drm::WlDrmState;
use self::input::*;
use cursor::CursorElement;

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
    start_time: std::time::Instant,
    log: slog::Logger,

    // render
    egl: EGLDisplay,
    renderer: Gles2Renderer,
    dmabuf_global: DmabufGlobal,
    swapchain: Swapchain<GbmDevice<std::fs::File>, GbmBuffer<()>>,
    direct_scanout: bool,

    // management
    output: Output,
    seat: Seat<Self>,
    space: Space<Window>,
    popups: PopupManager,
    pointer_location: Point<f64, Logical>,
    cursor_element: CursorElement,
    pending_windows: Vec<Window>,

    // wayland state
    compositor_state: CompositorState,
    data_device_state: DataDeviceState,
    drm_state: WlDrmState,
    dmabuf_state: DmabufState,
    output_state: OutputManagerState,
    seat_state: SeatState<Self>,
    shell_state: XdgShellState,
    shm_state: ShmState,
    viewporter_state: ViewporterState,
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn commit(&mut self, dh: &DisplayHandle, surface: &WlSurface) {
        on_commit_buffer_handler(dh, surface);
        if let Err(err) = import_surface_tree(dh, &mut self.renderer, surface, &self.log) {
            slog::warn!(self.log, "Failed to load client buffer: {}", err);
        }

        self.space.commit(surface);
        self.popups.commit(surface);

        // send the initial configure if relevant
        if let Some(idx) = self
            .pending_windows
            .iter_mut()
            .position(|w| w.toplevel().wl_surface() == surface)
        {
            let window = self.pending_windows.swap_remove(idx);

            #[cfg_attr(not(feature = "xwayland"), allow(irrefutable_let_patterns))]
            if let SurfaceKind::Xdg(ref toplevel) = window.toplevel() {
                let (initial_configure_sent, max_size) = with_states(surface, |states| {
                    let attributes = states
                        .data_map
                        .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                        .unwrap();
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
                    self.pending_windows.push(window);
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
                    self.space.map_window(&window, loc, false);
                }
            }

            return;
        }

        if let Some(popup) = self.popups.find_popup(surface) {
            let PopupKind::Xdg(ref popup) = popup;
            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
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
        _dh: &DisplayHandle,
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
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
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

    fn new_toplevel(&mut self, _dh: &DisplayHandle, surface: ToplevelSurface) {
        let window = Window::new(SurfaceKind::Xdg(surface));
        self.pending_windows.push(window);
    }

    fn new_popup(
        &mut self,
        _dh: &DisplayHandle,
        surface: PopupSurface,
        positioner: PositionerState,
    ) {
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

    fn grab(&mut self, dh: &DisplayHandle, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        let seat: Seat<State> = Seat::from_resource(&seat).unwrap();
        let ret = self.popups.grab_popup(dh, surface.into(), &seat, serial);

        if let Ok(mut grab) = ret {
            if let Some(keyboard) = seat.get_keyboard() {
                if keyboard.is_grabbed()
                    && !(keyboard.has_grab(serial)
                        || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
                {
                    grab.ungrab(dh, PopupUngrabStrategy::All);
                    return;
                }
                keyboard.set_focus(dh, grab.current_grab().as_ref(), serial);
                keyboard.set_grab(PopupKeyboardGrab::new(&grab), serial);
            }
            if let Some(pointer) = seat.get_pointer() {
                if pointer.is_grabbed()
                    && !(pointer.has_grab(serial)
                        || pointer
                            .has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
                {
                    grab.ungrab(dh, PopupUngrabStrategy::All);
                    return;
                }
                pointer.set_grab(PopupPointerGrab::new(&grab), serial, 0);
            }
        }
    }
}

/*
impl ExportDmabufHandler for State {
    fn capture_frame(
        &mut self,
        dh: &DisplayHandle,
        _output: WlOutput,
        overlay_cursor: bool,
    ) -> Result<Capture, CaptureError> {
        self.space
            .send_frames(self.start_time.elapsed().as_millis() as u32);
        // check for "direct scanout"
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

        let physical_output_size = self.output.current_mode().unwrap().size;
        let buffer_size = physical_output_size
            .to_logical(1)
            .to_buffer(1, Transform::Normal);

        let elements = if overlay_cursor {
            self.cursor_element.set_location(
                self.pointer_location
                    .to_physical_precise_round(self.output.current_scale().fractional_scale()),
            );
            vec![self.cursor_element.clone()]
        } else {
            Vec::new()
        };

        let offscreen = self
            .swapchain
            .acquire()
            .map_err(|err| CaptureError::Temporary(Box::new(err)))?
            .unwrap();
        let age = offscreen.age();

        // EGLDevice code path
        //self.renderer.bind(offscreen.buffer.clone()).map_err(|err| CaptureError::Temporary(Box::new(err)))?;
        // GBM code path
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
            .bind(dmabuf)
            .map_err(|err| CaptureError::Temporary(Box::new(err)))?;

        self.space
            .render_output(
                &dh,
                &mut self.renderer,
                &self.output,
                age as usize,
                [0.0, 0.0, 0.0, 1.0],
                &*elements,
            )
            .map_err(|err| CaptureError::Temporary(Box::new(err)))?;
        let res = self
            .renderer
            .export_framebuffer(buffer_size)
            .map(|dmabuf| Capture {
                dmabuf: dbg!(dmabuf),
                presentation_time: std::time::Instant::now(),
            })
            .map_err(|err| CaptureError::Temporary(Box::new(err)))?;
        self.renderer
            .unbind()
            .map_err(|err| CaptureError::Temporary(Box::new(err)))?;
        Ok(res)
    }

    fn start_time(&mut self) -> std::time::Instant {
        self.start_time
    }
}
*/

delegate_compositor!(State);
delegate_data_device!(State);
delegate_dmabuf!(State);
delegate_wl_drm!(State);
delegate_output!(State);
delegate_seat!(State);
delegate_shm!(State);
delegate_xdg_shell!(State);
delegate_viewporter!(State);

fn main() -> smithay::reexports::calloop::Result<()> {
    use slog::Drain;

    let args = Args::parse();
    let (w, h) = args
        .resolution
        .split_once("x")
        .expect("resolution should be in format <W>x<H>");
    let size = (
        w.parse::<u32>()
            .expect(&format!("{} is no valid integer", w)) as i32,
        h.parse::<u32>()
            .expect(&format!("{} is no valid integer", h)) as i32,
    );

    let log = ::slog::Logger::root(
        slog_term::FullFormat::new(slog_term::PlainSyncDecorator::new(std::io::stdout()))
            .build()
            .fuse(),
        slog::o!(),
    );
    let _guard = slog_scope::set_global_logger(log.clone());
    slog_stdlog::init().expect("Failed to init logger");

    let mut display = Display::<State>::new().unwrap();
    let dh = display.handle();

    // init state
    let compositor_state = CompositorState::new::<State, _>(&dh, log.clone());
    let data_device_state = DataDeviceState::new::<State, _>(&dh, log.clone());
    let mut dmabuf_state = DmabufState::new();
    let mut drm_state = WlDrmState;
    let output_state = OutputManagerState::new_with_xdg_output::<State>(&dh);
    let seat_state = SeatState::new();
    let shell_state = XdgShellState::new::<State, _>(&dh, log.clone());
    let viewporter_state = ViewporterState::new::<State, _>(&dh, log.clone());

    // init render backend
    let user_node = DrmNode::from_path(&args.device_path).expect("Invalid render node path");

    /* // EGL Device code path, no working allocator
    let device = EGLDevice::enumerate()
        .expect("Failed to enumerate EGLDevice")
        .find(|dev| {
            if let Ok(Some(node)) = dev.try_get_render_node() {
                if let Some(Ok(user_node)) = user_node.node_with_type(node.ty()) {
                    return user_node == node;
                }
            }
            false
        })
        .expect(&format!("Could not find node matching: {:?}", user_node));
    */

    // GBM device code path
    let drm_node = std::fs::File::open(
        user_node
            .dev_path_with_type(NodeType::Render)
            .or_else(|| user_node.dev_path())
            .unwrap_or_else(|| std::path::PathBuf::from(&args.device_path)),
    )
    .expect("Failed to open drm device");
    let device = GbmDevice::new(drm_node).expect("Failed to open gbm device");

    let egl = EGLDisplay::new(&device, log.clone()).expect("Failed to create EGLDisplay");
    let _guard = egl
        .bind_wl_display(&dh)
        .expect("Failed to bind egl display");
    let context = EGLContext::new(&egl, log.clone()).expect("Failed to create EGLContext");

    /* // EGL Devicecode  path, no working allocator
    let alloc_context = EGLContext::new_shared(&egl, &context, log.clone()).expect("Failed to create shared EGLContext");
    let allocator = GlAllocator::new(unsafe { Gles2Renderer::new(alloc_context, log.clone()).expect("Failed to create allocator") });
    */
    // GBM device code path
    let allocator = device;

    let modifiers = context
        .dmabuf_texture_formats()
        .into_iter()
        .filter(|x| x.code == Fourcc::Nv12)
        .map(|x| x.modifier)
        .collect();
    let swapchain = Swapchain::new(
        allocator,
        size.0 as u32,
        size.1 as u32,
        Fourcc::Nv12,
        modifiers,
    );
    let mut renderer =
        unsafe { Gles2Renderer::new(context, log.clone()) }.expect("Failed to initialize renderer");
    let formats = Bind::<Dmabuf>::supported_formats(&renderer)
        .expect("Failed to query formats")
        .into_iter()
        .collect::<Vec<_>>();
    //egl.bind_wl_display(&dh).expect("Failed to bind EGLDisplay");
    let shm_state = ShmState::new::<State, _>(&dh, Vec::from(renderer.shm_formats()), log.clone());
    let dmabuf_global = dmabuf_state.create_global::<State, _>(&dh, formats.clone(), log.clone());
    let _drm_global = drm_state.create_global::<State>(
        &dh,
        std::path::PathBuf::from(&args.device_path),
        formats,
        &dmabuf_global,
    );
    let cursor_element =
        CursorElement::new(&mut renderer, (size.0 as f64 / 2.0, size.1 as f64 / 2.0));

    // init input backend
    let mut libinput_context = Libinput::new_with_udev(NixInterface);
    libinput_context
        .udev_assign_seat(&args.input_seat)
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
        refresh: (args.framerate * 1000) as i32,
    };
    output.change_current_state(Some(mode), None, None, None);
    output.set_preferred(mode);

    let mut space = Space::new(log.clone());
    space.map_output(&output, (0, 0));

    output_conf_state.add_heads([output.clone()].iter());
    output_conf_state.update();

    let mut seat = Seat::<State>::new(&dh, "seat-0", log.clone());
    seat.add_keyboard(XkbConfig::default(), 200, 25, move |seat, focus| {
        if let Some(surface) = focus {
            let client = dh.get_client(surface.id());
            set_data_device_focus(&dh, seat, client.ok());
        } else {
            set_data_device_focus(&dh, seat, None);
        }
    })
    .expect("Failed to add keyboard to seat");
    seat.add_pointer(|_| {});

    let state = State {
        start_time: std::time::Instant::now(),
        log: log.clone(),

        egl,
        renderer,
        dmabuf_global,
        swapchain,
        direct_scanout: false,

        space,
        popups: PopupManager::new(log.clone()),
        output,
        seat,
        pointer_location: (320.0, 240.0).into(),
        cursor_element,
        pending_windows: Vec::new(),

        compositor_state,
        data_device_state,
        drm_state,
        dmabuf_state,
        output_state,
        seat_state,
        shell_state,
        shm_state,
        viewporter_state,
    };

    // init event loop
    let mut event_loop = EventLoop::<Data>::try_new().expect("Unable to create event_loop");
    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, data| {
            let dh = data.display.handle();
            data.state.process_input_event(&dh, event)
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
            Generic::new(display.backend().poll_fd(), Interest::READ, Mode::Level),
            |_, _, data| {
                data.display.dispatch_clients(&mut data.state).unwrap();
                Ok(PostAction::Continue)
            },
        )
        .expect("Failed to init wayland server source");

    let mut data = Data { display, state };
    loop {
        event_loop.dispatch(std::time::Duration::from_millis(16), &mut data)?;
        data.state.space.refresh(&data.display.handle());
        data.state.popups.cleanup();
        data.display
            .flush_clients()
            .expect("Failed to flush clients");
    }
}
