//! What a shell has, as data: one page for a desktop and a phone.
//!
//! The page is the same on every platform; a shell that lacks something does not
//! get a different page, it gets the same page with that part hidden
//! (`data-feature` in `index.html`, hidden by `app.js` from `/api/state`) and
//! the routes behind it refused. Hiding is not the control: a request that skips
//! the page meets the refusal here.

use super::http::Response;

/// What this shell can do. Every field has a `data-feature` of its name in
/// `index.html`; `every_feature_is_named_in_the_page` fails otherwise.
// Independent switches, one per thing a shell may lack: not a state machine
// in disguise, so the lint's advice (an enum) would only hide the table.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    /// The folder question, Open, the file manager's favourites.
    pub folder: bool,
    /// "Start when I log in".
    pub background_choice: bool,
    /// The coordinator's address and the updates choice (the invitation stays).
    pub advanced: bool,
    /// Pause and the interval: a phone runs on the policy's schedule.
    pub sync_controls: bool,
    /// Move the vault, back up, a portable copy.
    pub disks: bool,
    /// Copy this account to a USB key.
    pub usb_key: bool,
    /// Another account beside this one (instances).
    pub another_account: bool,
    /// How much of the account's files stay here, on the space question of the
    /// first run.
    pub keep_question: bool,
    /// "I forgot my passphrase" while the account is open. A phone's service
    /// is this process, so the node would never let go of the store: it asks on
    /// its Unlock screen, with nothing open.
    pub forgot_in_settings: bool,
    /// Unlock with a fingerprint or a face.
    pub biometric: bool,
    /// Download over mobile data, or only on Wi-Fi.
    pub metered: bool,
    /// Remove the account from this machine, or delete it: the page stops the
    /// service, tells the network, erases the home and tells the shell. A
    /// computer removes a machine with `install/clean` and the tray, which also
    /// take the service away; the page does not yet.
    pub remove_account: bool,
    /// Lend space to other members. An iPhone ends an app it does not show.
    pub hosting: bool,
    /// Check for updates now. A computer runs `itsanas update`, an Android
    /// phone asks for the signed APK; an iPhone is installed by whoever signs it.
    pub update_check: bool,
}

impl Features {
    /// A computer: everything except what only a phone has.
    pub const DESKTOP: Self = Self {
        folder: true,
        background_choice: true,
        advanced: true,
        sync_controls: true,
        disks: true,
        usb_key: true,
        another_account: true,
        keep_question: false,
        forgot_in_settings: true,
        biometric: false,
        metered: false,
        remove_account: false,
        hosting: true,
        update_check: true,
    };

    /// A phone: its files are in the app, its loop runs when the policy says, and it has the one
    /// network the app was built for.
    pub const PHONE: Self = Self {
        folder: false,
        background_choice: false,
        advanced: false,
        sync_controls: false,
        disks: false,
        usb_key: false,
        another_account: false,
        keep_question: true,
        forgot_in_settings: false,
        biometric: true,
        metered: true,
        remove_account: true,
        hosting: true,
        update_check: true,
    };

    /// An iPhone: a phone that cannot serve anybody in the background, is
    /// installed by whoever signs it (so it has no update to check for), and
    /// has no mobile-data policy because it has no background sync.
    pub const IPHONE: Self = Self {
        hosting: false,
        metered: false,
        biometric: false,
        update_check: false,
        ..Self::PHONE
    };

