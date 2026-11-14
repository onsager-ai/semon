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
    failure: Option<String>,
    notice: Option<(Notice, u64)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Notice {
    Due,
    Renewing,
    Failure(String, u8),
    Urgent(u8),
    Renewed,
}
impl Schedule {
    fn attempt(&mut self, until: u64, now: u64, pending: bool) -> bool {
        if now >= until || now < self.next_attempt {
            return false;
        }
        if !pending
            && !self.hint
            && self.failure.is_none()
            && !origin_custody::renewal::window(until, now).1
        {
            return false;
        }
        self.next_attempt = now.saturating_add(MINUTE);
        self.hint = false;
        true
    }
    fn outcome(&mut self, until: u64, now: u64, attempt: Option<Result<()>>) -> Option<Notice> {
        let days = until.saturating_sub(now).div_ceil(DAY);
        let severity = if days <= 7 {
            7
        } else if days <= 14 {
            14
        } else {
            30
        };
        let changed_failure =
            matches!(&attempt, Some(Err(error)) if self.failure.as_ref() != Some(error));
        if let Some(Err(error)) = &attempt {
            self.failure = Some(error.clone());
        }
        let outcome = if let Some(error) = &self.failure {
            let failure = Notice::Failure(error.clone(), severity);
            match &self.notice {
                Some((Notice::Failure(previous, level), at)) if previous == error => {
                    if *level == severity && now.saturating_sub(*at) < DAY {
                        return None;
                    }
                    if severity <= 14 {
                        Notice::Urgent(severity)
                    } else {
                        failure
                    }
                }
                Some((Notice::Urgent(level), at)) if !changed_failure => {
                    if *level == severity && now.saturating_sub(*at) < DAY {
                        return None;
                    }
                    Notice::Urgent(severity)
                }
                _ => failure,
            }
        } else if attempt.is_some() {
            Notice::Renewing
        } else if origin_custody::renewal::window(until, now).0 {
            Notice::Due
        } else {
            return None;
        };
        // Attempt start is not a changed outcome. Keep the previous reminder
        // timestamp so alternating starts and failures cannot flood stderr.
        if outcome == Notice::Renewing && self.notice.is_some() {
            return None;
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|(previous, at)| previous == &outcome && now.saturating_sub(*at) < DAY)
        {
            return None;
        }
        self.notice = Some((outcome.clone(), now));
        Some(outcome)
    }
    fn hint(&mut self, status: u16, due: bool) {
        if due && authenticated(status) {
            self.hint = true;
        }
    }
    fn confirmed(&mut self) {
        self.notice = None;
        self.failure = None;
    }
}

