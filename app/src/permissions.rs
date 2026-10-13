#[derive(Clone, Copy, PartialEq)]
pub enum Permission {
    Accessibility,
    InputMonitoring,
}

#[cfg(target_os = "macos")]
mod platform {
    use std::process::Command;

    use core_foundation::{
        base::TCFType,
        boolean::CFBoolean,
        dictionary::{CFDictionary, CFDictionaryRef},
        string::CFString,
    };

    use super::Permission;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
        fn CGPreflightPostEventAccess() -> bool;
        fn CGRequestPostEventAccess() -> bool;
    }

    pub fn missing() -> Vec<Permission> {
        let mut missing = Vec::new();
        unsafe {
            if !AXIsProcessTrusted() || !CGPreflightPostEventAccess() {
                missing.push(Permission::Accessibility);
            }
            if !CGPreflightListenEventAccess() {
                missing.push(Permission::InputMonitoring);
            }
        }
        missing
    }

    pub fn request(permission: Permission) {
        match permission {
            Permission::Accessibility => unsafe {
                let options = CFDictionary::from_CFType_pairs(&[(
                    CFString::new("AXTrustedCheckOptionPrompt"),
                    CFBoolean::true_value(),
                )]);
                AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef());
                CGRequestPostEventAccess();
            },
            Permission::InputMonitoring => unsafe {
                CGRequestListenEventAccess();
            },
        }
        let pane = match permission {
            Permission::Accessibility => "Privacy_Accessibility",
            Permission::InputMonitoring => "Privacy_ListenEvent",
        };
        let _ = Command::new("open")
            .arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}"))
            .spawn();
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::Permission;

    pub fn missing() -> Vec<Permission> {
        Vec::new()
    }

    pub fn request(_: Permission) {}
}

pub use platform::{missing, request};
