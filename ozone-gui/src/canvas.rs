//! `Send`/`Sync` canvas wrappers.
//!
//! Aurea's `Canvas` is not `Send`, but Ozone shares one canvas between the
//! window content and the draw callback behind an `Arc<Mutex<…>>`. These
//! wrappers assert the threading contract (the canvas is only ever touched
//! under the lock on the UI thread) and forward `Element` to the inner canvas.

use std::os::raw::c_void;
use std::sync::{Arc, Mutex};

use aurea::Element;
use aurea::render::{Canvas, Rect};

use crate::lock;

/// A `Canvas` marked `Send`/`Sync` so it can live in an `Arc<Mutex<…>>` shared
/// with the draw callback. Sound because every access goes through the mutex.
pub(crate) struct SendableCanvas(pub(crate) Canvas);
unsafe impl Send for SendableCanvas {}
unsafe impl Sync for SendableCanvas {}

impl std::ops::Deref for SendableCanvas {
    type Target = Canvas;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for SendableCanvas {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Element for SendableCanvas {
    fn handle(&self) -> *mut c_void {
        self.0.handle()
    }
    unsafe fn invalidate_platform(&self, rect: Option<Rect>) {
        unsafe { Element::invalidate_platform(&self.0, rect) }
    }
}

/// The window-content handle: a shared `SendableCanvas`. Set as the window's
/// content so platform input/paint reach the same canvas the run loop draws to.
pub(crate) struct SharedCanvas(pub(crate) Arc<Mutex<SendableCanvas>>);
impl Element for SharedCanvas {
    fn handle(&self) -> *mut c_void {
        lock(self.0.as_ref()).handle()
    }
    unsafe fn invalidate_platform(&self, rect: Option<Rect>) {
        let g = lock(self.0.as_ref());
        unsafe { Element::invalidate_platform(&*g, rect) }
    }
}