pub(crate) struct Delivery {
    base: PathBuf,
    options: PushOptions,
    watch: bool,
    schedule: Schedule,
    #[cfg(test)]
    renew: Option<fn(&mut Renewal, u64) -> Result<()>>,
    #[cfg(test)]
    notices: Vec<String>,
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
            #[cfg(test)]
            renew: None,
            #[cfg(test)]
            notices: Vec::new(),
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
        let mut renewal = match Renewal::open(&self.base, &self.options.url) {
            Ok(renewal) => renewal,
            Err(error) => {
                // Install-lock contention is a renewal failure, not evidence
                // that a still-valid selected application credential was refused.
                let (config, credential) =
                    origin_custody::load_at(&self.base, Some(&self.options.url), now)?;
                if let Some(notice) =
                    self.schedule
                        .outcome(config.scope.valid_until, now, Some(Err(error)))
                {
                    self.emit(notice, config.scope.valid_until, now)?;
                }
                self.options.credential = credential;
                return Ok(());
            }
        };
        let until = renewal.configuration().scope.valid_until;
        if now >= until {
            return Err(self.recovery("credentials expired", &renewal.configuration().machine)?);
        }
        let attempt = !renewal.awaiting_confirmation()
            && self.schedule.attempt(until, now, renewal.pending());
        let result = if attempt {
            #[cfg(test)]
            let result = match self.renew {
                Some(renew) => renew(&mut renewal, now),
                None => renewal.renew(),
            };
            #[cfg(not(test))]
            let result = renewal.renew();
            Some(result)
        } else {
            None
        };
        if let Some(notice) = self.schedule.outcome(until, now, result) {
            self.emit(notice, until, now)?;
        }
        drop(renewal);
        #[cfg(not(test))]
        let now = Self::clock()?;
        self.options.credential =
            origin_custody::load_at(&self.base, Some(&self.options.url), now)?.1;
        Ok(())
    }
    fn emit(&mut self, notice: Notice, until: u64, now: u64) -> Result<()> {
        let host = origin_custody::receiver_host(&self.options.url)?;
        let days = until.saturating_sub(now).div_ceil(DAY);
        let message = match notice {
            Notice::Failure(_, _) if self.watch => format!(
                "Could not renew receiver {host} credentials; {days} days remain. Check the receiver is reachable; renewal will retry."
            ),
            Notice::Failure(_, _) | Notice::Urgent(_) if !self.watch => format!(
                "Could not renew receiver {host} credentials; {days} days remain. Check the receiver is reachable, then run semon push --receiver {} again.",
                self.options.url
            ),
            Notice::Urgent(_) => format!(
                "Automatic renewal has not completed; {days} days remain. Check receiver {host} is reachable; renewal will retry."
            ),
            Notice::Renewing => format!(
                "Receiver {host} credentials are renewing automatically; {days} days remain."
            ),
            Notice::Due => format!("Receiver {host} renewal is due soon; {days} days remain."),
            Notice::Renewed => format!("Receiver {host} credentials renewed."),
            Notice::Failure(_, _) => unreachable!(),
        };
        eprintln!("{message}");
        #[cfg(test)]
        self.notices.push(message);
        Ok(())
    }
    pub(crate) fn refused(&self) -> Result<String> {
        let renewal = Renewal::open(&self.base, &self.options.url)?;
        self.recovery("refused access", &renewal.configuration().machine)
    }
    pub(crate) fn recovery(&self, reason: &str, machine: &str) -> Result<String> {
        let host = origin_custody::receiver_host(&self.options.url)?;
        Ok(format!(
            "Receiver {host} {reason}.\nTo restore access, pair again: on the receiver, run semon receive enroll --machine {machine} with its listener/TLS options and confirm replacement if prompted; on the origin, run semon connect --to {}, enter the code, and approve replacement.",
            self.options.url
        ))
    }
    /// The response arrived on the enrolled HTTPS connection with redirects
    /// disabled. Authentication refusals and redirects cannot supply a hint.
    pub(crate) fn response(&mut self, generation: u64, status: u16, hint: bool) -> Result<()> {
        self.schedule.hint(status, hint);
        if (200..300).contains(&status) {
            let mut renewal = Renewal::open(&self.base, &self.options.url)?;
            if renewal.awaiting_confirmation() && renewal.configuration().generation == generation {
                renewal.confirm(generation)?;
                self.schedule.confirmed();
                self.emit(Notice::Renewed, 0, 0)?;
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
    const NOW: u64 = 1_800_000_000;
    const ENDPOINT: &str = "https://receiver.example:443";
    struct Fixture(PathBuf);
    impl Fixture {
        fn new(until: u64) -> Self {
            use crate::{
                credentials::Kind, enrollment::Binding, pairing::ActivationInput,
                provision_wire::Activated,
            };
            use ring::signature::EcdsaKeyPair;
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let f = Self(std::env::temp_dir().join(format!(
                "semon-origin-delivery-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )));
            let private = EcdsaKeyPair::generate_pkcs8(
                &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                &ring::rand::SystemRandom::new(),
            )
            .unwrap();
            origin_custody::install_at(
                &f.0,
                ENDPOINT,
                "laptop",
                &ActivationInput {
                    receiver_id: "a".repeat(64),
                    machine: "laptop".into(),
                    binding: Binding::Inapplicable,
                    kinds: vec![(Kind::MirrorPush, false)],
                    client_certificate_pins: vec![],
                    valid_until: until,
                },
                &Activated {
                    version: 1,
                    enrollment_id: "b".repeat(64),
                    credentials: vec![(Kind::MirrorPush, "old-bearer".into())],
                    client_certificate: None,
                },
                private.as_ref(),
                &[],
                NOW,
            )
            .unwrap();
            f
        }
        fn delivery(&self) -> Delivery {
            let (_, credential) = origin_custody::load_at(&self.0, Some(ENDPOINT), NOW).unwrap();
            Delivery::new(&PushOptions {
                url: ENDPOINT.into(),
                credential,
                sessions: Default::default(),
                state: self.0.join("push-state"),
            })
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn failed_renewal_keeps_valid_application_authority_and_exact_notice() {
        let f = Fixture::new(NOW + WINDOW - 1);
        let mut d = f.delivery();
        d.renew = Some(|_, _| Err("offline".into()));
        d.checkpoint_at(NOW).unwrap();
        assert_eq!(d.options.credential.token().unwrap().expose(), "old-bearer");
        let Credential::Enrolled { authority, .. } = &d.options.credential else {
            panic!("enrolled");
        };
        assert!(authority.delivery().is_ok());
        assert_eq!(
            d.notices,
            [
                "Could not renew receiver receiver.example:443 credentials; 30 days remain. Check the receiver is reachable, then run semon push --receiver https://receiver.example:443 again."
            ]
        );
        d.checkpoint_at(NOW + 1).unwrap();
        assert_eq!(d.notices.len(), 1);
        d.watch(true);
        d.checkpoint_at(NOW + MINUTE).unwrap();
        assert_eq!(d.options.credential.token().unwrap().expose(), "old-bearer");
        d.schedule.confirmed();
        d.checkpoint_at(NOW + MINUTE * 2).unwrap();
        assert_eq!(
            d.notices.last().unwrap(),
            "Could not renew receiver receiver.example:443 credentials; 30 days remain. Check the receiver is reachable; renewal will retry."
        );
    }
    #[test]
    fn idle_watch_crosses_window_installs_and_confirms_only_matching_success() {
        let f = Fixture::new(NOW + WINDOW);
        let mut d = f.delivery();
        d.watch(true);
        d.renew = Some(Renewal::install_test_generation);
        let Credential::Enrolled { authority, .. } = &d.options.credential else {
            panic!("enrolled");
        };
        let stale = crate::Stop::new().with_authority(authority.clone());
        let following = crate::Stop::new()
            .with_authority(authority.clone())
            .following_selection();
        d.checkpoint_at(NOW).unwrap();
        assert_eq!(
            d.notices,
            ["Receiver receiver.example:443 renewal is due soon; 30 days remain."]
        );
        d.checkpoint_at(NOW + MINUTE).unwrap();
        assert_eq!(
            origin_custody::load_at(&f.0, None, NOW + MINUTE)
                .unwrap()
                .0
                .generation,
            2
        );
        assert!(stale.is_stopped());
        assert!(!following.is_stopped());
        d.response(1, 200, false).unwrap();
        d.response(2, 401, true).unwrap();
        assert!(
            Renewal::open(&f.0, ENDPOINT)
                .unwrap()
                .awaiting_confirmation()
        );
        d.response(2, 200, false).unwrap();
        assert!(
            !Renewal::open(&f.0, ENDPOINT)
                .unwrap()
                .awaiting_confirmation()
        );
        assert_eq!(
            d.notices.last().unwrap(),
            "Receiver receiver.example:443 credentials renewed."
        );
        let count = d.notices.len();
        d.response(2, 200, false).unwrap();
        assert_eq!(d.notices.len(), count);
    }
    #[test]
    fn authenticated_hint_renews_outside_local_window_and_forged_hint_does_not() {
        let f = Fixture::new(NOW + WINDOW * 2);
        let mut d = f.delivery();
        d.renew = Some(Renewal::install_test_generation);
        d.response(1, 401, true).unwrap();
        d.checkpoint_at(NOW).unwrap();
        assert_eq!(
            origin_custody::load_at(&f.0, None, NOW)
                .unwrap()
                .0
                .generation,
            1
        );
        d.response(1, 200, true).unwrap();
        d.checkpoint_at(NOW).unwrap();
        assert_eq!(
            origin_custody::load_at(&f.0, None, NOW)
                .unwrap()
                .0
                .generation,
            2
        );
    }
    #[test]
    fn disconnect_during_renewal_prevents_later_delivery() {
        let f = Fixture::new(NOW + WINDOW - 1);
        let mut d = f.delivery();
        let base = f.0.clone();
        let (entered, waiting) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        // Hold the same install lock that the checkpoint holds through renewal.
        let worker_base = base.clone();
        std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                let renewal = Renewal::open(&worker_base, ENDPOINT).unwrap();
                entered.send(()).unwrap();
                released
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                drop(renewal);
            });
            waiting
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            assert!(origin_custody::Disconnect::open(&base, ENDPOINT).is_err());
            release.send(()).unwrap();
            worker.join().unwrap();
        });
        origin_custody::Disconnect::open(&base, ENDPOINT)
            .unwrap()
            .commit()
            .unwrap();
        d.renew = Some(Renewal::install_test_generation);
        assert!(d.checkpoint_at(NOW).is_err());
        assert!(origin_custody::load_at(&base, None, NOW).is_err());
    }
    #[test]
    fn boundary_hint_and_backoff() {
        let mut s = Schedule::default();
        assert!(!s.attempt(WINDOW + 100, 100, false));
        assert!(s.attempt(WINDOW + 100, 101, false));
        s.hint(200, true);
        assert!(!s.attempt(WINDOW + 100, 160, false));
        assert!(s.attempt(WINDOW + 100, 161, false));
        assert!(!s.attempt(200, 200, true));
    }
    #[test]
    fn hints_require_application_authentication() {
        for status in [200, 204, 413, 423, 429] {
            let mut s = Schedule::default();
            s.hint(status, true);
            assert!(s.attempt(WINDOW * 3, 100, false));
        }
        for status in [301, 401, 403, 404, 500] {
            let mut s = Schedule::default();
            s.hint(status, true);
            assert!(!s.attempt(WINDOW * 3, 100, false));
        }
    }
    #[test]
    fn unchanged_failure_and_attempt_starts_stay_quiet() {
        let mut s = Schedule::default();
        let until = WINDOW + 100;
        assert_eq!(
            s.outcome(until, 100, Some(Err("temporary".into()))),
            Some(Notice::Failure("temporary".into(), 30))
        );
        assert_eq!(s.outcome(until, 160, Some(Ok(()))), None);
        assert_eq!(s.outcome(until, 161, Some(Err("temporary".into()))), None);
        assert_eq!(s.outcome(until, 162, None), None);
        assert!(
            s.outcome(until, 100 + DAY, Some(Err("temporary".into())))
                .is_some()
        );
    }
    #[test]
    fn severity_crossings_and_daily_reminders_are_independent_of_attempts() {
        let mut s = Schedule::default();
        assert!(
            s.outcome(30 * DAY, 1, Some(Err("offline".into())))
                .is_some()
        );
        assert_eq!(
            s.outcome(30 * DAY, 16 * DAY, None),
            Some(Notice::Urgent(14))
        );
        assert_eq!(
            s.outcome(30 * DAY, 16 * DAY + MINUTE, Some(Err("offline".into()))),
            None
        );
        assert_eq!(s.outcome(30 * DAY, 23 * DAY, None), Some(Notice::Urgent(7)));
        assert_eq!(s.outcome(30 * DAY, 24 * DAY, None), Some(Notice::Urgent(7)));
    }
}
