//! Interned Sublime scope names, reversible scope stacks and allocation-free selectors.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    collections::HashMap,
    fmt,
    str::FromStr,
    sync::{LazyLock, Mutex, MutexGuard},
};

/// A process-wide, thread-safe atom identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Atom(u32);

/// An interned sequence of any number of atoms. IDs are cheap to copy.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Scope(u32);

/// Invalid scope or selector text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid scope or selector: {0}")]
pub struct ParseScopeError(pub String);

/// Scope interner. Use [`lock_global_scope_repo`] when interning batches.
#[derive(Debug, Default)]
pub struct ScopeRepository {
    atoms: HashMap<String, Atom>,
    names: Vec<String>,
    sequences: Vec<Vec<Atom>>,
    scopes: HashMap<String, Scope>,
}
static REPOSITORY: LazyLock<Mutex<ScopeRepository>> = LazyLock::new(|| {
    let mut repo = ScopeRepository::default();
    repo.names.push(String::new());
    repo.sequences.push(Vec::new());
    repo.scopes.insert(String::new(), Scope(0));
    Mutex::new(repo)
});

/// Locks the global interner, recovering its contents after a poisoned lock.
pub fn lock_global_scope_repo() -> MutexGuard<'static, ScopeRepository> {
    REPOSITORY.lock().unwrap_or_else(|e| e.into_inner())
}
impl ScopeRepository {
    /// Interns a dotted name without imposing a scope-depth limit.
    pub fn build(&mut self, text: &str) -> Result<Scope, ParseScopeError> {
        // Canonical names dominate deserialization and parser lookups. Avoid a
        // temporary split/join allocation when the exact name is already interned.
        if let Some(scope) = self.scopes.get(text) {
            return Ok(*scope);
        }
        let canonical = text
            .split('.')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(".");
        let text = canonical.as_str();
        if let Some(scope) = self.scopes.get(text) {
            return Ok(*scope);
        }
        if text.chars().any(char::is_whitespace) {
            return Err(ParseScopeError(text.into()));
        }
        let sequence = text
            .split('.')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let next = Atom(self.atoms.len() as u32);
                *self.atoms.entry(s.into()).or_insert(next)
            })
            .collect();
        let scope = Scope(self.names.len() as u32);
        self.names.push(text.into());
        self.sequences.push(sequence);
        self.scopes.insert(text.into(), scope);
        Ok(scope)
    }
    /// Resolves a name while the repository is already locked.
    pub fn to_string(&self, scope: Scope) -> String {
        self.names[scope.0 as usize].clone()
    }
    fn prefix(&self, a: Scope, b: Scope) -> bool {
        self.sequences[b.0 as usize].starts_with(&self.sequences[a.0 as usize])
    }
}
impl Scope {
    /// Interns a scope name.
    pub fn new(text: &str) -> Result<Self, ParseScopeError> {
        lock_global_scope_repo().build(text)
    }
    /// Returns the atom count.
    pub fn len(self) -> u32 {
        lock_global_scope_repo().sequences[self.0 as usize].len() as u32
    }
    /// Whether this is the empty scope.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    /// Returns a printable scope name.
    pub fn build_string(self) -> String {
        lock_global_scope_repo().to_string(self)
    }
    /// Whether all this scope's atoms prefix `other`.
    pub fn is_prefix_of(self, other: Scope) -> bool {
        lock_global_scope_repo().prefix(self, other)
    }
}
impl FromStr for Scope {
    type Err = ParseScopeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}
impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.build_string())
    }
}
impl fmt::Debug for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl Serialize for Scope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.build_string())
    }
}
impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Scope::new(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// How many active scopes a context temporarily clears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClearAmount {
    /// Clear all scopes.
    All,
    /// Clear at most the top N scopes.
    TopN(usize),
}
/// One compact scope-stack operation at a byte offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScopeStackOp {
    /// Push one scope.
    Push(Scope),
    /// Pop up to N scopes.
    Pop(usize),
    /// Save and remove scopes.
    Clear(ClearAmount),
    /// Restore the most recent clear.
    Restore,
    /// No operation.
    Noop,
}
/// Short name used by the line parser API.
pub type ScopeOp = ScopeStackOp;

