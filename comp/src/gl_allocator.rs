use smithay::{
    backend::{
        allocator::{Allocator, Buffer, Fourcc, Format, Modifier},
        renderer::{
            Offscreen,
            gles2::{Gles2Renderer, Gles2Renderbuffer, Gles2Texture, Gles2Error},
        },
    },
    utils::{Size, Buffer as BufferCoords},
};

pub struct GlAllocator {
    renderer: Gles2Renderer,
}

impl GlAllocator {
    pub fn new(renderer: Gles2Renderer) -> GlAllocator {
        GlAllocator {
            renderer
        }
    }
}

pub struct Renderbuffer {
    pub buffer: Gles2Texture,
    size: Size<i32, BufferCoords>,
}

impl Buffer for Renderbuffer {
    fn size(&self) -> Size<i32, BufferCoords> {
        self.size
    }
    fn format(&self) -> Format {
        Format {
            code: Fourcc::Argb8888,
            modifier: Modifier::Invalid
        }
    }
}

impl Allocator<Renderbuffer> for GlAllocator {
    type Error = Gles2Error;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier]
    ) -> Result<Renderbuffer, Self::Error> {
        assert_eq!(fourcc, Fourcc::Argb8888);
        //assert_eq!(modifiers.len(), 1);
        //assert_eq!(modifiers[0], Modifier::Invalid);

        let size = (width as i32, height as i32).into();
        Ok(Renderbuffer {
            buffer: self.renderer.create_buffer(size)?,
            size,
        })
    }
}