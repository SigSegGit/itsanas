# Android

There is no one-click Android installer, and this page exists so that the
absence is a stated position rather than a gap you discover after an hour.

## What is honest today

**The whole data path compiles for `aarch64-linux-android` and is checked on
every push** — `crypto`, `store`, `sync`, `policy`, `discover`, `placement`,
`wire`. See the `android-core` job in `.github/workflows/ci.yml`. That is a real
guarantee and it is not the same as an app.

**What does not exist:** the app. No Kotlin, no JNI bridge, no
`WorkManager` scheduling, no UI, no storage-access-framework wiring, no APK.
Writing an installer for it would be writing an installer for something that is
not there.

**`itsanas-policy` is the part written for the phone and it is used today by
`itsanas daemon`**, so the schedule the app will follow is already exercised by
a desktop rather than being invented on the day somebody starts the app. See
`docs/PORTING.md`.

## What you can do on a phone right now

### Termux, for a shell

[Termux](https://termux.dev) gives you a Linux userland on Android without root.
`install/linux.sh` will not work there unchanged — Termux has no `/proc/meminfo`
in the shape the script expects on some devices, no `apt-get` (it uses `pkg`),
and no systemd at all — so do it by hand:

```sh
pkg update && pkg install rust git binutils
git clone <repo> && cd itsanas
cargo build --release
./target/release/itsanas --version
```

Two things to know before you spend the time:

- **Android kills background processes aggressively.** Termux needs a wake-lock
  and an exemption from battery optimisation, and Samsung's One UI is stricter
  than stock. A daemon left running overnight will usually be dead by morning.
- **This is a shell, not an app.** No notification, no file picker, no
  integration with the gallery or Documents. Useful for testing that the core
  runs on your phone's CPU; not useful as a way to keep your photos synced.

### What it would take to do it properly

Recorded so that the estimate is on paper rather than in somebody's head:

| Piece | Why it is not optional |
| --- | --- |
| JNI bridge over `itsanas-store` and `itsanas-net` | The core is a library, not a service. Something has to call it. |
| `WorkManager` periodic work | `itsanas-policy` already decides *when*; Android decides whether to honour it. Doze and One UI deep sleep both need handling and neither is a line of code. |
| Storage Access Framework | Android 11 removed the file access that a sync tool needs. Either a scoped directory the user grants, or `MANAGE_EXTERNAL_STORAGE` and a Play Store review that usually says no. |
| Foreground service for the live case | Android 15 caps `dataSync` at about six hours a day. Fine for the policy's 30-second watching interval, not for "always on". |
| A UI for the two modes | Nicolas asked for a switch between "just show me the files" and "actually sync them". That is `Scope::Metadata` and `Scope::Everything`, which exist and are tested. |

The gap that matters most is not any of those: it is that
`Scope::Metadata` fetches the log but a deferred operation writes no index entry,
so a metadata round leaves the files *known but invisible*. `itsanas_store::catalogue`
was written for exactly this and the phone UI is what would use it. See
`docs/ROADMAP.md` M12.

## Why there is no installer here

An installer that printed "not yet" would be theatre. An installer that set up
Termux and built the CLI would suggest the phone is supported when what you get
is a process Android will kill.

When the app exists, this file is replaced by the store link and a page about
the permissions it asks for and why.
