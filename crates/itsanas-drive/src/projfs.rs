//! The Windows Projected File System, bound directly.
//!
//! # Why this is written here rather than taken from crates.io
//!
//! It was taken from crates.io. `projfs 0.1.2` is MIT, three hundred lines, and
//! it worked: the account appeared in Explorer and a file opened. Then
//! `cargo deny` failed the build on **RUSTSEC-2022-0040** — `projfs` depends on
//! `chashmap`, which depends on `owning_ref 0.3.3`, which is unsound in four
//! documented ways, has no patched version, and whose maintainer is
//! unresponsive. It was there for one type alias: a concurrent map holding one
//! directory cursor per enumeration.
//!
//! So the choice was between shipping a known use-after-free in a program whose
//! entire posture is `unsafe_code = "forbid"`, dropping a feature that works, or
//! writing the two hundred lines that call five documented Win32 functions. The
//! third is the only one that keeps both the feature and the claim.
//!
//! `projfs-sys` is kept — it is the generated header, MIT, with no dependencies
//! of its own and nothing to be unsound about.
//!
//! # Three faults in the original that are fixed here
//!
//! Not a criticism of a useful crate; the point is that owning this code means
//! owning these, and they are the reason a wrapper is not a formality.
//!
//! 1. **A panic could unwind into Windows.** No callback caught one, and
//!    `CallbackDataFlags::from_bits(data.Flags).unwrap()` panics the moment
//!    Windows sets a flag bit the crate does not know about. Unwinding across
//!    an `extern "C"` boundary is undefined behaviour. Every callback here runs
//!    inside `catch_unwind` and answers with an error code, the same discipline
//!    `itsanas-android` uses at the JVM boundary.
//! 2. **The instance was freed before virtualization stopped.** `Drop` ran
//!    `Box::from_raw(self.this)` and *then* `PrjStopVirtualizing`, so a callback
//!    arriving in that window read freed memory. Here the order is reversed,
//!    which is the only order that is correct.
//! 3. **Unknown flags were fatal rather than ignored.** A flag this code does
//!    not understand means "there is a feature here we do not use", not "stop".
//!
//! # What is not bound
//!
//! Writing. `ProjFS` reports a file dropped into the folder through
//! `NotificationCallback`, and honouring it means deciding what a local edit
//! does to an account that other machines also hold. `itsanas put` and
//! `itsanas folder` are how things get in, and the folder is read-only until
//! that question has an answer rather than an implementation.

#![cfg(windows)]
// The one place in this crate where unsafe is allowed, and every use of it is
// a call into a documented Win32 function or a dereference of a pointer that
// function just handed us. `scripts/check-unsafe.py` names this crate.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::{OsString, c_void};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::ptr;
use std::sync::Mutex;

use projfs_sys as sys;

/// One entry as Windows wants to hear about it.
///
/// Deliberately smaller than `PRJ_FILE_BASIC_INFO`: the fields left out are the
/// ones an account has no honest answer for. A creation time invented from the
/// clock is worse than a zero, because a zero is visibly absent and an invented
/// timestamp is believed.
#[derive(Debug, Clone)]
pub struct Info {
    /// The name within its directory, not a path.
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Last write, in Windows FILETIME units. Zero when unknown.
    pub written: i64,
}

/// What the folder shows, asked one question at a time.
///
/// `Sync` because Windows calls these from its own threads and may have several
/// enumerations open at once.
pub trait Source: Sync {
    /// Everything directly inside `directory`, which is `""` for the root and
    /// otherwise a backslash-separated path relative to it.
    ///
    /// # Errors
    ///
    /// Whatever the account could not answer. It reaches Windows as an error
    /// code and Explorer shows an empty or unopenable folder, so `NotFound`
    /// for a directory that is not there is worth more than a generic failure.
    fn list(&self, directory: &str) -> io::Result<Vec<Info>>;

    /// One entry.
    ///
    /// # Errors
    ///
    /// `NotFound` when the account has no such path. Windows asks this before
    /// showing anything, so any other error hides the file rather than
    /// reporting it.
    fn stat(&self, path: &str) -> io::Result<Info>;

    /// Fill `into` with the bytes of `path` starting at `offset`.
    ///
    /// # Errors
    ///
    /// If the bytes cannot be produced -- not held here and no peer has them,
    /// or the fetch failed. The read fails in the application that asked, which
    /// is the honest outcome: a short read would be silent corruption.
    fn read(&self, path: &str, offset: u64, into: &mut [u8]) -> io::Result<()>;
}

