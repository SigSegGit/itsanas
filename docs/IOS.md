# The iPhone app

A first version: open the account, list it folder by folder, add a file,
fetch a file, run a round, and the shared page for creating the account and
for Settings. It runs on iOS 14 and later, which includes the first iPhone SE
(it stops at iOS 15).

## How it is put together

| Piece | Where |
| --- | --- |
| The core | unchanged, built as a static library for `aarch64-apple-ios` |
| What both phones ask the core (open, list, fetch, add, round, status) | `crates/itsanas-cli/src/phone.rs`, called by Android through JNI and by the iPhone through C |
| The C boundary | `crates/itsanas-ios`, its header in `crates/itsanas-ios/include/itsanas.h` |
| The setup engine and Settings, as a page | `crates/itsanas-cli`, served on 127.0.0.1 and shown in a `WKWebView`, as on Android |
| The application | `ios/`, Swift and UIKit, described by `ios/project.yml` (XcodeGen) |

**Every answer is one JSON envelope**, `{"ok": …}` or `{"error": "<sentence>"}`,
freed once by `Core.swift`. The sentence is the one the command line prints.

**Secrets are asked by the app, not by the page.** The page's engine leaves its
question in a mailbox; the app polls `itsanas_next_request` while the page is
on screen, asks in a native alert and answers with `itsanas_reply`. A reply to
a question nobody waits on is dropped
(`red_team_a_stray_reply_cannot_answer_the_next_question`), and an engine
whose page was left stops asking
(`red_team_an_iphone_engine_whose_page_was_left_asks_nothing`).

**The unsafe code is two blocks**, in `crates/itsanas-ios/src/c.rs`: reading a C
string and taking back one the app was given. `scripts/check-unsafe.py` fails
if it spreads.

## What iOS does not allow that Android does

| Android | iPhone |
| --- | --- |
| A foreground service keeps the node open and syncing | No equivalent. The app closes the store when it leaves the screen |
| Hosting for other members while charging | Not offered: iOS ends a background process within minutes |
| Scheduled sync by the policy | A later step: `BGTaskScheduler` grants a few minutes when iOS chooses |
| Install an APK by hand | Every app is signed by an Apple ID, see below |

## Building

CI builds it on every change to `crates/itsanas-ios`, `ios/`, the crates it
depends on, or `scripts/build-ios.sh`: job **The iPhone app** in
`.github/workflows/ci.yml`, on a GitHub macOS runner. It builds the core for
the phone and the simulator, runs the Swift tests on a simulator, and keeps
an unsigned package, `ITSaNAS-unsigned-ipa`, as an artifact of the run.

On a Mac with Xcode:

```sh
brew install xcodegen
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
sh scripts/build-ios.sh test
open ios/ITSaNAS.xcodeproj
```

## Installing on an iPhone

Apple lets an app onto an iPhone only when it is signed by an Apple ID. Two
ways, both done by the owner of the Apple ID, who types it in Apple's own
tools:

**From the Mac, with Xcode** (free Apple ID: the signature lasts 7 days, then
the app is installed again the same way):

1. Plug the iPhone into the Mac and trust the Mac on the phone.
2. `sh scripts/build-ios.sh`, then `open ios/ITSaNAS.xcodeproj`.
3. Xcode → Settings → Accounts: add the Apple ID.
4. Target ITSaNAS → Signing & Capabilities: choose the team of that Apple ID;
   if `fr.ngas.itsanas` is refused, change the bundle identifier to one of
   your own.
5. Choose the iPhone as destination, then Product → Run.
6. On the phone: Settings → General → Device Management (or Profiles &
   Device Management) → trust the developer.

**From Windows, with the package from CI:** download the artifact
`ITSaNAS-unsigned-ipa` of a green run, and install it with a sideloading tool
that signs it with an Apple ID (Sideloadly, AltStore). The same 7-day limit
applies with a free Apple ID; an Apple Developer Program membership makes it a
year and opens TestFlight.

## From the PC, with the iPhone on USB

Windows sees the phone through Apple's driver (iTunes or Apple Devices).
`libimobiledevice` reads it from there without Xcode:

```sh
ideviceinfo -k ProductVersion
idevicesyslog | grep -i itsanas
```

These read the device's information and log; they do not build or sign.
