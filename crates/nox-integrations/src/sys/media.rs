//! Small system helpers for covers: raw LZ4 (libcompression, for Firefox's
//! mozLz4 session files) and image pixel size (ImageIO).

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSData, NSDictionary, NSNumber, NSString};
use std::ffi::c_void;

const COMPRESSION_LZ4_RAW: i32 = 0x101;

#[link(name = "compression")]
unsafe extern "C" {
    fn compression_decode_buffer(
        dst: *mut u8,
        dst_size: usize,
        src: *const u8,
        src_size: usize,
        scratch: *mut c_void,
        algorithm: i32,
    ) -> usize;
    fn compression_encode_buffer(
        dst: *mut u8,
        dst_size: usize,
        src: *const u8,
        src_size: usize,
        scratch: *mut c_void,
        algorithm: i32,
    ) -> usize;
}

/// Decodes a raw LZ4 block whose decoded size is known.
pub fn lz4_decode(src: &[u8], decoded_len: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; decoded_len];
    // SAFETY: buffers are valid for the sizes passed; libcompression
    // allocates its own scratch when given NULL.
    let n = unsafe {
        compression_decode_buffer(
            out.as_mut_ptr(),
            out.len(),
            src.as_ptr(),
            src.len(),
            std::ptr::null_mut(),
            COMPRESSION_LZ4_RAW,
        )
    };
    (n == decoded_len).then_some(out)
}

/// Encodes a raw LZ4 block (tests build synthetic mozLz4 files with it).
pub fn lz4_encode(src: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![0u8; src.len() + src.len() / 255 + 64];
    // SAFETY: as in `lz4_decode`.
    let n = unsafe {
        compression_encode_buffer(
            out.as_mut_ptr(),
            out.len(),
            src.as_ptr(),
            src.len(),
            std::ptr::null_mut(),
            COMPRESSION_LZ4_RAW,
        )
    };
    (n > 0).then(|| {
        out.truncate(n);
        out
    })
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithData(data: *const c_void, options: *const c_void) -> *mut c_void;
    fn CGImageSourceCopyPropertiesAtIndex(
        src: *mut c_void,
        index: usize,
        options: *const c_void,
    ) -> *mut c_void;
    static kCGImagePropertyPixelWidth: *const c_void;
    static kCGImagePropertyPixelHeight: *const c_void;
}

/// (width, height) in pixels of an encoded image, if ImageIO can read it.
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let data = NSData::with_bytes(bytes);
    // SAFETY: NSData is toll-free bridged to CFData; Create/Copy results are
    // owned by Retained; the property keys are immutable ImageIO constants.
    unsafe {
        let src = Retained::from_raw(CGImageSourceCreateWithData(
            Retained::as_ptr(&data).cast(),
            std::ptr::null(),
        ) as *mut AnyObject)?;
        let props = Retained::from_raw(CGImageSourceCopyPropertiesAtIndex(
            Retained::as_ptr(&src) as *mut c_void,
            0,
            std::ptr::null(),
        ) as *mut NSDictionary<NSString, AnyObject>)?;
        let num = |k: *const c_void| {
            props
                .objectForKey(&*(k as *const NSString))
                .and_then(|v| v.downcast_ref::<NSNumber>().map(|n| n.unsignedIntValue()))
        };
        Some((
            num(kCGImagePropertyPixelWidth)?,
            num(kCGImagePropertyPixelHeight)?,
        ))
    }
}

/// Case-, diacritic- and width-insensitive folding (Foundation).
pub fn fold(s: &str) -> String {
    use objc2_foundation::NSStringCompareOptions as O;
    NSString::from_str(s)
        .stringByFoldingWithOptions_locale(
            O::CaseInsensitiveSearch | O::DiacriticInsensitiveSearch | O::WidthInsensitiveSearch,
            None,
        )
        .to_string()
}

/// The user's region code ("FR"), for the right iTunes storefront.
pub fn region() -> String {
    let l = objc2_foundation::NSLocale::currentLocale();
    l.regionCode()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "US".into())
}
