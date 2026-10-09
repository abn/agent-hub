//! Size limits shared by the stores and the surfaces.

use crate::error::{Error, Result};

/// Maximum bytes of a serialised event payload.
pub const EVENT_PAYLOAD_BYTES_MAX: usize = 256 * 1024;

/// Maximum characters of an event summary.
pub const EVENT_SUMMARY_CHARS_MAX: usize = 1024;

/// Maximum bytes of an HTTP request body.
pub const REQUEST_BODY_BYTES_MAX: usize = 4 * 1024 * 1024;

/// Maximum bytes of an artifact blob.
pub const ARTIFACT_BYTES_MAX: usize = 50 * 1024 * 1024;

/// Maximum bytes of a request body on the agent transport.
///
/// An artifact reaches the hub as a JSON string argument, so the transport has
/// to carry the artifact cap plus the escaping that JSON adds to it and the
/// rest of the call around it. What a tool then accepts stays with the tool's
/// own check; this only keeps the transport from refusing first.
pub const AGENT_BODY_BYTES_MAX: usize =
    ARTIFACT_BYTES_MAX + ARTIFACT_BYTES_MAX / 8 + REQUEST_BODY_BYTES_MAX;

/// Maximum bytes of one session brain value.
///
/// The agent transport is sized to carry an artifact, so it does not bound a
/// brain value; this does. A brain holds working state, not documents, and
/// keeping a value under the REST body ceiling also keeps a brain file inside
/// its own budget for any sane number of entries.
pub const BRAIN_VALUE_BYTES_MAX: usize = REQUEST_BODY_BYTES_MAX;

/// Maximum bytes of one knowledge base page (1 MiB).
///
/// A page is a document a human reads and edits, and it reaches the hub as a
/// JSON string over REST: a mebibyte of text, escaped, still fits the request
/// body ceiling with the rest of the call around it.
pub const KB_PAGE_BYTES_MAX: usize = 1024 * 1024;

/// Soft file size limit for one session brain or knowledge base file (256 MiB).
pub const BRAIN_FILE_BYTES_SOFT: i64 = 256 * 1024 * 1024;

/// Hard file size limit for one session brain or knowledge base file (1 GiB).
pub const BRAIN_FILE_BYTES_HARD: i64 = 1024 * 1024 * 1024;

/// Maximum bytes of one knowledge base page path.
pub const KB_PATH_BYTES_MAX: usize = 512;

/// Rows one knowledge base history page returns when the caller names no
/// limit, for the project's write log and for one page's versions alike.
pub const KB_HISTORY_ROWS_DEFAULT: usize = 50;

/// The most rows one knowledge base history page returns. Zero is allowed and
/// returns the count alone.
pub const KB_HISTORY_ROWS_MAX: usize = 200;

/// Most pages one knowledge base export or import carries.
///
/// An import lints the whole folder before it writes anything, so the bundle
/// is held in memory at once; this and [`KB_BUNDLE_BYTES_MAX`] bound that.
pub const KB_BUNDLE_PAGES_MAX: usize = 10_000;

/// Most bytes of page content one knowledge base export or import carries
/// (256 MiB, the knowledge base file's soft limit).
pub const KB_BUNDLE_BYTES_MAX: u64 = 256 * 1024 * 1024;

/// Maximum bytes of the indexed body of one search document.
pub const SEARCH_BODY_BYTES_MAX: usize = 64 * 1024;

/// Maximum characters of the note a session leaves when it ends.
///
/// A handoff is a pointer to the work, not the work: the state itself is in the
/// brain the next agent picks up.
pub const HANDOFF_CHARS_MAX: usize = 4096;

/// Maximum characters of the note the human leaves with a decision.
///
/// A note says why, in a sentence or a few; anything longer belongs in a
/// reply the agent can thread.
pub const DECISION_NOTE_CHARS_MAX: usize = 2000;

/// Fewest and most suggested answers a question may offer.
///
/// One suggestion is not a choice, and past a handful the human is reading a
/// form rather than tapping an answer; a longer menu belongs in the body.
pub const QUESTION_OPTIONS_MIN: usize = 2;
/// Most suggested answers a question may offer.
pub const QUESTION_OPTIONS_MAX: usize = 6;

