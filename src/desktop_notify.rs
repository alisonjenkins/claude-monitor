use anyhow::Result;

/// Send a desktop notification about sessions needing attention
pub fn notify_attention(count: usize) -> Result<()> {
    let body = if count == 1 {
        "1 session needs attention".to_string()
    } else {
        format!("{} sessions need attention", count)
    };

    #[cfg(target_os = "linux")]
    {
        notify_rust::Notification::new()
            .summary("Claude Code")
            .body(&body)
            .urgency(notify_rust::Urgency::Normal)
            .show()?;
    }

    #[cfg(target_os = "macos")]
    {
        let _ = mac_notification_sys::send_notification(
            "Claude Code",
            None,
            &body,
            None,
        );
    }

    Ok(())
}
