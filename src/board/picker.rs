use super::directories::DirectoryScan;
use crate::kiro;
use std::collections::HashSet;
use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};

/// Directories offered per listing; keeps a picker scan bounded and fast.
pub const MAX_LISTING: usize = 200;
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
    /// A sibling of the listed directory, for bare project fragments.
    Sibling,
    Recursive,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Here => "here",
            Origin::Parent => "parent",
            Origin::Recent => "session",
            Origin::Typed => "typed",
            Origin::Directory => "dir",
            Origin::Sibling => "sibling",
            Origin::Recursive => "dir",
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
    pub(super) discovery: Option<DirectoryScan>,
    pub(super) indexed: Vec<String>,
    pub(super) scanning: bool,
    pub(super) scan_limited: bool,
}

impl DirPicker {
    pub fn open(base: String, recents: Vec<String>) -> Self {
        let mut picker = Self {
            base: base.clone(),
            query: String::new(),
            candidates: Vec::new(),
            selected: 0,
            truncated: false,
            notice: None,
            discovery: if cfg!(test) {
                None
            } else {
                Some(DirectoryScan::start(&base, false))
            },
            indexed: Vec::new(),
            scanning: !cfg!(test),
            scan_limited: false,
            recents,
        };
        picker.rebuild();
        picker
    }

    /// Recompute candidates from the current base and query. Selection stays
    /// on the same path when it survives, otherwise it returns to the top.
    pub fn rebuild(&mut self) {
        self.rebuild_ranking(true);
    }

