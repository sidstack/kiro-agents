//! Bounded local search over the discovered native conversations.
//!
//! There is no external search service and no persistent index: one in-memory
//! pass over Kiro's own stores answers a query. Conversation text is only ever
//! read from schema-aware adapters, never from terminal traffic, and snippets
//! are sanitized before a terminal sees them.

use super::{Conversation, LockState, StoreKind, TextRole};
use crate::fuzzy::{fuzzy_score, SUBSEQUENCE, SUBSTRING, WORD_START};
use crate::kiro::history::resume::{self, Decision};

/// Results kept for one query; the board renders only its viewport.
pub const MAX_RESULTS: usize = 50;
/// Characters of context kept on each side of a text match.
const SNIPPET_WINDOW: usize = 80;
/// Longest sanitized snippet the board may render.
const MAX_SNIPPET_CHARS: usize = 200;
/// Fuzzy matching is only tried against text this short, so a huge transcript
/// cannot make typing slow.
const MAX_FUZZY_TEXT_CHARS: usize = 400;

// Field bonuses stay far below the fuzzy band gaps in crate::fuzzy, so a
// better band always wins and the field only breaks ties inside a band.
const TITLE_BONUS: usize = 8_000;
const PROJECT_BONUS: usize = 5_000;
const AGENT_BONUS: usize = 3_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub role: TextRole,
    pub text: String,
    /// True when the text was replaced because it looked like a credential.
    pub redacted: bool,
}

/// One selectable search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub native_id: String,
    pub label: String,
    pub project: String,
    pub cwd: Option<String>,
    pub agent: Option<String>,
    pub stores: Vec<StoreKind>,
    pub lock: LockState,
    pub decision: Decision,
    /// Wrapper session that already supervises this conversation, when one of
    /// our own live sessions holds it.
    pub owned_by: Option<String>,
    pub snippet: Option<Snippet>,
    pub score: usize,
    pub notice: Option<String>,
    pub updated_at_ms: Option<u64>,
}

impl Hit {
    pub fn store_label(&self) -> String {
        self.stores
            .iter()
            .map(|store| store.label())
            .collect::<Vec<_>>()
            .join("+")
    }
}

/// Rank every conversation against `query`. Every whitespace-separated term
/// has to match somewhere (phrase/word semantics); an empty query lists the
/// most recently updated conversations.
pub fn search(records: &[Conversation], query: &str, owned: &[(String, String)]) -> Vec<Hit> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_owned).collect();
    let mut hits: Vec<Hit> = Vec::new();
    for record in records {
        let score = if terms.is_empty() {
            0
        } else {
            let mut total = 0usize;
            let mut matched = true;
            for term in &terms {
                match best_score(record, term) {
                    Some(value) => total += value,
                    None => {
                        matched = false;
                        break;
                    }
                }
            }
            if matched {
                total
            } else {
                continue;
            }
        };
        hits.push(hit(record, score, owned));
    }
    hits.sort_by(|a, b| {
        // Better score, then newer, then the cleaner row: a stale-lock notice
        // must not outrank an identical-scoring conversation without one.
        (b.score, b.updated_at_ms.unwrap_or(0), b.notice.is_none())
            .cmp(&(a.score, a.updated_at_ms.unwrap_or(0), a.notice.is_none()))
            .then_with(|| a.label.cmp(&b.label))
    });
    hits.truncate(MAX_RESULTS);
    // Snippets are only produced for the returned window, never for the whole
    // corpus, and only when the query can point at a match.
    if let Some(term) = query.split_whitespace().next() {
        for hit in &mut hits {
            if let Some(record) = records.iter().find(|row| row.native_id == hit.native_id) {
                hit.snippet = snippet_for(record, term);
            }
        }
    }
    hits
}

fn hit(record: &Conversation, score: usize, owned: &[(String, String)]) -> Hit {
    Hit {
        native_id: record.native_id.clone(),
        label: record.label(),
        project: record.project(),
        cwd: record.cwd.clone(),
        agent: record.agent.clone(),
        stores: record.stores.clone(),
        lock: record.lock,
        decision: resume::decide(record),
        owned_by: owned
            .iter()
            .find(|(native_id, _)| native_id == &record.native_id)
            .map(|(_, session)| session.clone()),
        snippet: None,
        score,
        notice: resume::notice(record),
        updated_at_ms: record.updated_at_ms,
    }
}