/// Where one open enumeration has got to.
///
/// `Peekable`, and that is load-bearing: a directory listing is handed to
/// Windows a buffer at a time, and the entry that does not fit must stay
/// unconsumed for the next call. Taking it and putting it back would drop one
/// file per buffer, invisibly, and only on directories large enough that
/// nobody counts.
type Cursor = std::iter::Peekable<std::vec::IntoIter<Info>>;

/// A running projection. Dropping it stops the folder.
pub struct Mount<S: Source> {
    context: sys::PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT,
    state: *mut State<S>,
    // Windows holds a pointer to this for the lifetime of the mount, so it must
    // not move. Keeping it boxed beside the context rather than inline is what
    // makes that true.
    _callbacks: Box<sys::PRJ_CALLBACKS>,
}

// There is deliberately no `unsafe impl Send` here, and the first version had
// one. It was written as `impl<S: Source> Send`, with no `S: Send` bound --
// which would have let a `Mount` carry a thread-bound `Source` across a thread
// boundary, and dropping it there would free that `Source` on the wrong
// thread. Nothing needs it: the mount is created and dropped on the thread that
// asked for it, and the callbacks come from Windows's own threads through the
// raw pointer rather than through this value. It was caught by the gate that
// asks for a reason next to every unsafe claim -- on its first run, which is
// the argument for that gate in one line.

struct State<S: Source> {
    source: S,
    /// One cursor per open enumeration.
    ///
    /// A plain `Mutex<HashMap<..>>`, and the lock is held across the call that
    /// produces a listing. That is a real serialisation and it is the right
    /// trade here: a listing is an in-memory lookup over an already-fetched
    /// catalogue, and the alternative — taking the cursor out, unlocking,
    /// filling, and putting it back — is a dance with a window in it, written
    /// in a callback that must never panic.
    cursors: Mutex<HashMap<[u8; 16], Option<Cursor>>>,
}

/// What went wrong starting the projection.
#[derive(Debug)]
pub enum Error {
    /// The directory could not be resolved to an absolute path.
    Root(io::Error),
    /// `PrjMarkDirectoryAsPlaceholder` refused, with its HRESULT.
    Mark(i32),
    /// `PrjStartVirtualizing` refused, with its HRESULT.
    Start(i32),
    /// The system has no entropy to make an instance id from.
    Entropy,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Root(why) => write!(f, "the folder could not be opened: {why}"),
            // 0x80070002 is ERROR_FILE_NOT_FOUND wrapped as an HRESULT, and
            // 0x800700C1 is what a machine without the ProjFS feature answers.
            // Printing the number is not enough on its own, but it is what the
            // documentation is indexed by.
            Self::Mark(hr) => write!(f, "Windows would not mark the folder (0x{hr:08X})"),
            Self::Start(hr) => write!(f, "Windows would not start the projection (0x{hr:08X})"),
            Self::Entropy => write!(f, "no entropy for the instance id"),
        }
    }
}

impl std::error::Error for Error {}

