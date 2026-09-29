pub fn version() -> String {
    if let Some(v) = option_env!("PULSE_VERSION").filter(|v| !v.is_empty()) {
        return v.to_string();
    }
    option_env!("PULSE_BUILD_VERSION")
        .unwrap_or("dev")
        .to_string()
}

pub fn build_version(main_version: &str, revision: &str, modified: bool) -> String {
    if !main_version.is_empty() && main_version != "(devel)" {
        return main_version.to_string();
    }
    if revision.is_empty() {
        return "dev".to_string();
    }
    let rev: String = revision.chars().take(12).collect();
    format!("dev-{rev}{}", if modified { "-dirty" } else { "" })
}

pub fn revision() -> String {
    option_env!("PULSE_REVISION").unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_version_cases() {
        for (main, revision, modified, want) in [
            ("v1.2.3", "", false, "v1.2.3"),
            ("v1.2.3", "abcdef1234567890", true, "v1.2.3"),
            ("(devel)", "abcdef1234567890", false, "dev-abcdef123456"),
            (
                "(devel)",
                "abcdef1234567890",
                true,
                "dev-abcdef123456-dirty",
            ),
            ("", "abcdef", false, "dev-abcdef"),
            ("(devel)", "", true, "dev"),
            ("", "", false, "dev"),
        ] {
            assert_eq!(build_version(main, revision, modified), want);
        }
    }
}
