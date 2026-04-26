//! In-process libmpv playback path (scaffold).
//!
//! This module is **additive groundwork** running alongside the existing
//! subprocess `mpv`/`vlc` players in this tree. It is gated behind the
//! `libmpv-inproc` Cargo feature and is **not** wired into `main.rs` yet.
//!
//! Pattern ported from `amp-dot/amp` (`src/fbo.rs`, `src/player.rs`,
//! `src/main.rs`): create a Send+Sync wrapper around `*mut mpv_handle`,
//! create an OpenGL `mpv_render_context`, and render frames into a Slint
//! texture-backed FBO from inside `Window::set_rendering_notifier`.
//!
//! # Planned wiring (NOT YET ACTIVE)
//!
//! Once UI code exposes a Slint `Image` property backed by a borrowed GL
//! texture, the `set_rendering_notifier` hookup in `main.rs` will look
//! roughly like the amp-dot/amp original (see
//! <https://github.com/amp-dot/amp/blob/main/src/main.rs>):
//!
//! ```ignore
//! ui.window().set_rendering_notifier(move |state, api| {
//!     match state {
//!         slint::RenderingState::RenderingSetup => {
//!             if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api {
//!                 let api_type = CString::new(\"opengl\").unwrap();
//!                 let mut init_params = mpv_opengl_init_params {
//!                     get_proc_address: Some(get_proc_address_mpv),
//!                     get_proc_address_ctx: get_proc_address as *const _ as *mut c_void,
//!                     extra_exts: std::ptr::null(),
//!                 };
//!                 let mut params = [
//!                     mpv_render_param { type_: MPV_RENDER_PARAM_API_TYPE,
//!                         data: api_type.as_ptr() as *mut c_void },
//!                     mpv_render_param { type_: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
//!                         data: &mut init_params as *mut _ as *mut c_void },
//!                     mpv_render_param { type_: 0, data: std::ptr::null_mut() },
//!                 ];
//!                 let mut ctx: *mut mpv_render_context = std::ptr::null_mut();
//!                 unsafe { mpv_render_context_create(&mut ctx, handle.get(), params.as_mut_ptr()) };
//!                 // store ctx in shared cell ...
//!             }
//!         }
//!         slint::RenderingState::BeforeRendering => { /* render_frame(...) */ }
//!         slint::RenderingState::RenderingTeardown => { /* drop ctx + GL resources */ }
//!         _ => {}
//!     }
//! }).unwrap();
//! ```
//!
//! TODO: implement the above wiring once `MainWindow` exposes a GL-backed
//! `Image` property and we are ready to switch a player variant to
//! in-process libmpv. Keep the subprocess players as fallback.

use glow::HasContext;
use libmpv_sys::*;
use std::ffi::{CStr, CString};
use std::os::raw::c_void;
use std::ptr;

// ---------------------------------------------------------------------------
// MpvHandle: Send + Sync wrapper around `*mut mpv_handle`.
// Ported from amp-dot/amp/src/player.rs.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct MpvHandle(*mut mpv_handle);

impl MpvHandle {
    pub fn get(&self) -> *mut mpv_handle {
        self.0
    }

    pub fn stop(&self) {
        let scmd = CString::new("stop").unwrap();
        let mut sargs = [scmd.as_ptr(), ptr::null()];
        unsafe {
            mpv_command(self.get(), sargs.as_mut_ptr());
            mpv_terminate_destroy(self.get());
        }
    }
}

unsafe impl Send for MpvHandle {}
unsafe impl Sync for MpvHandle {}

/// Create a new libmpv handle configured for embedded OpenGL output on a
/// Pi5-class device. Sets `vo=libmpv`, `gpu-api=opengl`, `hwdec=auto-safe`,
/// `cache=yes`, `terminal=no`, `osd-level=0`.
pub fn create_handle() -> MpvHandle {
    unsafe {
        let handle = mpv_create();
        if handle.is_null() {
            panic!("libmpv_inproc: mpv_create() returned null");
        }

        // Pi5-friendly defaults. hwdec=auto-safe lets libmpv pick V4L2/DRM
        // where available without forcing failures on unsupported codecs.
        let opts: &[(&str, &str)] = &[
            ("vo", "libmpv"),
            ("gpu-api", "opengl"),
            ("hwdec", "auto-safe"),
            ("cache", "yes"),
            ("demuxer-max-bytes", "150M"),
            ("demuxer-max-back-bytes", "75M"),
            ("vd-lavc-threads", "0"),
            ("terminal", "no"),
            ("stop-screensaver", "yes"),
        ];
        for (opt, val) in opts {
            let c_opt = CString::new(*opt).unwrap();
            let c_val = CString::new(*val).unwrap();
            mpv_set_property_string(handle, c_opt.as_ptr(), c_val.as_ptr());
        }

        let c_osd = CString::new("osd-level").unwrap();
        let mut zero: i64 = 0;
        mpv_set_property(
            handle,
            c_osd.as_ptr(),
            mpv_format_MPV_FORMAT_INT64,
            &mut zero as *mut _ as *mut c_void,
        );

        if mpv_initialize(handle) < 0 {
            panic!("libmpv_inproc: mpv_initialize() failed");
        }

        let c_warn = CString::new("warn").unwrap();
        mpv_request_log_messages(handle, c_warn.as_ptr());

        MpvHandle(handle)
    }
}

