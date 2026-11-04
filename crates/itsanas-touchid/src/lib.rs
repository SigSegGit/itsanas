//! Asks macOS for the person's fingerprint or password.

#![cfg_attr(target_os = "macos", allow(unsafe_code))]

/// Whether the person at this Mac just proved it. Waits for their answer, for two
/// minutes at most. Nobody answering is a no.
#[cfg(target_os = "macos")]
#[must_use]
pub fn ask(reason: &str) -> bool {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};
    use std::sync::mpsc;
    use std::time::Duration;

    let (said, heard) = mpsc::channel();
    let reply = RcBlock::new(move |ok: Bool, _error: *mut NSError| {
        let _ = said.send(ok.as_bool());
    });
    let reason = NSString::from_str(reason);
    // SAFETY: `LAContext::new` makes an object of the class it names. The reply
    // block is retained by the call until it has run, and takes exactly the
    // arguments Apple documents: a BOOL and an error pointer it does not read.
    unsafe {
        let context = LAContext::new();
        context.evaluatePolicy_localizedReason_reply(
            LAPolicy::DeviceOwnerAuthentication,
            &reason,
            &reply,
        );
    }
    heard
        .recv_timeout(Duration::from_secs(120))
        .unwrap_or(false)
}

/// Nothing to ask on another system.
#[cfg(not(target_os = "macos"))]
#[must_use]
pub const fn ask(_reason: &str) -> bool {
    false
}