/// Best score for one term across title, project, agent, id and text.
fn best_score(record: &Conversation, term: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut consider = |candidate: Option<usize>| {
        if let Some(candidate) = candidate {
            best = Some(best.map_or(candidate, |current| current.max(candidate)));
        }
    };
    consider(
        record
            .title
            .as_deref()
            .and_then(|title| fuzzy_score(term, title))
            .map(|band| band + TITLE_BONUS),
    );
    consider(fuzzy_score(term, &record.project()).map(|band| band + PROJECT_BONUS));
    consider(
        record
            .cwd
            .as_deref()
            .and_then(|cwd| fuzzy_score(term, cwd))
            .map(|band| band + PROJECT_BONUS),
    );
    consider(
        record
            .agent
            .as_deref()
            .and_then(|agent| fuzzy_score(term, agent))
            .map(|band| band + AGENT_BONUS),
    );
    consider(fuzzy_score(term, &record.native_id));
    for run in &record.text {
        consider(text_score(term, &run.text));
    }
    best
}

/// Full-text terms match literally, with a word-start bonus. Approximate
/// matching is tried only against short runs so typing stays responsive.
fn text_score(term: &str, text: &str) -> Option<usize> {
    if let Some(at) = find(text, term) {
        let band = if is_word_start(text, at) {
            WORD_START
        } else {
            SUBSTRING
        };
        return Some(band.saturating_sub(at / 8));
    }
    if text.chars().count() <= MAX_FUZZY_TEXT_CHARS {
        return fuzzy_score(term, text).map(|band| band.min(SUBSEQUENCE));
    }
    None
}

/// Case-insensitive search returning a byte offset into `text` itself.
///
/// Case folding is not byte preserving: `to_lowercase` can change length, so a
/// match offset inside the lowered text is not an offset into the original and
/// slicing the original at it can land inside a character and panic (U+0130
/// "İ" folds to "i" plus a combining dot, two characters and three bytes from
/// two). Every folded character therefore remembers the byte offset of the
/// character it came from, and the returned offset is always a character
/// boundary of `text`.
fn find(text: &str, term: &str) -> Option<usize> {
    if term.is_empty() {
        return Some(0);
    }
    CaseFolded::new(text).find(term)
}

/// A lowercased view of a string that maps each folded character back to the
/// byte offset of the source character it was produced from.
struct CaseFolded {
    lowered: String,
    /// One entry per character of `lowered`.
    source: Vec<usize>,
}

impl CaseFolded {
    fn new(text: &str) -> Self {
        let mut lowered = String::with_capacity(text.len());
        let mut source = Vec::new();
        for (at, ch) in text.char_indices() {
            for folded in ch.to_lowercase() {
                lowered.push(folded);
                source.push(at);
            }
        }
        Self { lowered, source }
    }

    fn find(&self, term: &str) -> Option<usize> {
        let byte_at = self.lowered.find(&term.to_lowercase())?;
        // A byte offset in the lowered view is a character boundary of that
        // view, so counting to it is safe; the mapped source offset is a
        // boundary of the original text.
        let folded_at = self.lowered[..byte_at].chars().count();
        // A fold can expand, so a term may start inside a folded sequence; the
        // whole source character is then the match, which stays honest for the
        // word-start test and the snippet window.
        self.source.get(folded_at).copied()
    }
}

fn is_word_start(text: &str, at: usize) -> bool {
    let before = text[..at].chars().next_back();
    match before {
        None => true,
        Some(ch) => !ch.is_alphanumeric(),
    }
}