/// Maximum characters of one suggested answer.
///
/// An option is a button label the human taps on a phone, so it is a phrase,
/// not a paragraph.
pub const QUESTION_OPTION_CHARS_MAX: usize = 80;

/// The shortest deadline an agent may put on a question or an approval, in
/// seconds. Shorter than this and the human has no real chance to see it.
pub const DEADLINE_SECS_MIN: u64 = 60;

/// The longest deadline an agent may put on a question or an approval, in
/// seconds: thirty days.
pub const DEADLINE_SECS_MAX: u64 = 30 * 24 * 60 * 60;

/// Maximum feed page size.
pub const FEED_LIMIT_MAX: i64 = 500;

/// Maximum session listing page size, and its default.
///
/// Its own pair rather than the feed's, so two unrelated page sizes are not
/// tied together.
pub const SESSION_LIST_LIMIT_MAX: i64 = 200;
/// Default session listing page size.
pub const SESSION_LIST_LIMIT_DEFAULT: i64 = 50;

/// Entries one brain listing returns.
///
/// The tree loads a directory at a time, so the cap bounds one level rather
/// than a whole brain. A level with more says so instead of truncating in
/// silence.
pub const BRAIN_LIST_ENTRIES_MAX: usize = 500;

/// Characters of a handoff note carried in a session listing.
pub const HANDOFF_SUMMARY_CHARS: usize = 200;

/// Feed events one session brief carries. The rest are counted, and
/// `feed_read` reads them in full.
pub const SESSION_BRIEF_EVENTS_MAX: usize = 20;

/// Newest events a session brief ranks before it keeps
/// [`SESSION_BRIEF_EVENTS_MAX`]. An older event still counts toward the rest.
pub const SESSION_BRIEF_EVENTS_SCAN: i64 = 200;

/// Events a session brief counts toward its rest before it stops, so a brief
/// on a large project never counts the whole feed. A count at the cap is a
/// lower bound.
pub const SESSION_BRIEF_COUNT_MAX: i64 = 10_000;

/// Answers and decisions one session brief carries.
pub const SESSION_BRIEF_ANSWERS_MAX: usize = 10;

/// Stale knowledge base pages one session brief names.
pub const SESSION_BRIEF_STALE_PAGES_MAX: usize = 10;

/// Characters of one summary, answer or decision note a session brief
/// carries. The brief points at the record; the record holds the whole text.
pub const SESSION_BRIEF_TEXT_CHARS: usize = 200;

/// Maximum search page size.
pub const SEARCH_LIMIT_MAX: i64 = 100;

/// Maximum rows a filtered search reads before it stops, bounding the scan
/// while still reaching far past a page of matching hits.
pub const SEARCH_FETCH_MAX: i64 = 5000;

/// Resources one `resources/list` page returns.
///
/// A listing names every knowledge base page the caller may read, so it is
/// paged rather than cut: a fuller listing carries a cursor to the next page.
pub const RESOURCE_LIST_PAGE_MAX: usize = 100;

/// Default feed page size.
pub const FEED_LIMIT_DEFAULT: i64 = 50;

/// Ceilings on the open action items an agent may leave on the human.
///
/// An open item is one that still waits on the human: the inbox status is
/// `action` or `waiting`. A resolved item frees its slot. A cap of zero
/// disables that check, which is the operator's valve to turn the guard off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboxCaps {
    /// Open items one actor may hold in one project.
    pub per_actor: i64,
    /// Open items every actor together may hold in one project.
    pub per_project: i64,
}

impl InboxCaps {
    /// Default open items one actor may hold in one project.
    pub const DEFAULT_PER_ACTOR: i64 = 100;
    /// Default open items every actor together may hold in one project.
    pub const DEFAULT_PER_PROJECT: i64 = 1000;

    /// Caps that enforce nothing.
    pub const fn disabled() -> Self {
        Self {
            per_actor: 0,
            per_project: 0,
        }
    }

    /// Whether either check is live.
    pub fn enabled(&self) -> bool {
        self.per_actor > 0 || self.per_project > 0
    }

