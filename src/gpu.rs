use piston_window::*;
use image::*;

use memory::Memory;

pub struct GPU {
    pub scanline: u32,
    pub width: u32,
    pub height: u32,
    window: PistonWindow,
    pub palette: Vec<DACPalette>,
}

#[derive(Clone)]
pub struct DACPalette {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl GPU {
    pub fn new() -> GPU {
        let (width, height) = (320, 200);
        GPU {
            scanline: 0,
            width: width,
            height: height,
            window: WindowSettings::new("x86emu", (width, height))
                .exit_on_esc(true)
                .opengl(OpenGL::V3_2)
                .build()
                .unwrap(),
            palette: vec![DACPalette { r: 0, g: 0, b: 0 }; 256],
        }
    }
    pub fn progress_scanline(&mut self, memory: &mut Memory) {
        // HACK to have a source of info to toggle CGA status register
        self.scanline += 1;
        if self.scanline > self.width {
            self.redraw_window(memory);
            self.scanline = 0;
        }
    }

    fn redraw_window(&mut self, memory: &mut Memory) {

        // println!("redraw_window");

        let mut canvas = ImageBuffer::new(self.width, self.height);

        let mut texture =
            Texture::from_image(&mut self.window.factory, &canvas, &TextureSettings::new())
                .unwrap();

        for y in 0..self.height {
            for x in 0..self.width {
                let offset = 0xA0000 + ((y * self.width) + x) as usize;
                let byte = memory.memory[offset];
                let ref pal = self.palette[byte as usize];
                canvas.put_pixel(x, y, Rgba([pal.r, pal.g, pal.b, 255]));
            }
        }

        texture
            .update(&mut self.window.encoder, &canvas)
            .unwrap();

        // HACK to redraw window without locking up
        for _ in 0..3 {
            match self.window.next() {
                Some(e) => {
                    self.window
                        .draw_2d(&e, |c, g| {
                            clear([1.0; 4], g);
                            image(&texture, c.transform, g);
                        });
                }
                None => {}
            }
        }
    }
}
