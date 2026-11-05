//! Shared Unicode-aware fuzzy ranking. The dashboard pickers and the history
//! search rank with one implementation so a query behaves the same everywhere.

/// True when the query fuzzy-matches the target at all.
pub fn matches(query: &str, target: &str) -> bool {
    fuzzy_score(query, target).is_some()
}

// Ranked bands, deliberately far apart so a band always beats a bonus.
pub(crate) const EXACT: usize = 100_000;
pub(crate) const PREFIX: usize = 60_000;
pub(crate) const WORD_START: usize = 45_000;
pub(crate) const SUBSTRING: usize = 30_000;
pub(crate) const SUBSEQUENCE: usize = 15_000;
/// Tight subsequence shaping: contiguous runs and word starts are rewarded,
/// skipped characters are penalised, so `code rev` outranks a scattered hit.
const ADJACENT: i64 = 120;
const WORD_BONUS: i64 = 60;
const GAP: i64 = 8;

/// Unicode case-insensitive fuzzy rank. `None` means the query characters do
/// not appear in order; otherwise a higher score is a better match: exact,
/// prefix, word start, contiguous substring, then a tight subsequence ranked
/// above a distant one. Matching is by character, never by byte.
pub fn fuzzy_score(query: &str, target: &str) -> Option<usize> {
    let target = target.to_lowercase();
    QueryScorer::new(query).score_lowercase(&target, &mut ScoreScratch::default())
}

/// One query prepared for repeated scoring. Pickers score a large candidate
/// list per keystroke, so normalizing the query once keeps case folding out of
/// the per-row path. Scores are identical to [`fuzzy_score`].
pub struct QueryScorer {
    chars: Vec<char>,
    text: String,
    /// Byte-level fast paths are only sound for an ASCII query: in UTF-8 an
    /// ASCII byte never hides inside a multi-byte character, so an ASCII query
    /// can be matched byte by byte, while any non-ASCII query stays on the
    /// character path.
    ascii_only: bool,
}

/// Scratch buffers reused across candidate rows while one query is scored.
#[derive(Default)]
pub struct ScoreScratch {
    chars: Vec<char>,
    dp: Vec<Option<i64>>,
    dp_next: Vec<Option<i64>>,
}