    /// Parse both caps from their raw environment values.
    ///
    /// Absent or empty values take the defaults; a negative value or one that
    /// is not a whole number is a startup error, so a typo fails loudly rather
    /// than silently disabling the guard.
    pub fn parse(per_actor: Option<&str>, per_project: Option<&str>) -> Result<Self> {
        Ok(Self {
            per_actor: parse_cap(
                "HUB_INBOX_ACTION_PER_AGENT",
                per_actor,
                Self::DEFAULT_PER_ACTOR,
            )?,
            per_project: parse_cap(
                "HUB_INBOX_ACTION_PER_PROJECT",
                per_project,
                Self::DEFAULT_PER_PROJECT,
            )?,
        })
    }
}

/// Ceiling on the events one project's feed holds.
///
/// The inbox cap bounds the open items waiting on the human; it says nothing
/// about ordinary signals, so a runaway agent can still fill the feed without
/// bound. The ceiling is checked in the event writer, in the same transaction
/// as the insert, and a cap of zero disables the check, the operator's valve,
/// matching [`InboxCaps`].
///
/// It bounds every writer on the agent surface: `signal_append`, questions and
/// answers, artifact publish and update, and the knowledge base's lifecycle
/// signal. The hub's own lifecycle (`session started`, `ended`, and the like)
/// and its audit trail (`system`) are exempt, so a full feed can never refuse
/// `session_start`; the knowledge base write itself is never refused, because
/// its signal is best-effort by contract and is dropped and logged when the
/// feed is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventCeiling {
    /// Events one project may hold, the hub's own audit trail excluded.
    pub per_project: i64,
}

impl EventCeiling {
    /// Default events one project may hold.
    pub const DEFAULT_PER_PROJECT: i64 = 1_000_000;

    /// A ceiling that enforces nothing.
    pub const fn disabled() -> Self {
        Self { per_project: 0 }
    }

    /// Whether the check is live.
    pub fn enabled(&self) -> bool {
        self.per_project > 0
    }

    /// Parse the ceiling from its raw environment value.
    ///
    /// Absent or empty values take the default; a negative value or one that
    /// is not a whole number is a startup error, so a typo fails loudly rather
    /// than silently disabling the guard.
    pub fn parse(value: Option<&str>) -> Result<Self> {
        Ok(Self {
            per_project: parse_cap("HUB_EVENTS_PER_PROJECT", value, Self::DEFAULT_PER_PROJECT)?,
        })
    }
}

fn parse_cap(name: &str, value: Option<&str>, default: i64) -> Result<i64> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(default);
    };
    let cap = value
        .parse::<i64>()
        .map_err(|_| Error::Config(format!("{name} is not a whole number: {value}")))?;
    if cap < 0 {
        return Err(Error::Config(format!(
            "{name} must be zero or more, got {cap}"
        )));
    }
    Ok(cap)
}

/// Reject an event whose summary or payload is over the cap.
pub fn check_event(summary: &str, payload_bytes: usize) -> Result<()> {
    if summary.chars().count() > EVENT_SUMMARY_CHARS_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "event summary exceeds {EVENT_SUMMARY_CHARS_MAX} characters"
        )));
    }
    if payload_bytes > EVENT_PAYLOAD_BYTES_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "event payload exceeds {EVENT_PAYLOAD_BYTES_MAX} bytes"
        )));
    }
    Ok(())
}

/// Reject a session brain value over the cap.
pub fn check_brain_value(bytes: usize) -> Result<()> {
    if bytes > BRAIN_VALUE_BYTES_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "brain value exceeds {BRAIN_VALUE_BYTES_MAX} bytes"
        )));
    }
    Ok(())
}

/// Reject a knowledge base page over the cap.
pub fn check_kb_page(bytes: usize) -> Result<()> {
    if bytes > KB_PAGE_BYTES_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "knowledge base page exceeds {KB_PAGE_BYTES_MAX} bytes"
        )));
    }
    Ok(())
}

/// Reject a knowledge base path that exceeds the cap.
pub fn check_kb_path(path: &str) -> Result<()> {
    if path.len() > KB_PATH_BYTES_MAX {
        return Err(Error::InvalidArgument(format!(
            "path exceeds {KB_PATH_BYTES_MAX} bytes"
        )));
    }
    Ok(())
}

/// Reject a write when the brain file has reached or exceeded the hard ceiling.
pub fn check_brain_file(file_bytes: i64) -> Result<()> {
    if file_bytes >= BRAIN_FILE_BYTES_HARD {
        return Err(Error::PayloadTooLarge(format!(
            "brain file exceeds {BRAIN_FILE_BYTES_HARD} bytes"
        )));
    }
    Ok(())
}

