//! GPUI 0.2 does not expose AppKit's magnify event. Forward local pinch events
//! without changing the platform window or intercepting other input.
use async_channel::Receiver;
use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSEvent, NSEventMask, NSView};
use std::{ffi::c_void, ptr::NonNull};

pub struct Pinch {
    pub x: f64,
    pub y: f64,
    pub factor: f64,
}
pub struct PinchMonitor {
    monitor: Retained<AnyObject>,
    _main: MainThreadMarker,
}
impl Drop for PinchMonitor {
    fn drop(&mut self) {
        // SAFETY: the token came from addLocalMonitor below, on the main thread.
        unsafe {
            NSEvent::removeMonitor(&self.monitor);
        }
    }
}
/// # Safety
/// `view` must be a live NSView from the window's AppKit handle. Call and drop
/// the returned monitor on the main thread. The monitor retains the view.
pub unsafe fn pinch_monitor(view: NonNull<c_void>) -> Option<(PinchMonitor, Receiver<Pinch>)> {
    let main = MainThreadMarker::new()?;
    // SAFETY: the caller supplies a live NSView; retain it for the callback.
    let view = unsafe { Retained::retain(view.cast::<NSView>().as_ptr()) }?;
    let (send, receive) = async_channel::bounded(16);
    let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
        // SAFETY: AppKit supplies a live event during this local callback.
        let current = unsafe { event.as_ref() };
        if view
            .window()
            .is_some_and(|window| window.windowNumber() == current.windowNumber())
        {
            let point = view.convertPoint_fromView(current.locationInWindow(), None);
            let y = if view.isFlipped() {
                point.y
            } else {
                view.bounds().size.height - point.y
            };
            let factor = (1. + current.magnification()).clamp(0.1, 10.);
            if factor.is_finite() && point.x.is_finite() && point.y.is_finite() {
                let _ = send.try_send(Pinch {
                    x: point.x,
                    y,
                    factor,
                });
            }
        }
        event.as_ptr()
    });
    // SAFETY: callback returns the unchanged valid event. AppKit copies the block.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::Magnify, &handler)
    }?;
    Some((
        PinchMonitor {
            monitor,
            _main: main,
        },
        receive,
    ))
}
