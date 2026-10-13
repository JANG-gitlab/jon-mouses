#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Outcome {
    Installed,
    Declined,
    Failed(String),
}

#[cfg(target_os = "macos")]
mod platform {
    use std::{fs, path::Path, process::Command};

    use super::Outcome;

    const SCRIPT: &str = include_str!("../helper/awdl.sh");
    const PLIST: &str = include_str!("../helper/app.jonmouses.awdl.plist");
    const SCRIPT_PATH: &str = "/Library/PrivilegedHelperTools/app.jonmouses.awdl";
    const PLIST_PATH: &str = "/Library/LaunchDaemons/app.jonmouses.awdl.plist";

    pub fn helper_installed() -> bool {
        fs::read_to_string(SCRIPT_PATH).is_ok_and(|script| script == SCRIPT)
            && fs::read_to_string(PLIST_PATH).is_ok_and(|plist| plist == PLIST)
    }

    fn quoted(text: &str) -> String {
        text.replace('\\', "\\\\").replace('"', "\\\"")
    }

    /// Installs a root launch daemon that keeps AWDL down while the app runs,
    /// so macOS asks for an administrator password once instead of every launch.
    pub fn install_helper(staging: &Path, prompt: &str) -> Outcome {
        let script = staging.join("awdl.sh");
        let plist = staging.join("awdl.plist");
        if let Err(error) = fs::write(&script, SCRIPT).and_then(|()| fs::write(&plist, PLIST)) {
            return Outcome::Failed(error.to_string());
        }
        let shell = format!(
            "mkdir -p /Library/PrivilegedHelperTools && \
             install -o root -g wheel -m 755 '{script}' {SCRIPT_PATH} && \
             install -o root -g wheel -m 644 '{plist}' {PLIST_PATH} && \
             (launchctl bootout system/app.jonmouses.awdl 2>/dev/null; true) && \
             launchctl bootstrap system {PLIST_PATH}",
            script = script.display(),
            plist = plist.display(),
        );
        let apple_script = format!(
            "do shell script \"{}\" with administrator privileges with prompt \"{}\"",
            quoted(&shell),
            quoted(prompt),
        );
        let result = Command::new("/usr/bin/osascript").arg("-e").arg(apple_script).output();
        let _ = fs::remove_file(&script);
        let _ = fs::remove_file(&plist);
        match result {
            Ok(output) if output.status.success() => Outcome::Installed,
            Ok(output) if String::from_utf8_lossy(&output.stderr).contains("-128") => Outcome::Declined,
            Ok(output) => Outcome::Failed(String::from_utf8_lossy(&output.stderr).trim().to_string()),
            Err(error) => Outcome::Failed(error.to_string()),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;

    use super::Outcome;

    pub fn helper_installed() -> bool {
        true
    }

    pub fn install_helper(_: &Path, _: &str) -> Outcome {
        Outcome::Installed
    }
}

pub use platform::{helper_installed, install_helper};