/// Show `source` at `at`, until the returned mount is dropped.
///
/// # Errors
///
/// If the directory cannot be resolved, or Windows refuses to mark it or to
/// start virtualizing — most often because the Projected File System feature is
/// not enabled on this machine.
pub fn mount<S: Source + 'static>(at: &Path, source: S) -> Result<Mount<S>, Error> {
    let root = at.canonicalize().map_err(Error::Root)?;
    let root = wide(root.as_os_str());

    let mut id = [0_u8; 16];
    getrandom::fill(&mut id).map_err(|_| Error::Entropy)?;
    let guid = guid_from(id);

    let state = Box::into_raw(Box::new(State {
        source,
        cursors: Mutex::new(HashMap::new()),
    }));

    let callbacks = Box::new(sys::PRJ_CALLBACKS {
        StartDirectoryEnumerationCallback: Some(Bind::<S>::start_enum),
        EndDirectoryEnumerationCallback: Some(Bind::<S>::end_enum),
        GetDirectoryEnumerationCallback: Some(Bind::<S>::get_enum),
        GetPlaceholderInfoCallback: Some(Bind::<S>::placeholder),
        GetFileDataCallback: Some(Bind::<S>::file_data),
        QueryFileNameCallback: None,
        NotificationCallback: None,
        CancelCommandCallback: None,
    });

    // SAFETY: `root` is a NUL-terminated wide string that outlives the call,
    // and `guid` is a GUID by construction. The two null pointers are the
    // documented "no target, no version info" arguments.
    let marked = unsafe {
        sys::PrjMarkDirectoryAsPlaceholder(root.as_ptr(), ptr::null(), ptr::null(), &raw const guid)
    };
    if marked != 0 {
        // Nothing has been handed to Windows yet, so the state is ours to free.
        // SAFETY: `state` came from `Box::into_raw` above and has not escaped.
        drop(unsafe { Box::from_raw(state) });
        return Err(Error::Mark(marked));
    }

    let mut context: sys::PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT = ptr::null_mut();
    // SAFETY: `root` and `callbacks` outlive the call; `state` outlives the
    // mount, which is what `Drop` below guarantees. `options` is the documented
    // null for "no options".
    let started = unsafe {
        sys::PrjStartVirtualizing(
            root.as_ptr(),
            &raw const *callbacks,
            state.cast::<c_void>(),
            ptr::null(),
            &raw mut context,
        )
    };
    if started != 0 {
        // SAFETY: virtualization did not start, so no callback can be running
        // and nothing else holds this pointer.
        drop(unsafe { Box::from_raw(state) });
        return Err(Error::Start(started));
    }

    Ok(Mount {
        context,
        state,
        _callbacks: callbacks,
    })
}

impl<S: Source> Drop for Mount<S> {
    fn drop(&mut self) {
        // **Stop first, free second.** The crate this replaces did the
        // opposite, so a callback arriving between the two read freed memory.
        // `PrjStopVirtualizing` is documented to return only once every
        // callback has finished, which is what makes the next line safe.
        if !self.context.is_null() {
            // SAFETY: `context` came from a successful `PrjStartVirtualizing`
            // and is stopped exactly once.
            unsafe { sys::PrjStopVirtualizing(self.context) };
        }
        // SAFETY: no callback can be running now, and this pointer came from
        // `Box::into_raw` in `mount`.
        drop(unsafe { Box::from_raw(self.state) });
    }
}

/// The callbacks, one set per `S`.
///
/// A struct rather than free functions because each needs to know which
/// `Source` it is calling, and `extern "C"` function pointers carry no context
/// beyond what Windows hands back.
struct Bind<S: Source>(std::marker::PhantomData<S>);

