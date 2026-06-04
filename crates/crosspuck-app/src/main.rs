use std::process::ExitCode;

#[cfg(target_os = "macos")]
mod bundle;
#[cfg(target_os = "macos")]
mod driver_install;
#[cfg(target_os = "macos")]
mod hid_backend;
#[cfg(target_os = "macos")]
mod logging;
#[cfg(all(target_os = "macos", feature = "profiling"))]
mod probe;
#[cfg(all(target_os = "macos", not(feature = "profiling")))]
mod probe {
    pub(crate) fn start_from_env() {}
    pub(crate) fn note_ui_timer_tick() {}
    pub(crate) fn note_menu_will_open() {}
    pub(crate) fn note_menu_refresh() {}
    pub(crate) fn note_driver_status_check() {}
    pub(crate) fn note_control_frame() {}
    pub(crate) fn note_input_report() {}
    pub(crate) fn note_hid_open_path_attempt() {}
    pub(crate) fn note_hid_interface_reopen_attempt() {}
    pub(crate) fn note_hid_interface_reopen_ok() {}
    pub(crate) fn note_hid_error_reopen_attempt() {}
    pub(crate) fn note_hid_error_reopen_ok() {}
    pub(crate) fn note_hid_main_refresh_attempt() {}
    pub(crate) fn note_hid_main_refresh_ok() {}

    pub(crate) fn with_callback_autorelease_pool<T>(body: impl FnOnce() -> T) -> T {
        body()
    }
}
#[cfg(target_os = "macos")]
mod runtime;
#[cfg(target_os = "macos")]
mod settings;
#[cfg(target_os = "macos")]
mod user_activity;

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    macos::run()
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    log::error!("crosspuck-app is only supported on macOS.");
    ExitCode::from(1)
}

