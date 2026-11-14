//! One renewal schedule shared by foreground traffic and the status heartbeat.
use crate::{Credential, PushOptions, Result, origin_custody, origin_custody::renewal::Renewal};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const MINUTE: u64 = 60;
const DAY: u64 = 86_400;

#[derive(Default)]
struct Schedule {
    next_attempt: u64,
    hint: bool,
    notice: Option<(Notice, u64)>,
}
#[derive(Clone, PartialEq, Eq)]
enum Notice {
    Due,
    Renewing,
    Failure(String),
    Urgent(u8),
}
impl Schedule {
    fn attempt(&mut self, until: u64, now: u64, pending: bool) -> bool {
        if now >= until || now < self.next_attempt {
            return false;
        }
        if !pending && !self.hint && !origin_custody::renewal::window(until, now).1 {
            return false;
        }
        self.next_attempt = now.saturating_add(MINUTE);
        self.hint = false;
        true
    }
    fn notice(&mut self, outcome: Notice, now: u64) -> bool {
        // Starting another attempt never resets failure suppression.
        if outcome == Notice::Renewing && self.notice.is_some() {
            return false;
        }
        let emit = self
            .notice
            .as_ref()
            .is_none_or(|(previous, at)| previous != &outcome || now.saturating_sub(*at) >= DAY);
        if emit {
            self.notice = Some((outcome, now));
        }
        emit
    }
}

pub(crate) struct Delivery {
    base: PathBuf,
    options: PushOptions,
    watch: bool,
    schedule: Schedule,
}
impl Delivery {
    pub(crate) fn new(options: &PushOptions) -> Option<Self> {
        let Credential::Enrolled { authority, .. } = &options.credential else {
            return None;
        };
        Some(Self {
            base: authority.base().into(),
            options: options.clone(),
            watch: false,
            schedule: Schedule::default(),
        })
    }
    pub(crate) fn watch(&mut self, watch: bool) {
        self.watch = watch;
    }
    pub(crate) fn options(&self) -> &PushOptions {
        &self.options
    }
    pub(crate) fn clock() -> Result<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|_| "system clock is before epoch".into())
    }
    pub(crate) fn checkpoint(&mut self) -> Result<()> {
        self.checkpoint_at(Self::clock()?)
    }
    fn checkpoint_at(&mut self, now: u64) -> Result<()> {
        // Open reconciles durable delivery before constructing any application client.
        let mut renewal = Renewal::open(&self.base, &self.options.url)?;
        let until = renewal.configuration().scope.valid_until;
        let host = origin_custody::receiver_host(&self.options.url)?;
        let days = until.saturating_sub(now).div_ceil(DAY);
        if now >= until {
            return Err(format!("Receiver {host} credentials expired."));
        }
        let attempt = !renewal.awaiting_confirmation()
            && self.schedule.attempt(until, now, renewal.pending());
        let failure = if attempt { renewal.renew().err() } else { None };
        let outcome = if let Some(error) = failure.as_ref() {
            Some(Notice::Failure(error.clone()))
        } else if !renewal.awaiting_confirmation()
            && days <= 14
            && self
                .schedule
                .notice
                .as_ref()
                .is_some_and(|(n, _)| matches!(n, Notice::Failure(_) | Notice::Urgent(_)))
        {
            Some(Notice::Urgent(if days <= 7 { 7 } else { 14 }))
        } else if attempt {
            Some(Notice::Renewing)
        } else if origin_custody::renewal::window(until, now).0 {
            Some(Notice::Due)
        } else {
            None
        };
        if let Some(outcome) = outcome {
            // A repeated failed attempt still observes severity crossings.
            let severity = if days <= 7 {
                7
            } else if days <= 14 {
                14
            } else {
                30
            };
            let outcome = match outcome {
                Notice::Failure(error) => Notice::Failure(format!("{severity}:{error}")),
                other => other,
            };
            if self.schedule.notice(outcome.clone(), now) {
                match outcome {
                    Notice::Failure(_) if self.watch => eprintln!(
                        "Could not renew receiver {host} credentials; {days} days remain. Check the receiver is reachable; renewal will retry."
                    ),
                    Notice::Failure(_) | Notice::Urgent(_) if !self.watch => eprintln!(
                        "Could not renew receiver {host} credentials; {days} days remain. Check the receiver is reachable, then run semon push --receiver {} again.",
                        self.options.url
                    ),
                    Notice::Urgent(_) => eprintln!(
                        "Automatic renewal has not completed; {days} days remain. Check receiver {host} is reachable; renewal will retry."
                    ),
                    Notice::Renewing => eprintln!(
                        "Receiver {host} credentials are renewing automatically; {days} days remain."
                    ),
                    Notice::Due => {
                        eprintln!("Receiver {host} renewal is due soon; {days} days remain.")
                    }
                    Notice::Failure(_) => unreachable!(),
                }
            }
        }
        drop(renewal);
        self.options.credential = origin_custody::load(&self.base, Some(&self.options.url))?.1;
        Ok(())
    }
    /// Only trust the fixed hint from the enrolled HTTPS peer after application
    /// authentication. Redirects and authentication refusals cannot supply it.
    pub(crate) fn response(&mut self, status: u16, hint: bool) -> Result<()> {
        if hint && authenticated(status) {
            self.schedule.hint = true;
        }
        if (200..300).contains(&status) {
            let mut renewal = Renewal::open(&self.base, &self.options.url)?;
            if renewal.awaiting_confirmation() {
                renewal.confirm(renewal.configuration().generation)?;
                self.schedule.notice = None;
                eprintln!(
                    "Receiver {} credentials renewed.",
                    origin_custody::receiver_host(&self.options.url)?
                );
            }
        }
        Ok(())
    }
}
fn authenticated(status: u16) -> bool {
    (200..300).contains(&status) || matches!(status, 413 | 423 | 429)
}

#[cfg(test)]
mod tests {
    use super::*;
    const WINDOW: u64 = 2_592_000;
    #[test]
    fn boundary_hint_and_backoff() {
        let mut s = Schedule::default();
        assert!(!s.attempt(WINDOW + 100, 100, false));
        assert!(s.attempt(WINDOW + 100, 101, false));
        s.hint = true;
        assert!(!s.attempt(WINDOW + 100, 160, false));
        assert!(s.attempt(WINDOW + 100, 161, false));
        assert!(!s.attempt(200, 200, true));
    }
    #[test]
    fn enabled_kind_hint_can_trigger_early() {
        let mut s = Schedule {
            hint: true,
            ..Schedule::default()
        };
        assert!(s.attempt(WINDOW * 3, 100, false));
        assert!(!s.hint);
        assert!(!s.attempt(WINDOW * 3, 160, false));
    }
    #[test]
    fn hints_require_application_authentication() {
        for status in [200, 204, 413, 423, 429] {
            assert!(authenticated(status));
        }
        for status in [301, 401, 403, 404, 500] {
            assert!(!authenticated(status));
        }
    }
    #[test]
    fn unchanged_failure_and_attempt_starts_stay_quiet() {
        let mut s = Schedule::default();
        let failure = Notice::Failure("30:temporary".into());
        assert!(s.notice(failure.clone(), 100));
        assert!(!s.notice(Notice::Renewing, 160));
        assert!(!s.notice(failure.clone(), 161));
        assert!(s.notice(failure, 100 + DAY));
    }
}
