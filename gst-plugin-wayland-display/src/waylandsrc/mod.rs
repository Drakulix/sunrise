use gst::glib;
use gst::prelude::*;

mod imp;

glib::wrapper! {
    pub struct WaylandDisplaySrc(ObjectSubclass<imp::WaylandDisplaySrc>) @extends gst_base::PushSrc, gst_base::BaseSrc, gst::Element, gst::Object;
}

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(
        Some(plugin),
        "waylanddisplaysrc",
        gst::Rank::Primary,
        WaylandDisplaySrc::static_type(),
    )
}
