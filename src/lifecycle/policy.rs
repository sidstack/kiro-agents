//! Eligibility decisions for idle unload and same-engine resume.
//!
//! This module reads no files, signals no process and launches nothing. The caller
//! gathers every fact and decides what to do with the answer; a decision here never
//! authorizes an action by itself. Unknown, inferred or unverified inputs fail closed,
//! so an absent native signal is blocking rather than permissive. See
//! `docs/journal/SESSION-LIFECYCLE.md` for the verification gates these gates encode.

use crate::lifecycle::Activity;
use std::time::Duration;

/// Engine generation recorded at creation. Never inferred from a CLI version string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    V2,
    V3,
}

impl Generation {
    /// Only the wrapper's own recorded values parse; anything else stays unsupported.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "v2" => Some(Self::V2),
            "v3" => Some(Self::V3),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::V2 => "v2",
            Self::V3 => "v3",
        }
    }
}

/// How a fact about a native conversation was established. Only `Verified` unlocks a gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Provenance {
    /// Read through a verified read-only native source for this exact conversation.
    Verified,
    /// Derived from wrapper bookkeeping alone; never links to native history.
    WrapperOnly,
    /// Absent, unrecognized or contradictory.
    #[default]
    Unknown,
}

/// Exact native identity, captured while the owned process still held its lock.
/// A missing field blocks resume instead of being guessed from cwd or recency.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NativeIdentity {
    pub native_id: Option<String>,
    pub cwd: Option<String>,
    /// Informational only: a session legitimately has no explicit Kiro profile.
    pub agent: Option<String>,
    pub generation: Option<Generation>,
    pub provenance: Provenance,
}

impl NativeIdentity {
    /// A wrapper-only row: it has no trustworthy link to native history.
    pub fn unlinked() -> Self {
        Self::default()
    }

    /// The first missing identity requirement, or None when the identity is usable.
    pub fn missing_reason(&self) -> Option<&'static str> {
        match self.provenance {
            Provenance::Verified => (),
            Provenance::WrapperOnly => return Some("identity came from wrapper metadata only"),
            Provenance::Unknown => return Some("identity provenance is unknown"),
        }
        if self.native_id.as_deref().is_none_or(str::is_empty) {
            return Some("native conversation id is missing");
        }
        if self.cwd.as_deref().is_none_or(str::is_empty) {
            return Some("original native cwd is missing");
        }
        if self.generation.is_none() {
            return Some("native generation is unverified");
        }
        None
    }

    pub fn is_verified(&self) -> bool {
        self.missing_reason().is_none()
    }
}

/// Result of a bounded read-only native history check for one native id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SavedHistory {
    /// A verified adapter found supported history belonging to this exact native id.
    VerifiedSupported { generation: Generation },
    /// A verified adapter found history this build cannot resume.
    VerifiedUnsupported,
    /// No verified adapter result, so nothing is known to be resumable.
    #[default]
    Unverified,
}

/// Whether an unsent native editor draft survives an unload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DraftSafety {
    /// A verified native mechanism reports no unsent draft.
    VerifiedEmpty,
    /// A verified native mechanism preserved the draft.
    VerifiedPreserved,
    /// Raw passthrough cannot read the native editor. Blocking.
    #[default]
    Unverified,
}

/// Where a turn-activity value came from. Only native events count as evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivityEvidence {
    /// A verified native turn event carrying this activity value.
    Verified(Activity),
    /// Output silence, terminal bytes or transcript keywords. Never sufficient.
    Inferred,
    /// Nothing observed, or a value this build does not recognize.
    #[default]
    Absent,
}

/// A live native session lock as reported by a read-only check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NativeLock {
    /// A live lock is held by a process this wrapper owns.
    OwnedLive,
    /// A live lock is held by a process this wrapper does not own.
    ForeignLive,
    /// A verified read-only check found no live lock for the native id.
    Free,
    /// No verified check ran, for example an unsupported store layout.
    #[default]
    Unverified,
}

/// Idle-unload configuration. Disabled by default: stopping is opt-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InactivityPolicy {
    pub enabled: bool,
    pub after: Duration,
}

/// Upper bound so a mistyped duration cannot schedule an effectively permanent idle clock.
const MAX_IDLE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Strict single-unit durations (`45s`, `30m`, `1h`). Zero, garbage and over-long fail.
pub fn parse_idle_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let unit = text.chars().last()?;
    let digits = text.get(..text.len() - unit.len_utf8())?;
    let amount: u64 = digits.parse().ok().filter(|amount| *amount > 0)?;
    let seconds = match unit {
        's' => amount,
        'm' => amount.checked_mul(60)?,
        'h' => amount.checked_mul(3_600)?,
        _ => return None,
    };
    (seconds <= MAX_IDLE.as_secs()).then(|| Duration::from_secs(seconds))
}

