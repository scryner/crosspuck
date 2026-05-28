#[cfg(not(test))]
use objc2_foundation::NSString;
#[cfg(not(test))]
use std::ffi::c_void;
use std::time::{Duration, Instant};

#[cfg(not(test))]
const ASSERTION_NAME: &str = "CrossPuck Controller Input";
#[cfg(not(test))]
const K_IOPM_USER_ACTIVE_LOCAL: IOPMUserActiveType = 0;
#[cfg(not(test))]
const K_IORETURN_SUCCESS: IOReturn = 0;

type IOReturn = i32;
type IOPMAssertionId = u32;
#[cfg(not(test))]
type IOPMUserActiveType = u32;

pub(crate) struct UserActivityReporter {
    core: ReporterCore<DefaultPowerManagement>,
}

impl UserActivityReporter {
    pub(crate) fn new(min_interval: Duration) -> Self {
        Self {
            core: ReporterCore::new(min_interval, DefaultPowerManagement::new()),
        }
    }

    pub(crate) fn note_controller_input(&mut self) {
        self.core.note_controller_input(Instant::now());
    }
}

struct ReporterCore<P: PowerManagement> {
    power_management: P,
    assertion_id: Option<IOPMAssertionId>,
    last_attempt_at: Option<Instant>,
    min_interval: Duration,
    logged_first_success: bool,
}

impl<P: PowerManagement> ReporterCore<P> {
    fn new(min_interval: Duration, power_management: P) -> Self {
        Self {
            power_management,
            assertion_id: None,
            last_attempt_at: None,
            min_interval,
            logged_first_success: false,
        }
    }

    fn note_controller_input(&mut self, now: Instant) {
        if !self.should_declare(now) {
            return;
        }
        self.last_attempt_at = Some(now);

        match self
            .power_management
            .declare_user_activity(self.assertion_id)
        {
            Ok(assertion_id) => {
                self.assertion_id = assertion_id;
                if !self.logged_first_success {
                    log::debug!("CrossPuck declared macOS user activity for controller input");
                    self.logged_first_success = true;
                }
            }
            Err(error) => {
                log::warn!(
                    "CrossPuck failed to declare macOS user activity: IOReturn=0x{error:08X}"
                );
            }
        }
    }

    fn should_declare(&self, now: Instant) -> bool {
        self.last_attempt_at
            .map(|last_attempt_at| {
                now.saturating_duration_since(last_attempt_at) >= self.min_interval
            })
            .unwrap_or(true)
    }
}

impl<P: PowerManagement> Drop for ReporterCore<P> {
    fn drop(&mut self) {
        let Some(assertion_id) = self.assertion_id.take() else {
            return;
        };

        if let Err(error) = self.power_management.release_assertion(assertion_id) {
            log::debug!(
                "CrossPuck failed to release macOS user activity assertion: id={} IOReturn=0x{error:08X}",
                assertion_id
            );
        }
    }
}

trait PowerManagement {
    fn declare_user_activity(
        &mut self,
        assertion_id: Option<IOPMAssertionId>,
    ) -> Result<Option<IOPMAssertionId>, IOReturn>;

    fn release_assertion(&mut self, assertion_id: IOPMAssertionId) -> Result<(), IOReturn>;
}

#[cfg(not(test))]
type DefaultPowerManagement = IokitPowerManagement;

#[cfg(test)]
type DefaultPowerManagement = NoopPowerManagement;

#[cfg(not(test))]
struct IokitPowerManagement;

#[cfg(not(test))]
impl IokitPowerManagement {
    fn new() -> Self {
        Self
    }
}

#[cfg(not(test))]
impl PowerManagement for IokitPowerManagement {
    fn declare_user_activity(
        &mut self,
        assertion_id: Option<IOPMAssertionId>,
    ) -> Result<Option<IOPMAssertionId>, IOReturn> {
        let name = NSString::from_str(ASSERTION_NAME);
        let mut next_assertion_id = assertion_id.unwrap_or_default();
        let result = unsafe {
            IOPMAssertionDeclareUserActivity(
                (&*name as *const NSString).cast::<c_void>(),
                K_IOPM_USER_ACTIVE_LOCAL,
                &mut next_assertion_id,
            )
        };

        if result == K_IORETURN_SUCCESS {
            Ok((next_assertion_id != 0).then_some(next_assertion_id))
        } else {
            Err(result)
        }
    }

