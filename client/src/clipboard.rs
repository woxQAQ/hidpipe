//! Native Pasteboard access, including conversion of screenshot TIFF data to PNG.
use hidpipe_proto::clipboard::{Clipboard, Content, MAX_IMAGE_BYTES, MAX_TEXT_BYTES};

use std::ffi::{c_char, c_void, CStr};
use std::io;

pub struct MacClipboard;

type Id = *mut c_void;

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Id;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> Id;
    fn objc_autoreleasePoolPop(pool: Id);
}

// Every invocation specifies the Objective-C method's exact ABI signature.
macro_rules! send {
    ($object:expr, $selector:literal, $result:ty $(, $arg:expr => $ty:ty)*) => {{
        let method: unsafe extern "C" fn(Id, Id $(, $ty)*) -> $result =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        method($object, sel_registerName(concat!($selector, "\0").as_ptr().cast()) $(, $arg)*)
    }};
}

struct Pool(Id);
impl Pool {
    fn new() -> Self {
        // The clipboard worker has no application run loop to drain autoreleases.
        Self(unsafe { objc_autoreleasePoolPush() })
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

unsafe fn class(name: &CStr) -> Id {
    objc_getClass(name.as_ptr())
}

unsafe fn string(value: &CStr) -> Id {
    send!(class(c"NSString"), "stringWithUTF8String:", Id, value.as_ptr() => *const c_char)
}

unsafe fn bytes(data: Id, limit: usize) -> io::Result<Vec<u8>> {
    if data.is_null() {
        return Err(io::Error::other("pasteboard data unavailable"));
    }
    let length = send!(data, "length", usize);
    if length > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "clipboard payload exceeds limit",
        ));
    }
    if length == 0 {
        return Ok(Vec::new());
    }
    let pointer = send!(data, "bytes", *const u8);
    if pointer.is_null() {
        return Err(io::Error::other("pasteboard data unavailable"));
    }
    Ok(std::slice::from_raw_parts(pointer, length).to_vec())
}

impl Clipboard for MacClipboard {
    fn read(&mut self) -> io::Result<Option<Content>> {
        let _pool = Pool::new();
        // AppKit objects stay inside this autorelease pool and worker thread.
        unsafe {
            let board = send!(class(c"NSPasteboard"), "generalPasteboard", Id);
            let png = send!(board, "dataForType:", Id, string(c"public.png") => Id);
            if !png.is_null() {
                return bytes(png, MAX_IMAGE_BYTES).map(Content::Png).map(Some);
            }
            let tiff = send!(board, "dataForType:", Id, string(c"public.tiff") => Id);
            if !tiff.is_null() {
                // Bound encoded input before asking AppKit to decode it.
                if send!(tiff, "length", usize) > MAX_IMAGE_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "clipboard TIFF exceeds limit",
                    ));
                }
                let bitmap = send!(class(c"NSBitmapImageRep"), "imageRepWithData:", Id, tiff => Id);
                if bitmap.is_null() {
                    return Err(io::Error::other("cannot decode clipboard TIFF"));
                }
                let properties = send!(class(c"NSDictionary"), "dictionary", Id);
                // NSBitmapImageFileTypePNG = 4.
                let png = send!(bitmap, "representationUsingType:properties:", Id,
                    4usize => usize, properties => Id);
                return bytes(png, MAX_IMAGE_BYTES).map(Content::Png).map(Some);
            }
            let text = send!(board, "stringForType:", Id, string(c"public.utf8-plain-text") => Id);
            if text.is_null() {
                return Ok(None);
            }
            // NSUTF8StringEncoding = 4; NSData preserves embedded NULs and newlines.
            let data = send!(text, "dataUsingEncoding:", Id, 4usize => usize);
            let text = String::from_utf8(bytes(data, MAX_TEXT_BYTES)?).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "clipboard is not UTF-8")
            })?;
            Ok(Some(Content::Text(text)))
        }
    }

    fn write(&mut self, content: &Content) -> io::Result<()> {
        let _pool = Pool::new();
        unsafe {
            let board = send!(class(c"NSPasteboard"), "generalPasteboard", Id);
            let payload = match content {
                Content::Text(text) => text.as_bytes(),
                Content::Png(bytes) => bytes.as_slice(),
            };
            let data = send!(class(c"NSData"), "dataWithBytes:length:", Id,
                payload.as_ptr() => *const u8, payload.len() => usize);
            if data.is_null() {
                return Err(io::Error::other("cannot allocate pasteboard data"));
            }
            let mime = match content {
                Content::Text(_) => string(c"public.utf8-plain-text"),
                Content::Png(_) => string(c"public.png"),
            };
            send!(board, "clearContents", isize);
            let success = send!(board, "setData:forType:", i8, data => Id, mime => Id);
            if success == 0 {
                Err(io::Error::other("cannot write pasteboard data"))
            } else {
                Ok(())
            }
        }
    }
}
