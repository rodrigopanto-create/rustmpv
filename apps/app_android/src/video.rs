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

struct Compositor {
    program: glow::Program,
    vertex_array: glow::VertexArray,
    sampler: glow::UniformLocation,
}

impl Compositor {
    fn new(gl: &glow::Context) -> Result<Self, String> {
        const VERTEX: &str = r#"#version 300 es
precision highp float;
out vec2 uv;
void main() {
    vec2 position = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    uv = position;
    gl_Position = vec4(position * 2.0 - 1.0, 0.0, 1.0);
}
"#;
        const FRAGMENT: &str = r#"#version 300 es
precision highp float;
uniform sampler2D video_texture;
in vec2 uv;
out vec4 color;
void main() {
    color = vec4(texture(video_texture, uv).rgb, 1.0);
}
"#;
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            let result = (|| {
                for (kind, source) in [
                    (glow::VERTEX_SHADER, VERTEX),
                    (glow::FRAGMENT_SHADER, FRAGMENT),
                ] {
                    let shader = gl.create_shader(kind)?;
                    gl.attach_shader(program, shader);
                    shaders.push(shader);
                    gl.shader_source(shader, source);
                    gl.compile_shader(shader);
                    if !gl.get_shader_compile_status(shader) {
                        return Err(format!(
                            "Shader de video: {}",
                            gl.get_shader_info_log(shader)
                        ));
                    }
                }
                gl.link_program(program);
                if !gl.get_program_link_status(program) {
                    return Err(format!(
                        "Programa de video: {}",
                        gl.get_program_info_log(program)
                    ));
                }
                let sampler = gl
                    .get_uniform_location(program, "video_texture")
                    .ok_or_else(|| "El shader de video no expone video_texture".to_owned())?;
                let vertex_array = gl.create_vertex_array()?;
                Ok(Self {
                    program,
                    vertex_array,
                    sampler,
                })
            })();
            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }
            if result.is_err() {
                gl.delete_program(program);
            }
            result
        }
    }

    fn paint(
        &self,
        gl: &glow::Context,
        texture: glow::Texture,
        info: egui::PaintCallbackInfo,
        destination: Option<glow::Framebuffer>,
    ) -> Result<(), String> {
        let viewport = info.viewport_in_pixels();
        let clip = egui::PaintCallbackInfo {
            viewport: info.clip_rect,
            ..info
        }
        .viewport_in_pixels();
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, destination);
            gl.draw_buffers(&[if destination.is_some() {
                glow::COLOR_ATTACHMENT0
            } else {
                glow::BACK
            }]);
            gl.viewport(
                viewport.left_px,
                viewport.from_bottom_px,
                viewport.width_px,
                viewport.height_px,
            );
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            );
            gl.disable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::RASTERIZER_DISCARD);
            gl.disable(glow::SAMPLE_COVERAGE);
            gl.disable(glow::SAMPLE_ALPHA_TO_COVERAGE);
            gl.color_mask(true, true, true, true);
            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vertex_array));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_sampler(0, None);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.uniform_1_i32(Some(&self.sampler), 0);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
        check_gl(gl, "componer video en pantalla")
    }

    fn destroy(self, gl: &glow::Context) {
        unsafe {
            gl.delete_vertex_array(self.vertex_array);
            gl.delete_program(self.program);
        }
    }
}

/// Accessed only on eframe's GL thread; the mutex also serves egui's Send + Sync callback.
pub struct Video {
    context: crate::egl::Context,
    render: usize,
    target: Option<Target>,
    compositor: Option<Compositor>,
    pub error: Option<String>,
}

impl Video {
    pub fn new(gl: &glow::Context, mpv: usize) -> Result<Self, String> {
        if !gl.version().is_embedded || gl.version().major < 3 {
            return Err("El video integrado requiere OpenGL ES 3".to_owned());
        }
        let context = crate::egl::Context::new()?;
        let current = context.enter()?;
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
        let mut video = Self {
            context,
            render: render as usize,
            target: None,
            compositor: None,
            error: None,
        };
        if let Err(error) = current.restore() {
            video.destroy(gl);
            return Err(error);
        }
        video.compositor = Some(match Compositor::new(gl) {
            Ok(compositor) => compositor,
            Err(error) => {
                video.destroy(gl);
                return Err(error);
            }
        });
        log::info!("libmpv: render OpenGL ES con contexto EGL aislado inicializado");
        Ok(video)
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
            if let Err(error) = check_gl(gl, "asignar textura de video") {
                gl.delete_texture(texture);
                return Err(error);
            }
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            for parameter in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, glow::CLAMP_TO_EDGE as i32);
            }
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
        // The compositor changes GL state; egui restores its pipeline after the callback.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_vertex_array(None);
            gl.use_program(None);
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
        // Complete the consumer's reads before mpv writes the same shared texture again.
        unsafe { gl.finish() };
        check_gl(gl, "sincronizar textura de eframe")?;
        let current = self.context.enter()?;
        let result = self.render(gl, &info);
        let restored = current.restore();
        let texture = result?;
        restored?;
        self.compositor
            .as_ref()
            .expect("video compositor created")
            .paint(gl, texture, info, destination)
    }

    fn render(
        &mut self,
        gl: &glow::Context,
        info: &egui::PaintCallbackInfo,
    ) -> Result<glow::Texture, String> {
        let viewport = info.viewport_in_pixels();
        check_gl(gl, "antes de renderizar video")?;
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_sampler(0, None);
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            gl.pixel_store_i32(glow::UNPACK_SKIP_ROWS, 0);
            gl.pixel_store_i32(glow::UNPACK_SKIP_PIXELS, 0);
        }
        self.resize(gl, [viewport.width_px, viewport.height_px])?;
        check_gl(gl, "crear framebuffer de video")?;
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
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
        // Finish producer writes before eframe samples the shared texture in its context.
        unsafe { gl.finish() };
        check_gl(gl, "renderizar libmpv")?;
        Ok(target.texture)
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
        unsafe { gl.finish() };
        let current = match self.context.enter() {
            Ok(current) => current,
            Err(error) => {
                log::error!("Liberar video: {error}");
                return;
            }
        };
        if self.render != 0 {
            unsafe { mpv_render_context_free(self.render as *mut _) };
            self.render = 0;
        }
        self.delete_target(gl);
        if let Err(error) = current.restore() {
            log::error!("Liberar video: {error}");
            return;
        }
        if let Some(compositor) = self.compositor.take() {
            compositor.destroy(gl);
        }
    }
}

fn check_gl(gl: &glow::Context, operation: &str) -> Result<(), String> {
    let mut errors = Vec::new();
    loop {
        let error = unsafe { gl.get_error() };
        if error == glow::NO_ERROR {
            break;
        }
        errors.push(format!("{error:#x}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("OpenGL al {operation}: {}", errors.join(", ")))
    }
}
