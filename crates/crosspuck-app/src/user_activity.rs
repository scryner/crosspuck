#[cfg(not(test))]
use objc2_foundation::NSString;
#[cfg(not(test))]
use std::ffi::c_void;
use std::time::{Duration, Instant};

#[cfg(not(test))]
const ASSERTION_NAME: &str = "CrossPuck Controller Input";
#[cfg(not(test))]
const DISPLAY_SLEEP_ASSERTION_TYPE: &str = "PreventUserIdleDisplaySleep";
#[cfg(not(test))]
const K_IOPM_ASSERTION_LEVEL_ON: IOPMAssertionLevel = 255;
#[cfg(not(test))]
const K_IOPM_USER_ACTIVE_LOCAL: IOPMUserActiveType = 0;
#[cfg(not(test))]
const K_IORETURN_SUCCESS: IOReturn = 0;

type IOReturn = i32;
type IOPMAssertionId = u32;
#[cfg(not(test))]
type IOPMAssertionLevel = u32;
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
    display_assertion_id: Option<IOPMAssertionId>,
    last_attempt_at: Option<Instant>,
    min_interval: Duration,
    logged_first_success: bool,
    logged_display_success: bool,
}

impl<P: PowerManagement> ReporterCore<P> {
    fn new(min_interval: Duration, power_management: P) -> Self {
        Self {
            power_management,
            assertion_id: None,
            display_assertion_id: None,
            last_attempt_at: None,
            min_interval,
            logged_first_success: false,
            logged_display_success: false,
        }
    }

    fn note_controller_input(&mut self, now: Instant) {
        if !self.should_declare(now) {
            return;
        }
        self.last_attempt_at = Some(now);

        if self.display_assertion_id.is_none() {
            match self.power_management.prevent_user_idle_display_sleep() {
                Ok(assertion_id) => {
                    self.display_assertion_id = Some(assertion_id);
                    if !self.logged_display_success {
                        log::info!(
                            "CrossPuck is preventing macOS display idle sleep during controller session"
                        );
                        self.logged_display_success = true;
                    }
                }
                Err(error) => {
                    log::warn!(
                        "CrossPuck failed to prevent macOS display idle sleep: IOReturn=0x{error:08X}"
                    );
                }
            }
        }

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
        if let Some(assertion_id) = self.display_assertion_id.take() {
            if let Err(error) = self.power_management.release_assertion(assertion_id) {
                log::debug!(
                    "CrossPuck failed to release macOS display idle sleep assertion: id={} IOReturn=0x{error:08X}",
                    assertion_id
                );
            }
        }

        if let Some(assertion_id) = self.assertion_id.take() {
            if let Err(error) = self.power_management.release_assertion(assertion_id) {
                log::debug!(
                    "CrossPuck failed to release macOS user activity assertion: id={} IOReturn=0x{error:08X}",
                    assertion_id
                );
            }
        }
    }
}

trait PowerManagement {
    fn prevent_user_idle_display_sleep(&mut self) -> Result<IOPMAssertionId, IOReturn>;

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
    fn prevent_user_idle_display_sleep(&mut self) -> Result<IOPMAssertionId, IOReturn> {
        let assertion_type = NSString::from_str(DISPLAY_SLEEP_ASSERTION_TYPE);
        let name = NSString::from_str(ASSERTION_NAME);
        let mut assertion_id = 0;
        let result = unsafe {
            IOPMAssertionCreateWithName(
                (&*assertion_type as *const NSString).cast::<c_void>(),
                K_IOPM_ASSERTION_LEVEL_ON,
                (&*name as *const NSString).cast::<c_void>(),
                &mut assertion_id,
            )
        };

        if result == K_IORETURN_SUCCESS && assertion_id != 0 {
            Ok(assertion_id)
        } else {
            Err(result)
        }
    }

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
    fn prevent_user_idle_display_sleep(&mut self) -> Result<IOPMAssertionId, IOReturn> {
        Ok(100)
    }

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
    fn IOPMAssertionCreateWithName(
        assertion_type: *const c_void,
        assertion_level: IOPMAssertionLevel,
        assertion_name: *const c_void,
        assertion_id: *mut IOPMAssertionId,
    ) -> IOReturn;

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
        display_prevents: u32,
        declares: Vec<Option<IOPMAssertionId>>,
        releases: Arc<Mutex<Vec<IOPMAssertionId>>>,
        results: Vec<Result<Option<IOPMAssertionId>, IOReturn>>,
        display_results: Vec<Result<IOPMAssertionId, IOReturn>>,
    }

    impl FakePowerManagement {
        fn with_results(results: Vec<Result<Option<IOPMAssertionId>, IOReturn>>) -> Self {
            Self {
                results,
                display_results: vec![Ok(100)],
                ..Self::default()
            }
        }
    }

    impl PowerManagement for FakePowerManagement {
        fn prevent_user_idle_display_sleep(&mut self) -> Result<IOPMAssertionId, IOReturn> {
            self.display_prevents += 1;
            self.display_results.remove(0)
        }

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
        assert_eq!(reporter.power_management.display_prevents, 1);
        assert_eq!(reporter.assertion_id, Some(42));
        assert_eq!(reporter.display_assertion_id, Some(100));
    }

    #[test]
    fn input_before_min_interval_is_throttled() {
        let now = Instant::now();
        let fake = FakePowerManagement::with_results(vec![Ok(Some(42))]);
        let mut reporter = ReporterCore::new(Duration::from_secs(30), fake);

        reporter.note_controller_input(now);
        reporter.note_controller_input(now + Duration::from_secs(29));

        assert_eq!(reporter.power_management.declares, vec![None]);
        assert_eq!(reporter.power_management.display_prevents, 1);
    }

    #[test]
    fn input_after_min_interval_declares_with_previous_assertion_id() {
        let now = Instant::now();
        let fake = FakePowerManagement::with_results(vec![Ok(Some(42)), Ok(Some(43))]);
        let mut reporter = ReporterCore::new(Duration::from_secs(30), fake);

        reporter.note_controller_input(now);
        reporter.note_controller_input(now + Duration::from_secs(30));

        assert_eq!(reporter.power_management.declares, vec![None, Some(42)]);
        assert_eq!(reporter.power_management.display_prevents, 1);
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
        assert_eq!(reporter.power_management.display_prevents, 1);
        assert_eq!(reporter.assertion_id, Some(42));
    }

    #[test]
    fn failed_display_sleep_prevention_is_throttled_and_retried() {
        let now = Instant::now();
        let fake = FakePowerManagement {
            results: vec![Ok(Some(42)), Ok(Some(43))],
            display_results: vec![Err(-1), Ok(100)],
            ..FakePowerManagement::default()
        };
        let mut reporter = ReporterCore::new(Duration::from_secs(30), fake);

        reporter.note_controller_input(now);
        reporter.note_controller_input(now + Duration::from_secs(1));
        reporter.note_controller_input(now + Duration::from_secs(30));

        assert_eq!(reporter.power_management.declares, vec![None, Some(42)]);
        assert_eq!(reporter.power_management.display_prevents, 2);
        assert_eq!(reporter.display_assertion_id, Some(100));
    }

    #[test]
    fn drop_releases_active_assertion() {
        let now = Instant::now();
        let fake = FakePowerManagement::with_results(vec![Ok(Some(42))]);
        let releases = Arc::clone(&fake.releases);
        let mut reporter = ReporterCore::new(Duration::from_secs(30), fake);

        reporter.note_controller_input(now);
        drop(reporter);

        assert_eq!(*releases.lock().unwrap(), vec![100, 42]);
    }
}