/// One candidate session as observed by the lifecycle owner. Defaults fail closed, so an
/// incompletely observed row is never eligible for an automatic unload.
#[derive(Debug, Clone, Default)]
pub struct InactivityFacts {
    pub owned_live: bool,
    pub frontend_attached: bool,
    pub handoff_in_progress: bool,
    pub pending_operation: bool,
    pub turn_running: bool,
    pub approval_pending: bool,
    pub evidence: ActivityEvidence,
    /// A failed status probe must not advance eligibility or imply a settled turn.
    pub probe_failed: bool,
    pub identity: NativeIdentity,
    pub history: SavedHistory,
    pub draft: DraftSafety,
    /// Elapsed time since the last verified activity. None means no trustworthy clock.
    pub idle: Option<Duration>,
}

/// Why automatic unload is not permitted. `reason` is shown to the user verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnloadBlock {
    Disabled,
    NotOwnedLive,
    FrontendAttached,
    HandoffInProgress,
    PendingOperation,
    TurnRunning,
    ApprovalPending,
    ActivityNotVerified,
    ActivityNotReady,
    ProbeFailed,
    IdleClockUntrusted,
    IdleBelowDeadline,
    IdentityUnverified,
    HistoryUnverified,
    HistoryUnsupported,
    HistoryGenerationMismatch,
    DraftUnverified,
}

impl UnloadBlock {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Disabled => "automatic inactivity is off",
            Self::NotOwnedLive => "no owned live process to unload",
            Self::FrontendAttached => "a frontend is attached",
            Self::HandoffInProgress => "a frontend handoff is in progress",
            Self::PendingOperation => "an operation is queued",
            Self::TurnRunning => "a turn or tool operation is running",
            Self::ApprovalPending => "an approval is waiting",
            Self::ActivityNotVerified => "turn activity has no verified native event",
            Self::ActivityNotReady => "turn activity is not a verified completed turn",
            Self::ProbeFailed => "the last status probe failed",
            Self::IdleClockUntrusted => "no trustworthy last-activity timestamp",
            Self::IdleBelowDeadline => "the idle deadline has not passed",
            Self::IdentityUnverified => "native identity is not verified",
            Self::HistoryUnverified => "native saved history is not verified",
            Self::HistoryUnsupported => "native saved history is unsupported",
            Self::HistoryGenerationMismatch => {
                "saved history belongs to a different native generation"
            }
            Self::DraftUnverified => "native draft safety is not verified",
        }
    }
}

/// The first blocking reason in safety order: liveness, then activity, then clock,
/// then the data-safety gates that must hold before a native conversation is released.
pub fn unload_decision(
    policy: &InactivityPolicy,
    facts: &InactivityFacts,
) -> Result<(), UnloadBlock> {
    let blockers = [
        (!policy.enabled).then_some(UnloadBlock::Disabled),
        (!facts.owned_live).then_some(UnloadBlock::NotOwnedLive),
        facts
            .frontend_attached
            .then_some(UnloadBlock::FrontendAttached),
        facts
            .handoff_in_progress
            .then_some(UnloadBlock::HandoffInProgress),
        facts
            .pending_operation
            .then_some(UnloadBlock::PendingOperation),
        facts.turn_running.then_some(UnloadBlock::TurnRunning),
        facts
            .approval_pending
            .then_some(UnloadBlock::ApprovalPending),
        match facts.evidence {
            ActivityEvidence::Verified(Activity::Ready) => None,
            ActivityEvidence::Verified(_) => Some(UnloadBlock::ActivityNotReady),
            _ => Some(UnloadBlock::ActivityNotVerified),
        },
        facts.probe_failed.then_some(UnloadBlock::ProbeFailed),
        match facts.idle {
            None => Some(UnloadBlock::IdleClockUntrusted),
            Some(idle) if idle < policy.after => Some(UnloadBlock::IdleBelowDeadline),
            Some(_) => None,
        },
        (!facts.identity.is_verified()).then_some(UnloadBlock::IdentityUnverified),
        match facts.history {
            SavedHistory::VerifiedSupported { generation } => (facts.identity.generation
                != Some(generation))
            .then_some(UnloadBlock::HistoryGenerationMismatch),
            SavedHistory::VerifiedUnsupported => Some(UnloadBlock::HistoryUnsupported),
            SavedHistory::Unverified => Some(UnloadBlock::HistoryUnverified),
        },
        match facts.draft {
            DraftSafety::Unverified => Some(UnloadBlock::DraftUnverified),
            _ => None,
        },
    ];
    match blockers.into_iter().flatten().next() {
        Some(block) => Err(block),
        None => Ok(()),
    }
}

