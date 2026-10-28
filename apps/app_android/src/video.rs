use eframe::egui;
use eframe::glow::{self, HasContext};
use std::ffi::{c_char, c_int, c_void};

#[repr(C)]
struct Param {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct GlInit {
    get_proc_address: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    context: *mut c_void,
}

#[repr(C)]
struct Fbo {
    id: c_int,
    width: c_int,
    height: c_int,
    format: c_int,
}

unsafe extern "C" {
    fn mpv_render_context_create(
        render: *mut *mut c_void,
        mpv: *mut c_void,
        params: *mut Param,
    ) -> c_int;
    fn mpv_render_context_render(render: *mut c_void, params: *mut Param) -> c_int;
    fn mpv_render_context_free(render: *mut c_void);
}

#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetProcAddress(name: *const c_char) -> *mut c_void;
}

unsafe extern "C" fn gl_proc(_: *mut c_void, name: *const c_char) -> *mut c_void {
    let address = unsafe { eglGetProcAddress(name) };
    if address.is_null() {
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, name) }
    } else {
        address
    }
}

struct Target {
    framebuffer: glow::Framebuffer,
    texture: glow::Texture,
    size: [i32; 2],
}

/// Accessed only on eframe's GL thread; the mutex also serves egui's Send + Sync callback.
pub struct Video {
    render: usize,
    target: Option<Target>,
    pub error: Option<String>,
}

impl Video {
    pub fn new(gl: &glow::Context, mpv: usize) -> Result<Self, String> {
        if !gl.version().is_embedded || gl.version().major < 3 {
            return Err("El video integrado requiere OpenGL ES 3".to_owned());
        }
        let mut init = GlInit {
            get_proc_address: gl_proc,
            context: std::ptr::null_mut(),
        };
        let mut params = [
            Param {
                kind: 1,
                data: c"opengl".as_ptr().cast_mut().cast(),
            },
            Param {
                kind: 2,
                data: (&mut init as *mut GlInit).cast(),
            },
            Param {
                kind: 0,
                data: std::ptr::null_mut(),
            },
        ];
        let mut render = std::ptr::null_mut();
        let code =
            unsafe { mpv_render_context_create(&mut render, mpv as *mut _, params.as_mut_ptr()) };
        if code < 0 {
            return Err(format!(
                "Inicializar video OpenGL: {}",
                crate::playback::error_text(code)
            ));
        }
        log::info!("libmpv: render OpenGL ES integrado inicializado");
        Ok(Self {
            render: render as usize,
            target: None,
            error: None,
        })
    }

    fn resize(&mut self, gl: &glow::Context, size: [i32; 2]) -> Result<(), String> {
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.size == size)
        {
            return Ok(());
        }
        self.delete_target(gl);
        unsafe {
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size[0],
                size[1],
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                None,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            let framebuffer = match gl.create_framebuffer() {
                Ok(framebuffer) => framebuffer,
                Err(error) => {
                    gl.delete_texture(texture);
                    return Err(error);
                }
            };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(framebuffer);
                gl.delete_texture(texture);
                return Err(format!("Framebuffer de video incompleto: {status:#x}"));
            }
            self.target = Some(Target {
                framebuffer,
                texture,
                size,
            });
        }
        log::info!("libmpv: area de video {}x{} px", size[0], size[1]);
        Ok(())
    }

    pub fn paint(&mut self, info: egui::PaintCallbackInfo, painter: &eframe::egui_glow::Painter) {
        if self.render == 0 || self.error.is_some() {
            return;
        }
        let gl = painter.gl();
        let viewport = info.viewport_in_pixels();
        if viewport.width_px <= 0 || viewport.height_px <= 0 {
            return;
        }
        let result = self.draw(gl, info, painter.intermediate_fbo());
        // mpv changes GL state; egui restores its pipeline after the callback.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        if let Err(error) = result {
            log::error!("Render de video: {error}");
            self.error = Some(error);
        }
    }

    fn draw(
        &mut self,
        gl: &glow::Context,
        info: egui::PaintCallbackInfo,
        destination: Option<glow::Framebuffer>,
    ) -> Result<(), String> {
        let viewport = info.viewport_in_pixels();
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.active_texture(glow::TEXTURE0);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
        }
        self.resize(gl, [viewport.width_px, viewport.height_px])?;
        let target = self.target.as_ref().expect("video target created");
        let mut fbo = Fbo {
            id: target.framebuffer.0.get() as i32,
            width: viewport.width_px,
            height: viewport.height_px,
            format: glow::RGBA8 as i32,
        };
        let mut flip = 1_i32;
        let mut block = 0_i32;
        let mut params = [
            Param {
                kind: 3,
                data: (&mut fbo as *mut Fbo).cast(),
            },
            Param {
                kind: 4,
                data: (&mut flip as *mut i32).cast(),
            },
            // Never wait for the next presentation time on the GUI thread.
            Param {
                kind: 12,
                data: (&mut block as *mut i32).cast(),
            },
            Param {
                kind: 0,
                data: std::ptr::null_mut(),
            },
        ];
        let code = unsafe { mpv_render_context_render(self.render as *mut _, params.as_mut_ptr()) };
        if code < 0 {
            return Err(format!(
                "Dibujar video: {}",
                crate::playback::error_text(code)
            ));
        }
        let clip = egui::PaintCallbackInfo {
            viewport: info.clip_rect,
            ..info
        }
        .viewport_in_pixels();
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(target.framebuffer));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, destination);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            );
            gl.blit_framebuffer(
                0,
                0,
                viewport.width_px,
                viewport.height_px,
                viewport.left_px,
                viewport.from_bottom_px,
                viewport.left_px + viewport.width_px,
                viewport.from_bottom_px + viewport.height_px,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        }
        Ok(())
    }

    fn delete_target(&mut self, gl: &glow::Context) {
        if let Some(target) = self.target.take() {
            unsafe {
                gl.delete_framebuffer(target.framebuffer);
                gl.delete_texture(target.texture);
            }
        }
    }

    pub fn destroy(&mut self, gl: &glow::Context) {
        if self.render != 0 {
            unsafe { mpv_render_context_free(self.render as *mut _) };
            self.render = 0;
        }
        self.delete_target(gl);
    }
}