/// Bounded context around a term, sanitized for terminal display. Conversation
/// text is never logged or stored by the wrapper; only this window is shown,
/// and credential-looking text is replaced rather than echoed.
pub fn snippet_for(record: &Conversation, query: &str) -> Option<Snippet> {
    let term = query.split_whitespace().next()?;
    let run = record
        .text
        .iter()
        .find(|run| find(&run.text, term).is_some())?;
    let at = find(&run.text, term)?;
    let chars: Vec<char> = run.text.chars().collect();
    // `find` returns an offset into this same text, so slicing at it is safe.
    let char_at = run.text[..at].chars().count();
    let start = char_at.saturating_sub(SNIPPET_WINDOW);
    let end = (char_at + term.chars().count() + SNIPPET_WINDOW).min(chars.len());
    let window: String = chars[start..end].iter().collect();
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < chars.len() { "…" } else { "" };
    let text = format!("{prefix}{window}{suffix}");
    // Conversation text is potentially sensitive: control characters are
    // stripped before a terminal sees it, whitespace is collapsed, the window
    // is bounded, and credential-looking text is replaced instead of echoed.
    let sanitized = if crate::features::looks_sensitive(&text) {
        None
    } else {
        let printable: String = text.chars().filter(|ch| !ch.is_control()).collect();
        let collapsed = printable.split_whitespace().collect::<Vec<_>>().join(" ");
        let bounded: String = collapsed.chars().take(MAX_SNIPPET_CHARS).collect();
        (!bounded.is_empty()).then_some(bounded)
    };
    match sanitized {
        Some(text) => Some(Snippet {
            role: run.role,
            text,
            redacted: false,
        }),
        None => Some(Snippet {
            role: run.role,
            text: "[content hidden: it looked like a credential]".into(),
            redacted: true,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::history::{TextRole, TextRun};

    fn record(
        id: &str,
        title: &str,
        cwd: &str,
        agent: Option<&str>,
        prompts: &[(&str, &str)],
        updated: u64,
    ) -> Conversation {
        Conversation {
            native_id: id.into(),
            stores: vec![StoreKind::Files],
            title: Some(title.into()),
            cwd: Some(cwd.into()),
            agent: agent.map(str::to_owned),
            updated_at_ms: Some(updated),
            lock: LockState::Free,
            text: prompts
                .iter()
                .map(|(role, text)| TextRun {
                    role: match *role {
                        "prompt" => TextRole::Prompt,
                        _ => TextRole::Response,
                    },
                    text: (*text).into(),
                })
                .collect(),
            transcript: true,
            unusable: None,
        }
    }

    fn corpus() -> Vec<Conversation> {
        vec![
            record(
                "11111111-1111-4111-8111-111111111111",
                "Repair fuzzy picker ranking",
                "/home/siddu/Projects/kiro-agents",
                Some("kiro_default"),
                &[("prompt", "ranking feels wrong for subsequence matches")],
                30,
            ),
            record(
                "22222222-2222-4222-8222-222222222222",
                "Ünïcödé titles and wheel scrolling",
                "/home/siddu/Projects/Ünïcödé-app",
                None,
                &[
                    ("prompt", "the wheel scrolls tmux instead of the prompt"),
                    ("response", "tmux copy mode now owns the wheel"),
                ],
                20,
            ),
            record(
                "33333333-3333-4333-8333-333333333333",
                "Deploy notes",
                "/tmp/deploy",
                None,
                &[("prompt", "ship the beta binary after the audit")],
                10,
            ),
        ]
    }

    fn ids(hits: &[Hit]) -> Vec<String> {
        hits.iter().map(|hit| hit.native_id.clone()).collect()
    }

    #[test]
    fn a_stale_lock_notice_does_not_outrank_an_identical_clean_row() {
        let mut corpus = corpus();
        // Same score (empty query), same recency; only the lock differs.
        for record in &mut corpus {
            record.updated_at_ms = Some(30);
        }
        corpus.retain(|record| record.title.as_deref() != Some("Deploy notes"));
        corpus[0].lock = LockState::Stale { pid: 42 };
        let hits = search(&corpus, "", &[]);
        assert_eq!(hits[0].native_id, "22222222-2222-4222-8222-222222222222");
        assert!(hits[0].notice.is_none());
        assert!(hits[1].notice.is_some());
    }

    #[test]
    fn an_empty_query_lists_every_conversation_newest_first() {
        let hits = search(&corpus(), "  ", &[]);
        assert_eq!(hits.len(), 3);
        assert_eq!(
            ids(&hits),
            [
                "11111111-1111-4111-8111-111111111111",
                "22222222-2222-4222-8222-222222222222",
                "33333333-3333-4333-8333-333333333333"
            ]
        );
    }

    #[test]
    fn exact_title_beats_a_word_hit_beats_an_approximate_match() {
        let hits = search(&corpus(), "ranking", &[]);
        assert_eq!(ids(&hits)[0], "11111111-1111-4111-8111-111111111111");
        // An approximate match still surfaces, but below the literal one.
        let hits = search(&corpus(), "rankng", &[]);
        assert_eq!(ids(&hits)[0], "11111111-1111-4111-8111-111111111111");
        assert!(hits[0].score < SUBSTRING, "{:?}", hits[0].score);
    }

    #[test]
    fn full_text_words_match_prompts_and_assistant_text() {
        let hits = search(&corpus(), "copy", &[]);
        assert_eq!(ids(&hits), ["22222222-2222-4222-8222-222222222222"]);
        assert_eq!(hits[0].snippet.as_ref().unwrap().role, TextRole::Response);
        let hits = search(&corpus(), "beta", &[]);
        assert_eq!(ids(&hits), ["33333333-3333-4333-8333-333333333333"]);
        assert_eq!(hits[0].snippet.as_ref().unwrap().role, TextRole::Prompt);
    }

    #[test]
    fn every_term_must_match_and_matching_keeps_beating_filtering() {
        let hits = search(&corpus(), "wheel tmux", &[]);
        assert_eq!(ids(&hits), ["22222222-2222-4222-8222-222222222222"]);
        assert!(search(&corpus(), "wheel deploy", &[]).is_empty());
    }

    #[test]
    fn unicode_and_case_are_folded_the_same_way_everywhere() {
        let hits = search(&corpus(), "ünï", &[]);
        assert_eq!(ids(&hits), ["22222222-2222-4222-8222-222222222222"]);
        let hits = search(&corpus(), "UNÏCÖDÉ", &[]);
        assert_eq!(ids(&hits), ["22222222-2222-4222-8222-222222222222"]);
    }

    #[test]
    fn project_agent_and_identifier_fields_are_all_searchable() {
        assert_eq!(
            ids(&search(&corpus(), "kiro-agents", &[])),
            ["11111111-1111-4111-8111-111111111111"]
        );
        assert_eq!(
            ids(&search(&corpus(), "kiro_default", &[])),
            ["11111111-1111-4111-8111-111111111111"]
        );
        assert_eq!(
            ids(&search(&corpus(), "33333333", &[])),
            ["33333333-3333-4333-8333-333333333333"]
        );
    }

    #[test]
    fn snippets_are_bounded_control_free_and_hide_credentials() {
        let long = format!("{} secret answer {}", "padding ".repeat(200), "needle");
        let lengthy = record(
            "44444444-4444-4444-8444-444444444444",
            "Long transcript",
            "/tmp",
            None,
            &[("prompt", &long)],
            1,
        );
        let snippet = snippet_for(&lengthy, "needle").unwrap();
        assert!(
            snippet.text.chars().count() <= MAX_SNIPPET_CHARS,
            "{}",
            snippet.text
        );
        assert!(snippet.text.chars().all(|ch| !ch.is_control()));
        assert!(snippet.text.contains("needle"), "{}", snippet.text);
        // Only a bounded window around the match is kept, not the transcript.
        assert!(
            !snippet.text.contains(&"padding ".repeat(20)),
            "{}",
            snippet.text
        );

        let leaky = record(
            "55555555-5555-4555-8555-555555555555",
            "Leaky",
            "/tmp",
            None,
            &[("prompt", "paste ghp_supersecrettoken to continue needle")],
            1,
        );
        let snippet = snippet_for(&leaky, "needle").unwrap();
        assert!(snippet.redacted);
        assert!(!snippet.text.contains("ghp_"), "{}", snippet.text);

        let escape = record(
            "66666666-6666-4666-8666-666666666666",
            "Escape artist",
            "/tmp",
            None,
            &[("prompt", "before\u{1b}[31mneedle after")],
            1,
        );
        let snippet = snippet_for(&escape, "needle").unwrap();
        assert!(!snippet.text.contains('\u{1b}'), "{:?}", snippet.text);
    }

    #[test]
    fn a_fold_that_grows_never_slices_the_original_text_mid_character() {
        // U+0130 lowercases to two characters, so the lowered text is a byte
        // longer than the source. Slicing the source at the lowered offset
        // used to land inside "é" and panic while building the snippet.
        assert_eq!(find("İ café", "é"), Some(6));
        // The neighbour the band decision sees is the "f" of café, read from
        // the original text rather than a byte past a character boundary.
        assert!(!is_word_start("İ café", find("İ café", "é").unwrap()));
        // A byte offset from the lowered view pointed two bytes too far ahead.
        assert_eq!(find("İ İstanbul needle", "NEEDLE"), Some(13));

        let row = record(
            "88888888-8888-4888-8888-888888888888",
            "İstanbul",
            "/tmp",
            None,
            &[("prompt", "İ café and a résumé")],
            1,
        );
        let snippet = snippet_for(&row, "é").unwrap();
        assert!(snippet.text.contains("café"), "{}", snippet.text);
    }

    #[test]
    fn every_text_match_offset_is_a_character_boundary_of_the_text() {
        let texts = [
            "İ café and a résumé",
            "İ😀needle in a transcript",
            "ẛǅﬁ needle tail",
            "İ\u{1b}[31méx needle",
            "İéİéİé needle",
            "🏳️‍🌈 ünïcödé needle",
        ];
        let terms = ["needle", "NEEDLE", "é", "İ", "caf", "ünï", "n"];
        for text in texts {
            for term in terms {
                let Some(at) = find(text, term) else { continue };
                assert!(text.is_char_boundary(at), "{text:?} {term:?} {at}");
                // The offset really points at the match, so the word-start
                // test and the snippet window look at the right neighbour.
                assert!(
                    text[at..].to_lowercase().starts_with(&term.to_lowercase()),
                    "{text:?} {term:?} {at} -> {:?}",
                    &text[at..]
                );
            }
        }
    }

    #[test]
    fn the_word_start_band_reads_the_original_text_not_the_folded_one() {
        // Both offsets are inside the original text now, so the character
        // before the match is the one the band decision considers, and slicing
        // to it never lands mid-character (the "é" case used to panic here).
        assert_eq!(text_score("needle", "a needle here"), Some(WORD_START));
        assert_eq!(text_score("needle", "aİneedle here"), Some(SUBSTRING));
        // The lowered text is a byte longer, so the old offset pointed inside
        // "and" and lost the word start that the original text has.
        assert_eq!(text_score("and", "İ and"), Some(WORD_START));
        assert_eq!(text_score("é", "İ café"), Some(SUBSTRING));
    }

    #[test]
    fn snippets_stay_centred_on_the_real_match_in_unicode_text() {
        let body = format!("{}İ😀needle{}", "ünïcödé ".repeat(60), " tail".repeat(40));
        let row = record(
            "99999999-9999-4999-8999-999999999999",
            "Unicode transcript",
            "/tmp",
            None,
            &[("prompt", &body)],
            1,
        );
        let snippet = snippet_for(&row, "needle").unwrap();
        assert!(snippet.text.contains('İ'), "{}", snippet.text);
        assert!(snippet.text.contains("needle"), "{}", snippet.text);
        assert!(snippet.text.starts_with('…'));
        assert!(snippet.text.chars().count() <= MAX_SNIPPET_CHARS + 2);
    }

    #[test]
    fn live_sessions_keep_their_resume_block_reason_on_the_row() {
        let mut live = record(
            "77777777-7777-4777-8777-777777777777",
            "Already open work",
            "/tmp",
            None,
            &[("prompt", "open conversation")],
            5,
        );
        live.lock = LockState::Open { pid: 4242 };
        let records = vec![live];
        let hits = search(
            &records,
            "open",
            &[(
                "77777777-7777-4777-8777-777777777777".into(),
                "wrap-9".into(),
            )],
        );
        assert_eq!(hits[0].owned_by.as_deref(), Some("wrap-9"));
        assert!(hits[0]
            .decision
            .reason()
            .unwrap()
            .contains("already has this conversation open"));
    }

    #[test]
    fn results_are_capped_and_never_include_non_matching_rows() {
        let many: Vec<Conversation> = (0..(MAX_RESULTS + 25))
            .map(|index| {
                record(
                    &format!("{:08}-0000-4000-8000-000000000000", 0x1000 + index),
                    "Session title",
                    "/tmp",
                    None,
                    &[("prompt", "common word")],
                    index as u64,
                )
            })
            .collect();
        let hits = search(&many, "common", &[]);
        assert_eq!(hits.len(), MAX_RESULTS);
        assert!(search(&many, "never-written", &[]).is_empty());
    }
}
