//! The two places a C pointer becomes a Rust value, and the one where it goes
//! back. Every other line of the crate is safe code.

use std::ffi::{CStr, CString, c_char};

use itsanas_cli::shell::Failure;

/// Read a string Swift passed, or fail with a sentence naming the argument.
///
/// Null is an error rather than an empty string: an app that passed nothing
/// for a passphrase has a bug, and treating it as "" would ask the store to
/// open with the empty passphrase.
pub(crate) fn text(pointer: *const c_char, what: &str) -> Result<String, Failure> {
    if pointer.is_null() {
        return Err(Failure::Usage(format!("{what} was not given")));
    }
    // SAFETY: the pointer is not null, and every entry point documents in
    // `include/itsanas.h` that it takes a NUL-terminated string which stays
    // valid for the duration of the call. Swift's `withCString` and its
    // implicit String-to-pointer bridging both give exactly that. The bytes
    // are copied into a `String` before the call returns.
    let raw = unsafe { CStr::from_ptr(pointer) };
    raw.to_str()
        .map(str::to_owned)
        .map_err(|_| Failure::Usage(format!("{what} was not UTF-8")))
}

/// Hand an answer to Swift. It must come back through [`free`].
pub(crate) fn give(answer: &str) -> *mut c_char {
    // An answer is JSON, which escapes NUL as `\u0000`; a string holding a raw
    // NUL cannot come out of `serde_json`. The fallback is still an answer the
    // app can read rather than a null it would have to special-case.
    CString::new(answer)
        .unwrap_or_else(|_| c"{\"error\":\"the core wrote an unreadable answer\"}".to_owned())
        .into_raw()
}

/// Take back an answer given by [`give`].
pub(crate) fn free(answer: *mut c_char) {
    if answer.is_null() {
        return;
    }
    // SAFETY: `include/itsanas.h` says every `char *` an entry point returns
    // is freed once, with `itsanas_free`, and nothing else; such a pointer was
    // made by `CString::into_raw` in `give` and has not been freed before.
    drop(unsafe { CString::from_raw(answer) });
}