/// Eight-byte form of a byte-indexed operation. Keeping the encoding with
/// `Scope` avoids exposing a constructor for arbitrary interned IDs.
#[derive(Clone, Copy, Debug)]
pub struct CompactScopeOp {
    offset_and_kind: u32,
    operand: u32,
}

impl CompactScopeOp {
    /// Returns None if the position or pop/clear count needs full width.
    #[inline]
    pub fn pack(at: usize, op: ScopeStackOp) -> Option<Self> {
        if at > (u32::MAX >> 3) as usize {
            return None;
        }
        let (kind, operand) = match op {
            ScopeStackOp::Push(Scope(id)) => (0, id),
            ScopeStackOp::Pop(n) => (1, u32::try_from(n).ok()?),
            ScopeStackOp::Clear(ClearAmount::TopN(n)) => (2, u32::try_from(n).ok()?),
            ScopeStackOp::Clear(ClearAmount::All) => (3, 0),
            ScopeStackOp::Restore => (4, 0),
            ScopeStackOp::Noop => (5, 0),
        };
        Some(Self {
            offset_and_kind: (at as u32) << 3 | kind,
            operand,
        })
    }
    /// Byte position within the line.
    #[inline]
    pub fn byte(self) -> usize {
        (self.offset_and_kind >> 3) as usize
    }
    /// Decodes the exact operation and byte position.
    #[inline]
    pub fn unpack(self) -> (usize, ScopeStackOp) {
        let op = match self.offset_and_kind & 7 {
            0 => ScopeStackOp::Push(Scope(self.operand)),
            1 => ScopeStackOp::Pop(self.operand as usize),
            2 => ScopeStackOp::Clear(ClearAmount::TopN(self.operand as usize)),
            3 => ScopeStackOp::Clear(ClearAmount::All),
            4 => ScopeStackOp::Restore,
            _ => ScopeStackOp::Noop,
        };
        (self.byte(), op)
    }
}

/// An invalid restore operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("scope restore without a corresponding clear")]
pub struct ScopeError;
/// A scope stack and the scopes saved by nested clears.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ScopeStack {
    /// Active scopes, from outermost to innermost.
    pub scopes: Vec<Scope>,
    clear_stack: Vec<Vec<Scope>>,
}
impl ScopeStack {
    /// Creates an empty stack.
    pub fn new() -> Self {
        Self::default()
    }
    /// Creates a stack without pending clears.
    pub fn from_vec(scopes: Vec<Scope>) -> Self {
        Self {
            scopes,
            clear_stack: Vec::new(),
        }
    }
    /// Active scopes.
    pub fn as_slice(&self) -> &[Scope] {
        &self.scopes
    }
    /// Number of active scopes.
    pub fn len(&self) -> usize {
        self.scopes.len()
    }
    /// Whether no scopes are active.
    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }
    /// Whether a later restore can recover hidden scopes (even an empty clear).
    pub fn has_pending_clears(&self) -> bool {
        !self.clear_stack.is_empty()
    }
    /// Pushes a scope.
    pub fn push(&mut self, scope: Scope) {
        self.scopes.push(scope);
    }
    /// Pops a scope if available.
    pub fn pop(&mut self) {
        self.scopes.pop();
    }
    /// Applies one parser operation, preserving nested clear/restore pairs.
    pub fn apply(&mut self, op: &ScopeStackOp) -> Result<(), ScopeError> {
        match *op {
            ScopeStackOp::Push(s) => self.push(s),
            ScopeStackOp::Pop(n) => self.scopes.truncate(self.len().saturating_sub(n)),
            ScopeStackOp::Clear(n) => {
                let at = match n {
                    ClearAmount::All => 0,
                    ClearAmount::TopN(n) => self.len().saturating_sub(n),
                };
                self.clear_stack.push(self.scopes.split_off(at));
            }
            ScopeStackOp::Restore => {
                let saved = self.clear_stack.pop().ok_or(ScopeError)?;
                self.scopes.extend(saved);
            }
            ScopeStackOp::Noop => {}
        }
        Ok(())
    }
}
impl FromStr for ScopeStack {
    type Err = ParseScopeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from_vec(
            s.split_whitespace()
                .map(Scope::new)
                .collect::<Result<_, _>>()?,
        ))
    }
}

/// Selector specificity; higher scores prefer deeper, more specific scope paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchScore(pub u128);
#[derive(Debug, Clone)]
enum Expr {
    Path(Vec<(Option<Scope>, usize)>),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}
