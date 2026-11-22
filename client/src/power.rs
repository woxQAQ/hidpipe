//! Keep the Mac reachable while it serves as the injection target.
//!
//! With keyboard and mouse attached to the Linux machine, macOS sees no
//! local user activity and idle-sleeps on its own timer. Sleep freezes
//! this process; the server drops the connection seconds later, and the
//! Mac stays unreachable until an unrelated wake. While connected we hold
//! a PreventUserIdleSystemSleep assertion: idle sleep is suppressed, the
//! display still sleeps on its own, and injected input (HID activity)
//! wakes it on the next edge switch. Lid close and low-battery sleep are
//! forced states and stay unaffected.

use std::io;

use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};

type IOPMAssertionID = u32;
type IOReturn = i32;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: CFStringRef,
        assertion_level: u32,
        assertion_name: CFStringRef,
        assertion_id: *mut IOPMAssertionID,
    ) -> IOReturn;
    fn IOPMAssertionRelease(assertion_id: IOPMAssertionID);
}

const IOPM_ASSERTION_LEVEL_ON: u32 = 255;
const IO_RETURN_SUCCESS: IOReturn = 0;

/// A held power assertion. The system stays awake exactly for the
/// lifetime of this value; Drop releases it.
pub struct Assertion(IOPMAssertionID);

impl Assertion {
    /// Suppress idle system sleep. The display is not affected.
    pub fn prevent_idle_system_sleep(reason: &str) -> io::Result<Self> {
        let assertion_type = CFString::from_static_string("PreventUserIdleSystemSleep");
        let name = CFString::new(reason);
        let mut id = 0;
        // SAFETY: both strings are live CFStringRefs; on success the
        // returned id is owned by the new guard.
        let result = unsafe {
            IOPMAssertionCreateWithName(
                assertion_type.as_concrete_TypeRef(),
                IOPM_ASSERTION_LEVEL_ON,
                name.as_concrete_TypeRef(),
                &mut id,
            )
        };
        if result == IO_RETURN_SUCCESS && id != 0 {
            Ok(Self(id))
        } else {
            Err(io::Error::other(format!(
                "IOPMAssertionCreateWithName failed: {result:#x}"
            )))
        }
    }
}

impl Drop for Assertion {
    fn drop(&mut self) {
        // SAFETY: the id is a live assertion created above, released once.
        unsafe { IOPMAssertionRelease(self.0) }
    }
}
