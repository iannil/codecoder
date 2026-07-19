// Structured review verdict + architecture-drift rubric (first-class citizen #4;
// see docs/design/2026-07-19-review-verdict-rubric.md).
//
// The `review` tool spawns a read-only sub-agent (ADR 0019). This module turns
// that sub-agent's free prose into a deterministic, machine-readable outcome:
// a Verdict (pass / needs_fix / rebuild) plus four drift Signals ported from the
// engineer-inspector skill and calibrated to codecoder (small files, strong
// glossary). These are PURE functions — the AgentLoop owns spawning and I/O.

use std::fmt;

/// The acceptance verdict, ordered by severity (Pass < NeedsFix < Rebuild).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    NeedsFix,
    Rebuild,
}

impl Verdict {
    fn severity(self) -> u8 {
        match self {
            Verdict::Pass => 0,
            Verdict::NeedsFix => 1,
            Verdict::Rebuild => 2,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::NeedsFix => "needs_fix",
            Verdict::Rebuild => "rebuild",
        }
    }
    /// Take the more-severe of two verdicts (the kernel guard: a lenient reviewer
    /// can never downgrade below what the signals imply).
    fn max(self, other: Verdict) -> Verdict {
        if other.severity() > self.severity() { other } else { self }
    }
    /// Parse a reviewer's self-reported token, tolerant of `-`/space separators.
    fn parse(token: &str) -> Option<Verdict> {
        match token.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "pass" => Some(Verdict::Pass),
            "needs_fix" | "needsfix" => Some(Verdict::NeedsFix),
            "rebuild" => Some(Verdict::Rebuild),
            _ => None,
        }
    }
}

/// Per-signal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalStatus {
    Ok,
    Warn,
    Fail,
}

impl SignalStatus {
    fn as_str(self) -> &'static str {
        match self {
            SignalStatus::Ok => "ok",
            SignalStatus::Warn => "warn",
            SignalStatus::Fail => "fail",
        }
    }
    /// Parse a status value; an unknown value is a soft `warn` (spec §4), not a
    /// hard error — it must not abort the whole parse.
    fn parse(v: &str) -> SignalStatus {
        match v.trim().to_ascii_lowercase().as_str() {
            "ok" | "pass" => SignalStatus::Ok,
            "fail" | "bad" => SignalStatus::Fail,
            _ => SignalStatus::Warn,
        }
    }
}

/// The four architecture-drift signals (engineer-inspector, calibrated). Default
/// `Ok` — a missing key is treated as no-evidence-of-drift, never manufactured
/// into a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signals {
    pub foundation: SignalStatus,
    pub over_engineering: SignalStatus,
    pub volume: SignalStatus,
    pub terminology: SignalStatus,
}

impl Default for Signals {
    fn default() -> Self {
        Signals {
            foundation: SignalStatus::Ok,
            over_engineering: SignalStatus::Ok,
            volume: SignalStatus::Ok,
            terminology: SignalStatus::Ok,
        }
    }
}

impl Signals {
    /// Verdict implied by the signals alone. Foundation-tampering is a red-line
    /// breach → rebuild; any other failure → needs_fix; else pass.
    fn derived(&self) -> Verdict {
        if self.foundation == SignalStatus::Fail {
            Verdict::Rebuild
        } else if [self.over_engineering, self.volume, self.terminology]
            .contains(&SignalStatus::Fail)
        {
            Verdict::NeedsFix
        } else {
            Verdict::Pass
        }
    }
}

impl fmt::Display for Signals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "foundation={}  over_engineering={}  volume={}  terminology={}",
            self.foundation.as_str(),
            self.over_engineering.as_str(),
            self.volume.as_str(),
            self.terminology.as_str(),
        )
    }
}

/// The parsed, aggregated outcome of a review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewOutcome {
    pub verdict: Verdict,
    pub signals: Signals,
    /// True when neither a VERDICT nor a SIGNALS line was found — the reviewer
    /// ignored the output contract, so we default to needs_fix and flag it.
    pub unparsed: bool,
}

