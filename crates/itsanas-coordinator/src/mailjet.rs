//! The coordinator's mail, sent through Mailjet's API by `curl`.
//!
//! Configured by three environment variables of the service: `MAILJET_API_KEY`,
//! `MAILJET_SECRET_KEY` and `ITSANAS_MAIL_FROM` (a sender address Mailjet has
//! validated). With any of them missing the coordinator sends nothing and
//! refuses to bind an address. The keys reach `curl` on its standard input
//! (`--config -`), never on its command line, where any local user's `ps`
//! would read them.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;

use itsanas_coord::mail::Mail;

/// What sending needs, read from the environment.
#[derive(Clone)]
pub struct Mailjet {
    key: String,
    secret: String,
    from: String,
}

impl std::fmt::Debug for Mailjet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mailjet")
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

/// Mailjet's v3.1 send endpoint.
const ENDPOINT: &str = "https://api.mailjet.com/v3.1/send";

impl Mailjet {
    /// The configuration in the environment, or `None` when any part is
    /// missing or empty.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let read = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        };
        Some(Self {
            key: read("MAILJET_API_KEY")?,
            secret: read("MAILJET_SECRET_KEY")?,
            from: read("ITSANAS_MAIL_FROM")?,
        })
    }

    /// The sender address, for the start-up lines.
    #[must_use]
    pub fn from(&self) -> &str {
        &self.from
    }

    /// Send every mail the outbox queues, until it closes. A failure is
    /// logged without the address or the text: the log is the operator's,
    /// and a code in it would open an account.
    pub fn run(&self, queue: &Receiver<Mail>) {
        for mail in queue {
            if let Err(why) = self.send(&mail) {
                eprintln!("itsanas-coordinator: a mail was not sent: {why}");
            }
        }
    }

    fn send(&self, mail: &Mail) -> Result<(), String> {
        let mut child = Command::new("curl")
            .args(["--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not run curl ({error}); is it installed?"))?;
        child
            .stdin
            .take()
            .ok_or("curl took no input")?
            .write_all(self.config(mail).as_bytes())
            .map_err(|error| format!("could not hand curl its request: {error}"))?;
        let output = child
            .wait_with_output()
            .map_err(|error| format!("curl did not finish: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            let said = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "Mailjet refused it ({}): {}",
                output.status,
                said.trim()
            ))
        }
    }

    /// The whole `curl` request, as the config text it reads on its input.
    fn config(&self, mail: &Mail) -> String {
        let body = serde_json::json!({
            "Messages": [{
                "From": { "Email": self.from, "Name": "ITSaNAS" },
                "To": [{ "Email": mail.to }],
                "Subject": mail.subject,
                "TextPart": mail.text,
            }]
        })
        .to_string();
        format!(
            "url = {}\nuser = {}\nheader = \"Content-Type: application/json\"\ndata-binary = {}\nproto = \"=https\"\nmax-time = 20\nsilent\nshow-error\nfail\n",
            quoted(ENDPOINT),
            quoted(&format!("{}:{}", self.key, self.secret)),
            quoted(&body),
        )
    }
}

/// `text` as one double-quoted value of a curl config file: a backslash and
/// a quote escaped, and nothing else can end the line (JSON has no raw
/// newline in it, and the keys none either; any that slipped in is escaped).
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailjet() -> Mailjet {
        Mailjet {
            key: "K".to_owned(),
            secret: "S\"ecret".to_owned(),
            from: "noreply@a.example".to_owned(),
        }
    }

    #[test]
    fn red_team_a_mail_cannot_add_a_line_to_curls_request() {
        // A member chooses the address and, through the account name, part of
        // the text: a quote or a newline in either must not end the value and
        // start an option of its own, such as another `url` or an `output`.
        let mail = Mail {
            to: "a@b.example\"\noutput = \"/etc/passwd".to_owned(),
            subject: "s".to_owned(),
            text: "line one\nurl = \"http://evil.example\"".to_owned(),
        };
        let config = mailjet().config(&mail);
        let lines: Vec<&str> = config.lines().collect();
        assert_eq!(lines.len(), 9, "{config}");
        for line in &lines {
            assert!(
                !line.starts_with("output")
                    && (!line.starts_with("url") || line.contains("api.mailjet.com")),
                "a value escaped its quotes: {line}"
            );
        }
        assert!(config.contains("user = \"K:S\\\"ecret\""), "{config}");
    }

    #[test]
    fn the_request_is_mailjets_v3_1_send_over_https_only() {
        let config = mailjet().config(&Mail {
            to: "to@b.example".to_owned(),
            subject: "Subject".to_owned(),
            text: "Text".to_owned(),
        });
        assert!(config.starts_with("url = \"https://api.mailjet.com/v3.1/send\"\n"));
        assert!(config.contains("proto = \"=https\""));
        assert!(
            config.contains(r#"\"To\":[{\"Email\":\"to@b.example\"}]"#),
            "{config}"
        );
        assert!(config.contains("fail\n"));
    }

    #[test]
    fn the_keys_are_never_in_the_debug_form() {
        let shown = format!("{:?}", mailjet());
        assert!(!shown.contains("\"K\""), "{shown}");
        assert!(!shown.contains("ecret"), "{shown}");
    }
}
