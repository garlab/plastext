//! ImageIO objects stay on the decoder process's single thread.
use super::{Frame, Info, Pixels, Region};
use core_foundation::{
    base::{CFGetTypeID, CFRelease, CFType, TCFType},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFDictionaryGetTypeID, CFDictionaryGetValue, CFDictionaryRef},
    number::{CFNumber, CFNumberGetTypeID, CFNumberGetValue, kCFNumberDoubleType},
    string::{CFString, CFStringGetCString, CFStringGetTypeID, CFStringRef, kCFStringEncodingUTF8},
    url::CFURL,
};
use core_graphics::{
    base::{kCGBitmapByteOrder32Big, kCGImageAlphaLast, kCGImageAlphaPremultipliedLast},
    color_space::{CGColorSpace, kCGColorSpaceSRGB},
    context::{CGContext, CGInterpolationQuality},
    data_provider::CGDataProvider,
    geometry::{CGPoint, CGRect, CGSize},
    image::CGImage,
};
use foreign_types::ForeignType;
use std::{cell::Cell, ffi::c_void, path::Path, ptr, sync::Arc};
type Ref = *const c_void;
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceGetModel(space: core_graphics::sys::CGColorSpaceRef) -> i32;
    fn CGColorSpaceGetBaseColorSpace(
        space: core_graphics::sys::CGColorSpaceRef,
    ) -> core_graphics::sys::CGColorSpaceRef;
}
#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithURL(url: Ref, options: CFDictionaryRef) -> Ref;
    fn CGImageSourceCopyProperties(source: Ref, options: CFDictionaryRef) -> Ref;
    fn CGImageSourceCopyPropertiesAtIndex(
        source: Ref,
        index: usize,
        options: CFDictionaryRef,
    ) -> Ref;
    fn CGImageSourceGetCount(source: Ref) -> usize;
    fn CGImageSourceGetPrimaryImageIndex(source: Ref) -> usize;
    fn CGImageSourceGetType(source: Ref) -> CFStringRef;
    fn CGImageSourceCreateImageAtIndex(
        source: Ref,
        index: usize,
        options: CFDictionaryRef,
    ) -> core_graphics::sys::CGImageRef;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: Ref,
        index: usize,
        options: CFDictionaryRef,
    ) -> core_graphics::sys::CGImageRef;
    fn CGImageSourceRemoveCacheAtIndex(source: Ref, index: usize);
    static kCGImageSourceShouldCache: CFStringRef;
    static kCGImageSourceCreateThumbnailFromImageAlways: CFStringRef;
    static kCGImageSourceThumbnailMaxPixelSize: CFStringRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFStringRef;
    static kCGImagePropertyPixelWidth: CFStringRef;
    static kCGImagePropertyPixelHeight: CFStringRef;
    static kCGImagePropertyOrientation: CFStringRef;
    static kCGImagePropertyProfileName: CFStringRef;
    static kCGImagePropertyColorModel: CFStringRef;
    static kCGImagePropertyGIFDictionary: CFStringRef;
    static kCGImagePropertyWebPDictionary: CFStringRef;
    static kCGImagePropertyPNGDictionary: CFStringRef;
}
struct Owned(Ref);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CFRelease(self.0);
            }
        }
    }
}
fn get(dict: Ref, key: CFStringRef) -> Ref {
    // SAFETY: inspect CF runtime types before reading metadata from untrusted files.
    unsafe {
        if dict.is_null() || CFGetTypeID(dict) != CFDictionaryGetTypeID() {
            ptr::null()
        } else {
            CFDictionaryGetValue(dict.cast(), key.cast())
        }
    }
}
fn number(value: Ref) -> Option<f64> {
    let mut result = 0.;
    unsafe {
        if !value.is_null()
            && CFGetTypeID(value) == CFNumberGetTypeID()
            && CFNumberGetValue(
                value.cast(),
                kCFNumberDoubleType,
                (&mut result as *mut f64).cast(),
            )
            && f64::is_finite(result)
        {
            Some(result)
        } else {
            None
        }
    }
}
fn string(value: Ref) -> String {
    let mut bytes = [0i8; 256];
    unsafe {
        if !value.is_null()
            && CFGetTypeID(value) == CFStringGetTypeID()
            && CFStringGetCString(
                value.cast(),
                bytes.as_mut_ptr(),
                bytes.len() as isize,
                kCFStringEncodingUTF8,
            ) != 0
        {
            std::ffi::CStr::from_ptr(bytes.as_ptr())
                .to_string_lossy()
                .into_owned()
        } else {
            String::new()
        }
    }
}
fn options(edge: Option<u32>) -> CFDictionary<CFString, CFType> {
    unsafe {
        let mut pairs = vec![(
            CFString::wrap_under_get_rule(kCGImageSourceShouldCache),
            CFBoolean::false_value().as_CFType(),
        )];
        if let Some(edge) = edge {
            pairs.extend([
                (
                    CFString::wrap_under_get_rule(kCGImageSourceCreateThumbnailFromImageAlways),
                    CFBoolean::true_value().as_CFType(),
                ),
                (
                    CFString::wrap_under_get_rule(kCGImageSourceCreateThumbnailWithTransform),
                    CFBoolean::true_value().as_CFType(),
                ),
                (
                    CFString::wrap_under_get_rule(kCGImageSourceThumbnailMaxPixelSize),
                    CFNumber::from(edge as i32).as_CFType(),
                ),
            ]);
        }
        CFDictionary::from_CFType_pairs(&pairs)
    }
}
pub(super) struct Native {
    source: Owned,
    info: Info,
    pub orientation: u32,
    animation: CFStringRef,
    last: Cell<Option<u32>>,
    base_index: usize,
}
impl Native {
    pub fn open(path: &Path) -> Result<Self, String> {
        let url = CFURL::from_path(path, false).ok_or("Invalid image path")?;
        let opts = options(None);
        let source = Owned(unsafe {
            CGImageSourceCreateWithURL(url.as_CFTypeRef(), opts.as_concrete_TypeRef())
        });
        if source.0.is_null() {
            return Err("No installed ImageIO decoder supports this image".into());
        }
        let all =
            Owned(unsafe { CGImageSourceCopyProperties(source.0, opts.as_concrete_TypeRef()) });
        let uti = string(unsafe { CGImageSourceGetType(source.0) }.cast());
        let format = uti
            .rsplit('.')
            .next()
            .unwrap_or("Image")
            .to_ascii_uppercase();
        let animation = unsafe {
            if uti.contains("gif") {
                kCGImagePropertyGIFDictionary
            } else if uti.contains("webp") {
                kCGImagePropertyWebPDictionary
            } else if uti.contains("png") {
                kCGImagePropertyPNGDictionary
            } else {
                ptr::null()
            }
        };
        let count = unsafe { CGImageSourceGetCount(source.0) };
        if count == 0 || count > 10_000 {
            return Err("Image frame count exceeds preview limits".into());
        }
        let mut base_index = if animation.is_null() {
            unsafe { CGImageSourceGetPrimaryImageIndex(source.0) }.min(count - 1)
        } else {
            0
        };
        if uti.ends_with(".ico") || uti.ends_with(".icns") {
            let mut largest = 0.;
            for index in 0..count.min(256) {
                let props = Owned(unsafe {
                    CGImageSourceCopyPropertiesAtIndex(source.0, index, opts.as_concrete_TypeRef())
                });
                let pixels = number(get(props.0, unsafe { kCGImagePropertyPixelWidth }))
                    .unwrap_or(0.)
                    * number(get(props.0, unsafe { kCGImagePropertyPixelHeight })).unwrap_or(0.);
                if pixels > largest {
                    largest = pixels;
                    base_index = index;
                }
            }
        }
        let props = Owned(unsafe {
            CGImageSourceCopyPropertiesAtIndex(source.0, base_index, opts.as_concrete_TypeRef())
        });
        let width = number(get(props.0, unsafe { kCGImagePropertyPixelWidth }))
            .ok_or("Missing image width")? as u32;
        let height = number(get(props.0, unsafe { kCGImagePropertyPixelHeight }))
            .ok_or("Missing image height")? as u32;
        let orientation = number(get(props.0, unsafe { kCGImagePropertyOrientation }))
            .unwrap_or(1.)
            .clamp(1., 8.) as u32;
        let (width, height) = if orientation >= 5 {
            (height, width)
        } else {
            (width, height)
        };
        let frames = if animation.is_null() { 1 } else { count as u32 };
        let loop_key = CFString::new("LoopCount");
        let loops = if animation.is_null() {
            None
        } else {
            number(get(get(all.0, animation), loop_key.as_concrete_TypeRef()))
                .or_else(|| number(get(get(props.0, animation), loop_key.as_concrete_TypeRef())))
        };
        let plays = match loops {
            Some(0.) => None,
            // ImageIO normalizes GIF's repeat count into total plays already.
            Some(n) => Some(n.max(1.) as u32),
            None => Some(1),
        };
        let mut color = string(get(props.0, unsafe { kCGImagePropertyProfileName }));
        if color.is_empty() {
            color = string(get(props.0, unsafe { kCGImagePropertyColorModel }));
        }
        let info = Info {
            width,
            height,
            frames,
            plays,
            format,
            color,
        };
        info.validate()?;
        Ok(Self {
            source,
            info,
            orientation,
            animation,
            last: Cell::new(None),
            base_index,
        })
    }
    pub fn info(&self) -> Info {
        self.info.clone()
    }
    /// ImageIO supplies the PNG's color space without materializing its pixels.
    /// Apply that profile to the bounded scanline outputs so tiles and the native
    /// overview have the same color, including grayscale and indexed PNGs.
    pub fn colorize_png(&self, pixels: Vec<Pixels>) -> Result<Vec<Pixels>, String> {
        let opts = options(None);
        let image = unsafe {
            CGImageSourceCreateImageAtIndex(self.source.0, 0, opts.as_concrete_TypeRef())
        };
        if image.is_null() {
            return Err("Cannot read PNG color profile".into());
        }
        let image = unsafe { CGImage::from_ptr(image) };
        let mut source = image.color_space();
        if unsafe { CGColorSpaceGetModel(source.as_ptr()) } == 5 {
            let base = unsafe { CGColorSpaceGetBaseColorSpace(source.as_ptr()) };
            if base.is_null() {
                return Err("Invalid PNG palette color space".into());
            }
            unsafe {
                core_foundation::base::CFRetain(base.cast());
                source = CGColorSpace::from_ptr(base);
            }
        }
        let gray = match unsafe { CGColorSpaceGetModel(source.as_ptr()) } {
            0 => true,
            1 => false,
            _ => return Err("Unsupported PNG color space".into()),
        };
        let destination = CGColorSpace::create_with_name(unsafe { kCGColorSpaceSRGB })
            .ok_or("Cannot create image color space")?;
        pixels
            .into_iter()
            .map(|p| {
                let r = p.region;
                let bytes = if gray {
                    p.rgba
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|p| [p[0], p[3]])
                        .collect()
                } else {
                    p.rgba
                };
                let channels = if gray { 2 } else { 4 };
                let provider = CGDataProvider::from_buffer(Arc::new(bytes));
                let image = CGImage::new(
                    r.width as usize,
                    r.height as usize,
                    8,
                    channels * 8,
                    r.width as usize * channels,
                    &source,
                    kCGImageAlphaLast,
                    &provider,
                    false,
                    0,
                );
                let mut context = CGContext::create_bitmap_context(
                    None,
                    r.width as usize,
                    r.height as usize,
                    8,
                    r.width as usize * 4,
                    &destination,
                    kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
                );
                context
                    .set_interpolation_quality(CGInterpolationQuality::CGInterpolationQualityNone);
                context.draw_image(
                    CGRect::new(
                        &CGPoint::new(0., 0.),
                        &CGSize::new(r.width as f64, r.height as f64),
                    ),
                    &image,
                );
                context.flush();
                Ok(Pixels {
                    region: r,
                    rgba: straight_rgba(context.data()),
                })
            })
            .collect()
    }
    pub fn render(&self, regions: &[Region]) -> Result<Frame, String> {
        plastext_assets::imaging::validate_regions(&self.info, regions)?;
        let index = regions[0].frame + self.base_index as u32;
        if let Some(previous) = self.last.replace(Some(index))
            && previous != index
        {
            unsafe {
                CGImageSourceRemoveCacheAtIndex(self.source.0, previous.saturating_sub(1) as usize);
                if index != previous + 1 {
                    CGImageSourceRemoveCacheAtIndex(self.source.0, previous as usize);
                }
            }
        }
        let whole = regions.len() == 1
            && regions[0].x == 0
            && regions[0].y == 0
            && (regions[0].width, regions[0].height) == self.info.scaled_size(regions[0].scale);
        let edge = if whole {
            Some(regions[0].width.max(regions[0].height))
        } else if self.orientation != 1 {
            Some(self.info.width.max(self.info.height))
        } else {
            None
        };
        let opts = options(edge);
        let image = unsafe {
            let image = if edge.is_some() {
                CGImageSourceCreateThumbnailAtIndex(
                    self.source.0,
                    index as usize,
                    opts.as_concrete_TypeRef(),
                )
            } else {
                CGImageSourceCreateImageAtIndex(
                    self.source.0,
                    index as usize,
                    opts.as_concrete_TypeRef(),
                )
            };
            if image.is_null() {
                return Err("ImageIO could not decode this frame".into());
            }
            CGImage::from_ptr(image)
        };
        let space = CGColorSpace::create_with_name(unsafe { kCGColorSpaceSRGB })
            .ok_or("Cannot create image color space")?;
        let mut pixels = Vec::with_capacity(regions.len());
        for &region in regions {
            let mut context = CGContext::create_bitmap_context(
                None,
                region.width as usize,
                region.height as usize,
                8,
                region.width as usize * 4,
                &space,
                kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
            );
            if whole {
                if region.scale >= super::SCALE_UNIT {
                    context.set_interpolation_quality(
                        CGInterpolationQuality::CGInterpolationQualityNone,
                    );
                }
                context.draw_image(
                    CGRect::new(
                        &CGPoint::new(0., 0.),
                        &CGSize::new(region.width as f64, region.height as f64),
                    ),
                    &image,
                );
            } else {
                let scale = region.scale as f64 / super::SCALE_UNIT as f64;
                let sx = region.x as f64 / scale;
                let sy = region.y as f64 / scale;
                // Keep codec/chroma blocks intact around the crop. ImageIO's
                // HEIF/AVIF region decoder can shift subsampled color on odd
                // crop origins; clipping the aligned result avoids tile seams.
                let alignment = 64.;
                let left = (sx / alignment).floor() * alignment;
                let top = (sy / alignment).floor() * alignment;
                let right = (((sx + region.width as f64 / scale) / alignment).ceil() * alignment)
                    .min(image.width() as f64);
                let bottom = (((sy + region.height as f64 / scale) / alignment).ceil() * alignment)
                    .min(image.height() as f64);
                let crop = image
                    .cropped(CGRect::new(
                        &CGPoint::new(left, top),
                        &CGSize::new(right - left, bottom - top),
                    ))
                    .ok_or("Invalid image crop")?;
                context.set_interpolation_quality(if scale >= 1. {
                    CGInterpolationQuality::CGInterpolationQualityNone
                } else {
                    CGInterpolationQuality::CGInterpolationQualityHigh
                });
                context.draw_image(
                    CGRect::new(
                        &CGPoint::new(
                            (left - sx) * scale,
                            region.height as f64 - (bottom - sy) * scale,
                        ),
                        &CGSize::new(crop.width() as f64 * scale, crop.height() as f64 * scale),
                    ),
                    &crop,
                );
            }
            context.flush();
            let rgba = straight_rgba(context.data());
            pixels.push(Pixels { region, rgba });
        }
        let delay_ms = if self.animation.is_null() {
            100
        } else {
            let props = Owned(unsafe {
                CGImageSourceCopyPropertiesAtIndex(self.source.0, index as usize, ptr::null())
            });
            let dict = get(props.0, self.animation);
            let key = CFString::new("UnclampedDelayTime");
            let fallback = CFString::new("DelayTime");
            plastext_assets::imaging::frame_delay(
                number(get(dict, key.as_concrete_TypeRef()))
                    .or_else(|| number(get(dict, fallback.as_concrete_TypeRef())))
                    .unwrap_or(0.1),
            )
        };
        Ok(Frame { pixels, delay_ms })
    }
}

fn straight_rgba(bytes: &[u8]) -> Vec<u8> {
    let mut rgba = bytes.to_vec();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let alpha = pixel[3] as u32;
        if alpha != 0 && alpha != 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    rgba
}
