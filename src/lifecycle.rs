//! Activity is independent of process liveness. Missing telemetry stays unknown.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    NeedsYou,
    Working,
    Ready,
    #[default]
    #[serde(other)]
    Unknown,
}

impl Activity {
    pub fn is_unknown(&self) -> bool {
        *self == Self::Unknown
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "activity unknown",
            Self::NeedsYou => "needs you",
            Self::Working => "working",
            Self::Ready => "ready",
        }
    }
}

/// UI classification only; it never authorizes stopping or resuming a process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    NeedsYou,
    Working,
    Ready,
    Inactive,
}

impl Filter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::NeedsYou,
        Self::Working,
        Self::Ready,
        Self::Inactive,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::NeedsYou => "Needs you",
            Self::Working => "Working",
            Self::Ready => "Ready",
            Self::Inactive => "Inactive",
        }
    }

    pub fn next(self, backwards: bool) -> Self {
        let index = Self::ALL.iter().position(|&f| f == self).unwrap_or(0);
        Self::ALL[(index + if backwards { 4 } else { 1 }) % Self::ALL.len()]
    }

    pub fn matches(self, live: bool, activity: Activity) -> bool {
        match self {
            Self::All => true,
            Self::Inactive => !live,
            Self::NeedsYou => live && activity == Activity::NeedsYou,
            Self::Working => live && activity == Activity::Working,
            Self::Ready => live && activity == Activity::Ready,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_is_not_ready_and_stale_activity_cannot_make_dead_sessions_active() {
        for filter in [Filter::NeedsYou, Filter::Working, Filter::Ready] {
            assert!(!filter.matches(true, Activity::Unknown));
            for activity in [
                Activity::Unknown,
                Activity::NeedsYou,
                Activity::Working,
                Activity::Ready,
            ] {
                assert!(!filter.matches(false, activity));
                assert!(Filter::Inactive.matches(false, activity));
            }
        }
        assert!(Filter::All.matches(true, Activity::Unknown));
        assert!(Filter::Ready.matches(true, Activity::Ready));
        assert!(Filter::Working.matches(true, Activity::Working));
        assert!(Filter::NeedsYou.matches(true, Activity::NeedsYou));
    }

    #[test]
    fn filters_cycle_in_both_directions() {
        for (index, filter) in Filter::ALL.into_iter().enumerate() {
            assert_eq!(filter.next(false), Filter::ALL[(index + 1) % 5]);
            assert_eq!(filter.next(true), Filter::ALL[(index + 4) % 5]);
        }
    }

    #[test]
    fn future_activity_values_fail_closed() {
        let activity: Activity = serde_json::from_str("\"future_state\"").unwrap();
        assert_eq!(activity, Activity::Unknown);
    }
}