impl<S: Source> Bind<S> {
    /// The state Windows was given, and the path the callback is about.
    ///
    /// # Safety
    ///
    /// `data` must be the pointer Windows passed to a callback of this
    /// instance, so `InstanceContext` is the `State<S>` from `mount`.
    unsafe fn unpack<'a>(data: *const sys::PRJ_CALLBACK_DATA) -> Option<(&'a State<S>, String)> {
        // SAFETY: the caller's contract. `as_ref` answers `None` rather than
        // trusting a null, which is what a defensive read of a C pointer looks
        // like even when the documentation says it cannot be null.
        let data = unsafe { data.as_ref() }?;
        // SAFETY: same.
        let state = unsafe { data.InstanceContext.cast::<State<S>>().as_ref() }?;
        // SAFETY: `FilePathName` is a NUL-terminated wide string owned by
        // Windows for the duration of the callback.
        let path = unsafe { from_wide(data.FilePathName) };
        Some((state, path))
    }

    unsafe extern "C" fn start_enum(
        data: *const sys::PRJ_CALLBACK_DATA,
        id: *const sys::GUID,
    ) -> sys::HRESULT {
        guard(|| {
            // SAFETY: called by Windows with this instance's data.
            let (state, _) = unsafe { Self::unpack(data) }.ok_or(io::ErrorKind::InvalidData)?;
            // SAFETY: Windows passes a valid enumeration id.
            let id = unsafe { id.as_ref() }.ok_or(io::ErrorKind::InvalidData)?;
            lock(&state.cursors).insert(guid_bytes(id), None);
            Ok(())
        })
    }

    unsafe extern "C" fn end_enum(
        data: *const sys::PRJ_CALLBACK_DATA,
        id: *const sys::GUID,
    ) -> sys::HRESULT {
        guard(|| {
            // SAFETY: called by Windows with this instance's data.
            let (state, _) = unsafe { Self::unpack(data) }.ok_or(io::ErrorKind::InvalidData)?;
            // SAFETY: Windows passes a valid enumeration id.
            let id = unsafe { id.as_ref() }.ok_or(io::ErrorKind::InvalidData)?;
            lock(&state.cursors).remove(&guid_bytes(id));
            Ok(())
        })
    }

    unsafe extern "C" fn get_enum(
        data: *const sys::PRJ_CALLBACK_DATA,
        id: *const sys::GUID,
        _pattern: sys::PCWSTR,
        handle: sys::PRJ_DIR_ENTRY_BUFFER_HANDLE,
    ) -> sys::HRESULT {
        guard(|| {
            // SAFETY: called by Windows with this instance's data.
            let (state, directory) =
                unsafe { Self::unpack(data) }.ok_or(io::ErrorKind::InvalidData)?;
            // SAFETY: the caller's contract; needed for the restart flag.
            let flags = unsafe { data.as_ref() }.map_or(0, |data| data.Flags);
            // Testing the bit rather than parsing the word. A flag this code
            // does not know is a feature it does not use, not a reason to fail.
            let restart =
                flags & sys::PRJ_CALLBACK_DATA_FLAGS_PRJ_CB_DATA_FLAG_ENUM_RESTART_SCAN != 0;
            // SAFETY: Windows passes a valid enumeration id.
            let id = unsafe { id.as_ref() }.ok_or(io::ErrorKind::InvalidData)?;
            let key = guid_bytes(id);

            let mut cursors = lock(&state.cursors);
            let cursor = cursors.get_mut(&key).ok_or(io::ErrorKind::InvalidData)?;
            if cursor.is_none() || restart {
                *cursor = Some(state.source.list(&directory)?.into_iter().peekable());
            }
            let Some(cursor) = cursor.as_mut() else {
                return Ok(());
            };

            // Stop at the first entry Windows will not take: the buffer is
            // full, and the next call continues from here. `peek` before
            // `next` is what makes that true, and getting it wrong drops one
            // file per buffer -- invisibly, and only on large directories.
            while let Some(entry) = cursor.peek() {
                let name = wide(std::ffi::OsStr::new(&entry.name));
                let mut basic = basic_info(entry);
                // SAFETY: `name` is NUL-terminated and lives to the end of the
                // iteration; `basic` is a fully initialised struct; `handle` is
                // the buffer Windows passed to this callback.
                let hr =
                    unsafe { sys::PrjFillDirEntryBuffer(name.as_ptr(), &raw mut basic, handle) };
                if hr != 0 {
                    break;
                }
                cursor.next();
            }
            Ok(())
        })
    }

    unsafe extern "C" fn placeholder(data: *const sys::PRJ_CALLBACK_DATA) -> sys::HRESULT {
        guard(|| {
            // SAFETY: called by Windows with this instance's data.
            let (state, path) = unsafe { Self::unpack(data) }.ok_or(io::ErrorKind::InvalidData)?;
            // SAFETY: the caller's contract.
            let data = unsafe { data.as_ref() }.ok_or(io::ErrorKind::InvalidData)?;
            let entry = state.source.stat(&path)?;

            // SAFETY: `PRJ_PLACEHOLDER_INFO` is a plain-old-data struct of
            // integers, arrays and a union of integers, for which an all-zero
            // value is what the API documents as "nothing but the basic info".
            let mut info: sys::PRJ_PLACEHOLDER_INFO = unsafe { std::mem::zeroed() };
            info.FileBasicInfo = basic_info(&entry);
            let size = u32::try_from(size_of_val(&info)).unwrap_or(u32::MAX);

            // SAFETY: the context and the file name are Windows's own, valid
            // for this callback; `info` is initialised above and `size` is its
            // real size.
            let hr = unsafe {
                sys::PrjWritePlaceholderInfo(
                    data.NamespaceVirtualizationContext,
                    data.FilePathName,
                    &raw const info,
                    size,
                )
            };
            if hr == 0 {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(hr).into())
            }
        })
    }

    unsafe extern "C" fn file_data(
        data: *const sys::PRJ_CALLBACK_DATA,
        offset: sys::UINT64,
        length: sys::UINT32,
    ) -> sys::HRESULT {
        guard(|| {
            // SAFETY: called by Windows with this instance's data.
            let (state, path) = unsafe { Self::unpack(data) }.ok_or(io::ErrorKind::InvalidData)?;
            // SAFETY: the caller's contract.
            let data = unsafe { data.as_ref() }.ok_or(io::ErrorKind::InvalidData)?;

            let wanted = usize::try_from(length).unwrap_or(usize::MAX);
            let mut buffer = Aligned::new(data.NamespaceVirtualizationContext, wanted)
                .ok_or(io::ErrorKind::OutOfMemory)?;
            state.source.read(&path, offset, buffer.as_mut())?;

            // SAFETY: `buffer` is a ProjFS-aligned allocation of `length` bytes
            // from this context, filled above, and still alive here.
            let hr = unsafe {
                sys::PrjWriteFileData(
                    data.NamespaceVirtualizationContext,
                    &raw const data.DataStreamId,
                    buffer.raw,
                    offset,
                    length,
                )
            };
            if hr == 0 {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(hr).into())
            }
        })
    }
}

