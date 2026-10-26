use crate::kiro;
use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};

/// Directories offered per listing; keeps a picker scan bounded and fast.
pub const MAX_LISTING: usize = 200;
/// Candidates shown at once; the rest stay behind the search query.
pub const MAX_CANDIDATES: usize = 40;
/// Recent session directories offered from the board registry.
pub const RECENT_LIMIT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The directory the picker is listing.
    Here,
    /// Its parent, for keyboard-only upward navigation.
    Parent,
    /// A directory that already has a session in the registry.
    Recent,
    /// A directory the query itself resolves to.
    Typed,
    /// A subdirectory of the listed directory.
    Directory,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Here => "here",
            Origin::Parent => "parent",
            Origin::Recent => "session",
            Origin::Typed => "typed",
            Origin::Directory => "dir",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: String,
    pub origin: Origin,
}

/// Keyboard-first directory choice: current directory, recent session
/// directories, bounded filesystem browsing, and a typed path fallback.
#[derive(Debug, Clone)]
pub struct DirPicker {
    pub base: String,
    pub query: String,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
    pub truncated: bool,
    pub notice: Option<String>,
    pub(super) recents: Vec<String>,
}

impl DirPicker {
    pub fn open(base: String, recents: Vec<String>) -> Self {
        let mut picker = Self {
            base,
            query: String::new(),
            candidates: Vec::new(),
            selected: 0,
            truncated: false,
            notice: None,
            recents,
        };
        picker.rebuild();
        picker
    }

    /// Recompute candidates from the current base and query. Selection stays
    /// on the same path when it survives, otherwise it returns to the top.
    pub fn rebuild(&mut self) {
        let anchor = self
            .candidates
            .get(self.selected)
            .map(|row| row.path.clone());
        let query = self.query.trim().to_owned();
        let lower = query.to_lowercase();
        let mut rows: Vec<Candidate> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        let mut truncated = false;

        // A query that is itself a path always wins, including `~` and
        // relative paths, so the typed fallback never needs an exact listing.
        let target = if query.is_empty() {
            None
        } else {
            resolve(&self.base, &query)
                .filter(|path| path.is_dir())
                .and_then(|path| path.to_str().map(str::to_owned))
        };
        if let Some(target) = &target {
            seen.push(target.clone());
            rows.push(Candidate {
                path: target.clone(),
                origin: Origin::Typed,
            });
        }

        // Listing root and name fragment for the query, so `../x/` completes
        // inside x while `code-rev` filters the current listing.
        let (listing_root, fragment) = match &target {
            Some(target) if self.query.ends_with('/') => (target.clone(), String::new()),
            Some(target) => (parent_of(target), file_name(target)),
            None => expand_input(&self.base, &query)
                .and_then(|path| {
                    let parent = path.parent()?.canonicalize().ok()?;
                    let fragment = path.file_name()?.to_str()?.to_owned();
                    Some((parent.to_str()?.to_owned(), fragment))
                })
                .unwrap_or_else(|| (self.base.clone(), query.clone())),
        };
        let fragment_lower = fragment.to_lowercase();

        if query.is_empty() {
            seen.push(self.base.clone());
            rows.push(Candidate {
                path: self.base.clone(),
                origin: Origin::Here,
            });
            for path in self.recent_paths() {
                if path == self.base || seen.contains(&path) {
                    continue;
                }
                seen.push(path.clone());
                rows.push(Candidate {
                    path,
                    origin: Origin::Recent,
                });
            }
            if let Some(parent) = parent_of_opt(&self.base) {
                seen.push(parent.clone());
                rows.push(Candidate {
                    path: parent,
                    origin: Origin::Parent,
                });
            }
            let (names, cut) = kiro::subdirectories(Path::new(&self.base), MAX_LISTING);
            truncated = cut;
            for name in names {
                if rows.len() >= MAX_CANDIDATES {
                    truncated = true;
                    break;
                }
                let path = join(&self.base, &name);
                if seen.contains(&path) {
                    continue;
                }
                seen.push(path.clone());
                rows.push(Candidate {
                    path,
                    origin: Origin::Directory,
                });
            }
        } else {
            for path in self.recent_paths() {
                if matches(&lower, &path) && !seen.contains(&path) {
                    seen.push(path.clone());
                    rows.push(Candidate {
                        path,
                        origin: Origin::Recent,
                    });
                }
            }
            let (names, cut) = kiro::subdirectories(Path::new(&listing_root), MAX_LISTING);
            truncated = truncated || cut;
            let mut matched: Vec<(usize, String)> = names
                .into_iter()
                .filter(|name| matches(&fragment_lower, name))
                .map(|name| (score(&fragment_lower, &name), name))
                .collect();
            matched.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            for (_, name) in matched {
                if rows.len() >= MAX_CANDIDATES {
                    truncated = true;
                    break;
                }
                let path = join(&listing_root, &name);
                if seen.contains(&path) {
                    continue;
                }
                seen.push(path.clone());
                rows.push(Candidate {
                    path,
                    origin: Origin::Directory,
                });
            }
        }

        self.truncated = truncated;
        self.candidates = rows;
        self.selected = match anchor {
            Some(path) => self
                .candidates
                .iter()
                .position(|row| row.path == path)
                .unwrap_or(0),
            None => 0,
        };
        self.selected = self.selected.min(self.candidates.len().saturating_sub(1));
        self.notice = if self.candidates.is_empty() && !query.is_empty() {
            Some(format!(
                "No directory matches {query}. Keep typing a path, or press Ctrl+U to search again."
            ))
        } else {
            None
        };
    }

