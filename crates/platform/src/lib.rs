//! OS integration behind traits: URL scheme, Trash, reveal, IPC and file associations (spec §3.3).

#[cfg(target_os = "macos")]
pub mod gestures;
pub mod images;
pub mod instance;
pub mod mermaid;
pub mod pdf;
pub mod tex;

/// Set the running macOS application's Dock / app-switcher icon, including when
/// launched as a bare executable. Call on the UI thread after AppKit starts.
#[cfg(target_os = "macos")]
pub fn set_application_icon(bytes: &[u8]) -> Result<(), &'static str> {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let main_thread = MainThreadMarker::new().ok_or("App icon must be set on the main thread")?;
    let data = NSData::with_bytes(bytes);
    let image =
        NSImage::initWithData(NSImage::alloc(), &data).ok_or("Cannot decode the app icon")?;
    if !image.isValid() {
        return Err("App icon contains no valid image representations");
    }
    let application = NSApplication::sharedApplication(main_thread);
    // SAFETY: AppKit is accessed on the main thread and receives a valid,
    // non-null image. NSApplication retains it after this function returns.
    unsafe {
        application.setApplicationIconImage(Some(&image));
    }
    Ok(())
}
