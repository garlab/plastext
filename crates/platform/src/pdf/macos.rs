//! Minimal Core Graphics PDF bindings. Native objects never leave the worker.
use super::{PageInfo, SCALE_UNIT, Tile, TilePixels};
use core_graphics::{
    base::{kCGBitmapByteOrder32Little, kCGImageAlphaPremultipliedFirst},
    color_space::CGColorSpace,
    context::CGContext,
    geometry::{CGAffineTransform, CGPoint, CGRect, CGSize},
};
use foreign_types::ForeignType;
use std::{
    ffi::{CString, c_char, c_void},
    os::unix::ffi::OsStrExt,
    path::Path,
    ptr,
};

type PdfRef = *mut c_void;
type PageRef = *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDataProviderCreateWithFilename(filename: *const c_char) -> *mut c_void;
    fn CGDataProviderRelease(provider: *mut c_void);
    fn CGPDFDocumentCreateWithProvider(provider: *mut c_void) -> PdfRef;
    fn CGPDFDocumentRelease(document: PdfRef);
    fn CGPDFDocumentGetNumberOfPages(document: PdfRef) -> usize;
    fn CGPDFDocumentIsUnlocked(document: PdfRef) -> bool;
    fn CGPDFDocumentGetPage(document: PdfRef, page: usize) -> PageRef;
    fn CGPDFPageGetBoxRect(page: PageRef, box_type: i32) -> CGRect;
    fn CGPDFPageGetRotationAngle(page: PageRef) -> i32;
    fn CGPDFPageGetDrawingTransform(
        page: PageRef,
        box_type: i32,
        rect: CGRect,
        rotate: i32,
        preserve_aspect: bool,
    ) -> CGAffineTransform;
    fn CGContextDrawPDFPage(context: core_graphics::sys::CGContextRef, page: PageRef);
}

pub(super) struct NativeDocument(PdfRef);
impl Drop for NativeDocument {
    fn drop(&mut self) {
        // SAFETY: this object owns the non-null Create-rule document reference.
        unsafe {
            CGPDFDocumentRelease(self.0);
        }
    }
}
impl NativeDocument {
    pub fn open(path: &Path) -> Result<Self, String> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: NUL-terminated path; release the provider after the document retains it.
        let document = unsafe {
            let provider = CGDataProviderCreateWithFilename(path.as_ptr());
            if provider.is_null() {
                return Err("Cannot read PDF snapshot".into());
            }
            let document = CGPDFDocumentCreateWithProvider(provider);
            CGDataProviderRelease(provider);
            document
        };
        if document.is_null() {
            return Err("Cannot parse this PDF; Binary is still available".into());
        }
        let document = Self(document);
        // SAFETY: live owned document; this call does not execute PDF actions.
        if !unsafe { CGPDFDocumentIsUnlocked(document.0) } {
            return Err(
                "Password-protected PDF; password entry is not supported yet. Use Binary.".into(),
            );
        }
        Ok(document)
    }

    fn page(&self, page: u32) -> Result<PageRef, String> {
        if page == 0 {
            return Err("PDF page numbers start at 1".into());
        }
        // SAFETY: live document; borrowed page remains valid while self is alive.
        let page = unsafe { CGPDFDocumentGetPage(self.0, page as usize) };
        if page.is_null() {
            Err("PDF page is missing".into())
        } else {
            Ok(page)
        }
    }

    pub fn info(&self, number: u32) -> Result<PageInfo, String> {
        let page = self.page(number)?;
        // SAFETY: all references are live, borrowed from self on the worker thread.
        let (pages, crop, rotation) = unsafe {
            (
                CGPDFDocumentGetNumberOfPages(self.0),
                CGPDFPageGetBoxRect(page, 1),
                CGPDFPageGetRotationAngle(page),
            )
        };
        let (width, height) = if rotation.rem_euclid(180) == 90 {
            (crop.size.height, crop.size.width)
        } else {
            (crop.size.width, crop.size.height)
        };
        PageInfo {
            pages: pages.try_into().map_err(|_| "Too many PDF pages")?,
            page: number,
            width,
            height,
        }
        .validate()
    }

    pub fn render(&self, tile: Tile) -> Result<TilePixels, String> {
        let info = self.info(tile.page)?;
        let (width, height) = tile.dimensions(info)?;
        let page = self.page(tile.page)?;
        let scale = tile.scale as f64 / SCALE_UNIT as f64;
        let mut context = CGContext::create_bitmap_context(
            None,
            width as usize,
            height as usize,
            8,
            width as usize * 4,
            &CGColorSpace::create_device_rgb(),
            kCGBitmapByteOrder32Little | kCGImageAlphaPremultipliedFirst,
        );
        context.set_rgb_fill_color(1., 1., 1., 1.);
        context.fill_rect(CGRect::new(
            &CGPoint::new(0., 0.),
            &CGSize::new(width as f64, height as f64),
        ));
        // A bitmap's row zero is the page's top. Map the complete rotated crop box
        // into the tile's clipped drawing surface without allocating a full page.
        context.translate(
            -(tile.x as f64),
            tile.y as f64 + height as f64 - info.height * scale,
        );
        // GetDrawingTransform only scales DOWN. Apply raster zoom explicitly;
        // otherwise Retina/high-zoom pages are centered at their unscaled size.
        context.scale(scale, scale);
        let rect = CGRect::new(&CGPoint::new(0., 0.), &CGSize::new(info.width, info.height));
        // SAFETY: valid borrowed page and bitmap context; Core Graphics handles
        // crop offsets and /Rotate. The context clips output to at most 512² pixels.
        unsafe {
            context.concat_ctm(CGPDFPageGetDrawingTransform(page, 1, rect, 0, true));
            CGContextDrawPDFPage(context.as_ptr(), page);
        }
        context.flush();
        Ok(TilePixels {
            tile,
            width,
            height,
            bgra: context.data().to_vec(),
        })
    }
}

pub(super) fn resident_bytes(pid: u32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
    // SAFETY: proc_pidinfo initializes this exact structure on a full-size result.
    let count = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if count == size {
        Some(unsafe { info.assume_init() }.pti_resident_size)
    } else {
        None
    }
}

pub(super) fn restrict_worker() -> Result<(), String> {
    unsafe extern "C" {
        fn sandbox_init(profile: *const c_char, flags: u64, error: *mut *mut c_char) -> i32;
        fn sandbox_free_error(error: *mut c_char);
    }
    // Seatbelt is deprecated, so this is defense in depth, not the future TeX
    // sandbox. Fail closed if it cannot be installed. Reads remain available to
    // Core Graphics/font services; writes, networking and child execution do not.
    let profile = c"(version 1)(allow default)(deny network*)(deny file-write*)(deny process-exec)(deny process-fork)";
    let mut error = ptr::null_mut();
    // SAFETY: constant NUL-terminated profile; error is allocated by sandbox_init.
    let result = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) };
    if result != 0 {
        let message = if error.is_null() {
            "Cannot restrict PDF worker".into()
        } else {
            unsafe { std::ffi::CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        };
        unsafe {
            sandbox_free_error(error);
        }
        return Err(message);
    }
    // SAFETY: process-local priority and core-dump resource limits.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
        libc::setrlimit(
            libc::RLIMIT_CORE,
            &libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            },
        );
    }
    Ok(())
}