/// Parsed Sublime selector. Matching never allocates.
#[derive(Debug, Clone)]
pub struct Selector(Expr);
impl Selector {
    /// Number of successive stack depths that get distinct specificity weights.
    pub const UNSATURATED_DEPTHS: usize = 21;

    /// Parses the complete logical selector syntax including immediate-child `>`.
    pub fn parse(text: &str) -> Result<Self, ParseScopeError> {
        text.parse()
    }
    /// Returns the selector's score on a scope stack, or `None`.
    pub fn does_match(&self, scopes: &[Scope]) -> Option<MatchScore> {
        self.does_match_in(scopes, &lock_global_scope_repo())
    }
    /// Matches using an already locked scope repository.
    pub fn does_match_in(&self, scopes: &[Scope], repo: &ScopeRepository) -> Option<MatchScore> {
        self.0.eval(scopes, repo).map(MatchScore)
    }
    /// Whether every alternative matches a single scope with positive
    /// specificity below one depth step. Below saturation, any match one
    /// level deeper out-ranks all shallower matches of this form.
    pub fn is_depth_ordered(&self, repo: &ScopeRepository) -> bool {
        fn eligible(expr: &Expr, repo: &ScopeRepository) -> bool {
            match expr {
                Expr::Path(path) if path.len() == 1 => path[0]
                    .0
                    .is_none_or(|s| (1..64).contains(&repo.sequences[s.0 as usize].len())),
                Expr::Or(a, b) => eligible(a, repo) && eligible(b, repo),
                _ => false,
            }
        }
        eligible(&self.0, repo)
    }
    /// True when the selector accepts a stack.
    pub fn matches(&self, scopes: &[Scope]) -> bool {
        self.does_match(scopes).is_some()
    }
}
impl Expr {
    fn eval(&self, stack: &[Scope], repo: &ScopeRepository) -> Option<u128> {
        match self {
            Self::Path(path) => path_score(path, stack, repo, path.len(), stack.len(), None),
            Self::Not(e) => {
                if e.eval(stack, repo).is_none() {
                    Some(1)
                } else {
                    None
                }
            }
            Self::And(a, b) => Some(a.eval(stack, repo)?.max(b.eval(stack, repo)?)),
            Self::Or(a, b) => a
                .eval(stack, repo)
                .into_iter()
                .chain(b.eval(stack, repo))
                .max(),
        }
    }
}
fn path_score(
    path: &[(Option<Scope>, usize)],
    stack: &[Scope],
    repo: &ScopeRepository,
    n: usize,
    before: usize,
    exact: Option<usize>,
) -> Option<u128> {
    if n == 0 {
        return Some(1);
    }
    let (scope, distance) = path[n - 1];
    for at in (0..before).rev() {
        if exact.is_some_and(|target| at != target)
            || scope.is_some_and(|s| !repo.prefix(s, stack[at]))
        {
            continue;
        }
        let prefix = if n == 1 {
            Some(1)
        } else if distance > 0 {
            at.checked_sub(distance)
                .and_then(|previous| path_score(path, stack, repo, n - 1, at, Some(previous)))
        } else {
            path_score(path, stack, repo, n - 1, at, None)
        };
        if let Some(prefix) = prefix {
            // ST-BEHAVIOR: unverified numeric score beyond documented specificity ordering.
            let atoms = scope.map_or(1, |s| repo.sequences[s.0 as usize].len()) as u128;
            return Some(prefix.saturating_add(
                atoms.saturating_mul(1u128 << (at.min(Selector::UNSATURATED_DEPTHS - 1) * 6)),
            ));
        }
    }
    None
}