/// A buffer allocated the way `PrjWriteFileData` requires.
struct Aligned {
    raw: *mut c_void,
    len: usize,
}

impl Aligned {
    fn new(context: sys::PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT, len: usize) -> Option<Self> {
        // SAFETY: `context` is the one Windows passed to the callback, and
        // `len` is the length it asked for.
        let raw = unsafe { sys::PrjAllocateAlignedBuffer(context, len as sys::size_t) };
        if raw.is_null() {
            return None;
        }
        Some(Self { raw, len })
    }
}

impl AsMut<[u8]> for Aligned {
    fn as_mut(&mut self) -> &mut [u8] {
        // SAFETY: `raw` is a live allocation of `len` bytes owned by this
        // value, and `&mut self` means nothing else is looking at it. The
        // bytes are uninitialised, which is why this is only ever handed to a
        // writer -- `Source::read` fills what it uses and the rest is padding
        // Windows discards.
        unsafe { std::slice::from_raw_parts_mut(self.raw.cast::<u8>(), self.len) }
    }
}

impl Drop for Aligned {
    fn drop(&mut self) {
        // SAFETY: allocated by `PrjAllocateAlignedBuffer` and freed once.
        unsafe { sys::PrjFreeAlignedBuffer(self.raw) };
    }
}

/// Run a callback body, and never let anything out of it but a code.
///
/// A panic unwinding across `extern "C"` is undefined behaviour, so the one
/// thing this must not do is propagate. The crate this replaces had no such
/// guard and three `unwrap`s inside the boundary.
fn guard<F>(body: F) -> sys::HRESULT
where
    F: FnOnce() -> Result<(), Fault>,
{
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => 0,
        Ok(Err(fault)) => fault.0,
        // No message: there is nobody to tell, and Windows wants a number.
        // `E_FAIL`.
        Err(_) => -1_i32,
    }
}

/// An error on its way back to Windows, already as an HRESULT.
struct Fault(sys::HRESULT);

impl From<io::Error> for Fault {
    fn from(why: io::Error) -> Self {
        if let Some(code) = why.raw_os_error() {
            return Self(code);
        }
        Self(match why.kind() {
            // The constants are `u32` and the field is `i32`. All three are
            // small positive numbers, so the reinterpretation is exact; saying
            // so with `cast_signed` rather than `as` is what keeps it exact if
            // one of them ever is not.
            io::ErrorKind::NotFound => sys::IO_ERROR_FILE_NOT_FOUND.cast_signed(),
            io::ErrorKind::WouldBlock => sys::IO_ERROR_IO_PENDING.cast_signed(),
            io::ErrorKind::InvalidData => sys::IO_ERROR_INSUFFICIENT_BUFFER.cast_signed(),
            _ => -1_i32,
        })
    }
}

impl From<io::ErrorKind> for Fault {
    fn from(kind: io::ErrorKind) -> Self {
        io::Error::from(kind).into()
    }
}