    /// Every feature, by the name the page uses.
    #[must_use]
    pub const fn all(self) -> [(&'static str, bool); 14] {
        [
            ("folder", self.folder),
            ("background_choice", self.background_choice),
            ("advanced", self.advanced),
            ("sync_controls", self.sync_controls),
            ("disks", self.disks),
            ("usb_key", self.usb_key),
            ("another_account", self.another_account),
            ("keep_question", self.keep_question),
            ("forgot_in_settings", self.forgot_in_settings),
            ("biometric", self.biometric),
            ("metered", self.metered),
            ("remove_account", self.remove_account),
            ("hosting", self.hosting),
            ("update_check", self.update_check),
        ]
    }

    /// The refusal for a route whose feature this shell lacks: `None` when it
    /// has it. The sentence names what is missing, so a person who reached it
    /// by hand knows it is not a fault.
    pub(super) fn refuse(enabled: bool, what: &str) -> Option<Response> {
        (!enabled).then(|| {
            Response::refuse(
                403,
                &format!("this app has no {what}: it is not offered here"),
            )
        })
    }
}

/// What a shell is told when the page changes something it also draws or
/// remembers: `theme`, `removed`, `signedOut`, `closed`, `biometric`, `metered`.
/// A desktop has nothing to draw, so it is told nothing.
pub trait Host: Send + Sync {
    fn event(&self, name: &str, value: &str);
    /// What the shell holds for a setting it owns (`biometric`, `metered`):
    /// `on` or `off`, or `None` when it does not say.
    fn value(&self, _name: &str) -> Option<String> {
        None
    }
}

impl Features {
    /// [`Features::DESKTOP`] on this computer: Windows Hello and Touch ID exist
    /// on two of the three systems.
    pub const THIS_COMPUTER: Self = Self {
        biometric: cfg!(any(windows, target_os = "macos")),
        ..Self::DESKTOP
    };
}

/// Start `itsanas update` on its own, its words in `<home>/update.log`.
fn start_update(home: &std::path::Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let log = std::fs::File::create(home.join("update.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("update")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        // No console window flashing over the page.
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn().map(drop)
}

/// The computer's own: its unlock, when the page turns it on or off.
#[derive(Debug)]
pub struct ComputerUnlock {
    pub home: std::path::PathBuf,
    pub instance: Option<String>,
}

impl Host for ComputerUnlock {
    fn event(&self, name: &str, value: &str) {
        use crate::setup::unlock;
        if name == "update" {
            // `itsanas update`, apart from this page: it checks, installs and
            // restarts every node, this page's own included, which a call made
            // inside the page would not live through. What it said is in
            // `<home>/update.log`.
            if let Err(why) = start_update(&self.home) {
                eprintln!("itsanas: the update check did not start: {why}");
            }
            return;
        }
        if name != "biometric" {
            return;
        }
        let service: Box<dyn crate::setup::service::ServiceControl> = Box::new(
            crate::setup::service::Platform::of_this_machine(&self.home, self.instance.as_deref()),
        );
        let done = if value == "on" {
            unlock::enable(&self.home, service.as_ref(), unlock::platform_gate)
        } else {
            unlock::disable(&self.home, service.as_ref(), unlock::platform_gate)
        };
        if let Err(why) = done {
            eprintln!("itsanas: the unlock did not change: {why}");
        }
    }

    fn value(&self, name: &str) -> Option<String> {
        (name == "biometric").then(|| {
            if crate::setup::unlock::is_on(&self.home) {
                "on"
            } else {
                "off"
            }
            .to_owned()
        })
    }
}

/// A shell with nothing of its own to tell.
#[derive(Debug)]
pub struct NoHost;

impl Host for NoHost {
    fn event(&self, _: &str, _: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_has_what_only_a_phone_has_and_a_computer_the_rest() {
        let phone = Features::PHONE.all();
        let desktop = Features::DESKTOP.all();
        for ((name, on_phone), (_, on_desktop)) in phone.into_iter().zip(desktop) {
            // Every Android phone and computer lends space and checks for
            // updates; only an iPhone cannot.
            if matches!(name, "hosting" | "update_check") {
                assert!(on_phone && on_desktop, "{name}");
                assert!(
                    Features::IPHONE.all().contains(&(name, false)),
                    "an iPhone offered {name}"
                );
                continue;
            }
            let phone_only = matches!(
                name,
                "keep_question" | "biometric" | "metered" | "remove_account"
            );
            assert_eq!(on_phone, phone_only, "{name} on a phone");
            assert_eq!(on_desktop, !phone_only, "{name} on a desktop");
        }
    }
}