    fn recent_paths(&self) -> Vec<String> {
        self.recents
            .iter()
            .filter(|path| Path::new(path).is_dir())
            .take(RECENT_LIMIT)
            .cloned()
            .collect()
    }

    pub fn press(&mut self, ch: char) {
        self.query.push(ch);
        self.rebuild();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.rebuild();
    }

    pub fn clear_query(&mut self) {
        if !self.query.is_empty() {
            self.query.clear();
            self.rebuild();
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.candidates.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.candidates.len() as i64;
        let next = (self.selected as i64 + i64::from(delta)).clamp(0, len - 1);
        self.selected = next as usize;
    }

    /// Descend into the highlighted directory and list its children, so
    /// browsing needs no typing at all.
    pub fn descend(&mut self) -> bool {
        let Some(row) = self.candidates.get(self.selected) else {
            self.notice = Some("Nothing to open. Type a path or press ← to go up.".to_owned());
            return false;
        };
        if row.path == self.base {
            self.notice =
                Some("Already listing this directory. Press ← for its parent.".to_owned());
            return false;
        }
        self.base = row.path.clone();
        self.query.clear();
        self.selected = 0;
        self.notice = None;
        self.rebuild();
        true
    }

    /// Move one level up. Left arrow is used because legacy terminals encode
    /// Ctrl+H as Backspace.
    pub fn parent(&mut self) -> bool {
        let Some(parent) = parent_of_opt(&self.base) else {
            self.notice = Some("Already at the top directory.".to_owned());
            return false;
        };
        self.base = parent;
        self.query.clear();
        self.selected = 0;
        self.notice = None;
        self.rebuild();
        true
    }

    /// The chosen directory, or None when there is nothing to choose.
    pub fn accept(&mut self) -> Option<String> {
        let row = self.candidates.get(self.selected)?.clone();
        self.notice = None;
        Some(row.path)
    }
}

fn join(base: &str, name: &str) -> String {
    if base == "/" {
        format!("/{name}")
    } else {
        format!("{base}/{name}")
    }
}

fn parent_of(path: &str) -> String {
    parent_of_opt(path).unwrap_or_else(|| path.to_owned())
}

fn parent_of_opt(path: &str) -> Option<String> {
    let parent = Path::new(path).parent()?;
    let text = parent.to_string_lossy().to_string();
    if text.is_empty() || text == path {
        return None;
    }
    Some(text)
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Expand `~`, `~/x` and relative paths against the picker's base directory.
/// Only an existing path resolves, so a half-typed query stays a filter.
pub fn resolve(base: &str, text: &str) -> Option<PathBuf> {
    std::fs::canonicalize(expand_input(base, text)?).ok()
}

fn expand_input(base: &str, text: &str) -> Option<PathBuf> {
    let text = text.trim();
    if text == "~" || text.starts_with("~/") {
        let home = env::var_os("HOME").map(PathBuf::from)?;
        Some(if text == "~" {
            home
        } else {
            home.join(&text[2..])
        })
    } else if Path::new(text).is_absolute() {
        Some(PathBuf::from(text))
    } else {
        Some(PathBuf::from(base).join(text))
    }
}

/// Substring first, then a subsequence, so short fragments still find a name.
pub fn matches(query: &str, target: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let target = target.to_lowercase();
    if target.contains(query) {
        return true;
    }
    let mut offset = 0usize;
    for ch in query.chars() {
        let rest = &target[offset..];
        match rest.find(ch) {
            Some(index) => offset += index + ch.len_utf8(),
            None => return false,
        }
    }
    true
}

fn score(query: &str, name: &str) -> usize {
    let lower = name.to_lowercase();
    if lower.starts_with(query) {
        4
    } else if lower.contains(query) {
        3
    } else if matches(query, &lower) {
        1
    } else {
        0
    }
}

/// Agent choice for one directory. Discovery is asynchronous: the listing
/// arrives later from a Kiro capture, and a stale result is discarded.
#[derive(Debug, Clone)]
pub struct AgentPicker {
    pub cwd: String,
    pub query: String,
    pub names: Vec<String>,
    /// `None` is the Kiro default row, always present so Enter works with no
    /// discovery result and no typing.
    pub options: Vec<Option<String>>,
    pub selected: usize,
    pub status: AgentStatus,
    pub token: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Loading,
    Ready,
    /// Kiro answered in an unknown shape: names are not invented.
    Unrecognized,
    Failed(String),
}

impl AgentPicker {
    fn new(cwd: String, token: u64) -> Self {
        Self {
            cwd,
            query: String::new(),
            names: Vec::new(),
            options: vec![None],
            selected: 0,
            status: AgentStatus::Loading,
            token,
            truncated: false,
        }
    }

    pub fn open(cwd: String, token: u64, timeout: Duration) -> (Self, kiro::AgentScanHandle) {
        let picker = Self::new(cwd, token);
        let receiver = kiro::spawn_agent_scan(token, &picker.cwd, timeout);
        (picker, receiver)
    }

    fn reset_for_retry(&mut self, token: u64) {
        self.token = token;
        self.names.clear();
        self.status = AgentStatus::Loading;
        self.truncated = false;
        self.refilter();
    }

    pub fn retry(&mut self, token: u64, timeout: Duration) -> kiro::AgentScanHandle {
        self.reset_for_retry(token);
        kiro::spawn_agent_scan(token, &self.cwd, timeout)
    }

    /// Apply a completed scan, ignoring anything from a cancelled request.
    pub fn apply(&mut self, scan: kiro::AgentScan) -> bool {
        if scan.token != self.token || scan.cwd != self.cwd {
            return false;
        }
        match scan.result {
            Ok(list) => {
                self.names = list.names;
                self.truncated = list.truncated;
                self.status = if list.unrecognized && self.names.is_empty() {
                    AgentStatus::Unrecognized
                } else {
                    AgentStatus::Ready
                };
            }
            Err(message) => self.status = AgentStatus::Failed(message),
        }
        self.refilter();
        true
    }

    fn refilter(&mut self) {
        let anchor = self.selected_name();
        let query = self.query.trim().to_lowercase();
        let mut options = vec![None];
        options.extend(
            self.names
                .iter()
                .filter(|name| matches(&query, name))
                .map(|name| Some(name.clone())),
        );
        if options.len() == 1 && !query.is_empty() {
            options.push(Some(self.query.trim().to_owned()));
        }
        self.options = options;
        self.selected = match anchor {
            // A typed fragment moves the highlight onto the first matching
            // profile, so Enter picks it without pressing ↓.
            None if !query.is_empty() && self.options.len() > 1 => 1,
            Some(name) => self
                .options
                .iter()
                .position(|option| option.as_deref() == Some(name.as_str()))
                .unwrap_or(usize::from(!query.is_empty() && self.options.len() > 1)),
            _ => 0,
        };
    }

    fn selected_name(&self) -> Option<String> {
        self.options
            .get(self.selected)
            .and_then(|option| option.clone())
    }

    pub fn press(&mut self, ch: char) {
        self.query.push(ch);
        self.refilter();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.refilter();
    }

    pub fn move_selection(&mut self, delta: i32) {
        let len = self.options.len() as i64;
        let next = (self.selected as i64 + i64::from(delta)).clamp(0, len - 1);
        self.selected = next as usize;
    }

    /// Enter creates without typing: the highlighted row decides. A typed
    /// fragment with no matching profile is passed to Kiro verbatim as the
    /// explicit manual fallback, never treated as a discovered profile.
    pub fn accept(&self) -> Accepted {
        match self.options.get(self.selected) {
            Some(Some(name)) if self.names.contains(name) => Accepted::Agent(name.clone()),
            Some(Some(name)) => Accepted::Manual(name.clone()),
            _ => Accepted::Default,
        }
    }

    #[cfg(test)]
    pub fn loading(&self) -> bool {
        self.status == AgentStatus::Loading
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accepted {
    /// Launch without `--agent` and let Kiro apply its own default.
    Default,
    Agent(String),
    /// Explicit typing when nothing was discovered.
    Manual(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // Model tests never invoke the real Kiro executable or its configuration.
    fn test_picker(
        cwd: String,
        token: u64,
        _timeout: Duration,
    ) -> (AgentPicker, std::sync::mpsc::Receiver<kiro::AgentScan>) {
        let (_, receiver) = std::sync::mpsc::channel();
        (AgentPicker::new(cwd, token), receiver)
    }

    fn tree() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "api-gateway",
            "code-reviewer",
            "legacy-app",
            "nested/inner-deep",
            "node_modules",
            ".hidden",
        ] {
            fs::create_dir_all(root.path().join(name)).unwrap();
        }
        root
    }

    fn path(root: &Path, name: &str) -> String {
        root.join(name).to_string_lossy().to_string()
    }

    fn names(picker: &DirPicker) -> Vec<(&str, Origin)> {
        picker
            .candidates
            .iter()
            .map(|row| {
                (
                    Path::new(&row.path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("/"),
                    row.origin,
                )
            })
            .collect()
    }

    #[test]
    fn opens_on_current_recent_and_browsable_directories_without_typing() {
        let root = tree();
        let base = path(root.path(), "nested");
        let recent = path(root.path(), "api-gateway");
        let picker = DirPicker::open(
            base.clone(),
            vec![recent.clone(), path(root.path(), "gone")],
        );
        assert_eq!(
            names(&picker),
            vec![
                ("nested", Origin::Here),
                ("api-gateway", Origin::Recent),
                (
                    root.path().file_name().unwrap().to_str().unwrap(),
                    Origin::Parent
                ),
                ("inner-deep", Origin::Directory),
            ]
        );
        assert_eq!(picker.selected, 0);
        assert!(!picker.truncated);
    }

    #[test]
    fn arrows_then_enter_choose_a_directory_with_no_typing() {
        let root = tree();
        let mut picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        let index = picker
            .candidates
            .iter()
            .position(|row| row.path.ends_with("/code-reviewer"))
            .unwrap();
        picker.move_selection(index as i32);
        let chosen = picker.accept().unwrap();
        assert!(chosen.ends_with("/code-reviewer"), "{chosen}");
        // Selection never runs past the listing, and Enter always answers.
        picker.move_selection(100);
        assert_eq!(picker.selected, picker.candidates.len() - 1);
        picker.move_selection(-500);
        assert_eq!(picker.selected, 0);
        assert!(picker.accept().is_some());
    }

    #[test]
    fn search_filters_by_fragment_and_subsequence_and_recovers() {
        let root = tree();
        let mut picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        for ch in "rev".chars() {
            picker.press(ch);
        }
        assert_eq!(names(&picker), vec![("code-reviewer", Origin::Directory)]);
        picker.query.clear();
        picker.rebuild();
        for ch in "cdwr".chars() {
            picker.press(ch);
        }
        assert_eq!(names(&picker), vec![("code-reviewer", Origin::Directory)]);
        for ch in "zz".chars() {
            picker.press(ch);
        }
        assert!(picker.candidates.is_empty());
        assert!(picker.accept().is_none());
        assert!(picker
            .notice
            .as_deref()
            .unwrap()
            .contains("No directory matches"));
        picker.backspace();
        picker.backspace();
        assert!(!picker.candidates.is_empty());
        assert!(picker.notice.is_none());
    }

    #[test]
    fn unicode_and_case_insensitive_search_never_panics() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("Ünïcödé-app")).unwrap();
        fs::create_dir(root.path().join("plain")).unwrap();
        let mut picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        for ch in "ünï".chars() {
            picker.press(ch);
        }
        assert_eq!(names(&picker), vec![("Ünïcödé-app", Origin::Directory)]);
        picker.query.clear();
        picker.rebuild();
        for ch in "öé".chars() {
            picker.press(ch);
        }
        assert_eq!(names(&picker).len(), 1);
        assert!(matches("é", "Ünïcödé-app"));
        assert!(!matches("zz", "Ünïcödé-app"));
    }

    #[test]
    fn typed_absolute_relative_and_home_paths_resolve_without_listing() {
        let root = tree();
        let base = path(root.path(), "nested");
        let mut picker = DirPicker::open(base.clone(), Vec::new());
        for ch in path(root.path(), "legacy-app").chars() {
            picker.press(ch);
        }
        assert_eq!(picker.candidates[0].origin, Origin::Typed);
        assert_eq!(picker.accept().unwrap(), path(root.path(), "legacy-app"));

        let mut picker = DirPicker::open(base.clone(), Vec::new());
        for ch in "../code-reviewer".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.candidates[0].origin, Origin::Typed);
        assert_eq!(picker.accept().unwrap(), path(root.path(), "code-reviewer"));

        let mut picker = DirPicker::open(base.clone(), Vec::new());
        for ch in "inner-deep".chars() {
            picker.press(ch);
        }
        assert_eq!(
            picker.accept().unwrap(),
            path(root.path(), "nested/inner-deep")
        );
        assert!(resolve(&base, "does-not-exist-at-all").is_none());
    }

    #[test]
    fn partial_absolute_and_relative_paths_complete_in_their_parent() {
        let root = tree();
        let mut picker = DirPicker::open(path(root.path(), "nested"), Vec::new());
        picker.query = "../code-rev".into();
        picker.rebuild();
        assert_eq!(picker.accept().unwrap(), path(root.path(), "code-reviewer"));
        picker.query = path(root.path(), "legacy-");
        picker.rebuild();
        assert_eq!(picker.accept().unwrap(), path(root.path(), "legacy-app"));
        assert!(matches("üé", "Ünïcödé-app"));
    }

    #[test]
    fn manual_agent_is_an_explicit_choice_and_default_stays_selectable() {
        let mut picker = AgentPicker::new("/tmp".into(), 1);
        for ch in "custom-agent".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.options, vec![None, Some("custom-agent".into())]);
        assert_eq!(picker.accept(), Accepted::Manual("custom-agent".into()));
        picker.move_selection(-1);
        assert_eq!(picker.accept(), Accepted::Default);
    }

