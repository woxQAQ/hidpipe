//! Main-display geometry refreshed outside the input thread.

use std::ffi::c_void;
use std::io;
use std::sync::{mpsc, Arc, OnceLock, RwLock};
use std::thread::{self, Thread};

use core_graphics::display::{
    CGDirectDisplayID, CGDisplay, CGDisplayChangeSummaryFlags,
    CGDisplayRegisterReconfigurationCallback,
};
use core_graphics::geometry::CGRect;

#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub display_id: CGDirectDisplayID,
    pub bounds: CGRect,
}

impl PartialEq for Geometry {
    fn eq(&self, other: &Self) -> bool {
        self.display_id == other.display_id
            && self.bounds.origin.x == other.bounds.origin.x
            && self.bounds.origin.y == other.bounds.origin.y
            && self.bounds.size.width == other.bounds.size.width
            && self.bounds.size.height == other.bounds.size.height
    }
}

pub struct DisplayCache {
    geometry: RwLock<Geometry>,
}

// Both callback state and the cache live until process exit. No user-info
// pointer or teardown race is involved when CoreGraphics invokes the callback.
static CACHE: OnceLock<Result<Arc<DisplayCache>, String>> = OnceLock::new();
static WORKER: OnceLock<Thread> = OnceLock::new();

impl DisplayCache {
    pub fn main_display() -> io::Result<&'static Self> {
        CACHE
            .get_or_init(|| {
                let (ready_tx, ready_rx) = mpsc::sync_channel(1);
                thread::Builder::new()
                    .name("display-geometry".into())
                    .spawn(move || {
                        let _ = WORKER.set(thread::current());
                        // SAFETY: the callback accesses only process-lifetime
                        // statics and never dereferences the null user-info.
                        let status = unsafe {
                            CGDisplayRegisterReconfigurationCallback(changed, std::ptr::null())
                        };
                        if status != 0 {
                            let _ = ready_tx.send(Err(format!(
                                "display callback registration failed: {status}"
                            )));
                            return;
                        }
                        let geometry = match query_geometry() {
                            Ok(geometry) => geometry,
                            Err(error) => {
                                let _ = ready_tx.send(Err(error));
                                return;
                            }
                        };
                        let cache = Arc::new(Self {
                            geometry: RwLock::new(geometry),
                        });
                        if ready_tx.send(Ok(Arc::clone(&cache))).is_err() {
                            return;
                        }
                        loop {
                            // Unpark tokens coalesce display notifications and
                            // preserve a notification received during a query.
                            thread::park();
                            let geometry = match query_geometry() {
                                Ok(geometry) => geometry,
                                Err(error) => {
                                    eprintln!(
                                        "[{}] display refresh failed: {error}",
                                        crate::timestamp()
                                    );
                                    continue;
                                }
                            };
                            *cache.geometry.write().unwrap() = geometry;
                            eprintln!(
                                "[{}] display geometry refreshed: id={} bounds={:?}",
                                crate::timestamp(),
                                geometry.display_id,
                                geometry.bounds,
                            );
                        }
                    })
                    .map_err(|error| error.to_string())?;
                ready_rx.recv().map_err(|error| error.to_string())?
            })
            .as_ref()
            .map(Arc::as_ref)
            .map_err(|error| io::Error::other(error.clone()))
    }

    pub fn snapshot(&self) -> Geometry {
        *self.geometry.read().unwrap()
    }
}

unsafe extern "C" fn changed(_display: CGDirectDisplayID, flags: u32, _user_info: *const c_void) {
    if flags & CGDisplayChangeSummaryFlags::kCGDisplayBeginConfigurationFlag.bits() == 0 {
        if let Some(worker) = WORKER.get() {
            worker.unpark();
        }
    }
}

fn query_geometry() -> Result<Geometry, String> {
    let display = CGDisplay::main();
    let bounds = display.bounds();
    if display.id == 0
        || !bounds.origin.x.is_finite()
        || !bounds.origin.y.is_finite()
        || !bounds.size.width.is_finite()
        || !bounds.size.height.is_finite()
        || bounds.size.width < 1.0
        || bounds.size.height < 1.0
        || !(bounds.origin.x + bounds.size.width).is_finite()
        || !(bounds.origin.y + bounds.size.height).is_finite()
    {
        return Err(format!(
            "invalid main display: id={} bounds={bounds:?}",
            display.id
        ));
    }
    Ok(Geometry {
        display_id: display.id,
        bounds,
    })
}
