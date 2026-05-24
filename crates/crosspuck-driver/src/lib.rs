#[cfg(windows)]
mod windows;

#[cfg(not(windows))]
pub fn platform_note() -> &'static str {
    "crosspuck-driver builds the production hid.dll on Windows targets"
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(not(windows))]
    fn non_windows_build_has_platform_note() {
        assert!(super::platform_note().contains("hid.dll"));
    }
}
