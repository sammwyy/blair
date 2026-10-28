//! Serializes EGL tests to avoid concurrent driver context setup and teardown.

use std::sync::{Mutex, MutexGuard};

use smithay::backend::{
    egl::{EGLContext, EGLDevice, EGLDisplay},
    renderer::gles::GlesRenderer,
};

static EGL_TEST: Mutex<()> = Mutex::new(());

/// Declared before test resources so EGL teardown finishes before the next test.
#[must_use]
pub struct EglTest {
    _guard: MutexGuard<'static, ()>,
}

impl EglTest {
    pub fn new() -> Self {
        Self {
            _guard: EGL_TEST.lock().unwrap_or_else(|error| error.into_inner()),
        }
    }

    pub fn renderer(&self) -> GlesRenderer {
        let device = EGLDevice::enumerate().unwrap().next().unwrap();
        let display = unsafe { EGLDisplay::new(device).unwrap() };
        let context = EGLContext::new(&display).unwrap();
        unsafe { GlesRenderer::new(context).unwrap() }
    }
}
