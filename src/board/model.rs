use crate::protocol::SessionInfo;

use crate::lifecycle::Filter;

/// Filter and selection state for the session board rows received from the host.
#[derive(Debug, Clone, Default)]
pub struct Board {
    pub rows: Vec<SessionInfo>,
    pub filter: Filter,
    pub query: String,
    selected: Option<String>,
}

impl Board {
    /// Replace the host snapshot and re-anchor the selection to a visible row.
    pub fn replace_rows(&mut self, rows: Vec<SessionInfo>) {
        self.rows = rows;
        self.reanchor();
    }

    /// Row indices that pass the activity filter and the search query.
    pub fn visible_indices(&self) -> Vec<usize> {
        let query = self.query.to_lowercase();
        let mut rows: Vec<_> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| self.matches(row, &query))
            .map(|(index, _)| index)
            .collect();
        if !query.is_empty() {
            rows.sort_by_key(|&index| {
                let row = &self.rows[index];
                let label = row.display_name().to_lowercase();
                let rank = if label == query {
                    0
                } else if label.starts_with(&query) {
                    1
                } else if label.contains(&query) {
                    2
                } else {
                    3
                };
                (rank, index)
            });
        }
        rows
    }

    /// The selected row, or None when nothing is selected or visible.
    pub fn selected(&self) -> Option<&SessionInfo> {
        let id = self.selected.as_deref()?;
        self.rows.iter().find(|row| row.id == id)
    }

    /// Select a known row while retaining the current visibility rules.
    pub fn select_id(&mut self, id: &str) {
        self.selected = Some(id.to_owned());
        self.reanchor();
    }

    /// Move the selection by `delta` visible rows, wrapping in both directions.
    pub fn move_selection(&mut self, delta: i32) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            self.selected = None;
            return;
        }
        let len = visible.len() as i64;
        let current = visible
            .iter()
            .position(|&index| Some(self.rows[index].id.as_str()) == self.selected_id());
        let base = current
            .map(|index| index as i64)
            .unwrap_or(if delta < 0 { 0 } else { -1 });
        let next = (base + i64::from(delta)).rem_euclid(len);
        self.selected = Some(self.rows[visible[next as usize]].id.clone());
    }

    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.selected = None;
        self.reanchor();
    }

    pub fn cycle_filter(&mut self, backwards: bool) {
        self.filter = self.filter.next(backwards);
        self.reanchor();
    }

    fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    fn matches(&self, row: &SessionInfo, query: &str) -> bool {
        if !self.filter.matches(row.state == "live", row.activity) {
            return false;
        }
        if query.is_empty() {
            return true;
        }
        let label = row.display_name();
        let matches = [
            row.id.as_str(),
            row.cwd.as_str(),
            row.state.as_str(),
            label.as_str(),
        ]
        .into_iter()
        .chain(row.agent.as_deref())
        .any(|field| super::picker::matches(query, field));
        matches
    }

    /// Keep the selected id when it is still visible, otherwise select the first
    /// visible row; no visible row clears the selection.
    fn reanchor(&mut self) {
        let visible = self.visible_indices();
        let keep = match self.selected.as_deref() {
            Some(id) if visible.iter().any(|&index| self.rows[index].id == id) => {
                Some(id.to_string())
            }
            _ => visible.first().map(|&index| self.rows[index].id.clone()),
        };
        self.selected = keep;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, cwd: &str, agent: Option<&str>, state: &str) -> SessionInfo {
        SessionInfo {
            activity: Default::default(),
            name: None,
            title: None,
            title_source: None,
            id: id.into(),
            cwd: cwd.into(),
            agent: agent.map(str::to_string),
            generation: "v2".into(),
            pid: None,
            state: state.into(),
            exit_code: None,
            notice: None,
        }
    }

    fn sample() -> Vec<SessionInfo> {
        vec![
            row(
                "wrap-1",
                "/home/siddu/Projects/Ünïcödé-app",
                Some("Özel"),
                "live",
            ),
            row("wrap-2", "/tmp/build", Some("review"), "exited"),
            row("wrap-3", "/tmp/scratch", None, "stopped"),
        ]
    }

    fn ids(board: &Board) -> Vec<String> {
        board
            .visible_indices()
            .iter()
            .map(|&i| board.rows[i].id.clone())
            .collect()
    }

    #[test]
    fn defaults_to_all_and_searches_case_insensitively() {
        let mut board = Board::default();
        board.replace_rows(sample());
        assert_eq!(ids(&board), ["wrap-1", "wrap-2", "wrap-3"]);
        board.set_query("ÜNÏCÖ".into());
        assert_eq!(ids(&board), ["wrap-1"]);
        board.set_query("özel".into());
        assert_eq!(ids(&board), ["wrap-1"]);
        board.set_query("STOP".into());
        assert_eq!(ids(&board), ["wrap-3"]);
        board.set_query("".into());
        assert_eq!(ids(&board), ["wrap-1", "wrap-2", "wrap-3"]);
        board.set_query("nope".into());
        assert!(ids(&board).is_empty());
        assert!(board.selected().is_none());
        board.filter = Filter::All;
        assert!(ids(&board).is_empty());
    }

    #[test]
    fn selection_survives_refresh_reorder_and_removal() {
        let mut board = Board::default();
        board.replace_rows(sample());
        assert_eq!(
            board.selected().map(|row| row.id.clone()),
            Some("wrap-1".into())
        );
        board.filter = Filter::All;
        board.move_selection(1);
        assert_eq!(
            board.selected().map(|row| row.id.clone()),
            Some("wrap-2".into())
        );
        board.replace_rows(sample());
        assert_eq!(
            board.selected().map(|row| row.id.clone()),
            Some("wrap-2".into())
        );
        board.replace_rows(vec![
            sample()[2].clone(),
            sample()[1].clone(),
            sample()[0].clone(),
        ]);
        assert_eq!(
            board.selected().map(|row| row.id.clone()),
            Some("wrap-2".into())
        );
        board.replace_rows(vec![sample()[0].clone(), sample()[2].clone()]);
        assert_eq!(
            board.selected().map(|row| row.id.clone()),
            Some("wrap-1".into())
        );
        board.set_query("scratch".into());
        assert_eq!(
            board.selected().map(|row| row.id.clone()),
            Some("wrap-3".into())
        );
    }

    #[test]
    fn movement_wraps_and_empty_board_stays_empty() {
        let mut board = Board::default();
        assert!(board.visible_indices().is_empty());
        assert!(board.selected().is_none());
        board.move_selection(1);
        board.move_selection(-7);
        assert!(board.selected().is_none());
        board.replace_rows(vec![]);
        assert!(board.selected().is_none());

        board.filter = Filter::All;
        board.replace_rows(sample());
        board.move_selection(-1);
        assert_eq!(
            board.selected().map(|row| row.id.clone()),
            Some("wrap-3".into())
        );
        board.move_selection(1);
        assert_eq!(
            board.selected().map(|row| row.id.clone()),
            Some("wrap-1".into())
        );
        board.move_selection(3);
        assert_eq!(
            board.selected().map(|row| row.id.clone()),
            Some("wrap-1".into())
        );
        board.move_selection(-6);
        assert_eq!(
            board.selected().map(|row| row.id.clone()),
            Some("wrap-1".into())
        );
        board.set_query("stopped".into());
        board.move_selection(1);
        assert_eq!(
            board.selected().map(|row| row.id.clone()),
            Some("wrap-3".into())
        );
    }
}
