use std::{
    fs::File,
    io::{Seek, SeekFrom},
    os::unix::io::{FromRawFd, IntoRawFd},
    time::Instant,
};
use smithay::{
    backend::allocator::{
        Buffer,
        dmabuf::Dmabuf,
    },
    reexports::{
        wayland_server::{
            self,
            Client,
            DelegateDispatch,
            DelegateGlobalDispatch,
            Dispatch,
            GlobalDispatch,
            DisplayHandle,
            backend::GlobalId,
            protocol::wl_output::WlOutput,
        },
    },
};
use wayland_protocols_wlr::export_dmabuf::v1::server::{
    zwlr_export_dmabuf_frame_v1::{self, ZwlrExportDmabufFrameV1, Flags},
    zwlr_export_dmabuf_manager_v1::{self, ZwlrExportDmabufManagerV1},
};

/// Export Dmabuf global state
#[derive(Debug)]
pub struct ExportDmabufState {
    global: GlobalId,
}

impl ExportDmabufState {
    /// Create a new dmabuf global
    pub fn new<D>(display: &DisplayHandle) -> ExportDmabufState
    where
        D: GlobalDispatch<ZwlrExportDmabufManagerV1, ()>
            + Dispatch<ZwlrExportDmabufManagerV1, ()>
            + Dispatch<ZwlrExportDmabufFrameV1, ()>
            + ExportDmabufHandler
            + 'static,
    {
        ExportDmabufState {
            global: display.create_global::<D, ZwlrExportDmabufManagerV1, ()>(1, ()),
        }
    }

    /// Returns the export dmabuf global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

pub enum CaptureError {
    Temporary(Box<dyn std::error::Error>),
    Permanent(Box<dyn std::error::Error>),
    Resizing,
}

pub struct Capture {
    pub dmabuf: Dmabuf,
    pub presentation_time: Instant,
}

pub trait ExportDmabufHandler {
    fn capture_frame(&mut self, dh: &DisplayHandle, output: WlOutput, overlay_cursor: bool) -> Result<Capture, CaptureError>;
    fn start_time(&mut self) -> Instant;
}

impl<D> DelegateGlobalDispatch<ZwlrExportDmabufManagerV1, (), D> for ExportDmabufState
where
    D: GlobalDispatch<ZwlrExportDmabufManagerV1, ()>
     + Dispatch<ZwlrExportDmabufManagerV1, ()>
     + Dispatch<ZwlrExportDmabufFrameV1, ()>
     + ExportDmabufHandler,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: wayland_server::New<ZwlrExportDmabufManagerV1>,
        _global_data: &(),
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> DelegateDispatch<ZwlrExportDmabufManagerV1, (), D> for ExportDmabufState
where
    D: GlobalDispatch<ZwlrExportDmabufManagerV1, ()>
     + Dispatch<ZwlrExportDmabufManagerV1, ()>
     + Dispatch<ZwlrExportDmabufFrameV1, ()>
     + ExportDmabufHandler,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ZwlrExportDmabufManagerV1,
        request: <ZwlrExportDmabufManagerV1 as wayland_server::Resource>::Request,
        _data: &(),
        dhandle: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwlr_export_dmabuf_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => {
                let frame = data_init.init(frame, ());
                match state.capture_frame(dhandle, output, overlay_cursor != 0) {
                    Ok(Capture { dmabuf, presentation_time }) => {
                        let format = dmabuf.format();
                        let modifier: u64 = format.modifier.into();
                        frame.frame(
                            dmabuf.width(),
                            dmabuf.height(),
                            0,
                            0,
                            if dmabuf.y_inverted() { 1 } else { 0 },
                            Flags::Transient,
                            format.code as u32,
                            (modifier >> 32) as u32,
                            (modifier & 0xFFFFFFFF) as u32,
                            dmabuf.num_planes() as u32,
                        );
                        for (i, (handle, (offset, stride))) in dmabuf.handles().zip(dmabuf.offsets().zip(dmabuf.strides())).enumerate() {
                            let mut file = unsafe { File::from_raw_fd(handle) };
                            let size = match file.seek(SeekFrom::End(0)) {
                                Ok(size) => size,
                                Err(err) => {
                                    eprintln!("Temporary Capture Error: {}", err);
                                    frame.cancel(zwlr_export_dmabuf_frame_v1::CancelReason::Temporary);
                                    return;
                                }
                            };
                            if let Err(err) = file.rewind() {
                                eprintln!("Temporary Capture Error: {}", err);
                                frame.cancel(zwlr_export_dmabuf_frame_v1::CancelReason::Temporary);
                                return;
                            }
                            let handle = file.into_raw_fd();
                            frame.object(
                                i as u32,
                                handle,
                                size as u32,
                                offset,
                                stride,
                                i as u32,
                            );
                        }
                        let duration = presentation_time.duration_since(state.start_time());
                        let (tv_sec, tv_nsec) = (duration.as_secs(), duration.subsec_nanos());
                        frame.ready(
                            (tv_sec >> 32) as u32,
                            (tv_sec & 0xFFFFFFFF) as u32,
                            tv_nsec,
                        );
                    },
                    Err(err) => {
                        frame.cancel(match err {
                            CaptureError::Temporary(err) => {
                                eprintln!("Temporary Capture Error: {}", err);
                                zwlr_export_dmabuf_frame_v1::CancelReason::Temporary
                            },
                            CaptureError::Permanent(err) => {
                                eprintln!("Permanent Capture Error: {}", err);
                                zwlr_export_dmabuf_frame_v1::CancelReason::Permanent
                            },
                            CaptureError::Resizing => {
                                zwlr_export_dmabuf_frame_v1::CancelReason::Resizing
                            }
                        })
                    }
                }
            },
            zwlr_export_dmabuf_manager_v1::Request::Destroy => {},
            _ => {},
        }
    }
}

impl<D> DelegateDispatch<ZwlrExportDmabufFrameV1, (), D> for ExportDmabufState
where
    D: GlobalDispatch<ZwlrExportDmabufManagerV1, ()>
     + Dispatch<ZwlrExportDmabufManagerV1, ()>
     + Dispatch<ZwlrExportDmabufFrameV1, ()>
     + ExportDmabufHandler,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ZwlrExportDmabufFrameV1,
        request: <ZwlrExportDmabufFrameV1 as wayland_server::Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwlr_export_dmabuf_frame_v1::Request::Destroy => {},
            _ => {},
        }
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_export_dmabuf {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::export_dmabuf::v1::server::zwlr_export_dmabuf_manager_v1::ZwlrExportDmabufManagerV1: ()
        ] => $crate::export_dmabuf::ExportDmabufState);

        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::export_dmabuf::v1::server::zwlr_export_dmabuf_manager_v1::ZwlrExportDmabufManagerV1: ()
        ] => $crate::export_dmabuf::ExportDmabufState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::export_dmabuf::v1::server::zwlr_export_dmabuf_frame_v1::ZwlrExportDmabufFrameV1: ()
        ] => $crate::export_dmabuf::ExportDmabufState);
    };
}