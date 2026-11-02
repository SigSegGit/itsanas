# One interface, and where it stops

A feature lives in the library, and a shell draws only what it alone can
(`docs/ARCHITECTURE.md`). The setup page and Settings are the same page on a
computer and on a phone, from the same engine; what a phone lacks is hidden by
`Features` (`crates/itsanas-cli/src/setup/web/features.rs`) and refused by the
routes behind it. This table says, for every capability, whether each platform
has it.

Each cell is `✅` (it has it) or `⬜` followed by the fact that explains why not.
`scripts/check-parity.py` reads the source and refuses a table that has drifted:
a command, a page heading, a route or a `Features` field with no row, a phone
feature marked present when it is off (or the other way), an empty cell, a `⬜`
with no reason. A row is added in the commit that adds the thing.

A cell about Android describes what the library and the page do for the phone.
The app draws the page in a web view and keeps native only the screens that must
never be a web page (the secrets, Unlock, the file list, the picker, sharing and
the fingerprint); until the app does, `docs/HANDOVER.md` §8 says where it stands.

## The page

| Capability | Windows | macOS | Linux | Android |
|---|---|---|---|---|
| Welcome · `/api/state` · `/api/plan` · `/api/run` · `/api/quit` | ✅ | ✅ | ✅ | ✅ |
| Your account · `/api/name` · `/api/suggest` (existing account first, a positive check before a new one) | ✅ | ✅ | ✅ | ✅ |
| Your folder and your space · `/api/free` | ✅ | ✅ | ✅ | ⬜ a phone has no synced folder: its files are in the app |
| Space you offer (one suggestion rule on every platform) | ✅ | ✅ | ✅ | ✅ |
| Files kept here | ✅ | ✅ | ✅ | ✅ |
| Ready · Final check · This page is closed | ✅ | ✅ | ✅ | ✅ |
| Settings | ✅ | ✅ | ✅ | ✅ |
| Syncing · `/api/control` (pause, interval, sync now) | ✅ | ✅ | ✅ | ⬜ a phone syncs on the policy's schedule and has its own Sync button; pause and the interval are hidden |
| This machine | ✅ | ✅ | ✅ | ✅ |
| Disks · `/api/disk` | ✅ | ✅ | ✅ | ⬜ a phone has one app-private store: no vault to move, no backup drive, no portable copy |
| Copy this account to a USB key · `/api/keys` · `/api/copy-to-key` | ✅ | ✅ | ✅ | ⬜ a phone cannot write a program for another system onto a key |
| Appearance · `/api/theme` | ✅ | ✅ | ✅ | ✅ |
| Security · `/api/host` · `/api/forgot` · `/api/words` · `/api/passphrase` | ✅ | ✅ | ✅ | ✅ |
| Devices of this account · `/api/devices` · `/api/withdraw` | ✅ | ✅ | ✅ | ✅ |
| Machines to sync with (Advanced) · `/api/peers` | ✅ | ✅ | ✅ | ⬜ a phone offers no Advanced section: it syncs with the machines the coordinator lists |
| About and legal | ✅ | ✅ | ✅ | ✅ |
| Sign out · `/api/signout` | ✅ | ✅ | ✅ | ✅ |
| Language · `/api/language` | ✅ | ✅ | ✅ | ✅ |
| Open and pin the folder · `/api/open` · `/api/pin` · `/api/pick` | ✅ | ✅ | ✅ | ⬜ a phone has no synced folder to open or add to a file manager |
| Another account beside this one · `/api/another` | ✅ | ✅ | ✅ | ⬜ a phone keeps one account; a second is a second install |

## What a shell has

| Capability | Windows | macOS | Linux | Android |
|---|---|---|---|---|
| `feature:folder` | ✅ | ✅ | ✅ | ⬜ a phone has no synced folder: its files are in the app |
| `feature:background_choice` | ✅ | ✅ | ✅ | ⬜ the app's sync service always runs; there is no logon to choose at |
| `feature:advanced` | ✅ | ✅ | ✅ | ⬜ a phone is built for one network and updates through the app, not the node |
| `feature:sync_controls` | ✅ | ✅ | ✅ | ⬜ a phone syncs on the policy's schedule |
| `feature:disks` | ✅ | ✅ | ✅ | ⬜ a phone has one app-private store |
| `feature:usb_key` | ✅ | ✅ | ✅ | ⬜ a phone cannot write a program for another system onto a key |
| `feature:another_account` | ✅ | ✅ | ✅ | ⬜ a phone keeps one account |
| `feature:keep_question` | ⬜ a computer keeps everything until Settings says otherwise | ⬜ a computer keeps everything until Settings says otherwise | ⬜ a computer keeps everything until Settings says otherwise | ✅ |
| `feature:biometric` | ⬜ the computers' windows ask the passphrase; no fingerprint reader is used | ⬜ the computers' windows ask the passphrase; no fingerprint reader is used | ⬜ the computers' windows ask the passphrase; no fingerprint reader is used | ✅ |
| `feature:metered` | ⬜ a computer has no mobile-data choice to make | ⬜ a computer has no mobile-data choice to make | ⬜ a computer has no mobile-data choice to make | ✅ |

