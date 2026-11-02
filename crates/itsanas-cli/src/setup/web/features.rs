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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Features {
    /// The folder question, Open, the file manager's favourites.
    pub(crate) folder: bool,
    /// "Start when I log in".
    pub(crate) background_choice: bool,
    /// The coordinator's address and the updates choice (the invitation stays).
    pub(crate) advanced: bool,
    /// Pause and the interval: a phone runs on the policy's schedule.
    pub(crate) sync_controls: bool,
    /// Move the vault, back up, a portable copy.
    pub(crate) disks: bool,
    /// Copy this account to a USB key.
    pub(crate) usb_key: bool,
    /// Another account beside this one (instances).
    pub(crate) another_account: bool,
    /// How much of the account's files stay here, on the space question of the
    /// first run.
    pub(crate) keep_question: bool,
    /// Unlock with a fingerprint or a face.
    pub(crate) biometric: bool,
    /// Download over mobile data, or only on Wi-Fi.
    pub(crate) metered: bool,
}

impl Features {
    /// A computer: everything except what only a phone has.
    pub(crate) const DESKTOP: Self = Self {
        folder: true,
        background_choice: true,
        advanced: true,
        sync_controls: true,
        disks: true,
        usb_key: true,
        another_account: true,
        keep_question: false,
        biometric: false,
        metered: false,
    };

    /// A phone: its files are in the app, it dials out only, and it has the one
    /// network the app was built for.
    pub(crate) const PHONE: Self = Self {
        folder: false,
        background_choice: false,
        advanced: false,
        sync_controls: false,
        disks: false,
        usb_key: false,
        another_account: false,
        keep_question: true,
        biometric: true,
        metered: true,
    };

    /// Every feature, by the name the page uses.
    pub(crate) const fn all(self) -> [(&'static str, bool); 10] {
        [
            ("folder", self.folder),
            ("background_choice", self.background_choice),
            ("advanced", self.advanced),
            ("sync_controls", self.sync_controls),
            ("disks", self.disks),
            ("usb_key", self.usb_key),
            ("another_account", self.another_account),
            ("keep_question", self.keep_question),
            ("biometric", self.biometric),
            ("metered", self.metered),
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
pub(crate) trait Host: Send + Sync {
    fn event(&self, name: &str, value: &str);
    /// What the shell holds for a setting it owns (`biometric`, `metered`):
    /// `on` or `off`, or `None` when it does not say.
    fn value(&self, _name: &str) -> Option<String> {
        None
    }
}

/// A shell with nothing of its own to tell.
#[derive(Debug)]
pub(crate) struct NoHost;

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
            let phone_only = matches!(name, "keep_question" | "biometric" | "metered");
            assert_eq!(on_phone, phone_only, "{name} on a phone");
            assert_eq!(on_desktop, !phone_only, "{name} on a desktop");
        }
    }
}