impl FromStr for Selector {
    type Err = ParseScopeError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut parser = SelectorParser { text, at: 0 };
        let expr = parser.or(false)?;
        parser.ws();
        if parser.at != text.len() {
            return Err(ParseScopeError(text.into()));
        }
        Ok(Self(expr))
    }
}
struct SelectorParser<'a> {
    text: &'a str,
    at: usize,
}
impl SelectorParser<'_> {
    fn ws(&mut self) {
        while self.text[self.at..].starts_with(char::is_whitespace) {
            self.at += self.text[self.at..].chars().next().unwrap().len_utf8();
        }
    }
    fn eat(&mut self, c: char) -> bool {
        self.ws();
        if self.text[self.at..].starts_with(c) {
            self.at += c.len_utf8();
            true
        } else {
            false
        }
    }
    fn or(&mut self, pipe: bool) -> Result<Expr, ParseScopeError> {
        let mut e = if pipe { self.and()? } else { self.or(true)? };
        loop {
            let found = if pipe { self.eat('|') } else { self.eat(',') };
            if !found {
                break;
            }
            let rhs = if pipe { self.and()? } else { self.or(true)? };
            e = Expr::Or(Box::new(e), Box::new(rhs));
        }
        Ok(e)
    }
    fn and(&mut self) -> Result<Expr, ParseScopeError> {
        let mut e = self.minus()?;
        while self.eat('&') {
            e = Expr::And(Box::new(e), Box::new(self.minus()?));
        }
        Ok(e)
    }
    fn minus(&mut self) -> Result<Expr, ParseScopeError> {
        let mut e = if self.eat('-') {
            Expr::Not(Box::new(self.primary()?))
        } else {
            self.primary()?
        };
        while self.eat('-') {
            e = Expr::And(Box::new(e), Box::new(Expr::Not(Box::new(self.primary()?))));
        }
        Ok(e)
    }
    fn primary(&mut self) -> Result<Expr, ParseScopeError> {
        if self.eat('(') {
            let e = self.or(false)?;
            if !self.eat(')') {
                return Err(ParseScopeError(self.text.into()));
            }
            return Ok(e);
        }
        let mut path = Vec::new();
        let mut distance = 0;
        loop {
            self.ws();
            let rest = &self.text[self.at..];
            if rest.is_empty() || rest.starts_with(['|', ',', '&', ')', '-']) {
                break;
            }
            if self.eat('>') {
                distance += 1;
                continue;
            }
            let begin = self.at;
            while self.at < self.text.len() {
                let c = self.text[self.at..].chars().next().unwrap();
                if c.is_whitespace() || "|,&()>".contains(c) {
                    break;
                }
                self.at += c.len_utf8();
            }
            if begin == self.at {
                return Err(ParseScopeError(self.text.into()));
            }
            let name = &self.text[begin..self.at];
            path.push((
                if name == "*" {
                    None
                } else {
                    Some(Scope::new(name)?)
                },
                distance,
            ));
            distance = 0;
        }
        if distance > 0 {
            return Err(ParseScopeError(self.text.into()));
        }
        Ok(Expr::Path(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unlimited_scopes_and_reversible_clear() {
        let s = Scope::new(
            "keyword.operator.assignment.augmented.bitwise.shift.left.unsigned.actionscript",
        )
        .unwrap();
        assert_eq!(s.len(), 9);
        let mut stack = ScopeStack::from_vec(vec![s]);
        stack.apply(&ScopeOp::Clear(ClearAmount::All)).unwrap();
        stack.apply(&ScopeOp::Clear(ClearAmount::TopN(3))).unwrap();
        stack.apply(&ScopeOp::Restore).unwrap();
        stack.apply(&ScopeOp::Restore).unwrap();
        assert_eq!(stack.as_slice(), &[s]);
    }
    #[test]
    fn selector_operators() {
        let stack: ScopeStack = "source.php meta.block.php keyword.control.php"
            .parse()
            .unwrap();
        for s in [
            "keyword",
            "meta keyword",
            "meta > keyword",
            "source > > keyword",
            "(source | text) & keyword - comment",
            "- comment",
        ] {
            assert!(Selector::parse(s).unwrap().matches(stack.as_slice()), "{s}");
        }
        for s in [
            "control",
            "keyword meta",
            "source > keyword",
            "source - (keyword | string)",
        ] {
            assert!(
                !Selector::parse(s).unwrap().matches(stack.as_slice()),
                "{s}"
            );
        }
    }
}

#[cfg(test)]
mod child_regression {
    use super::*;
    #[test]
    fn adjacent_edges_cannot_backtrack_to_an_earlier_parent() {
        let stack: ScopeStack = "outer source source keyword".parse().unwrap();
        assert!(
            !Selector::parse("outer > source > keyword")
                .unwrap()
                .matches(stack.as_slice())
        );
        assert!(
            Selector::parse("outer > source > > keyword")
                .unwrap()
                .matches(stack.as_slice())
        );
    }
}
