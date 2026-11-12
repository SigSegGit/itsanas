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

**The passphrase stays in the Keychain**, on this device and readable from the
first unlock after the phone starts, once it has opened the account
(`red_team_a_passphrase_that_does_not_open_is_not_remembered`). Coming back to
the app opens the account with it; Sign out forgets it
(`a_remembered_passphrase_comes_back_until_it_is_forgotten`).

**Touch ID or Face ID can lock the app's screen**, from Settings. The unlock
screen then asks for a finger before opening the account
(`red_team_touch_id_on_makes_the_unlock_screen_ask_for_a_finger`), and a refused
finger leaves the passphrase field. It locks the screen and does not encrypt the
item, which a round in the background has to read. The engine is told there is
no remembered passphrase
(`red_team_touch_id_keeps_the_passphrase_from_the_engine`), and the lock does not
turn on with nothing to open
(`red_team_touch_id_with_nothing_remembered_stays_off_and_says_so`). A finger
enrolled after the lock was turned on opens nothing: the first finger records
which ones are enrolled (the system's `evaluatedPolicyDomainState`), and then
only the passphrase opens, until the lock is turned off and on again
(`red_team_a_finger_enrolled_after_touch_id_was_turned_on_opens_nothing`).

**A delete waits in a trash**, 30 days on this phone, as on Android: swipe a
file; the trash button restores it or deletes it from every machine
(`a_trashed_file_leaves_the_list_comes_back_and_can_be_deleted`). The days
are set by the button of the trash screen, one at least
(`testTrashDaysAreKeptAndNeverBelowOne`), and apply to files deleted from then on.

**Settings hides what an iPhone does not do**: lending space, the mobile-data
choices and the update check (`Features::IPHONE`), and `/api/hosting` refuses
(`red_team_an_iphone_is_refused_hosting_whatever_the_page_hides`).

**A round runs while the app is away**, when iOS grants the time and chooses
when: a short refresh brings the listing, a longer task on the charger moves
contents unless the network is metered
(`a_background_pass_opens_with_what_is_remembered_and_goes_for_a_round`). The
trash empties in it too
(`a_background_pass_deletes_what_the_trash_has_held_long_enough`). It runs with Touch ID on too
(`a_background_pass_runs_with_touch_id_on`).

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
| Scheduled sync by the policy | `BGTaskScheduler` runs a round when iOS grants one; see above |
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

## The hand test

Steps 1 to 5, 8 and 9 run by themselves now, on a simulator, in the Mac job
of every run that touches the iPhone app: `scripts/iphone-e2e.sh` (called by
`sh scripts/build-ios.sh test`) starts a coordinator and a computer of the
account on the Mac, and the UI tests in `ios/ITSaNASUITests` do what a person
did by hand:

1. log in to an existing account with its name and passphrase: the file list appears;
2. a file put on the computer is listed after Sync, and opens;
3. the trash: the days button keeps `1`, a deleted file waits in the trash with
   its days, Restore brings it back, and "Delete" from the trash removes it from
   the computer too after a round;
4. Touch ID on in Settings: back from the home screen a finger is asked, one
   that is not enrolled leaves the passphrase field, which opens; the enrolled
   one opens;
5. after the enrolment changes no finger opens, the passphrase does, and Touch
   ID turned off and on again opens with a finger;
8. Settings shows no Disks, no USB key, no Advanced section, no lending of space;
9. Sign out locks the app, and the app opened again asks for the passphrase.

The fingers are the simulator's. What is left for a person is what no
simulator does: a real iPhone, a real finger, and iOS deciding when a round
runs. Each step says what to do and what must be seen; note the step's number
and OK or KO, with a screenshot for a KO.

0. **Get the build** from the last green run of `main` (or of the release's tag), then sign and install
   it with your Apple ID in Sideloadly or AltStore:

   ```sh
   gh run download --repo SigSegGit/itsanas --name ITSaNAS-unsigned-ipa --dir ios-ipa $(gh run list --repo SigSegGit/itsanas --branch main --workflow CI --status success --limit 1 --json databaseId -q '.[0].databaseId')
   ```

   Keep `bash scripts/iphone-watch.sh` running on the PC, with the phone on
   USB, from the start: it keeps the app's log lines, collects its crash reports
   and ends with a verdict (background rounds seen, no crash, no panic). Open the
   account on the phone (log in, or create one).
6. **A round in the background.** First check that iOS's Settings, General,
   Background App Refresh lists ITSaNAS, switched on: if it is missing, the
   free signing dropped the background modes, and say so. Then put a new file
   on a computer, leave the app for the home screen, and leave the phone on
   its charger on Wi-Fi for an hour or more. The PC's log shows a line
   `ITSaNAS background` when iOS grants a round; open the app without pressing
   Sync: the new file is listed.
7. **The same with Touch ID on** (Settings, Security, the fingerprint), as
   step 6: the round still runs, and the app still asks for a finger when opened.

And on Windows, for the passphrase sealed behind Windows Hello:

10. On a PC brought to the release that carries it (`itsanas update`), in
    Settings, Security, turn on Windows Hello, then
    "Change my passphrase".
    Windows Hello is asked before anything changes; refuse it: the passphrase
    is unchanged and the old one still opens. Do it again and accept: sign out
    of Windows and back in, and the service starts, with Windows Hello, on the
    new passphrase.

## From the PC, with the iPhone on USB

Windows sees the phone through Apple's driver (iTunes or Apple Devices).
`libimobiledevice` reads it from there without Xcode:

```sh
ideviceinfo -k ProductVersion
idevicesyslog | grep -i itsanas
```

These read the device's information and log; they do not build or sign.
