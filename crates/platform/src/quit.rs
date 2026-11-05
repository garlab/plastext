//! Route AppKit termination requests through the application's dirty-file check.
//! GPUI's on_app_quit runs after termination is irrevocable, so it cannot guard edits.
use objc2::{
    MainThreadMarker,
    runtime::{AnyClass, AnyObject, ClassBuilder, Sel},
    sel,
};
use objc2_app_kit::NSApplication;
use std::cell::{Cell, RefCell};

thread_local! {
    static REQUEST: RefCell<Option<Box<dyn Fn()>>> = RefCell::new(None);
    static ALLOWED: Cell<bool> = const { Cell::new(false) };
}

extern "C" fn should_terminate(_: &AnyObject, _: Sel, _: &AnyObject) -> usize {
    if ALLOWED.with(|allowed| allowed.replace(false)) {
        return 1;
    } // NSTerminateNow
    REQUEST.with(|request| {
        if let Some(request) = request.borrow().as_ref() {
            request();
        }
    });
    0 // NSTerminateCancel; the app can ask again after its asynchronous prompt.
}

/// Install once on the UI thread. The callback must enqueue work, not enter GPUI
/// synchronously: AppKit may invoke this while an application action is running.
pub fn install(request: impl Fn() + 'static) -> Result<(), &'static str> {
    let main = MainThreadMarker::new().ok_or("quit handler requires the main thread")?;
    let application = NSApplication::sharedApplication(main);
    let delegate = application
        .delegate()
        .ok_or("application has no delegate")?;
    let object: &AnyObject = AsRef::<AnyObject>::as_ref(&*delegate);
    let original = object.class();
    let class = if let Some(class) = AnyClass::get(c"PlastextApplicationDelegate") {
        class
    } else {
        let mut builder = ClassBuilder::new(c"PlastextApplicationDelegate", original)
            .ok_or("cannot create application delegate subclass")?;
        // SAFETY: AppKit's method takes an application object and returns an
        // NSUInteger termination reply. No ivars are added; inherited GPUI
        // callbacks and their platform pointer keep their original layout.
        unsafe {
            builder.add_method(
                sel!(applicationShouldTerminate:),
                should_terminate as extern "C" fn(_, _, _) -> _,
            );
        }
        builder.register()
    };
    // SAFETY: the new class directly subclasses the current delegate, has the
    // same instance size and overrides only the compatible AppKit callback.
    // Installation happens on the main thread before editing is available.
    unsafe {
        AnyObject::set_class(object, class);
    }
    REQUEST.with(|callback| *callback.borrow_mut() = Some(Box::new(request)));
    Ok(())
}

/// Allow the next native termination only after the application approved it.
pub fn allow() {
    ALLOWED.with(|allowed| allowed.set(true));
}