    #[test]
    fn trailing_slash_lists_children_of_a_typed_directory() {
        let root = tree();
        let mut picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        for ch in "nested/".chars() {
            picker.press(ch);
        }
        assert!(names(&picker).iter().any(|(name, _)| *name == "inner-deep"));
    }

    #[test]
    fn tab_like_descent_and_left_arrow_ascent_browse_by_keyboard_only() {
        let root = tree();
        let mut picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        let target = picker
            .candidates
            .iter()
            .position(|row| row.path.ends_with("/nested"))
            .unwrap();
        picker.selected = target;
        assert!(picker.descend());
        assert!(picker.base.ends_with("/nested"));
        assert_eq!(names(&picker)[0], ("nested", Origin::Here));
        assert!(names(&picker).iter().any(|(name, _)| *name == "inner-deep"));
        assert!(picker.parent());
        assert_eq!(picker.base, root.path().to_string_lossy());
        // Root has no parent, and the notice explains instead of failing.
        let mut at_root = DirPicker::open("/".to_owned(), Vec::new());
        assert!(!at_root.parent());
        assert_eq!(
            at_root.notice.as_deref(),
            Some("Already at the top directory.")
        );
        assert!(at_root
            .candidates
            .iter()
            .any(|row| row.origin == Origin::Here));
        let mut picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        assert!(!picker.descend() && picker.notice.is_some() || picker.descend());
    }