    /// Recompute candidates, and when `keep_anchor` is false highlight the
    /// best-ranked match. A query edit re-ranks so Enter never needs an arrow,
    /// while an explicit refresh keeps the operator's highlighted path.
    fn rebuild_ranking(&mut self, keep_anchor: bool) {
        let anchor = self
            .candidates
            .get(self.selected)
            .map(|row| row.path.clone());
        let query = self.query.trim().to_owned();
        let lower = query.to_lowercase();
        let mut rows: Vec<Candidate> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
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
            seen.insert(target.clone());
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
            seen.insert(self.base.clone());
            rows.push(Candidate {
                path: self.base.clone(),
                origin: Origin::Here,
            });
            for path in self.recent_paths() {
                if path == self.base || seen.contains(&path) {
                    continue;
                }
                seen.insert(path.clone());
                rows.push(Candidate {
                    path,
                    origin: Origin::Recent,
                });
            }
            if let Some(parent) = parent_of_opt(&self.base) {
                seen.insert(parent.clone());
                rows.push(Candidate {
                    path: parent,
                    origin: Origin::Parent,
                });
            }
            let (names, cut) = kiro::subdirectories(Path::new(&self.base), MAX_LISTING);
            truncated = cut;
            for name in names {
                let path = join(&self.base, &name);
                if seen.contains(&path) {
                    continue;
                }
                seen.insert(path.clone());
                rows.push(Candidate {
                    path,
                    origin: Origin::Directory,
                });
            }
        } else {
            // Rank every match, sessions included, so the best fuzzy hit is
            // always the row Enter would take.
            let mut ranked: Vec<(usize, Origin, String)> = Vec::new();
            for path in self.recent_paths() {
                if seen.contains(&path) {
                    continue;
                }
                let by_name = (!fragment_lower.is_empty())
                    .then(|| fuzzy_score(&fragment_lower, &file_name(&path)))
                    .flatten();
                let score = max_score(by_name, fuzzy_score(&lower, &path));
                if let Some(score) = score {
                    ranked.push((score, Origin::Recent, path));
                }
            }
            let (names, cut) = kiro::subdirectories(Path::new(&listing_root), MAX_LISTING);
            truncated = truncated || cut;
            for name in names {
                if let Some(score) = fuzzy_score(&fragment_lower, &name) {
                    ranked.push((score, Origin::Directory, join(&listing_root, &name)));
                }
            }
            // A bare fragment usually names a sibling project, so the parent
            // is listed too, with the same bound and no recursive crawl. Any
            // query with a slash keeps explicit path semantics.
            if !self.query.contains('/') && query != "~" {
                if let Some(parent) = parent_of_opt(&listing_root) {
                    let (names, cut) = kiro::subdirectories(Path::new(&parent), MAX_LISTING);
                    truncated = truncated || cut;
                    for name in names {
                        let path = join(&parent, &name);
                        if seen.contains(&path) {
                            continue;
                        }
                        if let Some(score) = fuzzy_score(&fragment_lower, &name) {
                            ranked.push((score, Origin::Sibling, path));
                        }
                    }
                }
            }
            // Match the entire home-relative path, including nested fragments.
            // Leading /, ~ and . preserve explicit filesystem browsing semantics.
            if !query.starts_with(['/', '~', '.']) {
                let home = env::var_os("HOME").map(PathBuf::from);
                for path in &self.indexed {
                    let relative = home
                        .as_ref()
                        .and_then(|home| {
                            Path::new(path)
                                .strip_prefix(home)
                                .ok()
                                .map(|p| p.to_string_lossy().into_owned())
                        })
                        .unwrap_or_else(|| path.clone());
                    if let Some(score) = max_score(
                        fuzzy_score(&lower, &file_name(path)),
                        fuzzy_score(&lower, &relative),
                    ) {
                        ranked.push((score, Origin::Recursive, path.clone()));
                    }
                }
            }
            ranked.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then_with(|| origin_rank(a.1).cmp(&origin_rank(b.1)))
                    .then_with(|| {
                        Path::new(&a.2)
                            .file_name()
                            .cmp(&Path::new(&b.2).file_name())
                    })
            });
            for (_, origin, path) in ranked {
                if seen.contains(&path) {
                    continue;
                }
                seen.insert(path.clone());
                rows.push(Candidate { path, origin });
            }
        }

        if query.is_empty() {
            for path in &self.indexed {
                if seen.insert(path.clone()) {
                    rows.push(Candidate {
                        path: path.clone(),
                        origin: Origin::Recursive,
                    });
                }
            }
        }
        self.truncated = if self.discovery.is_some() {
            self.scan_limited
        } else {
            truncated
        };
        self.candidates = rows;
        self.selected = match anchor {
            Some(path) => self
                .candidates
                .iter()
                .position(|row| row.path == path)
                .unwrap_or(0),
            None => 0,
        };
        if !keep_anchor {
            // Rows are ranked best first, so the top match is highlighted.
            self.selected = 0;
        }
        self.selected = self.selected.min(self.candidates.len().saturating_sub(1));
        self.notice = if self.candidates.is_empty() && !query.is_empty() {
            Some(format!(
                "No directory matches {query}. Keep typing a path, or press Ctrl+U to search again."
            ))
        } else {
            None
        };
    }

    pub fn poll_directories(&mut self) -> bool {
        let Some(listing) = self.discovery.as_ref().and_then(DirectoryScan::poll) else {
            return false;
        };
        self.indexed = listing.paths;
        self.scanning = listing.scanning;
        self.scan_limited = listing.limited;
        // An unedited list preserves manual navigation; a typed query chooses
        // the best match when a newly discovered directory arrives.
        self.rebuild_ranking(self.query.is_empty() || self.selected != 0);
        true
    }

    pub fn refresh(&mut self) {
        self.discovery = Some(DirectoryScan::start(&self.base, true));
        self.scanning = true;
        self.rebuild();
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
        self.rebuild_ranking(false);
    }

    /// Editing pops one character, never a byte, so a composed letter stays
    /// whole and the remaining query re-ranks.
    pub fn backspace(&mut self) {
        self.query.pop();
        self.rebuild_ranking(false);
    }

    pub fn clear_query(&mut self) {
        if !self.query.is_empty() {
            self.query.clear();
            self.rebuild_ranking(false);
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

/// Listing order wins ties between equally ranked rows.
fn origin_rank(origin: Origin) -> u8 {
    match origin {
        Origin::Typed => 0,
        Origin::Here => 1,
        Origin::Recent => 2,
        Origin::Parent => 3,
        Origin::Directory => 4,
        Origin::Sibling => 5,
        Origin::Recursive => 6,
    }
}

fn max_score(a: Option<usize>, b: Option<usize>) -> Option<usize> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
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

/// True when the query fuzzy-matches the target at all.
pub fn matches(query: &str, target: &str) -> bool {
    fuzzy_score(query, target).is_some()
}

// Ranked bands, deliberately far apart so a band always beats a bonus.
const EXACT: usize = 100_000;
const PREFIX: usize = 60_000;
const WORD_START: usize = 45_000;
const SUBSTRING: usize = 30_000;
const SUBSEQUENCE: usize = 15_000;
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
    let query: Vec<char> = query.trim().to_lowercase().chars().collect();
    if query.is_empty() {
        return Some(0);
    }
    let target_chars: Vec<char> = target.to_lowercase().chars().collect();
    let target_text: String = target_chars.iter().collect();
    let query_text: String = query.iter().collect();
    if target_text == query_text {
        return Some(EXACT);
    }
    if let Some(byte_at) = target_text.find(&query_text) {
        let at = target_text[..byte_at].chars().count();
        let band = if at == 0 {
            PREFIX
        } else if is_word_start(&target_chars, at) {
            WORD_START
        } else {
            SUBSTRING
        };
        return Some(band.saturating_sub(at));
    }
    subsequence_score(&query, &target_chars).map(|score| score.min(SUBSTRING - 1))
}

fn is_word_start(target: &[char], at: usize) -> bool {
    match at.checked_sub(1).and_then(|index| target.get(index)) {
        None => true,
        Some(before) => !before.is_alphanumeric(),
    }
}

/// Best scoring subsequence alignment of `query` inside `target`.
fn subsequence_score(query: &[char], target: &[char]) -> Option<usize> {
    // One cell per target position keeps every query update O(query * target).
    let mut layer: Vec<Option<i64>> = target
        .iter()
        .enumerate()
        .map(|(at, ch)| (*ch == query[0]).then(|| SUBSEQUENCE as i64 + word_bonus(target, at)))
        .collect();
    for wanted in &query[1..] {
        let mut next = vec![None; target.len()];
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
        layer = next;
    }
    layer
        .into_iter()
        .flatten()
        .max()
        .map(|value| value.max(1) as usize)
}

fn word_bonus(target: &[char], at: usize) -> i64 {
    i64::from(is_word_start(target, at)) * WORD_BONUS
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
                self.status = if list.unrecognized {
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
        self.refilter_ranking(true);
    }

    /// Rebuild the option list. A query edit ranks profiles by fuzzy score and
    /// highlights the best one, so Enter creates with the top match and no
    /// arrow press. A discovery reply keeps the highlighted profile.
    fn refilter_ranking(&mut self, keep_anchor: bool) {
        let anchor = self.selected_name();
        let query = self.query.trim().to_lowercase();
        let mut options = vec![None];
        if query.is_empty() {
            options.extend(self.names.iter().map(|name| Some(name.clone())));
        } else {
            let mut ranked: Vec<(usize, String)> = self
                .names
                .iter()
                .filter_map(|name| Some((fuzzy_score(&query, name)?, name.clone())))
                .collect();
            ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            options.extend(ranked.into_iter().map(|(_, name)| Some(name)));
        }
        if options.len() == 1 && !query.is_empty() {
            options.push(Some(self.query.trim().to_owned()));
        }
        self.options = options;
        self.selected = if !keep_anchor {
            // A query edit highlights the top ranked profile, never a stale one.
            usize::from(!query.is_empty() && self.options.len() > 1)
        } else {
            match anchor {
                // A typed fragment moves the highlight onto the best matching
                // profile, so Enter picks it without pressing ↓.
                None if !query.is_empty() && self.options.len() > 1 => 1,
                Some(name) => self
                    .options
                    .iter()
                    .position(|option| option.as_deref() == Some(name.as_str()))
                    .unwrap_or(usize::from(!query.is_empty() && self.options.len() > 1)),
                _ => 0,
            }
        };
    }

    fn selected_name(&self) -> Option<String> {
        self.options
            .get(self.selected)
            .and_then(|option| option.clone())
    }

    pub fn press(&mut self, ch: char) {
        self.query.push(ch);
        self.refilter_ranking(false);
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.refilter_ranking(false);
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

    /// A temporary workspace with a controlled parent, so the bounded sibling
    /// scan sees a fixed listing instead of whatever else lives in `/tmp`.
    struct Fixture {
        _outer: tempfile::TempDir,
        root: PathBuf,
    }

    impl Fixture {
        fn path(&self) -> &Path {
            &self.root
        }
    }

    fn tree() -> Fixture {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("projects");
        fs::create_dir_all(&root).unwrap();
        for name in [
            "api-gateway",
            "code-reviewer",
            "legacy-app",
            "nested/inner-deep",
            "node_modules",
            ".hidden",
        ] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        // A project next to the listed directory: a bare fragment must find it
        // without pressing ← first.
        fs::create_dir_all(outer.path().join("design-system")).unwrap();
        Fixture {
            _outer: outer,
            root,
        }
    }

    fn empty_workspace(names: &[&str]) -> Fixture {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("projects");
        fs::create_dir_all(&root).unwrap();
        for name in names {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        Fixture {
            _outer: outer,
            root,
        }
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
        let root = empty_workspace(&["Ünïcödé-app", "plain"]);
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
        assert!(picker.candidates.len() <= MAX_LISTING + 2);
    }

    #[test]
    fn recursive_index_keeps_all_rows_and_matches_nested_path_fragments() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().to_string_lossy().into_owned();
        let mut picker = DirPicker::open(base.clone(), Vec::new());
        picker.indexed = (0..300)
            .map(|i| format!("{base}/projects/group{i:03}/deep-app"))
            .collect();
        picker.rebuild();
        assert!(picker.candidates.len() >= 300);
        for ch in "group299/dapp".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.candidates.len(), 1);
        assert!(picker.accept().unwrap().ends_with("/group299/deep-app"));
        picker.clear_query();
        for ch in "deep-app".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.candidates.len(), 300);
        picker.move_selection(299);
        assert_eq!(picker.selected, 299);
    }

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

    #[test]
    fn typing_a_sibling_fragment_needs_no_arrow_and_enter_takes_the_best_row() {
        let root = tree();
        let mut picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        for ch in "design".chars() {
            picker.press(ch);
        }
        // The parent listing is searched with the same bound as the child one.
        assert_eq!(names(&picker), vec![("design-system", Origin::Sibling)]);
        assert_eq!(picker.selected, 0);
        assert_eq!(
            picker.accept().unwrap(),
            root._outer.path().join("design-system").to_string_lossy()
        );
        // From a nested directory the same fragment style finds its sibling.
        let mut picker = DirPicker::open(path(root.path(), "nested"), Vec::new());
        for ch in "code-rev".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.candidates[0].origin, Origin::Sibling);
        assert_eq!(picker.accept().unwrap(), path(root.path(), "code-reviewer"));
    }

    #[test]
    fn editing_a_query_re_ranks_instead_of_keeping_a_stale_weak_match() {
        let root = tree();
        let mut picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        for ch in "api".chars() {
            picker.press(ch);
        }
        assert!(picker.accept().unwrap().ends_with("/api-gateway"));
        // A weak highlight is replaced by the new best fuzzy match.
        for ch in "-gateway".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.selected, 0);
        assert!(picker.accept().unwrap().ends_with("/api-gateway"));
        // Backspace re-ranks too, so a mistyped tail never strands the cursor.
        for ch in "zz".chars() {
            picker.press(ch);
        }
        assert!(picker.candidates.is_empty());
        picker.backspace();
        picker.backspace();
        assert_eq!(picker.selected, 0);
        assert!(picker.accept().unwrap().ends_with("/api-gateway"));
        // Ctrl+U restores the unranked listing with the cursor at the top.
        picker.clear_query();
        assert_eq!(picker.candidates[0].origin, Origin::Here);
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn arrows_still_move_the_highlight_after_typing() {
        let root = empty_workspace(&["code-review-bot", "code-reviewer"]);
        let mut picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        for ch in "code-rev".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.candidates.len(), 2);
        let best = picker.candidates[0].path.clone();
        picker.move_selection(1);
        assert_ne!(picker.candidates[picker.selected].path, best);
        picker.move_selection(-1);
        assert_eq!(picker.candidates[picker.selected].path, best);
        // An explicit refresh keeps the operator's row, a query edit does not.
        picker.move_selection(1);
        let moved = picker.candidates[picker.selected].path.clone();
        assert_ne!(moved, best);
        picker.rebuild();
        assert_eq!(picker.candidates[picker.selected].path, moved);
        picker.backspace();
        assert_eq!(picker.selected, 0);
        assert_eq!(picker.candidates[picker.selected].path, best);
    }

    #[test]
    fn unicode_editing_stays_whole_and_matches_either_case() {
        let root = empty_workspace(&["Ünïcödé-app", "api-gateway"]);
        let mut picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        for ch in "ÜNÏ".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.selected, 0);
        assert!(picker.accept().unwrap().ends_with("/Ünïcödé-app"));
        // One backspace removes one character, not one byte.
        picker.backspace();
        assert_eq!(picker.query, "ÜN");
        assert!(picker.accept().unwrap().ends_with("/Ünïcödé-app"));
        for ch in "ÏCÖDÉ".chars() {
            picker.press(ch);
        }
        assert!(picker.accept().unwrap().ends_with("/Ünïcödé-app"));
        assert!(fuzzy_score("ünïcödé", "Ünïcödé-app").unwrap() >= PREFIX);
    }

    #[test]
    fn unicode_query_that_matches_nothing_recovers_by_editing() {
        let root = empty_workspace(&["Ünïcödé-app", "plain"]);
        let mut picker = DirPicker::open(root.path().to_string_lossy().to_string(), Vec::new());
        for ch in "Ünïcödé-ápp".chars() {
            picker.press(ch);
        }
        assert!(picker.candidates.is_empty());
        assert!(picker.accept().is_none());
        picker.backspace();
        picker.backspace();
        assert!(picker.candidates.is_empty());
        picker.backspace();
        assert_eq!(picker.accept().unwrap(), path(root.path(), "Ünïcödé-app"));
        assert_eq!(picker.selected, 0);
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
    fn agent_typing_ranks_profiles_and_enter_needs_no_arrow() {
        let (mut picker, handle) = test_picker("/tmp".into(), 1, Duration::from_millis(50));
        picker.apply(kiro::AgentScan {
            token: 1,
            cwd: "/tmp".into(),
            result: Ok(kiro::AgentList {
                names: vec![
                    "review-bot".into(),
                    "code-reviewer".into(),
                    "planner".into(),
                ],
                truncated: false,
                unrecognized: false,
            }),
        });
        // Discovery order is kept until an actual fragment is typed.
        assert_eq!(picker.options[1], Some("review-bot".into()));
        assert_eq!(picker.accept(), Accepted::Default);
        for ch in "rev".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.selected, 1);
        assert_eq!(picker.accept(), Accepted::Agent("review-bot".into()));
        for ch in "iewer".chars() {
            picker.press(ch);
        }
        // Only a word-start match of the longer fragment survives.
        assert_eq!(picker.options, vec![None, Some("code-reviewer".into())]);
        assert_eq!(picker.accept(), Accepted::Agent("code-reviewer".into()));
        // Backspacing re-ranks: the prefix match is the top row again.
        for _ in 0..4 {
            picker.backspace();
        }
        assert_eq!(
            picker.options,
            vec![
                None,
                Some("review-bot".into()),
                Some("code-reviewer".into())
            ]
        );
        assert_eq!(picker.selected, 1);
        assert_eq!(picker.accept(), Accepted::Agent("review-bot".into()));
        drop(handle);
    }

    #[test]
    fn agent_editing_replaces_a_stale_highlight_and_a_late_reply_keeps_one() {
        let (mut picker, handle) = test_picker("/tmp".into(), 1, Duration::from_millis(50));
        // Typing before discovery lands keeps Enter usable.
        for ch in "rev".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.accept(), Accepted::Manual("rev".into()));
        picker.apply(kiro::AgentScan {
            token: 1,
            cwd: "/tmp".into(),
            result: Ok(kiro::AgentList {
                names: vec![
                    "planner".into(),
                    "code-reviewer".into(),
                    "review-bot".into(),
                ],
                truncated: false,
                unrecognized: false,
            }),
        });
        // The late reply highlights the best ranked profile.
        assert_eq!(picker.selected, 1);
        assert_eq!(picker.options[1], Some("review-bot".into()));
        assert_eq!(picker.accept(), Accepted::Agent("review-bot".into()));
        // A manual arrow move is replaced by the new best match on typing.
        picker.move_selection(1);
        assert_eq!(picker.accept(), Accepted::Agent("code-reviewer".into()));
        for ch in "-b".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.selected, 1);
        assert_eq!(picker.accept(), Accepted::Agent("review-bot".into()));
        drop(handle);
    }

    #[test]
    fn agent_unicode_query_and_manual_fallback_stay_explicit() {
        let (mut picker, handle) = test_picker("/tmp".into(), 2, Duration::from_millis(50));
        picker.apply(kiro::AgentScan {
            token: 2,
            cwd: "/tmp".into(),
            result: Ok(kiro::AgentList {
                names: vec!["Ünïcödé-review".into(), "planner".into()],
                truncated: false,
                unrecognized: false,
            }),
        });
        for ch in "ünï".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.selected, 1);
        assert_eq!(picker.accept(), Accepted::Agent("Ünïcödé-review".into()));
        picker.backspace();
        picker.backspace();
        picker.backspace();
        assert_eq!(picker.selected, 0);
        assert_eq!(picker.accept(), Accepted::Default);
        // A fragment that matches no discovered profile stays an explicit choice.
        for ch in "ünïzzz".chars() {
            picker.press(ch);
        }
        assert_eq!(picker.accept(), Accepted::Manual("ünïzzz".into()));
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
    #[test]
    fn unicode_before_a_word_boundary_does_not_confuse_byte_and_character_offsets() {
        assert!(
            fuzzy_score("review", "é-reviewer").unwrap()
                > fuzzy_score("review", "xxreviewer").unwrap()
        );
    }
}
