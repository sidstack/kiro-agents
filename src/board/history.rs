//! Quick-open picker over Kiro's own conversation stores.
//!
//! The scan runs once per open (Ctrl+R rescans) and typing only re-ranks the
//! rows already in memory, so editing never waits on the filesystem. Every row
//! says which store it came from and whether its text was actually read.

use crate::kiro::history::{self, Conversation, Hit, StoreKind};
use crate::protocol::SessionInfo;
use std::time::Duration;

/// How long one store scan may run before it reports partial coverage.
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(10);
/// What a scan actually covered, so the board never implies more than it read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    pub stores: Vec<StoreKind>,
    pub conversations: usize,
    pub unrecognised: usize,
    pub unreadable: usize,
    pub metadata_only: usize,
    pub partial: bool,
    pub truncated: bool,
}

#[derive(Debug, Default)]
pub struct HistoryPicker {
    pub query: String,
    pub records: Vec<Conversation>,
    pub hits: Vec<Hit>,
    pub selected: usize,
    pub scanning: bool,
    pub coverage: Coverage,
}

impl HistoryPicker {
    /// Rank the loaded rows against the current query. `rows` are the live
    /// wrapper sessions, used to recognise a conversation we already supervise.
    pub fn refresh(&mut self, rows: &[SessionInfo]) -> bool {
        let owned: Vec<(String, String)> = rows
            .iter()
            .filter_map(|row| {
                row.native_id
                    .as_ref()
                    .map(|native_id| (native_id.clone(), row.id.clone()))
            })
            .collect();
        let hits = history::search(&self.records, &self.query, &owned);
        let changed = hits != self.hits;
        self.selected = if self.selected < hits.len() {
            self.selected
        } else {
            0
        };
        self.hits = hits;
        changed
    }

    pub fn apply(&mut self, scan: history::HistoryScan, rows: &[SessionInfo]) -> bool {
        self.scanning = false;
        self.coverage = Coverage {
            stores: scan.result.stores,
            conversations: scan.result.conversations.len(),
            unrecognised: scan.result.unrecognised,
            unreadable: scan.result.unreadable,
            metadata_only: scan
                .result
                .conversations
                .iter()
                .filter(|record| !record.transcript)
                .count(),
            partial: scan.result.partial,
            truncated: scan.result.truncated,
        };
        self.records = scan.result.conversations;
        self.refresh(rows)
    }

    pub fn press(&mut self, ch: char, rows: &[SessionInfo]) {
        if self.query.chars().count() < 120 {
            self.query.push(ch);
            self.selected = 0;
            self.refresh(rows);
        }
    }

    pub fn backspace(&mut self, rows: &[SessionInfo]) {
        self.query.pop();
        self.selected = 0;
        self.refresh(rows);
    }

    pub fn clear_query(&mut self, rows: &[SessionInfo]) {
        self.query.clear();
        self.selected = 0;
        self.refresh(rows);
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.hits.is_empty() {
            return;
        }
        let len = self.hits.len() as i64;
        self.selected = ((self.selected as i64 + i64::from(delta)).rem_euclid(len)) as usize;
    }

    pub fn selected_hit(&self) -> Option<&Hit> {
        self.hits.get(self.selected)
    }

    /// Status line under the rows: coverage first, then the selected row's
    /// resume state. A blocked row always states why.
    pub fn notice(&self) -> String {
        if self.scanning {
            return "Reading Kiro's own session stores (read-only)…".to_owned();
        }
        if self.coverage.stores.is_empty() {
            return "No native Kiro session store was found, so there is nothing to search. \
                    Set KIRO_HOME/KIRO_SESSIONS_DIR or start a session first."
                .to_owned();
        }
        let mut parts: Vec<String> = Vec::new();
        if self.hits.is_empty() && !self.records.is_empty() {
            parts.push(format!(
                "No conversation matches \"{}\" in {} rows.",
                self.query,
                self.records.len()
            ));
        }
        if let Some(hit) = self.selected_hit() {
            parts.push(match hit.decision.reason() {
                None => format!(
                    "Enter resumes in {}{}",
                    hit.cwd.clone().unwrap_or_default(),
                    hit.notice
                        .as_deref()
                        .map_or_else(String::new, |notice| format!(" · {notice}"))
                ),
                Some(reason) => reason.to_owned(),
            });
        }
        if let Some(hit) = self.selected_hit().filter(|hit| hit.owned_by.is_some()) {
            parts.push(format!(
                "Already supervised as session {}; Enter attaches that row.",
                hit.owned_by.clone().unwrap_or_default()
            ));
        }
        let stores = self
            .coverage
            .stores
            .iter()
            .map(|store| store.label())
            .collect::<Vec<_>>()
            .join("+");
        parts.push(format!(
            "Searched {} store(s) · {} rows · {} unrecognised · {} without conversation text",
            stores,
            self.coverage.conversations,
            self.coverage.unrecognised,
            self.coverage.metadata_only
        ));
        if self.coverage.partial || self.coverage.truncated || self.coverage.unreadable > 0 {
            parts.push(
                "Coverage is partial: the scan was bounded, so a complete search is not claimed."
                    .into(),
            );
        }
        parts.join("\n")
    }
}

