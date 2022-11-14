use smithay::{
    backend::renderer::{
        gles2::{Gles2Renderer, Gles2Texture, Gles2Frame, Gles2Error},
        ImportMem, Frame,
    },
    reexports::wayland_server::DisplayHandle,
    desktop::space::{RenderElement, SpaceOutputTuple},
    utils::{Logical, Point, Rectangle, Physical, Size, Scale, Transform},
};

static FALLBACK_CURSOR_DATA: &[u8] = include_bytes!("./cursor.rgba");

#[derive(Clone)]
pub struct CursorElement {
    loc: Point<f64, Physical>,
    texture: Gles2Texture,
}

impl CursorElement {
    pub fn new(renderer: &mut Gles2Renderer, loc: impl Into<Point<f64, Physical>>) -> CursorElement {
        CursorElement {
            loc: loc.into(),
            texture: renderer.import_memory(
                FALLBACK_CURSOR_DATA,
                (64, 64).into(),
                false
            ).expect("Failed to load cursor texture"),
        }
    }

    pub fn set_location(&mut self, loc: impl Into<Point<f64, Physical>>) {
        self.loc = loc.into();
    }
}

impl RenderElement<Gles2Renderer> for CursorElement {
    fn id(&self) -> usize { 0 }

    fn location(&self, _scale: impl Into<Scale<f64>>) -> Point<f64, Physical> {
        self.loc
    }

    fn geometry(&self, _scale: impl Into<Scale<f64>>) -> Rectangle<i32, Physical> {
        Rectangle::from_loc_and_size(
            self.loc.to_i32_round(),
            (64, 64)
        )
    }

    fn accumulated_damage(
        &self, 
        _scale: impl Into<Scale<f64>>, 
        _for_values: Option<SpaceOutputTuple<'_, '_>>
    ) -> Vec<Rectangle<i32, Physical>> {
        vec![]
    }

    fn draw(
        &self, 
        _dh: &DisplayHandle, 
        _renderer: &mut Gles2Renderer,
        frame: &mut Gles2Frame, 
        scale: impl Into<Scale<f64>>, 
        location: Point<f64, Physical>, 
        _damage: &[Rectangle<i32, Physical>], 
        _log: &::slog::Logger
    ) -> Result<(), Gles2Error> {
        let scale = scale.into();
        frame.render_texture_at(
            &self.texture,
            location.to_i32_round(),
            1,
            scale,
            Transform::Normal,
            &[Rectangle::from_loc_and_size(
                (0, 0),
                Size::<i32, Logical>::from((64, 64)).to_physical_precise_round(scale),
            )],
            1.0,
        )
    }
}