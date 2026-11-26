//! One updater per interactive profile, shared by all native windows.
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant, SystemTime},
};
use vel_platform::MenuEntry;
use vel_update::{CHECK_INTERVAL, Client, Install, Offer, Outcome, Platform, Reason, Stage, State};

pub(crate) const CHECK: &str = "check_for_updates";
pub(crate) const ACTIONS: &str = "update_actions";
pub(crate) const ACTION: &str = "update_action";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
pub(crate) enum Notice {
    Checking,
    Current,
    Skipped(String),
    Available(Offer, Result<Install, Reason>),
    Downloading(Offer),
    Ready(Offer),
    Recovered,
    Error(Reason, Option<Offer>),
}

impl Notice {
    pub fn actions(&self, action: &str) -> bool {
        match action {
            "restart" | "on_quit" => matches!(self, Self::Available(_, Ok(_)) | Self::Ready(_)),
            "notes" => self.offer().is_some() || matches!(self, Self::Error(..) | Self::Recovered),
            "skip" => matches!(self, Self::Available(..) | Self::Ready(_)),
            "later" => !matches!(self, Self::Checking | Self::Downloading(_)),
            _ => false,
        }
    }

    pub fn offer(&self) -> Option<&Offer> {
        match self {
            Self::Available(offer, _) | Self::Ready(offer) | Self::Error(_, Some(offer)) => {
                Some(offer)
            }
            _ => None,
        }
    }
}

pub(crate) fn menu(tr: &vel_i18n::Translator, notice: Option<&Notice>) -> Vec<MenuEntry> {
    [
        ("Restart to Update", "restart"),
        ("Install on Quit", "on_quit"),
        ("Release Notes", "notes"),
        ("Later", "later"),
        ("Skip This Version", "skip"),
    ]
    .into_iter()
    .filter(|(_, action)| notice.is_none_or(|notice| notice.actions(action)))
    .map(|(caption, action)| MenuEntry {
        caption: tr.tr(caption).into_owned(),
        command: Some(ACTION.into()),
        args: serde_json::json!({"action": action}).to_string(),
        ..Default::default()
    })
    .collect()
}

#[derive(Debug)]
enum Reply {
    Checked(Duration, Result<Option<Notice>, Reason>, bool),
    Staged(Offer, vel_update::Result<Box<Stage>>, bool),
    Skipped(vel_update::Result<()>),
}

#[derive(Debug)]
pub(crate) struct Updates {
    data: PathBuf,
    next: Instant,
    worker: Option<Receiver<Reply>>,
    pub notice: Option<Notice>,
    armed: Option<Box<Stage>>,
    immediate: bool,
    apply_on_quit: bool,
    system_ending: bool,
    pub revision: u64,
    startup_ticket: Option<PathBuf>,
    startup_failed: bool,
}

impl Updates {
    pub fn new(
        data: PathBuf,
        launched: Instant,
        startup_ticket: Option<PathBuf>,
        recovered: bool,
    ) -> Self {
        Self {
            data,
            next: launched + Duration::from_secs(30),
            worker: None,
            notice: recovered.then_some(Notice::Recovered),
            armed: None,
            immediate: false,
            apply_on_quit: false,
            system_ending: false,
            revision: 0,
            startup_ticket,
            startup_failed: false,
        }
    }

    fn set(&mut self, notice: Option<Notice>) {
        self.notice = notice;
        self.revision = self.revision.wrapping_add(1);
    }

    fn run(&mut self, operation: impl FnOnce() -> Reply + Send + 'static) -> bool {
        let (tx, rx) = mpsc::channel();
        if std::thread::Builder::new()
            .name("velocity-update".into())
            .spawn(move || {
                let _ = tx.send(operation());
            })
            .is_err()
        {
            return false;
        }
        self.worker = Some(rx);
        true
    }

    pub fn check(&mut self, manual: bool) {
        if self.worker.is_some() || self.armed.is_some() {
            return;
        }
        let data = self.data.clone();
        if manual {
            self.set(Some(Notice::Checking));
        }
        if !self.run(move || {
            let now = SystemTime::now();
            let mut state = State::load(&data);
            let result = Client::new(VERSION, Platform::current())
                .and_then(|client| client.check(&data, &mut state, now, manual))
                .map(|outcome| match outcome {
                    Outcome::NotDue => None,
                    Outcome::Current => manual.then_some(Notice::Current),
                    Outcome::Skipped(version) => {
                        manual.then(|| Notice::Skipped(version.to_string()))
                    }
                    Outcome::Available(offer) => {
                        let install = std::env::current_exe()
                            .map_err(|_| Reason::ManualInstall)
                            .and_then(|exe| {
                                Install::detect(&exe, Platform::current(), VERSION)
                                    .map_err(|error| error.reason)
                            });
                        Some(Notice::Available(offer, install))
                    }
                })
                .map_err(|error| error.reason);
            let due = state.due_in(SystemTime::now());
            Reply::Checked(
                if due.is_zero() { CHECK_INTERVAL } else { due },
                result,
                manual,
            )
        }) {
            self.next = Instant::now() + CHECK_INTERVAL;
            if manual {
                self.set(Some(Notice::Error(Reason::State, None)));
            }
        }
    }