/// Reject a write when the brain file and incoming write would exceed the hard ceiling.
pub fn check_brain_file_projected(file_bytes: i64, incoming_bytes: usize) -> Result<()> {
    if file_bytes.saturating_add(incoming_bytes as i64) >= BRAIN_FILE_BYTES_HARD {
        return Err(Error::PayloadTooLarge(format!(
            "brain file projected size exceeds {BRAIN_FILE_BYTES_HARD} bytes"
        )));
    }
    Ok(())
}

/// The rows a knowledge base history page returns, from the limit a caller
/// named. A limit over the cap is refused rather than cut, so a caller never
/// mistakes a short page for the end of the history.
pub fn kb_history_rows(limit: Option<usize>) -> Result<usize> {
    let limit = limit.unwrap_or(KB_HISTORY_ROWS_DEFAULT);
    if limit > KB_HISTORY_ROWS_MAX {
        return Err(Error::InvalidArgument(format!(
            "limit is at most {KB_HISTORY_ROWS_MAX}"
        )));
    }
    Ok(limit)
}

/// Reject a handoff note over the cap.
pub fn check_handoff(handoff: &str) -> Result<()> {
    if handoff.chars().count() > HANDOFF_CHARS_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "handoff note exceeds {HANDOFF_CHARS_MAX} characters limit={HANDOFF_CHARS_MAX}"
        )));
    }
    Ok(())
}

/// Reject a decision note over the cap.
pub fn check_decision_note(note: &str) -> Result<()> {
    if note.chars().count() > DECISION_NOTE_CHARS_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "decision note exceeds {DECISION_NOTE_CHARS_MAX} characters limit={DECISION_NOTE_CHARS_MAX}"
        )));
    }
    Ok(())
}

/// The suggested answers of a question, trimmed, or the reason they are
/// refused.
///
/// Between [`QUESTION_OPTIONS_MIN`] and [`QUESTION_OPTIONS_MAX`] entries, each
/// one line of at most [`QUESTION_OPTION_CHARS_MAX`] characters once trimmed,
/// none blank and no two the same. Nothing is dropped or cut: a list the hub
/// would have to edit is refused whole, so the human is never offered an
/// answer the agent did not write.
pub fn check_question_options(options: &[String]) -> Result<Vec<String>> {
    if options.len() < QUESTION_OPTIONS_MIN || options.len() > QUESTION_OPTIONS_MAX {
        return Err(Error::InvalidArgument(format!(
            "options must hold {QUESTION_OPTIONS_MIN} to {QUESTION_OPTIONS_MAX} entries, got {}",
            options.len()
        )));
    }
    let mut trimmed: Vec<String> = Vec::with_capacity(options.len());
    for (index, option) in options.iter().enumerate() {
        let option = option.trim();
        if option.is_empty() {
            return Err(Error::InvalidArgument(format!("options[{index}] is blank")));
        }
        if option
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        {
            return Err(Error::InvalidArgument(format!(
                "options[{index}] must be one line of text"
            )));
        }
        let chars = option.chars().count();
        if chars > QUESTION_OPTION_CHARS_MAX {
            return Err(Error::InvalidArgument(format!(
                "options[{index}] is {chars} characters; the limit is {QUESTION_OPTION_CHARS_MAX}"
            )));
        }
        if trimmed.iter().any(|seen| seen == option) {
            return Err(Error::InvalidArgument(format!(
                "options[{index}] repeats \"{option}\""
            )));
        }
        trimmed.push(option.to_string());
    }
    Ok(trimmed)
}

/// Reject an artifact blob over the cap.
pub fn check_artifact(bytes: usize) -> Result<()> {
    if bytes > ARTIFACT_BYTES_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "artifact exceeds {ARTIFACT_BYTES_MAX} bytes"
        )));
    }
    Ok(())
}

/// The most terms one search query contributes. A query longer than this is a
/// paste, not a search, and the terms past it are left out.
pub const SEARCH_TERMS_MAX: usize = 64;

/// The most bytes of a made-safe search query handed to the engine, well under
/// the engine's own 16 KiB refusal so quoting can never push a query past it.
pub const SEARCH_QUERY_BYTES_MAX: usize = 8 * 1024;
