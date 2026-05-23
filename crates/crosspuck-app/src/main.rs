use std::process::ExitCode;

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
    use crosspuck_core::state::{snapshot_host_state, ServiceState};
    use objc2::rc::Retained;
    use objc2::{sel, MainThreadMarker};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSMenu, NSMenuItem, NSSquareStatusItemLength,
        NSStatusBar, NSStatusItem,
    };
    use objc2_foundation::{NSAutoreleasePool, NSString};
    use std::process::ExitCode;

    struct MenuBarObjects {
        _status_item: Retained<NSStatusItem>,
        _menu: Retained<NSMenu>,
        _state_item: Retained<NSMenuItem>,
    }

    pub fn run() -> ExitCode {
        let mtm = MainThreadMarker::new().expect("crosspuck-app must run on the main thread");

        unsafe {
            let _pool = NSAutoreleasePool::new();
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

            let menu_objects = build_menu_bar(&app, mtm);
            Box::leak(Box::new(menu_objects));

            app.run();
        }

        ExitCode::SUCCESS
    }

    unsafe fn build_menu_bar(app: &NSApplication, mtm: MainThreadMarker) -> MenuBarObjects {
        let status_bar = NSStatusBar::systemStatusBar();
        let status_item = status_bar.statusItemWithLength(NSSquareStatusItemLength);

        if let Some(button) = status_item.button(mtm) {
            button.setTitle(&NSString::from_str("CP"));
        }

        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);

        let state = snapshot_host_state().unwrap_or(ServiceState::PuckDisconnected);
        let state_title = NSString::from_str(&format!("상태: {}", state.menu_label()));
        let empty_key = NSString::from_str("");
        let state_item = menu.addItemWithTitle_action_keyEquivalent(&state_title, None, &empty_key);
        state_item.setEnabled(false);

        let separator = NSMenuItem::separatorItem(mtm);
        menu.addItem(&separator);

        let quit_title = NSString::from_str("종료");
        let quit_key = NSString::from_str("q");
        let quit_item = menu.addItemWithTitle_action_keyEquivalent(
            &quit_title,
            Some(sel!(terminate:)),
            &quit_key,
        );
        quit_item.setTarget(Some(app.as_ref()));

        status_item.setMenu(Some(&menu));

        MenuBarObjects {
            _status_item: status_item,
            _menu: menu,
            _state_item: state_item,
        }
    }
}