impl QueryScorer {
    pub fn new(query: &str) -> Self {
        let query = query.trim().to_lowercase();
        Self {
            ascii_only: query.is_ascii(),
            chars: query.chars().collect(),
            text: query,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// `target` must already be lowercased, as an indexed row's name and
    /// relative path are, so case folding never runs per row.
    pub fn score_lowercase(&self, target: &str, scratch: &mut ScoreScratch) -> Option<usize> {
        if self.is_empty() {
            return Some(0);
        }
        // Every query character must occur at all: a byte scan rejects most
        // rows before any allocation or dynamic programming happens.
        if self.ascii_only {
            for ch in self.text.chars() {
                if !target.contains(ch) {
                    return None;
                }
            }
        }
        let ScoreScratch {
            chars: target_chars,
            dp,
            dp_next,
        } = scratch;
        target_chars.clear();
        target_chars.extend(target.chars());
        if self.text == *target {
            return Some(EXACT);
        }
        if let Some(byte_at) = target.find(&self.text) {
            let at = target[..byte_at].chars().count();
            let band = if at == 0 {
                PREFIX
            } else if is_word_start(target_chars, at) {
                WORD_START
            } else {
                SUBSTRING
            };
            return Some(band.saturating_sub(at));
        }
        // Accept-only greedy pass on bytes: it accepts exactly when the table
        // below can find an alignment, so scoring is unchanged.
        if self.ascii_only && !contains_subsequence(target.as_bytes(), self.text.as_bytes()) {
            return None;
        }
        subsequence_score(&self.chars, target_chars, dp, dp_next)
            .map(|score| score.min(SUBSTRING - 1))
    }
}

/// True when every query byte appears in the target in order.
fn contains_subsequence(target: &[u8], query: &[u8]) -> bool {
    let mut wanted = 0;
    for byte in target {
        if *byte == query[wanted] {
            wanted += 1;
            if wanted == query.len() {
                return true;
            }
        }
    }
    false
}

fn is_word_start(target: &[char], at: usize) -> bool {
    match at.checked_sub(1).and_then(|index| target.get(index)) {
        None => true,
        Some(before) => !before.is_alphanumeric(),
    }
}

/// Best scoring subsequence alignment of `query` inside `target`.
fn word_bonus(target: &[char], at: usize) -> i64 {
    i64::from(is_word_start(target, at)) * WORD_BONUS
}

fn subsequence_score(
    query: &[char],
    target: &[char],
    layer: &mut Vec<Option<i64>>,
    next: &mut Vec<Option<i64>>,
) -> Option<usize> {
    // One cell per target position keeps every query update O(query * target).
    // Both layers are caller-owned so a scored batch never reallocates per row.
    layer.resize(target.len(), None);
    for (at, ch) in target.iter().enumerate() {
        layer[at] = (*ch == query[0]).then(|| SUBSEQUENCE as i64 + word_bonus(target, at));
    }
    for wanted in &query[1..] {
        // Reused buffers must not carry a previous row's or layer's cells.
        next.clear();
        next.resize(target.len(), None);
        let mut distant: Option<i64> = None;
        for at in 0..target.len() {
            distant = distant.map(|value| value - GAP);
            if at >= 2 {
                if let Some(value) = layer[at - 2] {
                    let candidate = value - GAP;
                    distant = Some(distant.map_or(candidate, |best| best.max(candidate)));
                }
            }
            let adjacent = at
                .checked_sub(1)
                .and_then(|before| layer[before])
                .map(|value| value + ADJACENT);
            let best = match (distant, adjacent) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (Some(value), None) | (None, Some(value)) => Some(value),
                _ => None,
            };
            if target[at] == *wanted {
                next[at] = best.map(|value| value + word_bonus(target, at));
            }
        }
        if next.iter().all(Option::is_none) {
            return None;
        }
        std::mem::swap(layer, next);
    }
    layer
        .iter()
        .copied()
        .flatten()
        .max()
        .map(|value| value.max(1) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_score_ranks_exact_prefix_word_start_and_tight_matches() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        let exact = fuzzy_score("code-reviewer", "code-reviewer").unwrap();
        let prefix = fuzzy_score("code-rev", "code-reviewer").unwrap();
        let word = fuzzy_score("rev", "code-reviewer").unwrap();
        let inside = fuzzy_score("de-re", "code-reviewer").unwrap();
        let tight = fuzzy_score("cdrev", "code-reviewer").unwrap();
        let distant = fuzzy_score("crer", "cold-drink-waterfall").unwrap();
        assert!(
            exact > prefix && prefix > word && word > inside && inside > tight,
            "{exact} {prefix} {word} {inside} {tight}"
        );
        // A tight subsequence beats a scattered one, and both still match.
        let near = fuzzy_score("coderev", "code_reviewer_app").unwrap();
        assert!(near > distant, "{near} {distant}");
        assert!(tight > distant, "{tight} {distant}");
        // Case folds are Unicode-aware and matching never looks inside a char.
        assert!(fuzzy_score("CODE", "code-reviewer").unwrap() >= prefix / 2);
        assert!(fuzzy_score("ünï", "Ünïcödé-app").is_some());
        assert!(fuzzy_score("CÖD", "ünïcödé-app").is_some());
        assert_eq!(fuzzy_score("zz", "code-reviewer"), None);
        assert_eq!(fuzzy_score("🙂x-app", "🙂x-app"), Some(EXACT));
        assert_eq!(fuzzy_score("🙂x", "🙂x-app"), Some(PREFIX));
        assert_eq!(fuzzy_score("uni", "ünïcödé-app"), None);
        assert!(matches("cdwr", "code-reviewer"));
    }
}