    #[test]
    fn listing_is_bounded_and_reported() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..(MAX_LISTING + 5) {
            fs::create_dir(root.path().join(format!("dir{index:04}"))).unwrap();
        }
        let picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        assert!(picker.truncated);
        assert!(picker.candidates.len() <= MAX_CANDIDATES);
    }

    #[test]
    fn agent_picker_creates_with_the_kiro_default_before_any_reply() {
        let (mut picker, handle) = test_picker("/tmp".into(), 1, Duration::from_millis(50));
        assert!(picker.loading());
        assert_eq!(picker.options, vec![None]);
        assert_eq!(picker.accept(), Accepted::Default);
        // A result from a cancelled request is ignored, not applied late.
        let stale = kiro::AgentScan {
            token: 9,
            cwd: "/tmp".into(),
            result: Ok(kiro::AgentList {
                names: vec!["ghost".into()],
                truncated: false,
                unrecognized: false,
            }),
        };
        assert!(!picker.apply(stale));
        assert!(picker.names.is_empty());
        drop(handle);
    }

    #[test]
    fn agent_names_arrive_later_and_filter_in_place() {
        let (mut picker, handle) = test_picker("/tmp".into(), 3, Duration::from_millis(50));
        assert!(picker.apply(kiro::AgentScan {
            token: 3,
            cwd: "/tmp".into(),
            result: Ok(kiro::AgentList {
                names: vec!["reviewer".into(), "planner".into(), "kiro_default".into()],
                truncated: false,
                unrecognized: false,
            }),
        }));
        assert!(!picker.loading());
        assert_eq!(
            picker.options,
            vec![
                None,
                Some("reviewer".into()),
                Some("planner".into()),
                Some("kiro_default".into())
            ]
        );
        for ch in "plan".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.options, vec![None, Some("planner".into())]);
        // Typing highlights the matching profile: Enter needs no arrow.
        assert_eq!(picker.selected, 1);
        assert_eq!(picker.accept(), Accepted::Agent("planner".into()));
        assert_eq!(picker.selected_name().as_deref(), Some("planner"));
        drop(handle);
    }

    #[test]
    fn arrow_enter_selects_without_typing_and_typing_is_manual_fallback() {
        let (mut picker, handle) = test_picker("/tmp".into(), 1, Duration::from_millis(50));
        picker.apply(kiro::AgentScan {
            token: 1,
            cwd: "/tmp".into(),
            result: Ok(kiro::AgentList {
                names: vec!["reviewer".into()],
                truncated: false,
                unrecognized: false,
            }),
        });
        assert_eq!(picker.accept(), Accepted::Default);
        picker.move_selection(1);
        assert_eq!(picker.accept(), Accepted::Agent("reviewer".into()));
        picker.move_selection(-1);
        assert_eq!(picker.accept(), Accepted::Default);
        drop(handle);
        // Nothing discovered: an explicitly typed name still launches, unverified.
        let (mut empty, handle) = test_picker("/tmp".into(), 2, Duration::from_millis(50));
        empty.apply(kiro::AgentScan {
            token: 2,
            cwd: "/tmp".into(),
            result: Ok(kiro::AgentList {
                names: Vec::new(),
                truncated: false,
                unrecognized: true,
            }),
        });
        assert_eq!(empty.status, AgentStatus::Unrecognized);
        for ch in "custom-agent".chars() {
            empty.press(ch);
        }
        assert_eq!(empty.accept(), Accepted::Manual("custom-agent".into()));
        drop(handle);
    }

    #[test]
    fn discovery_failure_is_recoverable_text_not_a_panic() {
        let (mut picker, handle) = test_picker("/tmp".into(), 4, Duration::from_millis(50));
        picker.apply(kiro::AgentScan {
            token: 4,
            cwd: "/tmp".into(),
            result: Err(
                "Kiro agent listing failed (exit 1). Log in to Kiro and press Ctrl+R to retry."
                    .into(),
            ),
        });
        assert!(matches!(picker.status, AgentStatus::Failed(_)));
        assert_eq!(picker.accept(), Accepted::Default);
        picker.reset_for_retry(5);
        assert!(picker.loading());
        assert_eq!(picker.token, 5);
        // A reply from the cancelled first request never lands afterwards.
        assert!(!picker.apply(kiro::AgentScan {
            token: 4,
            cwd: "/tmp".into(),
            result: Ok(kiro::AgentList {
                names: vec!["late".into()],
                truncated: false,
                unrecognized: false,
            }),
        }));
        assert!(picker.names.is_empty());
        drop(handle);
    }
}
