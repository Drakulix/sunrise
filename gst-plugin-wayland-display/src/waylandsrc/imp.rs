use std::sync::{mpsc::Receiver, Mutex};
use std::thread::JoinHandle;

use smithay::reexports::calloop::channel::Sender;

use gst::glib;
use gst::glib::once_cell::sync::Lazy;
use gst::prelude::*;
use gst::subclass::prelude::*;

use gst_base::subclass::base_src::CreateSuccess;
use gst_base::subclass::prelude::*;

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        "waylanddisplaysrc",
        gst::DebugColorFlags::empty(),
        Some("Wayland Display Source Bin"),
    )
});

static VIDEO_INFO: Lazy<gst_video::VideoInfo> = Lazy::new(|| {
    gst_video::VideoInfo::builder(gst_video::VideoFormat::Rgbx, 1920, 1080)
        //.par(gst::Fraction::new(1, 1))
        .fps(gst::Fraction::new(60, 1))
        .build()
        .expect("Failed to create video info")
});

pub struct SlogGstDrain;

impl slog::Drain for SlogGstDrain {
    type Ok = ();
    type Err = std::convert::Infallible;

    fn log(
        &self,
        record: &slog::Record,
        _values: &slog::OwnedKVList,
    ) -> std::result::Result<Self::Ok, Self::Err> {
        CAT.log::<super::WaylandDisplaySrc>(
            None,
            match record.level() {
                slog::Level::Critical | slog::Level::Error => gst::DebugLevel::Error,
                slog::Level::Warning => gst::DebugLevel::Warning,
                slog::Level::Info => gst::DebugLevel::Info,
                slog::Level::Debug => gst::DebugLevel::Debug,
                slog::Level::Trace => gst::DebugLevel::Trace,
            },
            record.file(),
            record.module(),
            record.line(),
            *record.msg(),
        );
        Ok(())
    }
}

pub struct WaylandDisplaySrc {
    state: Mutex<Option<State>>,
}

impl Default for WaylandDisplaySrc {
    fn default() -> Self {
        WaylandDisplaySrc {
            state: Mutex::new(None),
        }
    }
}

pub struct State {
    thread_handle: JoinHandle<()>,
    command_tx: Sender<Command>,
    buffer_rx: Receiver<gst::Buffer>,
}

pub enum Command {
    Quit,
}

#[glib::object_subclass]
impl ObjectSubclass for WaylandDisplaySrc {
    const NAME: &'static str = "GstWaylandDisplaySrc";
    type Type = super::WaylandDisplaySrc;
    type ParentType = gst_base::PushSrc;
    type Interfaces = ();
}

impl ObjectImpl for WaylandDisplaySrc {
    fn properties() -> &'static [glib::ParamSpec] {
        &[]
    }

    fn set_property(&self, _id: usize, _value: &glib::Value, _pspec: &glib::ParamSpec) {}

    fn property(&self, _id: usize, _pspec: &glib::ParamSpec) -> glib::Value {
        todo!()
    }

    fn constructed(&self) {
        self.parent_constructed();

        let obj = self.obj();
        obj.set_element_flags(gst::ElementFlags::SOURCE);
    }
}

impl GstObjectImpl for WaylandDisplaySrc {}

impl ElementImpl for WaylandDisplaySrc {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                "Wayland display source",
                "Source/Video",
                "GStreamer video src running a wayland compositor",
                "Victoria Brekenfeld <wayland@drakulix.de>",
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
            let caps = VIDEO_INFO.to_caps().unwrap();
            let src_pad_template = gst::PadTemplate::new(
                "src",
                gst::PadDirection::Src,
                gst::PadPresence::Always,
                &caps,
            )
            .unwrap();

            vec![src_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }

    fn change_state(
        &self,
        transition: gst::StateChange,
    ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
        let res = self.parent_change_state(transition);
        match dbg!(res) {
            Ok(gst::StateChangeSuccess::Success) => {
                if transition.next() == gst::State::Paused {
                    // this is a live source
                    Ok(gst::StateChangeSuccess::NoPreroll)
                } else {
                    Ok(gst::StateChangeSuccess::Success)
                }
            }
            x => x,
        }
    }
}

impl BaseSrcImpl for WaylandDisplaySrc {
    fn start(&self) -> Result<(), gst::ErrorMessage> {
        let mut state = self.state.lock().unwrap();
        if state.is_some() {
            return Ok(());
        }

        let (buffer_tx, buffer_rx) = std::sync::mpsc::sync_channel(1);
        let (command_tx, command_src) = smithay::reexports::calloop::channel::channel();
        let thread_handle = std::thread::spawn(move || {
            super::comp::init(
                buffer_tx,
                command_src,
                //TODO: Make all of this configurable
                String::from("/dev/dri/renderD128"),
                "seat-0",
                VIDEO_INFO.clone(),
            )
        });
        *state = Some(State {
            thread_handle,
            buffer_rx,
            command_tx,
        });

        Ok(())
    }

    fn stop(&self) -> Result<(), gst::ErrorMessage> {
        let mut state = self.state.lock().unwrap();
        if let Some(state) = state.take() {
            if let Err(err) = state.command_tx.send(Command::Quit) {
                gst::warning!(CAT, "Failed to send stop command: {}", err);
                return Ok(());
            };
            mem::drop(state.command_tx);
            if state.thread_handle.join().is_err() {
                gst::warning!(CAT, "Failed to join compositor thread");
            };
        }

        Ok(())
    }

    fn is_seekable(&self) -> bool {
        false
    }
}

impl PushSrcImpl for WaylandDisplaySrc {
    fn create(
        &self,
        _buffer: Option<&mut gst::BufferRef>,
    ) -> Result<CreateSuccess, gst::FlowError> {
        match self.state.lock().unwrap().as_mut() {
            Some(state) => state
                .buffer_rx
                .recv()
                .map(|buffer| CreateSuccess::NewBuffer(buffer))
                .map_err(|_| gst::FlowError::Eos),
            None => Err(gst::FlowError::Eos),
        }
    }
}