// ---------------------------------------------------------------------------
// MpvRenderCtx: Send + Sync wrapper around `*mut mpv_render_context`.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct MpvRenderCtx(pub *mut mpv_render_context);

impl MpvRenderCtx {
    pub fn get(&self) -> *mut mpv_render_context {
        self.0
    }
}

unsafe impl Send for MpvRenderCtx {}
unsafe impl Sync for MpvRenderCtx {}

/// libmpv calls this to resolve GL function pointers. The `ctx` we pass in is
/// a `&dyn Fn(&CStr) -> *const c_void` reinterpret-cast as a thin pointer.
/// Slint exposes a `get_proc_address` of compatible signature on the
/// `NativeOpenGL` graphics API variant.
unsafe extern "C" fn get_proc_address_trampoline(
    ctx: *mut c_void,
    name: *const std::os::raw::c_char,
) -> *mut c_void {
    if ctx.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let f = &*(ctx as *const &dyn Fn(&CStr) -> *const c_void);
    let cname = CStr::from_ptr(name);
    f(cname) as *mut c_void
}

/// Build an `mpv_render_context` for the given `MpvHandle`, using a
/// caller-supplied OpenGL symbol resolver (typically Slint's
/// `GraphicsAPI::NativeOpenGL { get_proc_address }`).
pub fn create_render_ctx(
    handle: &MpvHandle,
    get_proc_address: &dyn Fn(&CStr) -> *const c_void,
) -> MpvRenderCtx {
    unsafe {
        let api_type = CString::new("opengl").unwrap();

        // We need a stable pointer to the trait object reference itself.
        // Caller must keep `get_proc_address` alive until teardown.
        let trait_ref_ptr: *const &dyn Fn(&CStr) -> *const c_void =
            &get_proc_address as *const _;

        let mut init_params = mpv_opengl_init_params {
            get_proc_address: Some(get_proc_address_trampoline),
            get_proc_address_ctx: trait_ref_ptr as *mut c_void,
            extra_exts: ptr::null(),
        };

        let mut params = [
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                data: api_type.as_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: &mut init_params as *mut _ as *mut c_void,
            },
            mpv_render_param {
                type_: 0,
                data: ptr::null_mut(),
            },
        ];

        let mut ctx: *mut mpv_render_context = ptr::null_mut();
        let res = mpv_render_context_create(&mut ctx, handle.get(), params.as_mut_ptr());
        if res < 0 {
            panic!("libmpv_inproc: mpv_render_context_create failed: {}", res);
        }
        MpvRenderCtx(ctx)
    }
}

// ---------------------------------------------------------------------------
// GLResources: texture + FBO pair we render libmpv into.
// Ported verbatim from amp-dot/amp/src/fbo.rs.
// ---------------------------------------------------------------------------

pub struct GLResources {
    pub gl: glow::Context,
    pub texture: glow::NativeTexture,
    pub fbo: glow::NativeFramebuffer,
    pub width: u32,
    pub height: u32,
}

impl GLResources {
    pub fn new(gl: glow::Context, width: u32, height: u32) -> Self {
        unsafe {
            let texture = gl.create_texture().expect("Failed to create texture");
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));

            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
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
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );

            let fbo = gl.create_framebuffer().expect("Failed to create FBO");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);

            Self {
                gl,
                texture,
                fbo,
                width,
                height,
            }
        }
    }
}

impl Drop for GLResources {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_framebuffer(self.fbo);
            self.gl.delete_texture(self.texture);
        }
    }
}

// ---------------------------------------------------------------------------
// Frame rendering.
// ---------------------------------------------------------------------------

/// Render one libmpv frame into the GLResources FBO. Caller is responsible
/// for invoking this from the GL-active thread (typically inside Slint's
/// `RenderingState::BeforeRendering` callback).
pub fn render_frame(ctx: &MpvRenderCtx, gl: &GLResources) {
    unsafe {
        gl.gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(gl.fbo));
        gl.gl.viewport(0, 0, gl.width as i32, gl.height as i32);
        gl.gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.gl.clear(glow::COLOR_BUFFER_BIT);

        // Construct fbo param. mpv writes into fbo.fbo at fbo.w x fbo.h.
        let mut mpv_fbo = mpv_opengl_fbo {
            fbo: gl.fbo.0.get() as i32,
            w: gl.width as i32,
            h: gl.height as i32,
            internal_format: glow::RGBA8 as i32,
        };
        let mut flip_y: std::os::raw::c_int = 1;

        let mut params = [
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                data: &mut mpv_fbo as *mut _ as *mut c_void,
            },
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                data: &mut flip_y as *mut _ as *mut c_void,
            },
            mpv_render_param {
                type_: 0,
                data: ptr::null_mut(),
            },
        ];

        mpv_render_context_render(ctx.get(), params.as_mut_ptr());

        gl.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    }
}