/// Inputs for an explicit resume request. Only a user action may set `requested_by_user`.
#[derive(Debug, Clone, Default)]
pub struct ResumeFacts {
    pub requested_by_user: bool,
    pub identity: NativeIdentity,
    pub history: SavedHistory,
    /// The engine the user asked for, when an explicit `--v2`/`--v3` choice was made.
    pub requested_generation: Option<Generation>,
    pub lock: NativeLock,
    /// This wrapper already supervises a live child for the same row.
    pub managed_child_live: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeBlock {
    NotUserRequested,
    ChildAlreadyLive,
    IdentityUnverified,
    GenerationUnverified,
    GenerationMismatch,
    HistoryUnverified,
    HistoryUnsupported,
    LockUnverified,
    LockOwnedLive,
    LockForeignLive,
}

impl ResumeBlock {
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotUserRequested => "resume was not an explicit user action",
            Self::ChildAlreadyLive => "the managed process is already live; attach instead",
            Self::IdentityUnverified => "native identity is not verified",
            Self::GenerationUnverified => "native generation is unverified",
            Self::GenerationMismatch => "resume across generations is not permitted",
            Self::HistoryUnverified => "native saved history is not verified",
            Self::HistoryUnsupported => "native saved history is unsupported",
            Self::LockUnverified => "native lock state is unverified",
            Self::LockOwnedLive => "an owned native lock is already live",
            Self::LockForeignLive => "a foreign process holds the native lock",
        }
    }
}

/// The single engine a resume may use, or the first blocking reason. A returned
/// generation is a legality answer only: argv construction stays in `src/kiro/`.
pub fn resume_gate(facts: &ResumeFacts) -> Result<Generation, ResumeBlock> {
    if !facts.requested_by_user {
        return Err(ResumeBlock::NotUserRequested);
    }
    if facts.managed_child_live {
        return Err(ResumeBlock::ChildAlreadyLive);
    }
    if facts.identity.missing_reason().is_some() {
        return Err(ResumeBlock::IdentityUnverified);
    }
    let generation = facts
        .identity
        .generation
        .ok_or(ResumeBlock::GenerationUnverified)?;
    if facts
        .requested_generation
        .is_some_and(|requested| requested != generation)
    {
        return Err(ResumeBlock::GenerationMismatch);
    }
    match facts.history {
        SavedHistory::VerifiedSupported { generation: stored } => {
            if stored != generation {
                return Err(ResumeBlock::GenerationMismatch);
            }
        }
        SavedHistory::VerifiedUnsupported => return Err(ResumeBlock::HistoryUnsupported),
        SavedHistory::Unverified => return Err(ResumeBlock::HistoryUnverified),
    }
    match facts.lock {
        NativeLock::Free => (),
        NativeLock::Unverified => return Err(ResumeBlock::LockUnverified),
        NativeLock::OwnedLive => return Err(ResumeBlock::LockOwnedLive),
        NativeLock::ForeignLive => return Err(ResumeBlock::LockForeignLive),
    }
    Ok(generation)
}

/// How an inactive row became inactive. The two cases stay distinguishable metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deactivation {
    ExplicitStop,
    IdleUnload,
}

