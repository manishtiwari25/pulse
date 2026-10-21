pub const SIGNING_IDENTIFIER: &str = "pulse";

#[cfg(target_os = "macos")]
pub fn code_signed(path: &str) -> (bool, bool) {
    use std::process::Command;

    if path.is_empty() {
        return (false, false);
    }
    let output = Command::new("codesign").args(["-d", "-r-", path]).output();
    match output {
        Ok(out) if out.status.success() => {
            let mut text = String::from_utf8_lossy(&out.stderr).replace('"', "");
            text.push_str(&String::from_utf8_lossy(&out.stdout).replace('"', ""));
            (
                text.contains(&format!("designated => identifier {SIGNING_IDENTIFIER}"))
                    && !text.contains("cdhash"),
                true,
            )
        }
        _ => (false, std::path::Path::new(path).exists()),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn code_signed(_path: &str) -> (bool, bool) {
    (false, false)
}

pub fn executable() -> String {
    std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}
