//! The managed policy that makes Chromium pick the client certificate without
//! asking. Chromium reads policies only from root-owned files, so this is
//! printed for the administrator to install, never written.

use serde_json::{json, Value};

use crate::web::tls::CA_COMMON_NAME;

/// Where each Chromium-family browser looks for managed policy files.
pub const POLICY_DIRS: [&str; 3] = [
    "/etc/chromium/policies/managed/",
    "/etc/opt/chrome/policies/managed/",
    "/etc/brave/policies/managed/",
];

const LOOPBACK_PATTERNS: [&str; 2] = ["https://127.0.0.1:*", "https://localhost:*"];

/// The policy file's content. Each list entry is itself a JSON document, as
/// the policy requires.
pub fn policy_json() -> Value {
    let entries: Vec<String> = LOOPBACK_PATTERNS
        .iter()
        .map(|pattern| {
            json!({ "pattern": pattern, "filter": { "ISSUER": { "CN": CA_COMMON_NAME } } })
                .to_string()
        })
        .collect();
    json!({ "AutoSelectCertificateForUrls": entries })
}

/// What `setup-browser` prints for Chromium, Chrome and Brave.
pub fn instructions() -> String {
    let file = serde_json::to_string_pretty(&policy_json()).unwrap_or_default();
    let dirs = POLICY_DIRS
        .iter()
        .map(|d| format!("  {d}claude-monitor.json"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Chromium: to pick the client certificate without a prompt, install this managed \
         policy (not written by this command; it is root-owned):\n{file}\n\
         Put it in one of:\n{dirs}\n\
         NixOS: programs.chromium.enable = true; programs.chromium.extraOpts = \
         {{ AutoSelectCertificateForUrls = [ ... the two strings above ... ]; }};\n\
         It covers only https://127.0.0.1 and https://localhost certificates issued by \
         \"{CA_COMMON_NAME}\"."
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn every_entry_is_scoped_to_loopback_and_to_this_installs_ca() {
        let policy = policy_json();
        let entries = policy["AutoSelectCertificateForUrls"].as_array().unwrap();
        let parsed: Vec<Value> = entries
            .iter()
            .map(|e| serde_json::from_str(e.as_str().unwrap()).unwrap())
            .collect();

        let patterns: Vec<_> = parsed
            .iter()
            .map(|p| p["pattern"].as_str().unwrap())
            .collect();
        assert_eq!(patterns, vec!["https://127.0.0.1:*", "https://localhost:*"]);
        for entry in &parsed {
            assert_eq!(entry["filter"]["ISSUER"]["CN"], "claude-monitor local CA");
        }
    }

    #[test]
    fn the_instructions_say_where_it_goes_and_how_on_nixos() {
        let text = instructions();
        assert!(text.contains("/etc/chromium/policies/managed/claude-monitor.json"));
        assert!(text.contains("programs.chromium.extraOpts"));
        assert!(text.contains("AutoSelectCertificateForUrls"));
        assert!(text.contains("not written"));
    }
}