    fn release_assertion(&mut self, assertion_id: IOPMAssertionId) -> Result<(), IOReturn> {
        let result = unsafe { IOPMAssertionRelease(assertion_id) };
        if result == K_IORETURN_SUCCESS {
            Ok(())
        } else {
            Err(result)
        }
    }
}

#[cfg(test)]
struct NoopPowerManagement;

#[cfg(test)]
impl NoopPowerManagement {
    fn new() -> Self {
        Self
    }
}

#[cfg(test)]
impl PowerManagement for NoopPowerManagement {
    fn declare_user_activity(
        &mut self,
        assertion_id: Option<IOPMAssertionId>,
    ) -> Result<Option<IOPMAssertionId>, IOReturn> {
        Ok(assertion_id.or(Some(1)))
    }

    fn release_assertion(&mut self, _assertion_id: IOPMAssertionId) -> Result<(), IOReturn> {
        Ok(())
    }
}

#[cfg(not(test))]
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionDeclareUserActivity(
        assertion_name: *const c_void,
        user_type: IOPMUserActiveType,
        assertion_id: *mut IOPMAssertionId,
    ) -> IOReturn;

    fn IOPMAssertionRelease(assertion_id: IOPMAssertionId) -> IOReturn;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct FakePowerManagement {
        declares: Vec<Option<IOPMAssertionId>>,
        releases: Arc<Mutex<Vec<IOPMAssertionId>>>,
        results: Vec<Result<Option<IOPMAssertionId>, IOReturn>>,
    }

    impl FakePowerManagement {
        fn with_results(results: Vec<Result<Option<IOPMAssertionId>, IOReturn>>) -> Self {
            Self {
                results,
                ..Self::default()
            }
        }
    }

    impl PowerManagement for FakePowerManagement {
        fn declare_user_activity(
            &mut self,
            assertion_id: Option<IOPMAssertionId>,
        ) -> Result<Option<IOPMAssertionId>, IOReturn> {
            self.declares.push(assertion_id);
            self.results.remove(0)
        }

        fn release_assertion(&mut self, assertion_id: IOPMAssertionId) -> Result<(), IOReturn> {
            self.releases.lock().unwrap().push(assertion_id);
            Ok(())
        }
    }

    #[test]
    fn first_controller_input_declares_immediately() {
        let now = Instant::now();
        let fake = FakePowerManagement::with_results(vec![Ok(Some(42))]);
        let mut reporter = ReporterCore::new(Duration::from_secs(30), fake);

        reporter.note_controller_input(now);

        assert_eq!(reporter.power_management.declares, vec![None]);
        assert_eq!(reporter.assertion_id, Some(42));
    }

    #[test]
    fn input_before_min_interval_is_throttled() {
        let now = Instant::now();
        let fake = FakePowerManagement::with_results(vec![Ok(Some(42))]);
        let mut reporter = ReporterCore::new(Duration::from_secs(30), fake);

        reporter.note_controller_input(now);
        reporter.note_controller_input(now + Duration::from_secs(29));

        assert_eq!(reporter.power_management.declares, vec![None]);
    }

    #[test]
    fn input_after_min_interval_declares_with_previous_assertion_id() {
        let now = Instant::now();
        let fake = FakePowerManagement::with_results(vec![Ok(Some(42)), Ok(Some(43))]);
        let mut reporter = ReporterCore::new(Duration::from_secs(30), fake);

        reporter.note_controller_input(now);
        reporter.note_controller_input(now + Duration::from_secs(30));

        assert_eq!(reporter.power_management.declares, vec![None, Some(42)]);
        assert_eq!(reporter.assertion_id, Some(43));
    }

    #[test]
    fn failed_declare_is_still_throttled() {
        let now = Instant::now();
        let fake = FakePowerManagement::with_results(vec![Err(-1), Ok(Some(42))]);
        let mut reporter = ReporterCore::new(Duration::from_secs(30), fake);

        reporter.note_controller_input(now);
        reporter.note_controller_input(now + Duration::from_secs(1));
        reporter.note_controller_input(now + Duration::from_secs(30));

        assert_eq!(reporter.power_management.declares, vec![None, None]);
        assert_eq!(reporter.assertion_id, Some(42));
    }

    #[test]
    fn drop_releases_active_assertion() {
        let now = Instant::now();
        let fake = FakePowerManagement::with_results(vec![Ok(Some(42))]);
        let releases = Arc::clone(&fake.releases);
        let mut reporter = ReporterCore::new(Duration::from_secs(30), fake);

        reporter.note_controller_input(now);
        drop(reporter);

        assert_eq!(*releases.lock().unwrap(), vec![42]);
    }
}
