# The iPhone app

A first version: open the account, list it folder by folder, add a file,
fetch a file, delete it to a trash, run a round, and the shared page for creating the account and
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

**The passphrase stays in the Keychain**, on this device and readable only
while it is unlocked, once it has opened the account
(`red_team_a_passphrase_that_does_not_open_is_not_remembered`). Coming back to
the app opens the account with it; Sign out forgets it
(`a_remembered_passphrase_comes_back_until_it_is_forgotten`).

**Touch ID or Face ID can guard it**, from Settings. The item then needs the
finger enrolled today, the engine is told there is no remembered passphrase
(`red_team_touch_id_keeps_the_passphrase_from_the_engine`), and it does not turn
on with nothing to guard
(`red_team_touch_id_with_nothing_remembered_stays_off_and_says_so`).

**A delete waits in a trash**, 30 days on this phone, as on Android: swipe a
file; the trash button restores it or deletes it from every machine
(`a_trashed_file_leaves_the_list_comes_back_and_can_be_deleted`). The days are
not yet a setting here.

**Settings hides what an iPhone does not do**: lending space, the mobile-data
choices and the update check (`Features::IPHONE`), and `/api/hosting` refuses
(`red_team_an_iphone_is_refused_hosting_whatever_the_page_hides`).

**The unsafe code is two blocks**, in `crates/itsanas-ios/src/c.rs`: reading a C
string and taking back one the app was given. `scripts/check-unsafe.py` fails
if it spreads.

The #387 build has been installed by hand on a first iPhone SE, signed with a
free Apple ID.

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

## Releasing an iOS build

The repository has a release workflow at `.github/workflows/release-ios.yml`.
It builds the unsigned package and uploads it to a GitHub Release asset named
`ITSaNAS-unsigned.ipa`. This is intentionally not an App Store build: an iPhone
app must be signed by Apple before it can run on a device.

Use it in either of these ways:

- Manual run: Actions → `iOS release` → `Run workflow`
- Tag push: push a tag such as `ios-v0.1.0`

The asset on the release is the same package the CI job keeps as an artifact:
unsigned, for sideloading, not for the App Store.

## Installing on an iPhone with a free Apple ID

Apple lets an app onto an iPhone only when it is signed by an Apple ID. A free
Apple ID can do this, but the certificate expires after about 7 days, and then
you install the build again. The free option is enough for personal use; it is
not the same as the App Store or TestFlight.

### From a Mac, with Xcode

1. Plug the iPhone into the Mac and trust the Mac on the phone.
2. `sh scripts/build-ios.sh`, then `open ios/ITSaNAS.xcodeproj`.
3. Xcode → Settings → Accounts: add the Apple ID.
4. Select the ITSaNAS target → Signing & Capabilities: choose the team of that
   Apple ID. If `fr.ngas.itsanas` is refused, change the bundle identifier to
   one of your own.
5. Choose the iPhone as destination, then Product → Run.
6. On the phone: Settings → General → Device Management (or Profiles & Device
   Management) → trust the developer.

### From Windows, with the package from CI or a GitHub release

1. Download the `ITSaNAS-unsigned.ipa` from the GitHub Actions artifact or the
   GitHub Release.
2. Install it with a sideloading tool that signs it with your Apple ID, such as
   AltStore or SideStore (Sideloadly also works).
3. The same 7-day limit applies with a free Apple ID; after that, sign again.
4. The signed app is for your own phone; distribution to other people still
   needs an Apple-supported distribution route.

## From the PC, with the iPhone on USB

Windows sees the phone through Apple's driver (iTunes or Apple Devices).
`libimobiledevice` reads it from there without Xcode:

```sh
ideviceinfo -k ProductVersion
idevicesyslog | grep -i itsanas
```

These read the device's information and log; they do not build or sign.