/// Parse a sub-agent's review prose into a `ReviewOutcome`. Robust to arbitrary
/// surrounding text: the last `VERDICT:` and last `SIGNALS:` lines win.
pub fn parse_review(text: &str) -> ReviewOutcome {
    let mut parsed_verdict: Option<Verdict> = None;
    let mut signals: Option<Signals> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("verdict:") {
            // Take the first whitespace token after the colon; last valid wins.
            if let Some(v) = rest.split_whitespace().next().and_then(Verdict::parse) {
                parsed_verdict = Some(v);
            }
        } else if lower.starts_with("signals:") {
            // Parse key=value pairs from the ORIGINAL-case remainder.
            let rest = &trimmed[trimmed.find(':').map(|i| i + 1).unwrap_or(trimmed.len())..];
            let mut s = Signals::default();
            for pair in rest.split_whitespace() {
                if let Some((k, v)) = pair.split_once('=') {
                    let status = SignalStatus::parse(v);
                    match k.trim().to_ascii_lowercase().as_str() {
                        "foundation" => s.foundation = status,
                        "over_engineering" | "overengineering" => s.over_engineering = status,
                        "volume" => s.volume = status,
                        "terminology" => s.terminology = status,
                        _ => {} // unknown key ignored
                    }
                }
            }
            signals = Some(s); // last one wins
        }
    }

    let unparsed = parsed_verdict.is_none() && signals.is_none();
    let sig = signals.unwrap_or_default();
    let verdict = if unparsed {
        // Contract ignored — safe default forces attention, never silent pass.
        Verdict::NeedsFix
    } else {
        // Kernel guard: the more-severe of the reviewer's call and the signals.
        parsed_verdict.unwrap_or(Verdict::Pass).max(sig.derived())
    };

    ReviewOutcome { verdict, signals: sig, unparsed }
}

/// Format the deterministic result returned to the parent agent: a structured
/// header, then the sub-agent's full prose.
pub fn format_result(outcome: &ReviewOutcome, body: &str) -> String {
    let flag = if outcome.unparsed { " (unparsed)" } else { "" };
    format!(
        "REVIEW VERDICT: {}{}\nsignals: {}\n—\n{}",
        outcome.verdict.as_str(),
        flag,
        outcome.signals,
        body.trim(),
    )
}