/// The store label pair a row shows: provenance plus lock state.
pub fn row_state(hit: &Hit) -> String {
    let lock = match hit.lock {
        history::LockState::Open { pid } => format!("open pid {pid}"),
        history::LockState::Stale { .. } => "stale lock".into(),
        history::LockState::Unknown => "lock unknown".into(),
        history::LockState::Free => "closed".into(),
    };
    format!("{} · {lock}", store_labels(&hit.stores))
}

pub fn store_labels(stores: &[StoreKind]) -> String {
    stores
        .iter()
        .map(|store| store.label())
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::history::{LockState, TextRole, TextRun};

    fn record(id: &str, title: &str, updated: u64) -> Conversation {
        Conversation {
            native_id: id.into(),
            stores: vec![StoreKind::Files],
            title: Some(title.into()),
            cwd: Some("/tmp/project".into()),
            agent: None,
            updated_at_ms: Some(updated),
            lock: LockState::Free,
            text: vec![TextRun {
                role: TextRole::Prompt,
                text: format!("talk about {title}"),
            }],
            transcript: true,
            unusable: None,
        }
    }

    fn rows(native: Option<&str>, id: &str) -> Vec<SessionInfo> {
        vec![SessionInfo {
            name: None,
            title: None,
            title_source: None,
            native_id: native.map(str::to_owned),
            id: id.into(),
            cwd: "/tmp/project".into(),
            agent: None,
            generation: "v2".into(),
            pid: None,
            state: "live".into(),
            exit_code: None,
            notice: None,
        }]
    }

    fn picker() -> HistoryPicker {
        let records = vec![
            record(
                "11111111-1111-4111-8111-111111111111",
                "Repair the picker",
                30,
            ),
            record(
                "22222222-2222-4222-8222-222222222222",
                "Ünïcödé scrolling",
                20,
            ),
        ];
        let coverage = Coverage {
            stores: vec![StoreKind::Files],
            conversations: 2,
            ..Coverage::default()
        };
        let mut picker = HistoryPicker {
            records,
            coverage,
            ..HistoryPicker::default()
        };
        picker.refresh(&[]);
        picker
    }

    #[test]
    fn typing_narrows_without_losing_the_query_and_enter_takes_the_top_row() {
        let mut picker = picker();
        assert_eq!(picker.hits.len(), 2);
        for ch in "ünï".chars() {
            picker.press(ch, &[]);
        }
        assert_eq!(picker.query, "ünï");
        assert_eq!(picker.hits.len(), 1);
        assert_eq!(picker.selected, 0);
        assert_eq!(
            picker.selected_hit().unwrap().native_id,
            "22222222-2222-4222-8222-222222222222"
        );
        // Backspace keeps editing whole characters and re-ranks immediately.
        picker.backspace(&[]);
        assert_eq!(picker.query, "ün");
        assert_eq!(picker.hits.len(), 1);
    }

    #[test]
    fn a_conversation_our_live_session_holds_is_not_resumed_again() {
        let mut picker = picker();
        picker.records[0].lock = LockState::Open { pid: 4242 };
        picker.refresh(&rows(
            Some("11111111-1111-4111-8111-111111111111"),
            "wrap-1",
        ));
        let hit = picker
            .hits
            .iter()
            .find(|hit| hit.native_id == "11111111-1111-4111-8111-111111111111")
            .unwrap();
        assert_eq!(hit.owned_by.as_deref(), Some("wrap-1"));
        assert!(hit
            .decision
            .reason()
            .unwrap()
            .contains("already has this conversation"));
        assert!(picker
            .notice()
            .contains("Already supervised as session wrap-1"));
    }

    #[test]
    fn selection_wraps_and_an_empty_result_stays_empty() {
        let mut picker = picker();
        picker.move_selection(-1);
        assert_eq!(picker.selected, 1);
        picker.move_selection(1);
        assert_eq!(picker.selected, 0);
        picker.press('z', &[]);
        assert!(picker.hits.is_empty());
        assert!(picker.selected_hit().is_none());
        picker.move_selection(5);
        assert_eq!(picker.selected, 0);
        assert!(picker.notice().contains("No conversation matches"));
    }

    #[test]
    fn coverage_and_bounded_scans_are_reported_rather_than_hidden() {
        let mut picker = picker();
        assert!(picker.notice().contains("Searched v1 store(s) · 2 rows"));
        picker.coverage.partial = true;
        picker.coverage.metadata_only = 1;
        assert!(picker.notice().contains("Coverage is partial"));
        picker.coverage.stores.clear();
        assert!(picker.notice().contains("No native Kiro session store"));
        picker.scanning = true;
        assert!(picker.notice().contains("read-only"));
    }

    #[test]
    fn rows_name_their_store_and_lock_state() {
        let mut picker = picker();
        picker.records[0].stores.push(StoreKind::Workspaces);
        picker.records[1].lock = LockState::Unknown;
        picker.refresh(&[]);
        let states: Vec<String> = picker.hits.iter().map(row_state).collect();
        assert!(states.contains(&"v1+v2 · closed".to_owned()), "{states:?}");
        assert!(
            states.contains(&"v1 · lock unknown".to_owned()),
            "{states:?}"
        );
    }
}
