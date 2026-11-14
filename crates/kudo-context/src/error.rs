//! The engine's errors.

use std::fmt;

/// Why a context operation failed.
#[derive(Debug)]
pub enum Error {
    /// The store directory could not be read or written.
    Io(std::io::Error),
    /// kinora refused a store or resolve; the message is kinora's.
    Kinora(String),
    /// No context with this id exists in the store.
    UnknownContext(String),
    /// The prefix names more than one context: (the prefix, how many).
    AmbiguousContext(String, usize),
    /// No claim with this id exists in the store.
    UnknownClaim(String),
    /// The claim was already settled (submitted or released).
    ClaimSettled(String),
    /// The claim's deadline passed; its item may already be claimed again.
    ClaimExpired(String),
    /// The head moved since the submission's base: build on `current` (the
    /// head's versions now) and submit again. The claim stays live.
    HeadMoved { claim: String, current: Vec<String> },
    /// The context declares no repo of this name on this node.
    UnknownRepo { name: String, node: String },
    /// A repo declaration that cannot stand: why.
    InvalidRepo(String),
    /// The mirror refused this machine (credentials, permissions): a
    /// misconfiguration to fix, not an outage to work around.
    Mirror(String),
    /// The claim is held by another lease.
    ClaimNotHeld { claim: String, lease: String },
    /// A check's claim was submitted without a verdict.
    CheckNeedsVerdict(String),
    /// A verdict was given for a claim on an item that is not a check.
    NotACheck(String),
    /// A check, or a check's verdict, that cannot stand: why.
    InvalidCheck(String),
    /// An item dependency that cannot stand: why.
    InvalidDependency(String),
    /// Not a node name (empty, or with spaces).
    InvalidNode(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "context store i/o: {e}"),
            Self::Kinora(m) => write!(f, "context store: {m}"),
            Self::UnknownContext(id) => write!(f, "no context {id}"),
            Self::AmbiguousContext(prefix, n) => {
                write!(f, "{prefix} names {n} contexts: give more of the id")
            }
            Self::UnknownClaim(id) => write!(f, "no claim {id}"),
            Self::ClaimSettled(id) => write!(f, "claim {id} is already settled"),
            Self::ClaimExpired(id) => write!(f, "claim {id} is past its deadline"),
            Self::HeadMoved { claim, current } => write!(
                f,
                "the head moved since claim {claim} was taken: it is now {}",
                current.join(", ")
            ),
            Self::UnknownRepo { name, node } => {
                write!(f, "the context declares no repo {name} on {node}")
            }
            Self::InvalidRepo(why) => write!(f, "invalid repo: {why}"),
            Self::Mirror(why) => write!(f, "the context store mirror refused this machine: {why}"),
            Self::CheckNeedsVerdict(claim) => write!(
                f,
                "claim {claim} is on a check: submit it with a verdict (pass, fail, or snooze \
                 until a time) and a note"
            ),
            Self::NotACheck(claim) => write!(
                f,
                "claim {claim} is not on a check: submit it without a verdict"
            ),
            Self::InvalidCheck(why) => write!(f, "check: {why}"),
            Self::InvalidDependency(why) => write!(f, "item dependency: {why}"),
            Self::InvalidNode(node) => write!(f, "{node:?} is not a node name"),
            Self::ClaimNotHeld { claim, lease } => {
                write!(f, "claim {claim} is not held by lease {lease}")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