impl Deactivation {
    pub fn label(self) -> &'static str {
        match self {
            Self::ExplicitStop => "stopped",
            Self::IdleUnload => "inactive",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> InactivityPolicy {
        InactivityPolicy {
            enabled: true,
            after: Duration::from_secs(3_600),
        }
    }

    fn identity() -> NativeIdentity {
        NativeIdentity {
            native_id: Some("11111111-2222-3333-4444-555555555555".into()),
            cwd: Some("/tmp/project".into()),
            agent: Some("code-reviewer".into()),
            generation: Some(Generation::V2),
            provenance: Provenance::Verified,
        }
    }

    /// Every safety condition verified, past the deadline. Nothing here launches or stops.
    fn eligible_facts() -> InactivityFacts {
        InactivityFacts {
            owned_live: true,
            evidence: ActivityEvidence::Verified(Activity::Ready),
            identity: identity(),
            history: SavedHistory::VerifiedSupported {
                generation: Generation::V2,
            },
            draft: DraftSafety::VerifiedPreserved,
            idle: Some(Duration::from_secs(3_601)),
            ..Default::default()
        }
    }

    #[test]
    fn a_fully_observed_idle_ready_session_is_the_only_eligible_case() {
        assert_eq!(unload_decision(&policy(), &eligible_facts()), Ok(()));
        for draft in [DraftSafety::VerifiedEmpty, DraftSafety::VerifiedPreserved] {
            let mut facts = eligible_facts();
            facts.draft = draft;
            assert_eq!(unload_decision(&policy(), &facts), Ok(()));
        }
    }

    #[test]
    fn nothing_is_unloaded_when_inactivity_is_disabled() {
        let mut facts = eligible_facts();
        facts.frontend_attached = true;
        assert_eq!(
            unload_decision(
                &InactivityPolicy {
                    enabled: false,
                    ..Default::default()
                },
                &facts
            ),
            Err(UnloadBlock::Disabled)
        );
    }

    #[test]
    fn unknown_or_inferred_activity_never_qualifies_as_ready() {
        for evidence in [
            ActivityEvidence::Absent,
            ActivityEvidence::Inferred,
            ActivityEvidence::Verified(Activity::Unknown),
            ActivityEvidence::Verified(Activity::Working),
            ActivityEvidence::Verified(Activity::NeedsYou),
        ] {
            let mut facts = eligible_facts();
            facts.evidence = evidence;
            let decision = unload_decision(&policy(), &facts);
            assert!(matches!(
                decision,
                Err(UnloadBlock::ActivityNotVerified | UnloadBlock::ActivityNotReady)
            ));
        }
    }

    #[test]
    fn attached_handoff_queued_running_and_approval_states_are_never_unloaded() {
        for flag in [
            |facts: &mut InactivityFacts| facts.frontend_attached = true,
            |facts: &mut InactivityFacts| facts.handoff_in_progress = true,
            |facts: &mut InactivityFacts| facts.pending_operation = true,
            |facts: &mut InactivityFacts| facts.turn_running = true,
            |facts: &mut InactivityFacts| facts.approval_pending = true,
        ] {
            let mut facts = eligible_facts();
            flag(&mut facts);
            assert_ne!(unload_decision(&policy(), &facts), Ok(()));
        }
    }

    #[test]
    fn a_failed_probe_and_an_untrusted_clock_do_not_advance_eligibility() {
        let mut facts = eligible_facts();
        facts.probe_failed = true;
        assert_eq!(
            unload_decision(&policy(), &facts),
            Err(UnloadBlock::ProbeFailed)
        );
        let mut facts = eligible_facts();
        facts.idle = None;
        assert_eq!(
            unload_decision(&policy(), &facts),
            Err(UnloadBlock::IdleClockUntrusted)
        );
        let mut facts = eligible_facts();
        facts.idle = Some(Duration::from_secs(3_600 - 1));
        assert_eq!(
            unload_decision(&policy(), &facts),
            Err(UnloadBlock::IdleBelowDeadline)
        );
    }

    #[test]
    fn unverified_identity_history_or_draft_safety_blocks_unload() {
        for mutate in [
            |facts: &mut InactivityFacts| facts.identity = NativeIdentity::unlinked(),
            |facts: &mut InactivityFacts| {
                facts.history = SavedHistory::Unverified;
            },
            |facts: &mut InactivityFacts| {
                facts.history = SavedHistory::VerifiedUnsupported;
            },
            |facts: &mut InactivityFacts| facts.draft = DraftSafety::Unverified,
        ] {
            let mut facts = eligible_facts();
            mutate(&mut facts);
            assert_ne!(unload_decision(&policy(), &facts), Ok(()));
        }
        let mut facts = eligible_facts();
        facts.identity = NativeIdentity {
            provenance: Provenance::WrapperOnly,
            ..identity()
        };
        assert_eq!(
            facts.identity.missing_reason(),
            Some("identity came from wrapper metadata only")
        );
        assert_eq!(
            unload_decision(&policy(), &facts),
            Err(UnloadBlock::IdentityUnverified)
        );
    }

    #[test]
    fn unload_requires_saved_history_from_the_same_generation() {
        for (native, saved) in [
            (Generation::V2, Generation::V3),
            (Generation::V3, Generation::V2),
        ] {
            let mut facts = eligible_facts();
            facts.identity.generation = Some(native);
            facts.history = SavedHistory::VerifiedSupported { generation: saved };
            assert_eq!(
                unload_decision(&policy(), &facts),
                Err(UnloadBlock::HistoryGenerationMismatch)
            );
        }
    }

    #[test]
    fn a_half_observed_row_fails_closed() {
        assert_eq!(
            unload_decision(&policy(), &InactivityFacts::default()),
            Err(UnloadBlock::NotOwnedLive)
        );
        assert!(!NativeIdentity::unlinked().is_verified());
    }

    #[test]
    fn resume_requires_an_explicit_user_request_and_no_managed_child() {
        let facts = ResumeFacts {
            identity: identity(),
            history: SavedHistory::VerifiedSupported {
                generation: Generation::V2,
            },
            lock: NativeLock::Free,
            ..Default::default()
        };
        assert_eq!(
            resume_gate(&facts),
            Err(ResumeBlock::NotUserRequested),
            "host recovery must never infer a resume"
        );
        let facts = ResumeFacts {
            requested_by_user: true,
            managed_child_live: true,
            ..facts
        };
        assert_eq!(resume_gate(&facts), Err(ResumeBlock::ChildAlreadyLive));
    }

    #[test]
    fn resume_never_guesses_identity_history_or_lock_state() {
        let base = ResumeFacts {
            requested_by_user: true,
            identity: identity(),
            history: SavedHistory::VerifiedSupported {
                generation: Generation::V2,
            },
            lock: NativeLock::Free,
            ..Default::default()
        };
        assert_eq!(resume_gate(&base), Ok(Generation::V2));
        for mutate in [
            |facts: &mut ResumeFacts| facts.identity = NativeIdentity::unlinked(),
            |facts: &mut ResumeFacts| facts.history = SavedHistory::Unverified,
            |facts: &mut ResumeFacts| facts.history = SavedHistory::VerifiedUnsupported,
            |facts: &mut ResumeFacts| facts.lock = NativeLock::Unverified,
            |facts: &mut ResumeFacts| facts.lock = NativeLock::OwnedLive,
            |facts: &mut ResumeFacts| facts.lock = NativeLock::ForeignLive,
        ] {
            let mut facts = base.clone();
            mutate(&mut facts);
            assert_ne!(resume_gate(&facts), Ok(Generation::V2));
        }
        let mut missing_id = base.clone();
        missing_id.identity = NativeIdentity {
            native_id: None,
            ..identity()
        };
        assert_eq!(
            resume_gate(&missing_id),
            Err(ResumeBlock::IdentityUnverified)
        );
    }

    #[test]
    fn resume_stays_on_the_recorded_native_generation() {
        let base = ResumeFacts {
            requested_by_user: true,
            identity: identity(),
            history: SavedHistory::VerifiedSupported {
                generation: Generation::V2,
            },
            lock: NativeLock::Free,
            ..Default::default()
        };
        for requested in [Generation::V2, Generation::V3] {
            let mut facts = base.clone();
            facts.requested_generation = Some(requested);
            assert_eq!(
                resume_gate(&facts),
                if requested == Generation::V2 {
                    Ok(Generation::V2)
                } else {
                    Err(ResumeBlock::GenerationMismatch)
                }
            );
        }
        let mut cross_store = base.clone();
        cross_store.history = SavedHistory::VerifiedSupported {
            generation: Generation::V3,
        };
        assert_eq!(
            resume_gate(&cross_store),
            Err(ResumeBlock::GenerationMismatch)
        );
        let mut unknown_generation = base.clone();
        unknown_generation.identity = NativeIdentity {
            generation: None,
            ..identity()
        };
        assert_eq!(
            resume_gate(&unknown_generation),
            Err(ResumeBlock::IdentityUnverified)
        );
    }

    #[test]
    fn generations_are_only_the_recorded_values() {
        assert_eq!(Generation::parse("v2"), Some(Generation::V2));
        assert_eq!(Generation::parse("v3"), Some(Generation::V3));
        for value in ["", "V2", "v4", "3", "unknown", " v2 "] {
            assert_eq!(Generation::parse(value), None);
        }
    }

    #[test]
    fn idle_durations_are_strict_and_never_zero() {
        assert_eq!(parse_idle_duration("45s"), Some(Duration::from_secs(45)));
        assert_eq!(parse_idle_duration("30m"), Some(Duration::from_secs(1_800)));
        assert_eq!(
            parse_idle_duration(" 1h "),
            Some(Duration::from_secs(3_600))
        );
        for value in [
            "",
            "off",
            "0",
            "0s",
            "10",
            "1d",
            "1 h",
            "-1h",
            "1.5h",
            "99999999999h",
        ] {
            assert_eq!(parse_idle_duration(value), None, "{value}");
        }
    }

    #[test]
    fn explicit_stop_and_idle_unload_stay_distinguishable() {
        assert_ne!(
            Deactivation::ExplicitStop.label(),
            Deactivation::IdleUnload.label()
        );
    }
}
