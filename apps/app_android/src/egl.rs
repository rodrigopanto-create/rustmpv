use std::ffi::{c_int, c_void};

const NONE: c_int = 0x3038;
const DRAW: c_int = 0x3059;
const READ: c_int = 0x305A;

#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetCurrentDisplay() -> *mut c_void;
    fn eglGetCurrentContext() -> *mut c_void;
    fn eglGetCurrentSurface(which: c_int) -> *mut c_void;
    fn eglGetError() -> c_int;
    fn eglChooseConfig(
        display: *mut c_void,
        attributes: *const c_int,
        config: *mut *mut c_void,
        size: c_int,
        count: *mut c_int,
    ) -> c_int;
    fn eglCreatePbufferSurface(
        display: *mut c_void,
        config: *mut c_void,
        attributes: *const c_int,
    ) -> *mut c_void;
    fn eglCreateContext(
        display: *mut c_void,
        config: *mut c_void,
        shared: *mut c_void,
        attributes: *const c_int,
    ) -> *mut c_void;
    fn eglMakeCurrent(
        display: *mut c_void,
        draw: *mut c_void,
        read: *mut c_void,
        context: *mut c_void,
    ) -> c_int;
    fn eglDestroyContext(display: *mut c_void, context: *mut c_void) -> c_int;
    fn eglDestroySurface(display: *mut c_void, surface: *mut c_void) -> c_int;
}

fn error(operation: &str) -> String {
    format!("EGL al {operation}: {:#x}", unsafe { eglGetError() })
}

/// Shares textures with eframe, but keeps mpv's GL pipeline state private.
/// All operations, including destruction, run on eframe's render thread.
pub struct Context {
    display: usize,
    surface: usize,
    context: usize,
}

impl Context {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let display = eglGetCurrentDisplay();
            let shared = eglGetCurrentContext();
            if display.is_null() || shared.is_null() {
                return Err("No hay contexto EGL de eframe para compartir video".to_owned());
            }
            let attributes = [
                0x3033, 0x0001, // EGL_SURFACE_TYPE, EGL_PBUFFER_BIT
                0x3040, 0x0040, // EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT
                0x3024, 8, // EGL_RED_SIZE
                0x3023, 8, // EGL_GREEN_SIZE
                0x3022, 8, // EGL_BLUE_SIZE
                0x3021, 8, // EGL_ALPHA_SIZE
                NONE,
            ];
            let mut config = std::ptr::null_mut();
            let mut count = 0;
            if eglChooseConfig(display, attributes.as_ptr(), &mut config, 1, &mut count) == 0 {
                return Err(error("elegir configuracion de video"));
            }
            if count == 0 {
                return Err("No hay configuracion EGL Pbuffer OpenGL ES 3".to_owned());
            }
            let surface_attributes = [0x3057, 1, 0x3056, 1, NONE]; // EGL_WIDTH, EGL_HEIGHT
            let surface = eglCreatePbufferSurface(display, config, surface_attributes.as_ptr());
            if surface.is_null() {
                return Err(error("crear superficie de video"));
            }
            let mut result = Self {
                display: display as usize,
                surface: surface as usize,
                context: 0,
            };
            let context_attributes = [0x3098, 3, NONE]; // EGL_CONTEXT_CLIENT_VERSION
            let context = eglCreateContext(display, config, shared, context_attributes.as_ptr());
            if context.is_null() {
                return Err(error("crear contexto de video"));
            }
            result.context = context as usize;
            Ok(result)
        }
    }

    pub fn enter(&self) -> Result<CurrentContext, String> {
        unsafe {
            let mut previous = CurrentContext {
                display: eglGetCurrentDisplay() as usize,
                draw: eglGetCurrentSurface(DRAW) as usize,
                read: eglGetCurrentSurface(READ) as usize,
                context: eglGetCurrentContext() as usize,
                restored: true,
            };
            if previous.display == 0 || previous.context == 0 {
                return Err("No hay contexto EGL de eframe activo".to_owned());
            }
            if eglMakeCurrent(
                self.display as *mut _,
                self.surface as *mut _,
                self.surface as *mut _,
                self.context as *mut _,
            ) == 0
            {
                return Err(error("activar contexto de video"));
            }
            previous.restored = false;
            Ok(previous)
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            if self.context != 0
                && eglDestroyContext(self.display as *mut _, self.context as *mut _) == 0
            {
                log::error!("{}", error("destruir contexto de video"));
            }
            if self.surface != 0
                && eglDestroySurface(self.display as *mut _, self.surface as *mut _) == 0
            {
                log::error!("{}", error("destruir superficie de video"));
            }
        }
    }
}

pub struct CurrentContext {
    display: usize,
    draw: usize,
    read: usize,
    context: usize,
    restored: bool,
}

impl CurrentContext {
    fn restore_inner(&mut self) -> Result<(), String> {
        if unsafe {
            eglMakeCurrent(
                self.display as *mut _,
                self.draw as *mut _,
                self.read as *mut _,
                self.context as *mut _,
            )
        } == 0
        {
            return Err(error("restaurar contexto de eframe"));
        }
        self.restored = true;
        Ok(())
    }

    pub fn restore(mut self) -> Result<(), String> {
        self.restore_inner()
    }
}

impl Drop for CurrentContext {
    fn drop(&mut self) {
        if !self.restored {
            if let Err(error) = self.restore_inner() {
                log::error!("{error}");
            }
        }
    }
}
