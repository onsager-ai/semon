//! Test support for the retired renewal cycle, retained with origin renewal tests.
use crate::{
    Result,
    origin_custody::{self, Authority, renewal::Renewal},
};

const MINUTE: u64 = 60;
const DAY: u64 = 86_400;

#[derive(Default)]
struct Schedule {
    attempted: Option<u64>,
    notice: Option<(u64, bool)>,
}
impl Schedule {
    fn due(&mut self, now: u64, valid_until: u64) -> bool {
        if !origin_custody::renewal::window(valid_until, now).1
            || self
                .attempted
                .is_some_and(|last| now.saturating_sub(last) < MINUTE)
        {
            return false;
        }
        self.attempted = Some(now);
        true
    }
    fn failure(
        &mut self,
        now: u64,
        valid_until: u64,
        watch: bool,
        host: &str,
        endpoint: &str,
    ) -> Option<String> {
        let remaining = valid_until.saturating_sub(now);
        let severe = remaining <= 7 * DAY;
        if watch
            && self.notice.is_some_and(|(last, previous)| {
                !(severe && !previous) && now.saturating_sub(last) < DAY
            })
        {
            return None;
        }
        self.notice = Some((now, severe));
        let days = remaining.div_ceil(DAY);
        Some(if watch {
            format!(
                "Could not renew receiver {host} credentials; {days} days remain. Check the receiver is reachable; renewal will retry."
            )
        } else {
            format!(
                "Could not renew receiver {host} credentials; {days} days remain. Check the receiver is reachable, then run semon push --receiver {endpoint} again."
            )
        })
    }
}

#[cfg(test)]
type Configure = Box<dyn FnMut(&mut Renewal) + Send>;

pub(crate) struct Cycle {
    authority: Authority,
    endpoint: String,
    host: String,
    watch: bool,
    schedule: Schedule,
    installed: Option<Renewal>,
    #[cfg(test)]
    pub(crate) configure: Option<Configure>,
}
impl Cycle {
    pub(crate) fn new(authority: Authority, endpoint: String, watch: bool) -> Self {
        let host = origin_custody::receiver_host(&endpoint).expect("validated receiver endpoint");
        Self {
            authority,
            endpoint,
            host,
            watch,
            schedule: Schedule::default(),
            installed: None,
            #[cfg(test)]
            configure: None,
        }
    }
    fn confirm(&mut self, emit: &mut impl FnMut(String)) {
        let Some(renewal) = self.installed.as_mut() else {
            return;
        };
        if !self.authority.is_active() {
            self.installed = None;
            return;
        }
        let generation = renewal.configuration().generation;
        if self.authority.confirmed_generation() == generation
            && renewal.confirm(generation).is_ok()
        {
            emit(format!("Receiver {} credentials renewed.", self.host));
            self.schedule.notice = None;
            self.installed = None;
        }
    }
    pub(crate) fn tick(
        &mut self,
        base: &std::path::Path,
        clock: &dyn Fn() -> Result<u64>,
        emit: &mut impl FnMut(String),
    ) {
        self.confirm(emit);
        if self.installed.is_some() || !self.authority.is_active() {
            return;
        }
        let Ok(now) = clock() else {
            return;
        };
        let Ok((config, _)) = origin_custody::load_at(base, Some(&self.endpoint), now) else {
            return;
        };
        let opened = Renewal::open(base, &self.endpoint);
        if let Ok(renewal) = opened.as_ref()
            && renewal.awaiting_confirmation()
        {
            self.installed = opened.ok();
            self.confirm(emit);
            return;
        }
        if !self.schedule.due(now, config.scope.valid_until) {
            return;
        }
        let result = opened.and_then(|mut renewal| {
            #[cfg(test)]
            if let Some(configure) = self.configure.as_mut() {
                configure(&mut renewal);
            }
            renewal.renew_with_clock(clock)?;
            Ok(renewal)
        });
        match result {
            Ok(renewal) => {
                self.schedule.notice = None;
                self.installed = Some(renewal);
                self.confirm(emit);
            }
            Err(_) => {
                if self.authority.is_active()
                    && let Some(line) = self.schedule.failure(
                        now,
                        config.scope.valid_until,
                        self.watch,
                        &self.host,
                        &self.endpoint,
                    )
                {
                    emit(line);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_shot_notice_uses_its_exact_retry_guidance() {
        let mut schedule = Schedule::default();
        assert_eq!(
            schedule
                .failure(0, DAY, false, "host:443", "https://host:443")
                .unwrap(),
            "Could not renew receiver host:443 credentials; 1 days remain. Check the receiver is reachable, then run semon push --receiver https://host:443 again."
        );
    }
    #[test]
    fn timer_boundary_retry_and_notice_cadence() {
        let mut schedule = Schedule::default();
        let expiry = 40 * DAY;
        assert!(!schedule.due(10 * DAY, expiry));
        assert!(schedule.due(10 * DAY + 1, expiry));
        assert!(!schedule.due(10 * DAY + 60, expiry));
        assert!(schedule.due(10 * DAY + 61, expiry));
        assert!(
            schedule
                .failure(32 * DAY, expiry, true, "host", "url")
                .is_some()
        );
        assert!(
            schedule
                .failure(32 * DAY + MINUTE, expiry, true, "host", "url")
                .is_none()
        );
        assert!(
            schedule
                .failure(33 * DAY, expiry, true, "host", "url")
                .is_some()
        );
        assert!(
            schedule
                .failure(33 * DAY + MINUTE, expiry, true, "host", "url")
                .is_none()
        );
        assert!(
            schedule
                .failure(34 * DAY, expiry, true, "host", "url")
                .is_some()
        );
    }
}
