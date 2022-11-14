use smithay::{
    backend::{
        allocator::{
            dmabuf::{Dmabuf, DmabufBuilder, DmabufFlags},
            Fourcc,
        },
        egl::{
            EGLDisplay, EGLContext,
            native::{EGLNativeDisplay, EGLPlatform},
            ffi,
        },
        renderer::{
            gles2::Gles2Renderer,
            ImportDma,
        }
    },
    egl_platform,
};
use wayland_client::{
    protocol::wl_output::WlOutput,
    sys::client::wl_proxy,
    Display, GlobalManager,
};
use wayland_protocols::wlr::unstable::export_dmabuf::v1::client::{
    zwlr_export_dmabuf_frame_v1::Event,
    zwlr_export_dmabuf_manager_v1::ZwlrExportDmabufManagerV1,
};

struct State {
    dmabuf: Option<DmabufBuilder>,
    modi: u64,
}

struct WaylandPlatform {
    display: *mut wl_proxy,
}

unsafe impl Send for WaylandPlatform {}
unsafe impl Sync for WaylandPlatform {}
impl EGLNativeDisplay for WaylandPlatform {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        vec![
            // see: https://www.khronos.org/registry/EGL/extensions/KHR/EGL_KHR_platform_wayland.txt
            egl_platform!(PLATFORM_WAYLAND_KHR, self.display as *mut _, &["EGL_KHR_platform_wayland"]),
            // see: https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_wayland.txt
            egl_platform!(PLATFORM_WAYLAND_EXT, self.display as *mut _, &["EGL_EXT_platform_wayland"]),
        ]
    }
}


fn main() {
    let display = Display::connect_to_env().unwrap();
    let display_ptr = display.c_ptr();
    let mut event_queue = display.create_event_queue();

    let attached_display = display.attach(event_queue.token());
    let globals = GlobalManager::new(&attached_display);
    let registry = attached_display.get_registry();
    event_queue.sync_roundtrip(&mut (), |_, _, _| {}).unwrap();

    let output = globals
        .instantiate_exact::<WlOutput>(4)
        .expect("Unable to init output");
    let dmabuf_manager = globals
        .instantiate_exact::<ZwlrExportDmabufManagerV1>(1)
        .expect("Unable to init export_dmabuf_manager");

    let mut data = State { dmabuf: None, modi: 0 };
    dmabuf_manager.capture_output(0, &output)
        .quick_assign(move |data, event, mut state| {
            let state = state.get::<State>().unwrap();
            match event {
                Event::Frame {
                    width,
                    height,
                    format,
                    buffer_flags,
                    mod_high,
                    mod_low,
                    ..
                } => {
                    state.dmabuf = Some(Dmabuf::builder(
                        (width as i32, height as i32),
                        Fourcc::try_from(format).unwrap(),
                        DmabufFlags::from_bits_truncate(buffer_flags),
                    ));
                    state.modi = ((mod_high as u64) << 32) | (mod_low as u64);
                }

                Event::Object {
                    fd,
                    offset,
                    stride,
                    plane_index,
                    ..
                } => {
                    if let Some(dmabuf) = state.dmabuf.as_mut() {
                        dmabuf.add_plane(
                            fd,
                            plane_index,
                            offset,
                            stride,
                            state.modi.into(),
                        );
                    } else {
                        panic!("What.")
                    }
                }

                Event::Ready { .. } => {
                    let frame = state.dmabuf.take().unwrap().build().unwrap();
                
                    let platform = WaylandPlatform {
                        display: display_ptr,
                    };
                    let display = EGLDisplay::new(&platform, None).unwrap();
                    let context = EGLContext::new(&display, None).unwrap();
                    let mut renderer = unsafe { Gles2Renderer::new(context, None).unwrap() };
                    dbg!(renderer.import_dmabuf(&frame, None).unwrap());
                    std::process::exit(0);
                }

                Event::Cancel { reason } => {
                    data.destroy();

                    dbg!(reason);
                    panic!("Frame was cancelled due to a permanent error. If you just disconnected screen, this is not implemented yet.");
                }

                _ => unreachable!(),
            }
        });

    loop {
        event_queue.dispatch(&mut data, |_, _, _| {}).unwrap();
    }
}