/// Build the review task handed to the sub-agent: the drift rubric plus the
/// two-line output contract that `parse_review` consumes.
pub fn review_task(target: &str) -> String {
    format!(
        "You are a read-only architecture reviewer. Review {target}.\n\
         Use `diff` to inspect the changes; read `CONTEXT.md` and files under \
         `docs/adr/` to learn the project's red lines and glossary.\n\n\
         Judge the change against these four architecture-drift signals \
         (status = ok | warn | fail), citing concrete evidence for each:\n\
         - foundation: silently altering solidified ground — public type/trait \
         signatures, the message model, permission keys, session format, or any \
         contract fixed by an ADR. A breach here is a red line.\n\
         - over_engineering: unnecessary dependencies, abstractions, or \
         indirection introduced for edge cases.\n\
         - volume: files or functions growing disproportionately, duplicated \
         blocks, a module's responsibility drifting.\n\
         - terminology: new names that collide with the `_Avoid_` entries in \
         CONTEXT.md, or synonyms for existing glossary terms.\n\n\
         Write your findings as prose, then END your report with EXACTLY these \
         two lines (verbatim keys, lowercase statuses):\n\
         VERDICT: <pass|needs_fix|rebuild>\n\
         SIGNALS: foundation=<ok|warn|fail> over_engineering=<ok|warn|fail> \
         volume=<ok|warn|fail> terminology=<ok|warn|fail>\n\
         Choose rebuild only for a foundation breach; needs_fix for any other \
         failure; pass when only warnings or clean."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_with(tail: &str) -> String {
        format!("Some review prose.\nMore detail.\n{tail}")
    }

    #[test]
    fn parses_each_verdict() {
        for (tok, want) in
            [("pass", Verdict::Pass), ("needs_fix", Verdict::NeedsFix), ("rebuild", Verdict::Rebuild)]
        {
            let o = parse_review(&body_with(&format!(
                "VERDICT: {tok}\nSIGNALS: foundation=ok over_engineering=ok volume=ok terminology=ok"
            )));
            assert_eq!(o.verdict, want, "token {tok}");
            assert!(!o.unparsed);
        }
    }

    #[test]
    fn verdict_token_is_separator_tolerant_and_case_insensitive() {
        let o = parse_review(&body_with("verdict:  Needs-Fix\nSIGNALS: foundation=ok"));
        assert_eq!(o.verdict, Verdict::NeedsFix);
    }

    #[test]
    fn last_verdict_line_wins() {
        let o = parse_review(&body_with("VERDICT: pass\nnoise\nVERDICT: rebuild\nSIGNALS: foundation=ok"));
        assert_eq!(o.verdict, Verdict::Rebuild);
    }

    #[test]
    fn missing_both_lines_defaults_needs_fix_unparsed() {
        let o = parse_review("just some prose, no contract");
        assert_eq!(o.verdict, Verdict::NeedsFix);
        assert!(o.unparsed);
    }

    #[test]
    fn signals_derive_verdict_without_reviewer_verdict() {
        // No VERDICT line, but a foundation fail → rebuild (signals present, not unparsed).
        let o = parse_review(&body_with(
            "SIGNALS: foundation=fail over_engineering=ok volume=ok terminology=ok",
        ));
        assert_eq!(o.verdict, Verdict::Rebuild);
        assert!(!o.unparsed);
    }

    #[test]
    fn volume_fail_alone_is_needs_fix() {
        let o = parse_review(&body_with(
            "VERDICT: pass\nSIGNALS: foundation=ok over_engineering=ok volume=fail terminology=ok",
        ));
        // Guard upgrades the lenient pass to needs_fix.
        assert_eq!(o.verdict, Verdict::NeedsFix);
    }

    #[test]
    fn guard_upgrades_lenient_pass_on_foundation_fail() {
        let o = parse_review(&body_with(
            "VERDICT: pass\nSIGNALS: foundation=fail over_engineering=ok volume=ok terminology=ok",
        ));
        assert_eq!(o.verdict, Verdict::Rebuild);
    }

    #[test]
    fn strict_rebuild_honored_over_clean_signals() {
        let o = parse_review(&body_with(
            "VERDICT: rebuild\nSIGNALS: foundation=ok over_engineering=ok volume=ok terminology=ok",
        ));
        assert_eq!(o.verdict, Verdict::Rebuild);
    }

    #[test]
    fn unknown_signal_value_is_warn_not_fail() {
        let o = parse_review(&body_with(
            "VERDICT: pass\nSIGNALS: foundation=maybe over_engineering=ok volume=ok terminology=ok",
        ));
        assert_eq!(o.signals.foundation, SignalStatus::Warn);
        assert_eq!(o.verdict, Verdict::Pass); // warn does not fail
    }

    #[test]
    fn format_header_is_stable() {
        let o = parse_review(&body_with(
            "VERDICT: needs_fix\nSIGNALS: foundation=ok over_engineering=warn volume=fail terminology=ok",
        ));
        let out = format_result(&o, "the body");
        assert!(out.starts_with("REVIEW VERDICT: needs_fix\n"), "got: {out}");
        assert!(out.contains("volume=fail"));
        assert!(out.trim_end().ends_with("the body"));
    }

    #[test]
    fn format_flags_unparsed() {
        let o = parse_review("no contract here");
        let out = format_result(&o, "body");
        assert!(out.starts_with("REVIEW VERDICT: needs_fix (unparsed)"), "got: {out}");
    }

    #[test]
    fn review_task_states_contract_and_target() {
        let t = review_task("the current changes");
        assert!(t.contains("the current changes"));
        assert!(t.contains("VERDICT:"));
        assert!(t.contains("SIGNALS:"));
        assert!(t.contains("foundation") && t.contains("terminology"));
    }
}
