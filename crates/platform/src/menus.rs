//! Small AppKit additions missing from GPUI's menu API: toggle checkmarks and
//! suppression of the unvalidated, automatically inserted character palette.
use objc2::{
    MainThreadMarker, msg_send,
    runtime::{AnyClass, AnyObject, ClassBuilder, Sel},
    sel,
};
use objc2_app_kit::{NSApplication, NSMenu};
use std::cell::RefCell;

thread_local! {
    static WORD_WRAP: RefCell<Option<Box<dyn Fn() -> bool>>> = RefCell::new(None);
}

extern "C" fn will_open(this: &AnyObject, _: Sel, menu: &NSMenu) {
    // SAFETY: our subclass adds no ivars and the inherited delegate implements
    // menuWillOpen:. Preserve GPUI's pending-keystroke handling first.
    unsafe {
        let parent = AnyClass::get(c"PlastextMenuDelegate")
            .unwrap()
            .superclass()
            .unwrap();
        let _: () = msg_send![super(this, parent), menuWillOpen: menu];
    }
    let wrap = WORD_WRAP.with(|callback| callback.borrow().as_ref().is_some_and(|f| f()));
    for item in menu.itemArray() {
        // Our explicit Emoji & Symbols command is validated by the focused
        // input's GPUI action. AppKit's extra item targets NSApplication and
        // would otherwise remain enabled even in a read-only preview.
        if item.action() == Some(sel!(orderFrontCharacterPalette:)) {
            item.setHidden(true);
        }
        if item.title().to_string() == "Word Wrap" {
            item.setState(if wrap { 1 } else { 0 });
        }
    }
}

/// Install once after the native menus and quit delegate have been installed.
/// The callback only reads UI state and is invoked when a menu opens.
pub fn install(word_wrap: impl Fn() -> bool + 'static) -> Result<(), &'static str> {
    let main = MainThreadMarker::new().ok_or("menu handler requires the main thread")?;
    let app = NSApplication::sharedApplication(main);
    let delegate = app.delegate().ok_or("application has no delegate")?;
    let object: &AnyObject = AsRef::<AnyObject>::as_ref(&*delegate);
    let class = if let Some(class) = AnyClass::get(c"PlastextMenuDelegate") {
        class
    } else {
        let mut builder = ClassBuilder::new(c"PlastextMenuDelegate", object.class())
            .ok_or("cannot create menu delegate subclass")?;
        // SAFETY: NSMenuDelegate's method accepts an NSMenu and returns void.
        unsafe {
            builder.add_method(sel!(menuWillOpen:), will_open as extern "C" fn(_, _, _));
        }
        builder.register()
    };
    // SAFETY: same object layout; only the compatible delegate method changes.
    unsafe {
        AnyObject::set_class(object, class);
    }
    WORD_WRAP.with(|callback| *callback.borrow_mut() = Some(Box::new(word_wrap)));
    Ok(())
}
