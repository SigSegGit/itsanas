# Android

There is an app now. This page says what it is, what it has actually been seen
to do, and what it still does not do — because the version of this page that
said "there is no app" outlived the app by a week, and a page nobody rereads is
how that happens.

## The app

Kotlin and Compose over the same Rust core, through a JNI boundary in
`crates/itsanas-android`. It is not a port: the data path, the crypto, the
store, the sync round and the keystore handling are the same code the desktop
runs, cross-compiled. `crates/itsanas-node` holds the parts both shells use, so
the passphrase handling exists once rather than twice.

Build it:

```sh
sh scripts/build-apk.sh
```

It needs the Android SDK and NDK 27.3, and it produces
`android/app/build/outputs/apk/`. About 19.5 MB with three ABIs inside.

### What has actually been done with it

On an **Android 15 emulator**, on 2026-09-07, driven through the interface
rather than a test harness:

1. An account restored from its twenty-four words, typed into the phone.
2. One peer added — a desktop node belonging to a **different account** holding
   this one's sealed chunks — and a sync round run against it. Five files
   arrived: `5 here · 0 not here · 910 KiB`, which is the exact byte count of
   the five files, so they were fetched rather than listed.
3. A file opened, written out through a `FileProvider` and handed to the system
   chooser. A `.bin` has no viewer on a bare emulator, so it falls back to
   sharing.
4. The foreground service running, `isForeground=true`, with its notification.

`docs/PORTING.md` §4 has the measurements.

### What it has never done

- **Run on physical hardware.** An emulator is not a phone: it has no doze, no
  manufacturer battery policy, no real radio and no thermal limit.
- **Survived a night.** Android will stop a foreground service on a
  manufacturer skin that decides to, and One UI decides to. The notification
  makes that less likely, not impossible.
- **Been installed from anywhere but `adb install`.** There is no Play Store
  listing and no signing key, so putting it on a phone means enabling unknown
  sources.

### What it does not have

| Missing | What that costs you |
| --- | --- |
| Storage Access Framework wiring | It cannot watch a folder. Files come in and out through the share sheet and the picker, one at a time. |
| Automatic photo backup | The obvious use for a phone in a backup network, and not built. |
| `WorkManager` scheduling | Sync runs while the service is up. There is no periodic wake-up that survives the service being killed. |
| A metadata-only mode in the UI | `Scope::Metadata` exists and is tested in the core, but a metadata round leaves files *known but invisible* — see below. The app therefore only offers "fetch it". |

The gap that matters most is the last one: `Scope::Metadata` fetches the log,
but a deferred operation writes no index entry, so a metadata round leaves the
files known and unlistable. `itsanas_store::catalogue` was written for exactly
this and the phone UI is what would use it. See `docs/ROADMAP.md` M12.

## Termux, for a shell

Different thing, still useful, and it predates the app:

```sh
pkg install git && git clone https://github.com/SigSegGit/itsanas && cd itsanas
sh install/android-termux.sh
```

`install/linux.sh` will not work there — Termux has no `apt-get` (it uses
`pkg`), no systemd, and on some devices no `/proc/meminfo` the script can read.

This builds the command-line tool for the phone's own processor and stores a
file and reads it back on it. The app proves the core runs on ARM through JNI;
this proves the **whole test suite** runs on the real silicon, which is a
different guarantee and the reason it stays. Half the constants in this project
— the chunk size, the memory the key derivation may use, how much work an audit
round is allowed — are chosen for ARM devices.

It refuses a 32-bit Termux (the Google Play build, which is unmaintained), and
it handles the stale package mirror by name, because "E: Unable to locate
package rust" is the most common way this fails and it reads as if the package
did not exist.

Two things to know before you spend the time:

- **Android kills background processes aggressively.** Termux needs a wake-lock
  and an exemption from battery optimisation, and Samsung's One UI is stricter
  than stock. A daemon left running overnight will usually be dead by morning.
  The app's foreground service is the answer to this; Termux has no such thing.
- **This is a shell, not a client.** No notification, no file picker, no
  integration with the gallery or Documents. Use the app for that.

## Removing either of them

The app: uninstall it like any other, which takes the account with it — its
keystore and store live in the app's private directory and Android deletes
them. **That is the only copy of this device's sealed master secret.** If it is
also the only device on the account, the twenty-four words are all that is left.

Termux: `sh install/android-termux.sh --clean` inside Termux, which hands over
to `install/clean.sh` like every other entry point here.