#[cfg(target_os = "macos")]
mod macos {
    use crate::driver_install::{
        check_driver_install_status, install_driver, uninstall_driver, DriverInstallContext,
        DriverInstallState, DriverInstallStatus,
    };
    use crate::runtime::{
        start_host_service_with_config, AppState, HostServiceConfig, HostServiceHandle,
    };
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{
        define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
    };
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSCellImagePosition, NSImage, NSImageScaling,
        NSMenu, NSMenuDelegate, NSMenuItem, NSModalResponseOK, NSOpenPanel,
        NSSquareStatusItemLength, NSStatusBar, NSStatusItem,
    };
    use objc2_foundation::{
        NSAutoreleasePool, NSBundle, NSObject, NSObjectProtocol, NSString, NSTimer, NSURL,
    };
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;
    use std::sync::{Arc, Mutex};
    use std::thread;

    struct MenuBarObjects {
        _status_item: Retained<NSStatusItem>,
        _icon: Option<Retained<NSImage>>,
        _menu: Retained<NSMenu>,
        _state_items: StateMenuItems,
        _menu_delegate: Retained<StateMenuDelegate>,
        _refresh_timer: Retained<NSTimer>,
        _driver_controller: Retained<DriverInstallController>,
        _bottle_controller: Retained<BottleController>,
        _quit_controller: Retained<QuitController>,
    }

    #[derive(Clone)]
    struct StateMenuItems {
        status: Retained<NSMenuItem>,
        puck: Retained<NSMenuItem>,
        guest: Retained<NSMenuItem>,
        error: Retained<NSMenuItem>,
        driver_status: Retained<NSMenuItem>,
        driver_action: Retained<NSMenuItem>,
        driver_uninstall: Retained<NSMenuItem>,
        bottle_info: Retained<NSMenuItem>,
        bottle_choose: Retained<NSMenuItem>,
        bottle_reset: Retained<NSMenuItem>,
    }

    #[derive(Clone)]
    struct StateMenuDelegateIvars {
        app_state: AppState,
        driver_state: Arc<DriverMenuState>,
        items: StateMenuItems,
    }

    define_class!(
        #[unsafe(super = NSObject)]
        #[thread_kind = objc2::MainThreadOnly]
        #[ivars = StateMenuDelegateIvars]
        struct StateMenuDelegate;

        unsafe impl NSObjectProtocol for StateMenuDelegate {}

        unsafe impl NSMenuDelegate for StateMenuDelegate {
            #[unsafe(method(menuWillOpen:))]
            fn menu_will_open(&self, _menu: &NSMenu) {
                crate::probe::note_menu_will_open();
                let refresh = || {
                    self.ivars().driver_state.refresh_status();
                    refresh_state_items(
                        &self.ivars().app_state,
                        &self.ivars().driver_state,
                        &self.ivars().items,
                    );
                };
                crate::probe::with_callback_autorelease_pool(refresh);
            }
        }

        impl StateMenuDelegate {
            #[unsafe(method(refreshTimer:))]
            fn refresh_timer(&self, _timer: &NSTimer) {
                crate::probe::note_ui_timer_tick();
                let refresh = || {
                    refresh_state_items(
                        &self.ivars().app_state,
                        &self.ivars().driver_state,
                        &self.ivars().items,
                    );
                };
                crate::probe::with_callback_autorelease_pool(refresh);
            }
        }
    );

    impl StateMenuDelegate {
        fn new(
            mtm: MainThreadMarker,
            app_state: AppState,
            driver_state: Arc<DriverMenuState>,
            items: StateMenuItems,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(StateMenuDelegateIvars {
                app_state,
                driver_state,
                items,
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    struct DriverInstallControllerIvars {
        driver_state: Arc<DriverMenuState>,
        items: StateMenuItems,
    }

    define_class!(
        #[unsafe(super = NSObject)]
        #[thread_kind = objc2::MainThreadOnly]
        #[ivars = DriverInstallControllerIvars]
        struct DriverInstallController;

        unsafe impl NSObjectProtocol for DriverInstallController {}

        impl DriverInstallController {
            #[unsafe(method(validateMenuItem:))]
            fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
                refresh_driver_items(&self.ivars().driver_state, &self.ivars().items);
                let view = self.ivars().driver_state.menu_view();
                match item.action() {
                    Some(action) if action == sel!(installDriver:) => view.install_enabled,
                    Some(action) if action == sel!(uninstallDriver:) => view.uninstall_enabled,
                    _ => true,
                }
            }

            #[unsafe(method(installDriver:))]
            fn install_driver(&self, _sender: Option<&AnyObject>) {
                if self.ivars().driver_state.start_install() {
                    refresh_driver_items(&self.ivars().driver_state, &self.ivars().items);
                }
            }

            #[unsafe(method(uninstallDriver:))]
            fn uninstall_driver(&self, _sender: Option<&AnyObject>) {
                if self.ivars().driver_state.start_uninstall() {
                    refresh_driver_items(&self.ivars().driver_state, &self.ivars().items);
                }
            }
        }
    );

    impl DriverInstallController {
        fn new(
            mtm: MainThreadMarker,
            driver_state: Arc<DriverMenuState>,
            items: StateMenuItems,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(DriverInstallControllerIvars {
                driver_state,
                items,
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    struct BottleControllerIvars {
        driver_state: Arc<DriverMenuState>,
        items: StateMenuItems,
    }

    define_class!(
        #[unsafe(super = NSObject)]
        #[thread_kind = objc2::MainThreadOnly]
        #[ivars = BottleControllerIvars]
        struct BottleController;

        unsafe impl NSObjectProtocol for BottleController {}

        impl BottleController {
            #[unsafe(method(chooseBottle:))]
            fn choose_bottle(&self, _sender: Option<&AnyObject>) {
                let mtm = self.mtm();
                let panel = NSOpenPanel::openPanel(mtm);
                panel.setCanChooseDirectories(true);
                panel.setCanChooseFiles(false);
                panel.setAllowsMultipleSelection(false);
                panel.setMessage(Some(&NSString::from_str(
                    "Select the CrossOver bottle CrossPuck should use",
                )));

                // Start next to the current bottle when one is set, otherwise
                // at the bottles root that auto-detection scans.
                let context = self.ivars().driver_state.current_context();
                let start_dir = context
                    .bottle_path
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                    .or(context.crossover_bottles_dir)
                    .filter(|path| path.is_dir());
                if let Some(dir) = start_dir {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(&dir.to_string_lossy()));
                    panel.setDirectoryURL(Some(&url));
                }

                // An accessory (menu bar) app is not frontmost, so bring it
                // forward before running the modal or the picker hides behind
                // other windows.
                activate_app(mtm);
                if panel.runModal() != NSModalResponseOK {
                    return;
                }
                let Some(path) = panel.URL().and_then(|url| url.path()) else {
                    return;
                };
                let path = PathBuf::from(path.to_string());
                crate::settings::set_stored_bottle_path(&path);
                self.apply_bottle_path_change();
            }

            #[unsafe(method(resetBottle:))]
            fn reset_bottle(&self, _sender: Option<&AnyObject>) {
                crate::settings::clear_stored_bottle_path();
                self.apply_bottle_path_change();
            }
        }
    );

    impl BottleController {
        fn new(
            mtm: MainThreadMarker,
            driver_state: Arc<DriverMenuState>,
            items: StateMenuItems,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(BottleControllerIvars {
                driver_state,
                items,
            });
            unsafe { msg_send![super(this), init] }
        }

        fn apply_bottle_path_change(&self) {
            let driver_state = &self.ivars().driver_state;
            driver_state.set_bottle_path(crate::settings::resolve_bottle_path());
            driver_state.refresh_status();
            refresh_driver_items(driver_state, &self.ivars().items);
            refresh_bottle_items(driver_state, &self.ivars().items);
        }
    }

    struct QuitControllerIvars {
        app: Retained<NSApplication>,
        service_handle: HostServiceHandle,
    }

    define_class!(
        #[unsafe(super = NSObject)]
        #[thread_kind = objc2::MainThreadOnly]
        #[ivars = QuitControllerIvars]
        struct QuitController;

        unsafe impl NSObjectProtocol for QuitController {}

        impl QuitController {
            #[unsafe(method(quit:))]
            fn quit(&self, sender: Option<&AnyObject>) {
                self.ivars().service_handle.shutdown();
                self.ivars().app.terminate(sender);
            }
        }
    );

    impl QuitController {
        fn new(
            mtm: MainThreadMarker,
            app: Retained<NSApplication>,
            service_handle: HostServiceHandle,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(QuitControllerIvars {
                app,
                service_handle,
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    pub fn run() -> ExitCode {
        let logging_config = crate::logging::startup_config();
        let logging_initialized = crate::logging::init(&logging_config).is_ok();
        if logging_initialized {
            crate::logging::log_startup(&logging_config);
        }
        crate::probe::start_from_env();

        let mtm = MainThreadMarker::new().expect("crosspuck-app must run on the main thread");

        unsafe {
            let _pool = NSAutoreleasePool::new();
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

            let app_state = AppState::new();
            let service_handle = start_host_service_with_config(
                app_state.clone(),
                HostServiceConfig {
                    guest_runtime_overrides: logging_config.guest_runtime_overrides(),
                },
            );
            log::info!("CrossPuck host app started");
            let menu_objects = build_menu_bar(app.clone(), mtm, &app_state, service_handle);
            Box::leak(Box::new(menu_objects));

            app.run();
        }

        log::info!("CrossPuck host app stopped");
        ExitCode::SUCCESS
    }

    unsafe fn build_menu_bar(
        app: Retained<NSApplication>,
        mtm: MainThreadMarker,
        app_state: &AppState,
        service_handle: HostServiceHandle,
    ) -> MenuBarObjects {
        let status_bar = NSStatusBar::systemStatusBar();
        let status_item = status_bar.statusItemWithLength(NSSquareStatusItemLength);
        let status_icon = load_status_icon();

        if let Some(button) = status_item.button(mtm) {
            if let Some(icon) = status_icon.as_ref() {
                button.setTitle(&NSString::from_str(""));
                button.setImage(Some(icon.as_ref()));
                button.setImagePosition(NSCellImagePosition::ImageOnly);
                button.setImageScaling(NSImageScaling::ScaleProportionallyDown);
            } else {
                button.setTitle(&NSString::from_str("CP"));
            }
        }

        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let mut driver_context = DriverInstallContext::from_environment(bundle_resources_dir());
        driver_context.crossover_bottles_dir = crate::settings::resolve_bottles_dir();
        driver_context.bottle_path = crate::settings::resolve_bottle_path();
        let driver_state = Arc::new(DriverMenuState::new(driver_context));
        driver_state.refresh_status();

        let empty_key = NSString::from_str("");
        let bottle_menu = NSMenu::new(mtm);
        bottle_menu.setAutoenablesItems(false);
        let state_items = StateMenuItems {
            status: menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Status: Starting"),
                None,
                &empty_key,
            ),
            puck: menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Puck: -"),
                None,
                &empty_key,
            ),
            guest: menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Guest: -"),
                None,
                &empty_key,
            ),
            error: menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Last error: -"),
                None,
                &empty_key,
            ),
            driver_status: {
                let separator = NSMenuItem::separatorItem(mtm);
                menu.addItem(&separator);
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str("Driver: Checking..."),
                    None,
                    &empty_key,
                )
            },
            driver_action: menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Install Steam Driver..."),
                Some(sel!(installDriver:)),
                &empty_key,
            ),
            driver_uninstall: menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Uninstall Steam Driver..."),
                Some(sel!(uninstallDriver:)),
                &empty_key,
            ),
            bottle_info: bottle_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Bottle: -"),
                None,
                &empty_key,
            ),
            bottle_choose: bottle_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Choose Bottle..."),
                Some(sel!(chooseBottle:)),
                &empty_key,
            ),
            bottle_reset: bottle_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Reset to Default"),
                Some(sel!(resetBottle:)),
                &empty_key,
            ),
        };
        state_items.status.setEnabled(false);
        state_items.puck.setEnabled(false);
        state_items.guest.setEnabled(false);
        state_items.error.setEnabled(false);
        state_items.driver_status.setEnabled(false);
        state_items.bottle_info.setEnabled(false);

        // Advanced > Bottle Path > (bottle items)
        let separator = NSMenuItem::separatorItem(mtm);
        menu.addItem(&separator);
        let advanced_menu = NSMenu::new(mtm);
        advanced_menu.setAutoenablesItems(false);
        let bottle_path_item = advanced_menu.addItemWithTitle_action_keyEquivalent(
            &NSString::from_str("Bottle Path"),
            None,
            &empty_key,
        );
        bottle_path_item.setSubmenu(Some(&bottle_menu));
        let advanced_item = menu.addItemWithTitle_action_keyEquivalent(
            &NSString::from_str("Advanced"),
            None,
            &empty_key,
        );
        advanced_item.setSubmenu(Some(&advanced_menu));

        let driver_controller =
            DriverInstallController::new(mtm, Arc::clone(&driver_state), state_items.clone());
        state_items
            .driver_action
            .setTarget(Some(driver_controller.as_ref()));
        state_items
            .driver_uninstall
            .setTarget(Some(driver_controller.as_ref()));

        let bottle_controller =
            BottleController::new(mtm, Arc::clone(&driver_state), state_items.clone());
        state_items
            .bottle_choose
            .setTarget(Some(bottle_controller.as_ref()));
        state_items
            .bottle_reset
            .setTarget(Some(bottle_controller.as_ref()));

        let menu_delegate = StateMenuDelegate::new(
            mtm,
            app_state.clone(),
            Arc::clone(&driver_state),
            state_items.clone(),
        );
        menu.setDelegate(Some(ProtocolObject::from_ref(&*menu_delegate)));
        refresh_state_items(app_state, &driver_state, &state_items);
        let refresh_timer =
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.5,
                menu_delegate.as_ref(),
                sel!(refreshTimer:),
                None,
                true,
            );

        let separator = NSMenuItem::separatorItem(mtm);
        menu.addItem(&separator);

        let quit_controller = QuitController::new(mtm, app.clone(), service_handle);
        let quit_title = NSString::from_str("Quit");
        let quit_key = NSString::from_str("q");
        let quit_item =
            menu.addItemWithTitle_action_keyEquivalent(&quit_title, Some(sel!(quit:)), &quit_key);
        quit_item.setTarget(Some(quit_controller.as_ref()));

        status_item.setMenu(Some(&menu));

        MenuBarObjects {
            _status_item: status_item,
            _icon: status_icon,
            _menu: menu,
            _state_items: state_items,
            _menu_delegate: menu_delegate,
            _refresh_timer: refresh_timer,
            _driver_controller: driver_controller,
            _bottle_controller: bottle_controller,
            _quit_controller: quit_controller,
        }
    }

    fn refresh_state_items(
        app_state: &AppState,
        driver_state: &DriverMenuState,
        items: &StateMenuItems,
    ) {
        crate::probe::note_menu_refresh();
        let view = app_state.snapshot().menu_view();
        items
            .status
            .setTitle(&NSString::from_str(&format!("Status: {}", view.status)));
        items
            .puck
            .setTitle(&NSString::from_str(&format!("Puck: {}", view.puck)));
        items
            .guest
            .setTitle(&NSString::from_str(&format!("Guest: {}", view.guest)));
        items
            .error
            .setTitle(&NSString::from_str(&format!("Last error: {}", view.error)));
        refresh_driver_items(driver_state, items);
        refresh_bottle_items(driver_state, items);
    }

    fn refresh_bottle_items(driver_state: &DriverMenuState, items: &StateMenuItems) {
        let env_override = crate::settings::env_override_active();
        // When CROSSPUCK_BOTTLE_PATH is set it wins over any menu selection, so
        // say so and disable the picker rather than letting Choose/Reset look
        // like they do nothing.
        let label = match driver_state.current_context().bottle_path.as_deref() {
            Some(path) if env_override => {
                format!(
                    "Bottle: {} (set by CROSSPUCK_BOTTLE_PATH)",
                    display_path(path)
                )
            }
            Some(path) => format!("Bottle: {}", display_path(path)),
            None => match driver_state.discovered_bottle() {
                Some(path) => format!("Bottle: {} (auto)", display_path(&path)),
                None => "Bottle: (auto)".to_string(),
            },
        };
        items.bottle_info.setTitle(&NSString::from_str(&label));
        items.bottle_choose.setEnabled(!env_override);
        // "Reset to Default" only makes sense when a bottle has been chosen and
        // the env var is not overriding it.
        items
            .bottle_reset
            .setEnabled(!env_override && crate::settings::stored_bottle_path().is_some());
    }

    fn display_path(path: &Path) -> String {
        if let Some(home) = std::env::var_os("HOME") {
            if let Ok(rest) = path.strip_prefix(&home) {
                if rest.as_os_str().is_empty() {
                    return "~".to_string();
                }
                return format!("~/{}", rest.display());
            }
        }
        path.display().to_string()
    }

    // `NSApplication::activate()` is the modern replacement but is macOS 14+
    // only, so keep using `activateIgnoringOtherApps` for broader compatibility.
    #[allow(deprecated)]
    fn activate_app(mtm: MainThreadMarker) {
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }

    fn refresh_driver_items(driver_state: &DriverMenuState, items: &StateMenuItems) {
        let view = driver_state.menu_view();
        items
            .driver_status
            .setTitle(&NSString::from_str(&view.status_title));
        items
            .driver_action
            .setTitle(&NSString::from_str(&view.install_title));
        items.driver_action.setEnabled(view.install_enabled);
        items
            .driver_uninstall
            .setTitle(&NSString::from_str(&view.uninstall_title));
        items.driver_uninstall.setEnabled(view.uninstall_enabled);
    }

    #[derive(Debug)]
    struct DriverMenuState {
        context: Mutex<DriverInstallContext>,
        snapshot: Mutex<DriverMenuSnapshot>,
    }

    #[derive(Clone, Debug)]
    enum DriverMenuSnapshot {
        Checking,
        Installing,
        Uninstalling,
        Status(DriverInstallStatus),
        InstallFailed { message: String },
        UninstallFailed { message: String },
    }

    #[derive(Clone, Debug)]
    struct DriverMenuView {
        status_title: String,
        install_title: String,
        install_enabled: bool,
        uninstall_title: String,
        uninstall_enabled: bool,
    }

    impl DriverMenuState {
        fn new(context: DriverInstallContext) -> Self {
            Self {
                context: Mutex::new(context),
                snapshot: Mutex::new(DriverMenuSnapshot::Checking),
            }
        }

        /// Clone the current install context. The bottles root inside it can be
        /// swapped at runtime via [`set_bottles_dir`], so callers always read a
        /// fresh copy rather than capturing it once.
        fn current_context(&self) -> DriverInstallContext {
            self.context
                .lock()
                .map(|context| context.clone())
                .unwrap_or_default()
        }

        /// Point the driver workflow at an explicit CrossOver bottle, or
        /// `None` to fall back to automatic detection.
        fn set_bottle_path(&self, bottle_path: Option<PathBuf>) {
            if let Ok(mut context) = self.context.lock() {
                context.bottle_path = bottle_path;
            }
        }

        /// Bottle found by the most recent driver status check, if any.
        fn discovered_bottle(&self) -> Option<PathBuf> {
            self.snapshot
                .lock()
                .ok()
                .and_then(|snapshot| match &*snapshot {
                    DriverMenuSnapshot::Status(status) => status.bottle_path.clone(),
                    _ => None,
                })
        }

        fn refresh_status(&self) {
            if self.is_installing() {
                return;
            }
            crate::probe::note_driver_status_check();

            let status = check_driver_install_status(&self.current_context());
            if let Ok(mut snapshot) = self.snapshot.lock() {
                *snapshot = DriverMenuSnapshot::Status(status);
            }
        }

        fn start_install(self: &Arc<Self>) -> bool {
            let Ok(mut snapshot) = self.snapshot.lock() else {
                return false;
            };
            if !driver_snapshot_allows_install(&snapshot) {
                return false;
            }
            *snapshot = DriverMenuSnapshot::Installing;
            drop(snapshot);

            let state = Arc::clone(self);
            let context = self.current_context();
            thread::spawn(move || {
                let next = match install_driver(&context) {
                    Ok(result) => {
                        log::info!(
                            "CrossPuck driver installed: target={} backup={} sha256={} registry={}",
                            result.target_dll.display(),
                            result
                                .backup_path
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "-".to_string()),
                            result.installed_sha256,
                            result.registry_targets.join(",")
                        );
                        DriverMenuSnapshot::Status(check_driver_install_status(&context))
                    }
                    Err(error) => {
                        log::error!("CrossPuck driver install failed: {error}");
                        DriverMenuSnapshot::InstallFailed {
                            message: short_menu_message(&error.to_string()),
                        }
                    }
                };

                if let Ok(mut snapshot) = state.snapshot.lock() {
                    *snapshot = next;
                }
            });
            true
        }

        fn start_uninstall(self: &Arc<Self>) -> bool {
            let Ok(mut snapshot) = self.snapshot.lock() else {
                return false;
            };
            if !driver_snapshot_allows_uninstall(&snapshot) {
                return false;
            }
            *snapshot = DriverMenuSnapshot::Uninstalling;
            drop(snapshot);

            let state = Arc::clone(self);
            let context = self.current_context();
            thread::spawn(move || {
                let next = match uninstall_driver(&context) {
                    Ok(result) => {
                        log::info!(
                            "CrossPuck driver uninstalled: target={} removed={} registry={}",
                            result.target_dll.display(),
                            result.removed_driver,
                            result.registry_targets.join(",")
                        );
                        DriverMenuSnapshot::Status(check_driver_install_status(&context))
                    }
                    Err(error) => {
                        log::error!("CrossPuck driver uninstall failed: {error}");
                        DriverMenuSnapshot::UninstallFailed {
                            message: short_menu_message(&error.to_string()),
                        }
                    }
                };

                if let Ok(mut snapshot) = state.snapshot.lock() {
                    *snapshot = next;
                }
            });
            true
        }

        fn menu_view(&self) -> DriverMenuView {
            let snapshot = self
                .snapshot
                .lock()
                .map(|snapshot| snapshot.clone())
                .unwrap_or(DriverMenuSnapshot::InstallFailed {
                    message: "state lock poisoned".to_string(),
                });
            match snapshot {
                DriverMenuSnapshot::Checking => DriverMenuView {
                    status_title: "Driver: Checking...".to_string(),
                    install_title: "Install Steam Driver...".to_string(),
                    install_enabled: false,
                    uninstall_title: "Uninstall Steam Driver...".to_string(),
                    uninstall_enabled: false,
                },
                DriverMenuSnapshot::Installing => DriverMenuView {
                    status_title: "Driver: Installing...".to_string(),
                    install_title: "Installing...".to_string(),
                    install_enabled: false,
                    uninstall_title: "Uninstall Steam Driver...".to_string(),
                    uninstall_enabled: false,
                },
                DriverMenuSnapshot::Uninstalling => DriverMenuView {
                    status_title: "Driver: Uninstalling...".to_string(),
                    install_title: "Install Steam Driver...".to_string(),
                    install_enabled: false,
                    uninstall_title: "Uninstalling...".to_string(),
                    uninstall_enabled: false,
                },
                DriverMenuSnapshot::Status(status) => {
                    let uninstall_enabled = status_allows_uninstall(&status);
                    DriverMenuView {
                        status_title: status.status_title,
                        install_title: status.action_title,
                        install_enabled: status.action_enabled,
                        uninstall_title: "Uninstall Steam Driver...".to_string(),
                        uninstall_enabled,
                    }
                }
                DriverMenuSnapshot::InstallFailed { message } => DriverMenuView {
                    status_title: format!("Driver: Install failed: {message}"),
                    install_title: "Retry Steam Driver...".to_string(),
                    install_enabled: true,
                    uninstall_title: "Uninstall Steam Driver...".to_string(),
                    uninstall_enabled: false,
                },
                DriverMenuSnapshot::UninstallFailed { message } => DriverMenuView {
                    status_title: format!("Driver: Uninstall failed: {message}"),
                    install_title: "Install Steam Driver...".to_string(),
                    install_enabled: false,
                    uninstall_title: "Retry Uninstall...".to_string(),
                    uninstall_enabled: true,
                },
            }
        }

        fn is_installing(&self) -> bool {
            self.snapshot.lock().is_ok_and(|snapshot| {
                matches!(
                    *snapshot,
                    DriverMenuSnapshot::Installing | DriverMenuSnapshot::Uninstalling
                )
            })
        }
    }

    fn short_menu_message(message: &str) -> String {
        const MAX_LEN: usize = 96;
        let trimmed = message.trim();
        if trimmed.chars().count() <= MAX_LEN {
            return trimmed.to_string();
        }
        let mut shortened = trimmed.chars().take(MAX_LEN).collect::<String>();
        shortened.push_str("...");
        shortened
    }

    fn driver_snapshot_allows_install(snapshot: &DriverMenuSnapshot) -> bool {
        match snapshot {
            DriverMenuSnapshot::Status(status) => status.action_enabled,
            DriverMenuSnapshot::InstallFailed { .. } => true,
            DriverMenuSnapshot::Checking
            | DriverMenuSnapshot::Installing
            | DriverMenuSnapshot::Uninstalling
            | DriverMenuSnapshot::UninstallFailed { .. } => false,
        }
    }

    fn driver_snapshot_allows_uninstall(snapshot: &DriverMenuSnapshot) -> bool {
        match snapshot {
            DriverMenuSnapshot::Status(status) => status_allows_uninstall(status),
            DriverMenuSnapshot::UninstallFailed { .. } => true,
            DriverMenuSnapshot::Checking
            | DriverMenuSnapshot::Installing
            | DriverMenuSnapshot::Uninstalling
            | DriverMenuSnapshot::InstallFailed { .. } => false,
        }
    }

    fn status_allows_uninstall(status: &DriverInstallStatus) -> bool {
        matches!(
            status.state,
            DriverInstallState::Installed | DriverInstallState::UpdateAvailable
        )
    }

    unsafe fn bundle_resources_dir() -> Option<PathBuf> {
        NSBundle::mainBundle()
            .resourcePath()
            .map(|path| PathBuf::from(path.to_string()))
    }

    unsafe fn load_status_icon() -> Option<Retained<NSImage>> {
        let name = NSString::from_str("CrossPuckStatusTemplate");
        let pdf_ext = NSString::from_str("pdf");
        let pdf_source_path = NSString::from_str(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/Resources/CrossPuckStatusTemplate.pdf"
        ));
        if let Some(image) = load_template_image(&name, &pdf_ext, &pdf_source_path) {
            return Some(image);
        }

        let ext = NSString::from_str("png");
        let source_path = NSString::from_str(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/Resources/CrossPuckStatusTemplate.png"
        ));
        load_template_image(&name, &ext, &source_path)
    }

    unsafe fn load_template_image(
        name: &NSString,
        ext: &NSString,
        source_path: &NSString,
    ) -> Option<Retained<NSImage>> {
        let bundle_path = NSBundle::mainBundle().pathForResource_ofType(Some(name), Some(ext));
        let path = bundle_path.as_deref().unwrap_or(source_path);
        let image = NSImage::initWithContentsOfFile(NSImage::alloc(), path)?;
        image.setTemplate(true);
        Some(image)
    }
}
