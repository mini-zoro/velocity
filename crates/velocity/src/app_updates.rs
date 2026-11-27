use super::*;
use crate::updates::{self, Notice};
use vel_update::Reason;

impl App {
    pub(super) fn poll_updates(&mut self, window: &WindowHandle) -> bool {
        let mut sessions = self.sessions.borrow_mut();
        let Some(updates) = sessions.updates.as_mut() else {
            return false;
        };
        updates.poll(
            self.workspace.global_bool("update_check", true),
            Instant::now(),
        );
        if updates.take_immediate() {
            window.request_app_quit();
        }
        let changed = self.update_revision != updates.revision;
        self.update_revision = updates.revision;
        changed
    }

    pub(super) fn updating_on_quit(&self) -> bool {
        self.sessions
            .borrow()
            .updates
            .as_ref()
            .is_some_and(updates::Updates::preparing_quit)
    }

    pub(super) fn cancel_update_restart(&self) {
        if let Some(updates) = self.sessions.borrow_mut().updates.as_mut() {
            updates.cancel_restart();
        }
    }

    pub(super) fn update_starting(&self) -> bool {
        self.sessions
            .borrow()
            .updates
            .as_ref()
            .is_some_and(updates::Updates::awaiting_startup)
    }

    pub(super) fn update_label(&self) -> Option<(String, bool)> {
        let sessions = self.sessions.borrow();
        let updates = sessions.updates.as_ref()?;
        let notice = updates.notice.as_ref()?;
        let mut label = match notice {
            Notice::Checking => UiText::tr("Checking for Velocity updates…"),
            Notice::Current => UiText::tr("Velocity is up to date."),
            Notice::Skipped(version) => UiText::tr_fmt(
                "Velocity {version} was skipped.",
                [("version", version.clone())],
            ),
            Notice::Available(offer, Ok(_)) => UiText::tr_fmt(
                "Velocity {version} is available — Update Actions…",
                [("version", offer.version.to_string())],
            ),
            Notice::Available(offer, Err(reason)) => match reason {
                Reason::NotWritable => UiText::tr_fmt(
                    "Velocity {version} is available. The install folder is not writable.",
                    [("version", offer.version.to_string())],
                ),
                _ => UiText::tr_fmt(
                    "Velocity {version} is available for manual installation.",
                    [("version", offer.version.to_string())],
                ),
            },
            Notice::Downloading(offer) => UiText::tr_fmt(
                "Downloading and verifying Velocity {version}…",
                [("version", offer.version.to_string())],
            ),
            Notice::Ready(offer) => UiText::tr_fmt(
                "Velocity {version} is ready to install on quit.",
                [("version", offer.version.to_string())],
            ),
            Notice::Recovered => {
                UiText::tr("The update did not finish. Velocity reopened the previous version.")
            }
            Notice::Error(reason, _) => UiText::tr(match reason {
                Reason::Network => "Could not reach GitHub to check or download the update.",
                Reason::Release => "GitHub returned invalid Velocity release information.",
                Reason::MissingSignature => "Update blocked: the release signature is missing.",
                Reason::SigningKey => "Update blocked: this build has no update verification key.",
                Reason::Signature => "Update blocked: the release signature could not be verified.",
                Reason::Checksum => "Update blocked: the download checksum could not be verified.",
                Reason::NotWritable => "Update blocked: the install folder is not writable.",
                Reason::ManualInstall => {
                    "Install this Velocity update manually from the release page."
                }
                Reason::Staging => "The update could not be staged or verified.",
                Reason::State => "Velocity could not save or read its update state.",
            }),
        };
        label.refresh(&self.workspace.translations);
        Some((
            label.to_string(),
            !updates.worker_busy() && notice.actions("later"),
        ))
    }

    pub(super) fn open_update_actions(&self, window: &WindowHandle) -> bool {
        let sessions = self.sessions.borrow();
        let Some(updates) = sessions
            .updates
            .as_ref()
            .filter(|updates| !updates.worker_busy())
        else {
            return false;
        };
        let Some(notice) = &updates.notice else {
            return false;
        };
        window.show_context_menu(&updates::menu(&self.workspace.translations, Some(notice)))
    }

    pub(super) fn update_command_enabled(&self, name: &str, args: &Value) -> Option<bool> {
        if !matches!(name, updates::CHECK | updates::ACTION) {
            return None;
        }
        let sessions = self.sessions.borrow();
        let updates = sessions
            .updates
            .as_ref()
            .filter(|updates| !updates.worker_busy());
        Some(updates.is_some_and(|updates| {
            name == updates::CHECK
                || updates.notice.as_ref().is_some_and(|notice| {
                    args["action"]
                        .as_str()
                        .is_some_and(|action| notice.actions(action))
                })
        }))
    }

    pub(super) fn update_ui_command(
        &mut self,
        window: &WindowHandle,
        name: &str,
        args: &str,
    ) -> bool {
        if !matches!(name, updates::CHECK | updates::ACTION) {
            return false;
        }
        let args = serde_json::from_str::<Value>(args).unwrap_or_default();
        if self.update_command_enabled(name, &args) != Some(true) {
            return true;
        }
        if self.settings_window {
            self.forward_from_settings(Some(window), name, &args);
            return true;
        }
        if window.is_closed() && name == updates::CHECK {
            self.on_reopen(window);
        }
        let mut notes = None;
        if let Some(updates) = self.sessions.borrow_mut().updates.as_mut() {
            if name == updates::CHECK {
                updates.check(true);
            } else {
                match args["action"].as_str().unwrap_or_default() {
                    "restart" => updates.stage(true),
                    "on_quit" => updates.stage(false),
                    "notes" => {
                        notes =
                            Some(updates.notice.as_ref().and_then(Notice::offer).map_or_else(
                                || vel_update::RELEASES.into(),
                                vel_update::Offer::notes,
                            ))
                    }
                    "later" => updates.later(),
                    "skip" => updates.skip(),
                    _ => (),
                }
            }
        }
        if let Some(url) = notes {
            self.run_named(Some(window), "open_url", &json!({"url": url}));
        }
        window.request_frame();
        true
    }
}