## The commands

Every `itsanas` command works in a terminal on the three computers. The phone has
no terminal: what a command does for a member is a screen of the page or of the
app, or it is not done there.

| Command | Windows | macOS | Linux | Android |
|---|---|---|---|---|
| `itsanas init` | ✅ | ✅ | ✅ | ✅ |
| `itsanas login` | ✅ | ✅ | ✅ | ✅ |
| `itsanas passphrase` | ✅ | ✅ | ✅ | ✅ |
| `itsanas setup` | ✅ | ✅ | ✅ | ✅ |
| `itsanas signout` | ✅ | ✅ | ✅ | ✅ |
| `itsanas signin` | ✅ | ✅ | ✅ | ✅ |
| `itsanas settings` | ✅ | ✅ | ✅ | ✅ |
| `itsanas status` | ✅ | ✅ | ✅ | ✅ |
| `itsanas pause` | ✅ | ✅ | ✅ | ⬜ a phone syncs on the policy's schedule |
| `itsanas resume` | ✅ | ✅ | ✅ | ⬜ a phone syncs on the policy's schedule |
| `itsanas sync-now` | ✅ | ✅ | ✅ | ✅ |
| `itsanas interval` | ✅ | ✅ | ✅ | ⬜ a phone syncs on the policy's schedule |
| `itsanas whoami` | ✅ | ✅ | ✅ | ⬜ terminal only: the account's name is on the screen already |
| `itsanas check-passphrase` | ✅ | ✅ | ✅ | ✅ |
| `itsanas instances` | ✅ | ✅ | ✅ | ⬜ a phone keeps one account |
| `itsanas migrate` | ✅ | ✅ | ✅ | ⬜ a phone has one app-private home |
| `itsanas ls` | ✅ | ✅ | ✅ | ✅ |
| `itsanas put` | ✅ | ✅ | ✅ | ✅ |
| `itsanas get` | ✅ | ✅ | ✅ | ✅ |
| `itsanas rm` | ✅ | ✅ | ✅ | ✅ |
| `itsanas vault` | ✅ | ✅ | ✅ | ⬜ a phone has one app-private store |
| `itsanas backup` | ✅ | ✅ | ✅ | ⬜ a phone has no drive to back up to |
| `itsanas portable` | ✅ | ✅ | ✅ | ⬜ a phone cannot write a program for another system onto a drive |
| `itsanas copy-to-key` | ✅ | ✅ | ✅ | ⬜ a phone cannot write a program for another system onto a key |
| `itsanas folder` | ✅ | ✅ | ✅ | ⬜ a phone has no synced folder: its files are in the app |
| `itsanas scan` | ✅ | ✅ | ✅ | ⬜ terminal only: the app lists the account itself |
| `itsanas keep` | ✅ | ✅ | ✅ | ✅ |
| `itsanas space` | ✅ | ✅ | ✅ | ✅ |
| `itsanas pledge` | ✅ | ✅ | ✅ | ✅ |
| `itsanas device` | ✅ | ✅ | ✅ | ✅ |
| `itsanas listen` | ✅ | ✅ | ✅ | ⬜ a phone dials out only and hosts nothing for others |
| `itsanas announce` | ✅ | ✅ | ✅ | ⬜ a phone dials out only and publishes no address |
| `itsanas leave` | ✅ | ✅ | ✅ | ✅ |
| `itsanas rejoin` | ✅ | ✅ | ✅ | ⬜ a phone that left logs in again, with a new device id |
| `itsanas delete-account` | ✅ | ✅ | ✅ | ✅ |
| `itsanas serve` | ✅ | ✅ | ✅ | ⬜ a phone dials out only and hosts nothing for others |
| `itsanas daemon` | ✅ | ✅ | ✅ | ⬜ the app runs its rounds in its own sync service |
| `itsanas sync` | ✅ | ✅ | ✅ | ✅ |
| `itsanas coordinator` | ✅ | ✅ | ✅ | ⬜ a phone is built for one network |
| `itsanas invite` | ✅ | ✅ | ✅ | ⬜ terminal only: a phone uses an invitation, it does not make one |
| `itsanas register` | ✅ | ✅ | ✅ | ✅ |
| `itsanas peer` | ✅ | ✅ | ✅ | ✅ |
| `itsanas update` | ✅ | ✅ | ✅ | ✅ |
| `itsanas words` | ✅ | ✅ | ✅ | ✅ |
| `itsanas doctor` | ✅ | ✅ | ✅ | ⬜ terminal only: the final check of the page says the same in sentences |
| `itsanas bench` | ✅ | ✅ | ✅ | ⬜ terminal only: a measurement for whoever tunes a node |
| `itsanas convert-packs` | ✅ | ✅ | ✅ | ⬜ terminal only: a migration of stores made before packs |
| `itsanas gc` | ✅ | ✅ | ✅ | ⬜ terminal only: the app's rounds clear what they replace |