/// A mutex whose poisoning is not an event.
///
/// The only thing a panicking callback could have left behind is a half-updated
/// cursor for one enumeration, and Explorer's answer to a bad enumeration is to
/// ask again. Refusing to serve the folder ever again because one listing
/// panicked would be the larger fault.
fn lock<T>(what: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    what.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn basic_info(entry: &Info) -> sys::PRJ_FILE_BASIC_INFO {
    sys::PRJ_FILE_BASIC_INFO {
        IsDirectory: u8::from(entry.is_dir),
        FileSize: i64::try_from(entry.size).unwrap_or(i64::MAX),
        CreationTime: entry.written.into(),
        LastAccessTime: entry.written.into(),
        LastWriteTime: entry.written.into(),
        ChangeTime: entry.written.into(),
        FileAttributes: if entry.is_dir {
            0x0000_0010 // FILE_ATTRIBUTE_DIRECTORY
        } else {
            0x0000_0080 // FILE_ATTRIBUTE_NORMAL
        },
    }
}

/// A NUL-terminated UTF-16 copy, which is what every one of these calls wants.
fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// Read back one of Windows's own NUL-terminated wide strings.
///
/// # Safety
///
/// `raw` must be null, or point at a NUL-terminated sequence of `u16` that
/// stays alive for the duration of this call.
unsafe fn from_wide(raw: sys::PCWSTR) -> String {
    if raw.is_null() {
        return String::new();
    }
    let mut len = 0_usize;
    // SAFETY: the caller's contract guarantees a NUL within the allocation, so
    // this walk stops inside it.
    while unsafe { *raw.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` units are readable, having just been walked.
    let units = unsafe { std::slice::from_raw_parts(raw, len) };
    OsString::from_wide(units).to_string_lossy().into_owned()
}

/// Sixteen random bytes as a GUID, laid out the way Windows reads one.
const fn guid_from(bytes: [u8; 16]) -> sys::GUID {
    sys::GUID {
        Data1: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        Data2: u16::from_le_bytes([bytes[4], bytes[5]]),
        Data3: u16::from_le_bytes([bytes[6], bytes[7]]),
        Data4: [
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ],
    }
}

/// A GUID back as sixteen bytes, so it can key a map without a uuid crate.
fn guid_bytes(guid: &sys::GUID) -> [u8; 16] {
    let mut out = [0_u8; 16];
    out[0..4].copy_from_slice(&guid.Data1.to_le_bytes());
    out[4..6].copy_from_slice(&guid.Data2.to_le_bytes());
    out[6..8].copy_from_slice(&guid.Data3.to_le_bytes());
    out[8..16].copy_from_slice(&guid.Data4);
    out
}

#[cfg(test)]
mod tests {
    use super::{Info, basic_info, guid_bytes, guid_from, wide};

    #[test]
    fn a_guid_survives_the_round_trip_that_keys_the_cursor_map() {
        // Enumeration cursors are keyed by these bytes. If the conversion were
        // not injective, two open enumerations could share a cursor and
        // Explorer would show one directory's entries inside another -- with
        // nothing in the logs, because both lookups succeed.
        let bytes: [u8; 16] = core::array::from_fn(|i| u8::try_from(i * 7 % 251).unwrap_or(0));
        assert_eq!(guid_bytes(&guid_from(bytes)), bytes);

        let other: [u8; 16] = core::array::from_fn(|i| u8::try_from(i).unwrap_or(0));
        assert_ne!(guid_bytes(&guid_from(other)), bytes);
    }

    #[test]
    fn a_directory_is_flagged_as_one_and_a_file_is_not() {
        // Windows decides whether to offer a folder or a file from this bit
        // alone. Getting it wrong makes a directory unopenable rather than
        // wrong-looking, so it is worth a line.
        let dir = Info {
            name: "notes".to_owned(),
            is_dir: true,
            size: 0,
            written: 0,
        };
        let file = Info {
            name: "top.txt".to_owned(),
            is_dir: false,
            size: 27,
            written: 0,
        };
        assert_eq!(basic_info(&dir).IsDirectory, 1);
        assert_eq!(basic_info(&file).IsDirectory, 0);
        assert_eq!(basic_info(&file).FileSize, 27);
    }

    #[test]
    fn every_string_handed_to_windows_ends_in_a_nul() {
        // Every one of these calls takes a PCWSTR and walks it until it finds a
        // zero. A `Vec<u16>` without one is a read past the end of the
        // allocation, and it would work by accident most of the time.
        assert_eq!(wide(std::ffi::OsStr::new("a")), vec![b'a'.into(), 0]);
        assert_eq!(wide(std::ffi::OsStr::new("")), vec![0]);
    }
}
