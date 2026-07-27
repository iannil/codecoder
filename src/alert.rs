// Alert notification system for headless runner failures.
// Sends Slack-compatible webhook alerts.

/// Send a Slack-compatible webhook alert.
/// The payload is `{"text": "..."}` — compatible with Slack, Discord, Teams, etc.
pub fn send_alert(webhook: &str, text: &str) -> anyhow::Result<()> {
    let body = serde_json::json!({ "text": text });
    let resp = ureq::post(webhook)
        .set("Content-Type", "application/json")
        .send_json(&body)?;
    let status = resp.status();
    if status < 200 || status >= 300 {
        let body_text = resp.into_string().unwrap_or_default();
        anyhow::bail!("webhook returned {status}: {body_text}");
    }
    Ok(())
}

/// Build an alert message from a BG run outcome.
pub fn format_bg_alert(exit_code: i32, mission_state: &str, summary: &str) -> String {
    format!(
        "🔴 CodeCoder BG Alert\nExit Code: {exit_code}\nState: {mission_state}\nSummary: {summary}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bg_alert_includes_key_info() {
        let msg = format_bg_alert(2, "StuckNeedsFix", "milestone #3 failed after 3 retries");
        assert!(msg.contains("Exit Code: 2"));
        assert!(msg.contains("StuckNeedsFix"));
        assert!(msg.contains("milestone #3"));
    }
}