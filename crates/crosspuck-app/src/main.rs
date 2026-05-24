use std::process::ExitCode;

#[cfg(target_os = "macos")]
mod hid_backend;
#[cfg(target_os = "macos")]
mod runtime;

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    macos::run()
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    eprintln!("crosspuck-app is only supported on macOS.");
    ExitCode::from(1)
}

#[cfg(target_os = "macos")]
mod macos {
    use crate::runtime::{start_host_service, AppState, HostServiceHandle};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{
        define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
    };
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSCellImagePosition, NSImage, NSImageScaling,
        NSMenu, NSMenuDelegate, NSMenuItem, NSSquareStatusItemLength, NSStatusBar, NSStatusItem,
    };
    use objc2_foundation::{
        NSAutoreleasePool, NSBundle, NSObject, NSObjectProtocol, NSString, NSTimer,
    };
    use std::process::ExitCode;

    struct MenuBarObjects {
        _status_item: Retained<NSStatusItem>,
        _icon: Option<Retained<NSImage>>,
        _menu: Retained<NSMenu>,
        _state_items: StateMenuItems,
        _menu_delegate: Retained<StateMenuDelegate>,
        _refresh_timer: Retained<NSTimer>,
        _quit_controller: Retained<QuitController>,
    }

    #[derive(Clone)]
    struct StateMenuItems {
        status: Retained<NSMenuItem>,
        puck: Retained<NSMenuItem>,
        guest: Retained<NSMenuItem>,
        error: Retained<NSMenuItem>,
    }

    #[derive(Clone)]
    struct StateMenuDelegateIvars {
        app_state: AppState,
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
                refresh_state_items(&self.ivars().app_state, &self.ivars().items);
            }
        }

        impl StateMenuDelegate {
            #[unsafe(method(refreshTimer:))]
            fn refresh_timer(&self, _timer: &NSTimer) {
                refresh_state_items(&self.ivars().app_state, &self.ivars().items);
            }
        }
    );

    impl StateMenuDelegate {
        fn new(
            mtm: MainThreadMarker,
            app_state: AppState,
            items: StateMenuItems,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(StateMenuDelegateIvars { app_state, items });
            unsafe { msg_send![super(this), init] }
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
        let mtm = MainThreadMarker::new().expect("crosspuck-app must run on the main thread");

        unsafe {
            let _pool = NSAutoreleasePool::new();
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

            let app_state = AppState::new();
            let service_handle = start_host_service(app_state.clone());
            let menu_objects = build_menu_bar(app.clone(), mtm, &app_state, service_handle);
            Box::leak(Box::new(menu_objects));

            app.run();
        }

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

        let empty_key = NSString::from_str("");
        let state_items = StateMenuItems {
            status: menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("상태: 시작 중"),
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
                &NSString::from_str("최근 오류: -"),
                None,
                &empty_key,
            ),
        };
        state_items.status.setEnabled(false);
        state_items.puck.setEnabled(false);
        state_items.guest.setEnabled(false);
        state_items.error.setEnabled(false);

        let menu_delegate = StateMenuDelegate::new(mtm, app_state.clone(), state_items.clone());
        menu.setDelegate(Some(ProtocolObject::from_ref(&*menu_delegate)));
        refresh_state_items(app_state, &state_items);
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
        let quit_title = NSString::from_str("종료");
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
            _quit_controller: quit_controller,
        }
    }

    fn refresh_state_items(app_state: &AppState, items: &StateMenuItems) {
        let view = app_state.snapshot().menu_view();
        items
            .status
            .setTitle(&NSString::from_str(&format!("상태: {}", view.status)));
        items
            .puck
            .setTitle(&NSString::from_str(&format!("Puck: {}", view.puck)));
        items
            .guest
            .setTitle(&NSString::from_str(&format!("Guest: {}", view.guest)));
        items
            .error
            .setTitle(&NSString::from_str(&format!("최근 오류: {}", view.error)));
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