    pub fn stage(&mut self, restart: bool) {
        if self.worker.is_some() {
            return;
        }
        if self.armed.is_some() {
            self.immediate = restart;
            self.apply_on_quit = true;
            return;
        }
        let Some(Notice::Available(offer, Ok(install))) = self.notice.take() else {
            return;
        };
        let data = self.data.clone();
        self.set(Some(Notice::Downloading(offer.clone())));
        if !self.run(move || {
            let result = Client::new(VERSION, Platform::current())
                .and_then(|client| install.stage(&client, &data, offer.clone()))
                .map(Box::new);
            Reply::Staged(offer, result, restart)
        }) {
            self.set(Some(Notice::Error(Reason::State, None)));
        }
    }

    pub fn later(&mut self) {
        if self.worker.is_none() {
            self.armed = None;
            self.immediate = false;
            self.apply_on_quit = false;
            self.set(None);
        }
    }

    pub fn skip(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let offer = match &self.notice {
            Some(Notice::Available(offer, _) | Notice::Ready(offer)) => offer.clone(),
            _ => return,
        };
        self.armed = None;
        self.immediate = false;
        self.apply_on_quit = false;
        let data = self.data.clone();
        if !self.run(move || Reply::Skipped(State::skip(&data, offer.version))) {
            self.set(Some(Notice::Error(Reason::State, None)));
        }
    }

    pub fn poll(&mut self, automatic: bool, now: Instant) {
        if let Some(worker) = &self.worker {
            let reply = worker.try_recv();
            if matches!(reply, Err(TryRecvError::Empty)) {
                return;
            }
            self.worker = None;
            match reply {
                Ok(Reply::Checked(due, result, manual)) => {
                    self.next = now + due;
                    if manual || automatic {
                        match result {
                            Ok(Some(notice)) => self.set(Some(notice)),
                            Err(reason) if manual => self.set(Some(Notice::Error(reason, None))),
                            Ok(None) if matches!(self.notice, Some(Notice::Available(..))) => {
                                self.set(None)
                            }
                            _ => (),
                        }
                    }
                }
                Ok(Reply::Staged(offer, Ok(stage), restart)) => {
                    self.armed = Some(stage);
                    self.immediate = restart;
                    self.apply_on_quit = true;
                    self.set(Some(Notice::Ready(offer)));
                }
                Ok(Reply::Staged(offer, Err(error), _)) => {
                    self.set(Some(Notice::Error(error.reason, Some(offer))))
                }
                Ok(Reply::Skipped(Ok(()))) => self.later(),
                Ok(Reply::Skipped(Err(error))) => self.set(Some(Notice::Error(error.reason, None))),
                _ => {
                    self.next = now + CHECK_INTERVAL;
                    if matches!(
                        &self.notice,
                        Some(Notice::Checking | Notice::Downloading(_))
                    ) {
                        self.set(Some(Notice::Error(Reason::State, None)));
                    }
                }
            }
        }
        if automatic && now >= self.next {
            self.check(false);
        }
    }

    pub fn take_immediate(&mut self) -> bool {
        std::mem::take(&mut self.immediate)
    }

    pub fn preparing_quit(&self) -> bool {
        self.apply_on_quit && self.armed.is_some() && !self.system_ending
    }

    pub fn cancel_restart(&mut self) {
        self.apply_on_quit = false;
    }

    pub fn worker_busy(&self) -> bool {
        self.worker.is_some()
    }

    pub fn awaiting_startup(&self) -> bool {
        self.startup_ticket.is_some()
    }

    pub fn fail_startup(&mut self) {
        self.startup_failed = true;
    }

    pub fn ending_session(&mut self) {
        self.system_ending = true;
    }

    pub fn quit_approved(&mut self) -> bool {
        if !self.preparing_quit() {
            return true;
        }
        if let Some(stage) = self.armed.take()
            && let Err(error) = stage.launch()
        {
            eprintln!("velocity: could not start update helper: {error}");
            self.apply_on_quit = false;
            self.set(Some(Notice::Error(error.reason, None)));
            return false;
        }
        true
    }

    pub fn confirm_startup(&mut self) {
        if self.startup_failed {
            return;
        }
        if let Some(ticket) = self.startup_ticket.take()
            && let Err(error) = std::fs::File::create_new(ticket).and_then(|mut f| {
                use std::io::Write;
                write!(f, "{}", std::process::id())
            })
        {
            eprintln!("velocity: could not confirm update startup: {error}");
        }
    }
}
