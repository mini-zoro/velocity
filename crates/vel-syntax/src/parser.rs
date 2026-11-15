// Adapted from syntect PR #705 (MIT); see README.md and LICENSE-syntect.
// Suppression of a false positive clippy lint. Upstream issue:
//
//   mutable_key_type false positive for raw pointers
//   https://github.com/rust-lang/rust-clippy/issues/6745
//
// We use `*const MatchPattern` as key in our `SearchCache` hash map.
// Clippy thinks this is a problem since `MatchPattern` has interior mutability
// via `MatchPattern::regex::regex` which is an `AtomicLazyCell`.
// But raw pointers are hashed via the pointer itself, not what is pointed to.
// See https://github.com/rust-lang/rust/blob/1.54.0/library/core/src/hash/mod.rs#L717-L725
#![allow(clippy::mutable_key_type)]
// The interpreter phases pass one shared token transaction through semantic helpers.
#![allow(clippy::too_many_arguments)]

use super::syntax_definition::*;
use crate::syntax_definition::ContextId;
use crate::syntax_set::{SyntaxReference, SyntaxSet};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use vel_regex::escape_literal as escape;
use vel_regex::{Regex, Region};
use vel_scope::*;

/// Byte-indexed scope operations for one line.
pub type ScopeOperations = Vec<(usize, ScopeStackOp)>;
/// Corrected operations tagged with one-based source line numbers.
pub type ReplayedOps = Vec<(usize, ScopeOperations)>;

/// Errors that can occur while parsing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ParsingError {
    #[error("Somehow main context was popped from the stack")]
    /// The grammar popped its protected main context.
    MissingMainContext,
    /// A context is missing. Usually caused by a syntax referencing a another
    /// syntax that is not known to syntect. See e.g. <https://github.com/trishume/syntect/issues/421>
    #[error("Missing context with ID '{0:?}'")]
    MissingContext(ContextId),
    #[error("Bad index to match_at: {0}")]
    /// A pattern index does not identify a match rule.
    BadMatchIndex(usize),
    #[error("Tried to use a ContextReference that has not been resolved yet: {0:?}")]
    /// A requested local or external context could not be resolved.
    UnresolvedContextReference(ContextReference),
}

/// Keeps the current parser state (the internal syntax interpreter stack) between lines of parsing.
///
/// If you are parsing an entire file you create one of these at the start and use it
/// all the way to the end.
///
/// # Caching
///
/// One reason this is exposed is that since it implements `Clone` you can actually cache
/// these (probably along with a [`HighlightState`]) and only re-start parsing from the point of a change.
/// See the docs for [`HighlightState`] for more in-depth discussion of caching.
///
/// This state doesn't keep track of the current scope stack and parsing only returns changes to this stack
/// so if you want to construct scope stacks you'll need to keep track of that as well.
/// Note that [`HighlightState`] contains exactly this as a public field that you can use.
///
/// **Note:** Caching is for advanced users who have tons of time to maximize performance or want to do so eventually.
/// It is not recommended that you try caching the first time you implement highlighting.
///
/// [`HighlightState`]: ../highlighting/struct.HighlightState.html
/// Output of [`InterpreterState::parse_line`].
///
/// `ops` contains the scope-stack operations for the current line, as before.
/// `replayed` is non-empty only after a cross-line `fail` fires: it contains
/// the corrected ops for each buffered line (in chronological order) so that
/// callers who want full cross-line accuracy can re-apply them.
///
/// Callers that do not need cross-line accuracy can use `.ops` directly,
/// which behaves identically to the old `Vec<(usize, ScopeStackOp)>` return.
#[derive(Debug, Clone, Default)]
pub struct ParseLineOutput {
    /// Ops for the current line.
    pub ops: Vec<(usize, ScopeStackOp)>,
    /// Corrections to lines already handed back, each tagged with the 1-based
    /// number of the line it replaces. Non-empty only when a cross-line `fail`
    /// just resolved.
    ///
    /// The line number is ours. Upstream's is positional, and
    /// a second cross-line fail on one `parse_line` appends another run to it, so
    /// pairing the i-th entry with the i-th buffered line puts one line's ops on
    /// another's text.
    pub replayed: ReplayedOps,
    /// Warnings collected during parsing (e.g. branch point expiry).
    pub warnings: Vec<String>,
}

/// How often one zero-width escape may fire at the same byte
/// of a line before the parser stops believing it.
///
/// See the field it bounds, [`InterpreterState::zero_width_escape_fires`]. High
/// enough that a legitimate alt-cycle replay through the same offset — what
/// Markdown's `$$ … $$` maths blocks do under an embedded-LaTeX branch fail —
/// never reaches it.
const ZERO_WIDTH_ESCAPE_FIRE_LIMIT: u32 = 100;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct InterpreterState {
    stack: SharedStack,
    // Immutable document-relative base; resolving by name per token is expensive.
    base_context: ContextId,
    // Sticky optimization flag, excluded from stable semantic equality/fingerprints.
    has_while: bool,
    first_line: bool,
    // See issue #101. Contains indices of frames pushed by `with_prototype`s.
    // Doesn't look at `with_prototype`s below top of stack.
    proto_starts: Vec<usize>,
    /// Active branch points for backtracking support.
    branch_points: Vec<BranchPoint>,
    /// Line counter for 128-line branch point expiry.
    line_number: usize,
    /// Line strings buffered while branch points are active, for potential
    /// cross-line `fail` replay. Only the strings are stored; the ops are
    /// returned to callers immediately (same as before).
    pending_lines: Vec<String>,
    /// How many times a zero-width escape has fired at each
    /// byte of the line being parsed.
    ///
    /// Perl's POD declares six embedded language sections gated by
    /// `escape: (?=^{{pod}})` — a zero-width lookahead anchored to the start of
    /// a line. Against the embedded languages' own branch points that is an
    /// unbounded cycle: an alternative fails, the rewind restores the embed's
    /// escape, the alternative runs again, the same zero-width escape fires at
    /// the same byte, the escape pops back without advancing, and round again.
    /// Upstream's own test corpus hangs on `syntax_test_perl.pl` — their
    /// harness skips the file — and an editor cannot skip a file: the worker
    /// would spin on a core forever and the buffer would never colour.
    ///
    /// So a fire at a position that has already fired
    /// [`ZERO_WIDTH_ESCAPE_FIRE_LIMIT`] times is suppressed and the cursor
    /// advances one character, which is exactly what the `would_loop` guard
    /// below does for the push/pop version of the same shape. Cleared per
    /// `parse_line_inner_from`, so a cross-line replay re-entering the same
    /// offset starts fresh, and deliberately *not* snapshotted into a
    /// `BranchPoint`: the cycle to detect is the one a fail restores, so the
    /// count has to survive the fail to converge.
    ///
    /// This is upstream's diagnosis and upstream's fix, from the open PR #692
    /// against issue #650, taken alone — that PR carries several other changes
    /// on top of unmerged branches. It comes back out when upstream merges it.
    zero_width_escape_fires: HashMap<usize, u32>,
    /// The branch points whose alternatives have all run
    /// out at a byte of the line being parsed, by name.
    ///
    /// When a same-line `branch_point` exhausts at a zero-width lookahead,
    /// upstream advances one character past the lookahead's match so the same
    /// `(?=…)` cannot fire again. That is not what Sublime does, and the
    /// character it steps over is a character the parent context never gets to
    /// match: a stale keyword rule then matches *inside* an identifier —
    /// `package` in `$package`, `class` in `Foo.class;` — because the cursor
    /// landed in the middle of one.
    ///
    /// What Sublime does is fall through: the cursor stays where the branch
    /// matched and the parent context's *next* rule is given the position.
    /// So the cursor is rewound to the branch's own start and the branch's
    /// name is recorded here, and [`InterpreterState::find_best_match`] skips a
    /// `Branch` of that name at that byte — the one thing that would
    /// immediately re-enter the branch that just ran out. Everything else in
    /// the context is searched as usual.
    ///
    /// Entries are byte offsets into one line and never outlive it. They are
    /// dropped as the cursor advances past them, and if nothing at all matches
    /// at the rewound position, `parse_next_token` drops the entry and
    /// advances one character — upstream's behaviour, kept as the last resort
    /// it always was rather than the first answer.
    ///
    /// This is upstream's open PR #664 against issue #631, taken alone; it
    /// comes back out when upstream merges it.
    skipped_branches: Vec<(usize, String)>,
    /// While a cross-line `fail` is replaying a buffered
    /// line, the line that line actually *is*.
    ///
    /// `line_number` counts calls to `parse_line`, and a replay does not make
    /// one — so during a replay it still names the line the parser is really
    /// on, several lines later. A `branch_point` created by the replayed text
    /// was then stamped with that number, and the next `fail` for it compared
    /// the two, found them equal, and took the *same-line* path: it rewound
    /// the cursor to a position belonging to the replayed line and looked for
    /// the next reading there, instead of replaying that line again under it.
    ///
    /// Markdown is the file. An unterminated code span before a setext
    /// underline — CommonMark's example 91, `` `Foo `` then `----` — resolves
    /// through two nested branch points; the second one is created during the
    /// first one's replay, and the `fail` that follows on the *next* line was
    /// read as same-line. The underline came out with no scopes at all, and
    /// `markup.heading.2.markdown` was left on the stack for the rest of the
    /// document.
    replay_line: Option<Replay>,
    /// A replay asked for by a `fail` that fired *inside* a
    /// replay, for the driver to run once the current one has stopped.
    ///
    /// A cross-line `fail` used to run its own replay wherever it fired,
    /// including in the middle of one. Two loops were then walking the same
    /// buffered lines: the inner finished, the outer carried on from where it
    /// was and re-emitted its own reading over the lines the inner had just
    /// corrected. Corrections are keyed by line and the last one wins, so the
    /// *abandoned* alternative won — `syntax_test_scope_C#7.cs:620`, a tuple
    /// type spread over four lines, came out `meta.cast.cs meta.group.cs`
    /// where Sublime Text has `meta.sequence.tuple.cs` (probe
    /// `csharp_tuple_type`). Alternatives 3 and 4 of the `parens` branch both
    /// replayed inside alternative 2's replay; 4 was the right reading and 2's
    /// remaining iterations wrote over it.
    ///
    /// So a nested `fail` does the state work — restore the snapshot, advance
    /// the branch point — and then stops its line and leaves this behind. One
    /// driver runs the rounds in order, and because each round advances a
    /// branch point's `next_alternative` or removes it, the rounds are bounded
    /// by the alternatives that exist. Breaking out of the nested loops and
    /// keeping the recursion was tried instead and took C# from seconds to past
    /// the corpus budget.
    replay_pending: Option<ReplayRequest>,
    /// Corrected ops produced by a cross-line `fail` replay, to be returned
    /// as `ParseLineOutput::replayed` at the end of `parse_line`.
    /// each correction says which line it is for — the
    /// parser's own 1-based `line_number` — instead of being positional.
    ///
    /// Upstream appends here, with the comment "in case multiple cross-line
    /// fails fire on the same parse_line call", and hands the result out as a
    /// bare `Vec<Vec<_>>` whose i-th entry the consumer is expected to pair with
    /// the i-th buffered line. Those two cannot both be right: a second fail on
    /// the same line appends a second run, so the positional mapping slides ops
    /// from one line onto another's text. Upstream's #655 is that bug, reported
    /// as a panic; measured here it turned ShellScript from 893 failing
    /// assertions into 2,691, off 41 corrections.
    ///
    /// Naming the line removes the whole class: a later correction for a line
    /// simply replaces an earlier one, and no consumer has to reconstruct which
    /// lines were buffered.
    flushed_ops: ReplayedOps,
    /// Warnings accumulated during parsing, drained into `ParseLineOutput`.
    warnings: Vec<String>,
    /// Active escape patterns from embed operations. The escape regex takes
    /// strict precedence over normal patterns — it is checked first and can
    /// truncate the search region.
    escape_stack: Vec<EscapeEntry>,
}

/// A resolved escape pattern from an `embed` operation, stored on the escape stack.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct EscapeEntry {
    /// The resolved escape regex (backrefs substituted at push time).
    regex: Regex,
    /// Capture mapping for escape_captures scopes.
    captures: Option<CaptureMapping>,
    /// Stack depth at the time of the embed push — when escape fires,
    /// pop down to this depth.
    stack_depth: usize,
    /// The frame the `embed:` was declared in,
    /// when a `pop: N` took it off before the embed went on — the escape's own
    /// text still wears its meta scopes. `None` when it is still on the stack,
    /// which is where the escape finds it anyway.
    holder: Option<(ContextId, usize)>,
}

/// Snapshot of parser state at a branch point, used for backtracking.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct BranchPoint {
    trigger_captures: (Region, String),
    name: String,
    /// Index of the next alternative to try (0 = first alt already tried).
    next_alternative: usize,
    alternatives: Vec<ContextReference>,
    stack_snapshot: SharedStack,
    proto_starts_snapshot: Vec<usize>,
    /// Character position to rewind to. Despite the name, this is the
    /// branch match's *end* position — where the parser resumes from.
    match_start: usize,
    /// Real start of the branch_point match text. Together with
    /// `match_start` (above, which is the match end) this bounds the
    /// span on which `pat_scope` applies. Used to re-emit the keyword
    /// scope — e.g. `keyword.operator.comparison.sql` on `LIKE` —
    /// after a `fail` rewind, since the original Push/Pop pair was
    /// truncated off `ops` along with `alt[0]`'s subsequent work.
    trigger_match_start: usize,
    /// Scopes declared on the branch_point match itself (re-emitted on
    /// fail-retry over the [`trigger_match_start`, `match_start`) span).
    pat_scope: Vec<Scope>,
    /// Line number when the branch was created (for 128-line limit).
    line_number: usize,
    /// Length of ops vec at snapshot time — truncation point on fail.
    ops_snapshot_len: usize,
    /// Stack depth at creation — if stack shrinks below this, branch is invalid.
    stack_depth: usize,
    non_consuming_push_at_snapshot: (usize, usize, usize),
    first_line_snapshot: bool,
    with_prototype: Option<ContextReference>,
    /// `pending_lines.len()` at snapshot time, for cross-line replay truncation.
    pending_lines_snapshot_len: usize,
    escape_stack_snapshot: Vec<EscapeEntry>,
    /// Number of contexts to pop before pushing the alternative (for pop + branch).
    pop_count: usize,
    /// Ops emitted on the branch-creation line before the branch match.
    /// Used by cross-line fail replay to reconstruct the first buffered
    /// line without re-parsing its pre-branch prefix under the new
    /// alternative (which would misattribute pre-branch content to
    /// rules of the new alternative — e.g. in multi-line SQL `LIKE …
    /// ESCAPE …`, every non-whitespace before the `LIKE` fires
    /// `else-pop` in the escape-alternative, derailing the stack).
    prefix_ops: Vec<(usize, ScopeStackOp)>,
    /// Capture Push/Pop ops emitted alongside the branch_point match's
    /// `pat_scope`. Re-emitted on fail-retry between the pat_scope
    /// Push and Pop so captures like `keyword.declaration.data.haskell`
    /// on the first capture group of `(data)(?:\s+(family|instance))?`
    /// survive a branch swap — without this, a `data CtxCls ctx => …`
    /// (where `alt[0]` `data-signature` fails into `alt[1]` `data-context`)
    /// drops the keyword scope from the `data` token.
    capture_ops: Vec<(usize, ScopeStackOp)>,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct StateLevel {
    context: ContextId,
    prototypes: Vec<ContextId>,
    captures: Option<(Region, String)>,
    /// How many of this frame's `meta_content_scope` atoms
    /// were never pushed, because the context under it is a v2 `embed_scope`
    /// wrapper and the host's `embed_scope` *replaces* the embedded syntax's
    /// own scope.
    ///
    /// It has to be remembered rather than worked out later. "The frame below
    /// me has `embed_scope_replaces`" is only the same question at the moment
    /// of the push: a context that arrives on top of a wrapper later — pushed
    /// by a rule of the guest's, not by the embed — did push its own scope,
    /// and asking the stack cannot tell the two apart. Guessing it cost Python
    /// 46 → 98 failing assertions.
    ///
    /// A *count*, and almost always 0 or 1, because what `embed_scope`
    /// replaces is the embedded syntax's own `scope:` — the file scope
    /// `add_initial_contexts` puts at the front of `main`'s
    /// `meta_content_scope` — and nothing else that context happens to carry.
    /// See `mcs_replaced_by_embed_scope`.
    mcs_skipped: usize,
}

/// How many of a context's `meta_content_scope` atoms a v2
/// `embed_scope` stands in for.
///
/// Version 2 says `embed_scope` *replaces* the embedded syntax's scope. That
/// scope is one atom, and it is on the stack because `add_initial_contexts`
/// inserts it at the front of `main`'s `meta_content_scope`. So an embed that
/// enters `main` replaces exactly that atom — and an embed that names a
/// *context* of the guest instead (`embed: scope:source.toml…#toml`) replaces
/// nothing, because that context's `meta_content_scope` is the guest's own
/// (`meta.mapping.toml`) and has nothing to do with the file scope.
///
/// Skipping the whole `meta_content_scope` on `embed_scope_replaces` alone was
/// Python's PEP 723 block: TOML's `toml` context lost `meta.mapping.toml`, and
/// then `table-key-accessor`'s `clear_scopes: 1` took `source.toml.embedded.python`
/// off instead — the atom every assertion in the block is about.
fn mcs_replaced_by_embed_scope(
    replaces: bool,
    context_id: &ContextId,
    context: &Context,
    syntax_set: &SyntaxSet,
) -> usize {
    if !replaces {
        return 0;
    }
    let scope = syntax_set.syntaxes()[context_id.syntax_index].scope;
    usize::from(context.meta_content_scope.first() == Some(&scope))
}

/// Whether a branch point's alternative is still on the stack.
///
/// `stack_depth` is snapshotted *before* a `pop: N + branch:` does its pop, so
/// the alternative's own frame sits at `stack_depth - pop_count + 1` and is
/// present exactly while `stack_len > stack_depth - pop_count`. `handle_fail`
/// has always asked it that way; `exec_escape` asked
/// `stack_depth <= stack_len`, which for `N >= 2` prunes a branch point whose
/// alternative an embed escape left standing — and a later `fail` naming it
/// then silently does nothing.
///
/// That is the same failure as and the same predicate class as
/// upstream's #660: three ways of asking one question, one of which quietly
/// says no. So there is one place to ask it, and both callers ask here.
fn branch_point_is_live(bp: &BranchPoint, stack_len: usize) -> bool {
    stack_len > bp.stack_depth.saturating_sub(bp.pop_count)
}

/// Which of a `set:`'s targets may emit its `clear_scopes`
/// *before* the trigger token rather than after it, if any.
///
/// A `set:` pushes its targets' `meta_scope`s in the initial phase, so the
/// trigger text sees them, and their `meta_content_scope`s only in the
/// non-initial phase, after it. A `clear_scopes` on one of those targets is
/// therefore emittable early exactly when everything it can eat is already on
/// the stack by then: nothing of the *leaving* context sitting on top
/// (`cur_context`'s own meta scopes come off in the non-initial phase), and no
/// target *before* it in the list contributing a `meta_content_scope` that has
/// not been pushed yet.
///
/// Zsh is what this is for. `redirection-input` fires `(\d*)(<)` with
/// `set: [string-path-pattern-body, zsh-redirection-glob-range-end, …]`, and
/// the second of those carries `clear_scopes: 1  # clear string.unquoted` —
/// the atom the *first* one's `meta_scope` just pushed. Deferring that Clear
/// left `string.unquoted.shell` on the `<` itself and cleared it one column
/// on; ST scopes that column
/// `meta.string.glob.shell meta.range.shell.zsh punctuation.definition.range.begin.shell.zsh`,
/// with no `string`, asked directly (a `shellscript`
/// probe).
///
/// Makefile is why the `meta_content_scope` half is a condition and not an
/// assumption: `set: [value-to-be-defined, eat-whitespace-then-pop]` puts the
/// `clear_scopes: 1` on the *last* target, and what it is meant to eat is
/// `value-to-be-defined`'s `meta_content_scope`, which does not exist yet in
/// the initial phase. That one still belongs after the token.
fn early_set_clear(
    is_set: bool,
    cur_context: &Context,
    context_refs: &[ContextReference],
    syntax_set: &SyntaxSet,
) -> Result<Option<usize>, ParsingError> {
    if !is_set || !cur_context.meta_scope.is_empty() || !cur_context.meta_content_scope.is_empty() {
        return Ok(None);
    }
    for (i, r) in context_refs.iter().enumerate() {
        let ctx = r.resolve(syntax_set)?;
        if ctx.clear_scopes.is_some() {
            return Ok(Some(i));
        }
        if !ctx.meta_content_scope.is_empty() {
            return Ok(None);
        }
    }
    Ok(None)
}

/// Which line a cross-line replay is currently re-reading.
///
/// See [`InterpreterState::replay_line`]. Both halves are needed and neither can be
/// derived from the parser's counters, because a replay does not advance them.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct Replay {
    /// The line's own number, counting from 0, as `line_number - 1` would give
    /// at the top level.
    line_number: usize,
    /// Its index in `pending_lines`, so a branch point created on it can
    /// replay it rather than the lines after it.
    pending_index: usize,
    /// What the enclosing replay has already composed for this line, before
    /// the point the re-parse resumes from.
    ///
    /// A branch point created inside a replay sees an `ops` that starts empty
    /// at the resume position, so its own `prefix_ops` would describe only the
    /// tail of the line. Its correction then *replaces* the enclosing one —
    /// corrections are keyed by line — and everything before the resume point
    /// is lost. In Markdown that came out as an empty correction for the line
    /// the heading was on, and the next line's ops popped a stack nothing had
    /// pushed.
    prefix: Vec<(usize, ScopeStackOp)>,
}

/// One replay to run — everything a cross-line `fail` needs
/// in order to re-read the buffered lines under the reading it has just chosen.
///
/// Both cross-line paths reduce to this. They differ only in how `prefix` and
/// `resume_at` are worked out: an exhausted branch point re-emits the ops the
/// creation line already had and resumes one character past the branch match,
/// so the same branch cannot fire again; a next alternative re-emits those ops
/// plus the trigger's own scopes and the alternative's meta scopes, and resumes
/// at the end of the trigger match. Past that point the work is the same, which
/// is why it is one function — see `InterpreterState::run_replay`.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct ReplayRequest {
    /// Where in `pending_lines` the replay starts. Its first entry is the line
    /// the branch point was created on.
    from: usize,
    /// What that first line already has, verbatim, before `resume_at`.
    prefix: Vec<(usize, ScopeStackOp)>,
    /// Where the re-parse of that first line picks up.
    resume_at: usize,
    /// A branch point that ran out at `resume_at`, whose
    /// `Branch` must not be tried again there. See `skipped_branches`: this is
    /// that fall-through, for the branch that ran out on an *earlier* line.
    skip: Option<(usize, String)>,
}

#[derive(Debug)]
struct RegexMatch<'a> {
    regions: Region,
    context: &'a Context,
    pat_index: usize,
    from_with_prototype: bool,
    would_loop: bool,
    /// For escape matches (pat_index == usize::MAX): index into escape_stack.
    escape_index: usize,
}

/// Maps the pattern to the start index, which is -1 if not found.
type SearchCache = vel_regex::SearchCache;

thread_local! {
    // Scratch is not semantic parser state. A pool permits recursive branch replay to
    // borrow a second workspace without aliasing the caller's captures/search entries.
    static SEARCH_WORKSPACES: std::cell::RefCell<Vec<(SearchCache, Region)>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

/// What one `push:` of several contexts clears in
/// a version 1 syntax: all of their `clear_scopes` added up, applied once,
/// before any of their meta scopes.
///
/// Sublime lists it under Compatibility as "Multiple Target Push Actions with
/// `clear_scopes`", and its example is a `push: [context2, context3]` where
/// both carry `clear_scopes: 1`: version 1 takes two atoms off before pushing
/// either `meta_scope`, so `abc` comes out `meta.ctx2 meta.ctx3 identifier`
/// with the file's own `source.lang` cleared away, where version 2 interleaves
/// them — clear, push, clear, push — and leaves `source.lang meta.ctx3
/// identifier`. ST 4206 answers exactly that at both versions
/// (the `VerMultiClearV1.sublime-syntax` probe, the
/// `multiclear_v1` and `multiclear_v2` jobs); this emitted version 2's
/// interleaving whatever the file said.
///
/// `All` swallows the rest, being already everything there is.
fn summed_push_clear(
    context_refs: &[ContextReference],
    syntax_set: &SyntaxSet,
) -> Result<Option<ClearAmount>, ParsingError> {
    let mut total = 0;
    for r in context_refs.iter() {
        match r.resolve(syntax_set)?.clear_scopes {
            Some(ClearAmount::All) => return Ok(Some(ClearAmount::All)),
            Some(ClearAmount::TopN(n)) => total += n,
            None => {}
        }
    }
    Ok((total > 0).then_some(ClearAmount::TopN(total)))
}

/// Build the ordered Push/Pop ops for a match's `captures:` mapping over
/// its regex `regions`. Captures can appear in arbitrary source order
/// (e.g. `((bob)|(hi))*` matching `hibob` — the outer group must Push
/// before any inner group). Empty captures are skipped because they'd
/// otherwise sort a Pop before its Push. The returned ops are already
/// position-ordered and safe to append to a parser ops vec.
fn build_capture_ops(
    capture_map: &CaptureMapping,
    regions: &Region,
    version: u32,
) -> smallvec::SmallVec<[(usize, ScopeStackOp); 8]> {
    // In a version 1 syntax a capture that goes
    // *backwards* gets no scope at all, and this applied every one of them.
    //
    // Sublime lists it under Compatibility as "Regex Capture Group Order", and
    // its own example is `(?:(x)|(y))+` over `yx`: group 2 matches the `y` at
    // column 0 and group 1 the `x` at column 1. **The documented result is the
    // wrong way round** — it says `x` loses its scope — and ST 4206 answers the
    // opposite (the `VerCaptureOrderV1.sublime-syntax` probe,
    // the `captures_v1` job): `y` is bare and `x` keeps `identifier.x`. So the
    // rule is not about which group is lower-numbered, it is a *cursor*: the
    // groups are read in capture-index order and one that begins before the
    // last one it applied is dropped.
    //
    // Which cursor took a second probe, because the two readings that survive
    // that example disagree about the commonest case of all. Against the last
    // applied group's *end*, `((a)b)` would lose its inner group — group 1 is
    // [0,2) and group 2 begins at 1 — and half the version 1 definitions in
    // the set would quietly stop colouring their inner captures. Against its
    // *start*, nesting is untouched and only a genuine step backwards is
    // dropped. ST keeps the inner group (`VerCaptureNestV1`, the `nest_v1` and
    // `nest_siblings_v1` jobs), so it is the start.
    //
    // Version 2 applies all of them, which is what this did unconditionally.
    // The mapping is in the order the YAML hash was read, so the walk sorts by
    // capture index rather than trusting it, and only the groups that carry
    // scopes take part — an unscoped group is not something ST could report a
    // position for either.
    let mut ordered: smallvec::SmallVec<[&(usize, Vec<Scope>); 8]> = capture_map.iter().collect();
    if version < 2 {
        ordered.sort_by_key(|(cap_index, _)| *cap_index);
    }
    let mut last_start = 0;
    let mut map: smallvec::SmallVec<[((usize, i32), ScopeStackOp); 8]> = smallvec::SmallVec::new();
    for &(cap_index, ref scopes) in ordered.into_iter() {
        if let Some((cap_start, cap_end)) = regions.pos(cap_index) {
            if cap_start == cap_end {
                continue;
            }
            if version < 2 {
                if cap_start < last_start {
                    continue;
                }
                last_start = cap_start;
            }
            for scope in scopes.iter() {
                map.push((
                    (cap_start, -((cap_end - cap_start) as i32)),
                    ScopeStackOp::Push(*scope),
                ));
            }
            map.push(((cap_end, i32::MIN), ScopeStackOp::Pop(scopes.len())));
        }
    }
    map.sort_by_key(|a| a.0);
    map.into_iter().map(|((i, _), op)| (i, op)).collect()
}

// To understand the implementation of this, here's an introduction to how
// Sublime Text syntax definitions work.
//
// Let's say we have the following made-up syntax definition:
//
//     contexts:
//       main:
//         - match: A
//           scope: scope.a.first
//           push: context-a
//         - match: b
//           scope: scope.b
//         - match: \w+
//           scope: scope.other
//       context-a:
//         - match: a+
//           scope: scope.a.rest
//         - match: (?=.)
//           pop: true
//
// There are two contexts, `main` and `context-a`. Each context contains a list
// of match rules with instructions for how to proceed.
//
// Let's say we have the input string " Aaaabxxx". We start at position 0 in
// the string. We keep a stack of contexts, which at the beginning is just main.
//
// So we start by looking at the top of the context stack (main), and look at
// the rules in order. The rule that wins is the first one that matches
// "earliest" in the input string. In our example:
//
// 1. The first one matches "A". Note that matches are not anchored, so this
//    matches at position 1.
// 2. The second one matches "b", so position 5. The first rule is winning.
// 3. The third one matches "\w+", so also position 1. But because the first
//    rule comes first, it wins.
//
// So now we execute the winning rule. Whenever we matched some text, we assign
// the scope (if there is one) to the matched text and advance our position to
// after the matched text. The scope is "scope.a.first" and our new position is
// after the "A", so 2. The "push" means that we should change our stack by
// pushing `context-a` on top of it.
//
// In the next step, we repeat the above, but now with the rules in `context-a`.
// The result is that we match "a+" and assign "scope.a.rest" to "aaa", and our
// new position is now after the "aaa". Note that there was no instruction for
// changing the stack, so we stay in that context.
//
// In the next step, the first rule doesn't match anymore, so we go to the next
// rule where "(?=.)" matches. The instruction is to "pop", which means we
// pop the top of our context stack, which means we're now back in main.
//
// This time in main, we match "b", and in the next step we match the rest with
// "\w+", and we're done.
//
//
// ## Preventing loops
//
// These are the basics of how matching works. Now, you saw that you can write
// patterns that result in an empty match and don't change the position. These
// are called non-consuming matches. The problem with them is that they could
// result in infinite loops. Let's look at a syntax where that is the case:
//
//     contexts:
//       main:
//         - match: (?=.)
//           push: test
//       test:
//         - match: \w+
//           scope: word
//         - match: (?=.)
//           pop: true
//
// This is a bit silly, but it's a minimal example for explaining how matching
// works in that case.
//
// Let's say we have the input string " hello". In `main`, our rule matches and
// we go into `test` and stay at position 0. Now, the best match is the rule
// with "pop". But if we used that rule, we'd pop back to `main` and would still
// be at the same position we started at! So this would be an infinite loop,
// which we don't want.
//
// So what Sublime Text does in case a looping rule "won":
//
// * If there's another rule that matches at the same position and does not
//   result in a loop, use that instead.
// * Otherwise, go to the next position and go through all the rules in the
//   current context again. Note that it means that the "pop" could again be the
//   winning rule, but that's ok as it wouldn't result in a loop anymore.
//
// So in our input string, we'd skip one character and try to match the rules
// again. This time, the "\w+" wins because it comes first.

impl InterpreterState {
    /// Creates a state from a syntax definition, keeping its own reference-counted point to the
    /// main context of the syntax
    pub fn new(syntax: &SyntaxReference) -> InterpreterState {
        let start_state = StateLevel {
            mcs_skipped: 0,
            context: syntax.context_ids()["__start"],
            prototypes: Vec::new(),
            captures: None,
        };
        InterpreterState {
            stack: SharedStack(Arc::new(vec![start_state])),
            base_context: syntax.context_ids()["main"],
            has_while: false,
            first_line: true,
            proto_starts: Vec::new(),
            branch_points: Vec::new(),
            line_number: 0,
            pending_lines: Vec::new(),
            zero_width_escape_fires: HashMap::new(),
            skipped_branches: Vec::new(),
            replay_line: None,
            replay_pending: None,
            flushed_ops: Vec::new(),
            warnings: Vec::new(),
            escape_stack: Vec::new(),
        }
    }

    /// Parses a single line of the file. Because of the way regex engines work you unfortunately
    /// have to pass in a single line contiguous in memory. This can be bad for really long lines.
    /// Sublime Text avoids this by just not highlighting lines that are too long (thousands of characters).
    ///
    /// For efficiency reasons this returns only the changes to the current scope at each point in the line.
    /// You can use [`ScopeStack::apply`] on each operation in succession to get the stack for a given point.
    /// Look at the code in `highlighter.rs` for an example of doing this for highlighting purposes.
    ///
    /// The returned vector is in order both by index to apply at (the `usize`) and also by order to apply them at a
    /// given index (e.g popping old scopes before pushing new scopes).
    ///
    /// The [`SyntaxSet`] has to be the one that contained the syntax that was used to construct
    /// this [`InterpreterState`], or an extended version of it. Otherwise the parsing would return the
    /// wrong result or even panic. The reason for this is that contexts within the [`SyntaxSet`]
    /// are referenced via indexes.
    ///
    /// [`ScopeStack::apply`]: struct.ScopeStack.html#method.apply
    /// [`SyntaxSet`]: struct.SyntaxSet.html
    /// [`InterpreterState`]: struct.InterpreterState.html
    pub fn parse_line(
        &mut self,
        line: &str,
        syntax_set: &SyntaxSet,
    ) -> Result<ParseLineOutput, ParsingError> {
        self.parse_line_reusing(line, syntax_set, Vec::new())
    }

    pub(crate) fn parse_line_reusing(
        &mut self,
        line: &str,
        syntax_set: &SyntaxSet,
        ops: ScopeOperations,
    ) -> Result<ParseLineOutput, ParsingError> {
        if self.stack.is_empty() {
            return Err(ParsingError::MissingMainContext);
        }

        // Prune branch points older than 128 lines
        let cur_line = self.line_number;
        let warnings = &mut self.warnings;
        self.branch_points.retain(|bp| {
            let alive = cur_line.saturating_sub(bp.line_number) <= 128;
            if !alive {
                warnings.push(format!(
                    "branch point '{}' expired (exceeded 128-line rewind limit)",
                    bp.name
                ));
            }
            alive
        });
        self.line_number += 1;

        let ops = self.parse_line_inner_from_reusing(line, syntax_set, 0, None, ops)?;

        // Collect any corrected ops produced by a cross-line `fail` during the
        // parse above.  These are stored by `handle_fail` in `self.flushed_ops`.
        let replayed = std::mem::take(&mut self.flushed_ops);

        // Keep the line string for potential future cross-line replay.
        if !self.branch_points.is_empty() {
            self.pending_lines.push(line.to_string());
            let keep_from = self
                .branch_points
                .iter()
                .map(|bp| bp.pending_lines_snapshot_len)
                .min()
                .unwrap_or(0);
            if keep_from > 0 {
                self.pending_lines.drain(..keep_from);
                for bp in &mut self.branch_points {
                    bp.pending_lines_snapshot_len -= keep_from;
                }
            }
        } else {
            // No active branch points: any buffered strings are stale.
            self.pending_lines.clear();
        }

        self.stack.intern();
        let warnings = std::mem::take(&mut self.warnings);

        Ok(ParseLineOutput {
            ops,
            replayed,
            warnings,
        })
    }

    /// Returns `true` when the parser is inside a `branch_point` and the
    /// result of `parse_line` may be revised by a future `fail` action.
    /// Once the branch resolves (or if no branch was entered), this returns
    /// `false` and all ops emitted so far are final.
    pub fn is_speculative(&self) -> bool {
        !self.branch_points.is_empty()
    }

    pub(crate) fn relocate(&mut self, next_line: usize) -> bool {
        if self.is_speculative() && self.line_number != next_line {
            return false;
        }
        self.line_number = next_line;
        true
    }

    /// Inner parsing loop: processes `line` with the current parser state and
    /// returns the scope-stack operations.  Does **not** touch `pending_lines`
    /// or `flushed_ops`, so it is safe to call recursively from `handle_fail`
    /// for cross-line replay without re-entrancy issues.
    fn parse_line_inner(
        &mut self,
        line: &str,
        syntax_set: &SyntaxSet,
    ) -> Result<Vec<(usize, ScopeStackOp)>, ParsingError> {
        self.parse_line_inner_from(line, syntax_set, 0, None)
    }

    /// Parse `line` starting at `start_at` rather than column 0. Used by
    /// cross-line `fail` replay: the first buffered line's pre-branch
    /// prefix was correctly parsed under the pre-branch state, so the
    /// replay resumes *after* the branch match under the new alternative.
    /// When `start_at > 0` the `first_line` bookkeeping is skipped — the
    /// caller has already emitted (or preserved) the initial
    /// meta_content_scope push.
    fn parse_line_inner_from(
        &mut self,
        line: &str,
        syntax_set: &SyntaxSet,
        start_at: usize,
        // A branch that ran out at a byte of *this* string,
        // to suppress there. See `ReplayRequest::skip`.
        skip: Option<(usize, String)>,
    ) -> Result<Vec<(usize, ScopeStackOp)>, ParsingError> {
        self.parse_line_inner_from_reusing(line, syntax_set, start_at, skip, Vec::new())
    }

    fn parse_line_inner_from_reusing(
        &mut self,
        line: &str,
        syntax_set: &SyntaxSet,
        start_at: usize,
        skip: Option<(usize, String)>,
        mut res: ScopeOperations,
    ) -> Result<ScopeOperations, ParsingError> {
        let mut match_start = start_at;
        res.clear();

        if start_at == 0 && self.first_line {
            let cur_level = &self.stack[self.stack.len() - 1];
            let context = syntax_set.get_context(&cur_level.context)?;
            if !context.meta_content_scope.is_empty() {
                res.push((0, ScopeStackOp::Push(context.meta_content_scope[0])));
            }
            self.first_line = false;
        }

        if start_at == 0 && self.has_while {
            self.check_while_patterns(line, syntax_set, &mut match_start, &mut res)?;
        }

        let (mut search_cache, mut regions) = SEARCH_WORKSPACES
            .with(|pool| pool.borrow_mut().pop())
            .unwrap_or_default();
        search_cache.reset_line();
        // Used for detecting loops with push/pop, see long comment above.
        let mut non_consuming_push_at = (0, 0, 0);
        // Fresh for every call, replays included — a
        // cross-line replay re-entering the same offset is not a loop. See
        // `zero_width_escape_fires`.
        self.zero_width_escape_fires.clear();
        // And the exhausted branches belong to *this* call,
        // because they are byte offsets into the string it was handed. A
        // cross-line replay re-enters here with another line, so its entries
        // are put aside for the duration and the caller's are given back at
        // the end — carried either way, they would suppress a branch at an
        // offset that means nothing in the other string, or index past the end
        // of a shorter one. See `skipped_branches`.
        let outer_skipped = std::mem::take(&mut self.skipped_branches);
        self.skipped_branches.extend(skip);

        let mut stalled = 0usize;
        loop {
            let previous = match_start;
            if !self.parse_next_token(
                line,
                syntax_set,
                &mut match_start,
                &mut search_cache,
                &mut regions,
                &mut non_consuming_push_at,
                &mut res,
            )? {
                break;
            }
            if match_start == previous {
                stalled += 1;
            } else {
                stalled = 0;
            }
            if stalled > 1024 {
                self.warnings
                    .push("zero-width transition cycle; advanced one character".into());
                if let Some(c) = line[match_start..].chars().next() {
                    match_start += c.len_utf8();
                } else {
                    break;
                }
                stalled = 0;
                search_cache.clear();
            }
        }

        // See `outer_skipped` above.
        self.skipped_branches = outer_skipped;

        SEARCH_WORKSPACES.with(|pool| pool.borrow_mut().push((search_cache, regions)));

        Ok(res)
    }

    #[allow(clippy::too_many_arguments)]
    fn parse_next_token(
        &mut self,
        line: &str,
        syntax_set: &SyntaxSet,
        start: &mut usize,
        search_cache: &mut SearchCache,
        regions: &mut Region,
        non_consuming_push_at: &mut (usize, usize, usize),
        ops: &mut Vec<(usize, ScopeStackOp)>,
    ) -> Result<bool, ParsingError> {
        // The guard is armed over the *range* a non-consuming
        // push reached, not over one depth of it, and what it watches for is a
        // pop landing back where that push started.
        //
        // Upstream records `stack.len() + 1` — the depth a push of *one*
        // context reaches — so a `push:` of K contexts armed a depth the stack
        // never sat at, and a `pop: K` from the top unwound `pre+K → pre`
        // without ever visiting `pre+1`. Nothing flagged it, the position never
        // moved, and the same rule fired again: an unbounded loop with no outer
        // iteration cap. Reachable by any definition a user supplies, which
        // nothing bounds.
        //
        // `armed_pre` is the pre-push depth while the stack is anywhere inside
        // `(pre, post]`; the pop that loops is the one whose landing depth is
        // `pre` exactly. A pop that lands *below* `pre` is not a loop — the
        // context that owned the trigger is gone with it, which is what
        // upstream's `Pop(1)`-only test was standing in for (Haskell's
        // `immediately-pop2`), and this says directly.
        let armed_pre = {
            let (pos, pre, post) = *non_consuming_push_at;
            let depth = self.stack.len();
            if pos == *start && depth > pre && depth <= post {
                Some(pre)
            } else {
                None
            }
        };

        // Trim proto_starts that are no longer valid
        while self
            .proto_starts
            .last()
            .map(|start| *start >= self.stack.len())
            .unwrap_or(false)
        {
            self.proto_starts.pop();
        }

        let best_match =
            self.find_best_match(line, *start, syntax_set, search_cache, regions, armed_pre)?;

        if let Some(reg_match) = best_match {
            // Check if this is an escape match (sentinel pat_index)
            if reg_match.pat_index == usize::MAX {
                let (match_start, match_end) = reg_match.regions.pos(0).unwrap();
                // The escape that will not stop firing. See
                // `zero_width_escape_fires`.
                let zero_width = match_start == match_end;
                if zero_width
                    && self
                        .zero_width_escape_fires
                        .get(&match_start)
                        .copied()
                        .unwrap_or(0)
                        >= ZERO_WIDTH_ESCAPE_FIRE_LIMIT
                {
                    return match line[*start..].char_indices().nth(1) {
                        Some((i, _)) => {
                            *start += i;
                            Ok(true)
                        }
                        // End of line: no character to advance onto, and no
                        // point trying any more patterns.
                        None => Ok(false),
                    };
                }
                *start = match_end;
                self.exec_escape(
                    reg_match.escape_index,
                    match_start,
                    match_end,
                    &reg_match.regions,
                    syntax_set,
                    ops,
                )?;
                if zero_width {
                    *self.zero_width_escape_fires.entry(match_start).or_insert(0) += 1;
                }
                search_cache.clear();
                return Ok(true);
            }

            if reg_match.would_loop {
                // A push that doesn't consume anything (a regex that resulted
                // in an empty match at the current position) can not be
                // followed by a non-consuming pop. Otherwise we're back where
                // we started and would try the same sequence of matches again,
                // resulting in an infinite loop. In this case, Sublime Text
                // advances one character and tries again, thus preventing the
                // loop.

                // println!("pop_would_loop for match {:?}, start {}", reg_match, *start);

                // nth(1) gets the next character if there is one. Need to do
                // this instead of just += 1 because we have byte indices and
                // unicode characters can be more than 1 byte.
                if let Some((i, _)) = line[*start..].char_indices().nth(1) {
                    *start += i;
                    return Ok(true);
                } else {
                    // End of line, no character to advance and no point trying
                    // any more patterns.
                    return Ok(false);
                }
            }

            let match_end = reg_match.regions.pos(0).unwrap().1;

            // Check if this is a Fail operation — handle before advancing start
            let context = reg_match.context;
            let match_pattern = context.match_at(reg_match.pat_index)?;
            // Only when there is somewhere to rewind *to*.
            //
            // Sublime: "If a `fail` action specifies a `branch_point` that was
            // never pushed on the stack, or has already been popped off of it,
            // it will have no effect." It is the *action* that has no effect —
            // the rule is a rule like any other, so its text is consumed and
            // its `scope` applies, and the line carries on.
            //
            // `handle_fail` answers an unresolvable name with `Ok(false)`,
            // which is not "no effect": it is what `parse_next_token` returns
            // to say *stop parsing this line*. So a stray `fail` — CSS has one,
            // `- match: ';' fail: property-or-selector`, reachable outside any
            // branch — silently threw away the colouring of everything after it
            // on that line. Falling through instead sends it down the ordinary
            // path, where `MatchOperation::Fail` is already a no-op.
            let fail_that_resolves = match match_pattern.operation {
                MatchOperation::Fail(ref name) => self.can_rewind_to(name),
                _ => false,
            };
            if fail_that_resolves {
                let level_context = {
                    let id = &self.stack[self.stack.len() - 1].context;
                    syntax_set.get_context(id)?
                };
                return self.exec_pattern(
                    line,
                    &reg_match,
                    level_context,
                    syntax_set,
                    start,
                    non_consuming_push_at,
                    ops,
                    search_cache,
                );
            }

            // Inside a cross-line replay, a non-consuming
            // `Branch` whose match lands past every character of the replayed
            // line does not get a branch point.
            //
            // The one it would create is chained — opened by the replay that an
            // earlier branch's exhaustion started — and the *outer* parse
            // exhausts it on the next line, often an empty one. Its pop ops
            // then attach to this replayed line, which is the next line's
            // baseline, and the parent context collapses one boundary too
            // early. Skipping it is not losing the reading: the parent rule
            // fires again at the start of the outer line and the branch point
            // is anchored there instead.
            //
            // Markdown's link reference definition is the file:
            // `link-def-attr`'s `- match: $` created a branch point inside
            // `link-title-continuation`'s exhaustion replay, and
            // `meta.link.reference.def.markdown` came off on the blank line
            // where ST keeps it. Upstream's #677, whose diagnosis this is.
            if matches!(match_pattern.operation, MatchOperation::Branch { .. })
                && self.replay_line.is_some()
                && match_end >= line.len()
            {
                return Ok(false);
            }

            let consuming = match_end > *start;
            if !consuming {
                // The match doesn't consume any characters. If this is a
                // "push", remember the position and stack size so that we can
                // check the next "pop" for loops. Otherwise leave the state,
                // e.g. non-consuming "set" could also result in a loop.
                // how deep this push actually goes, so the
                // guard is armed over all of it. A `branch:` pushes exactly one
                // alternative whatever the list holds; a `push:`/`embed:`
                // pushes its whole list. A `pop: M` in front of any of them
                // takes the stack below where the trigger's own context sits,
                // so nothing can pop *back* to it and the range comes out empty
                // — disarmed, which is the right answer. See `armed_pre`.
                let pre = self.stack.len();
                let reach = match match_pattern.operation {
                    MatchOperation::Push {
                        ref ctx_refs,
                        pop_count,
                    } => Some(pre.saturating_sub(pop_count) + ctx_refs.len()),
                    MatchOperation::Branch { pop_count, .. } => {
                        Some(pre.saturating_sub(pop_count) + 1)
                    }
                    MatchOperation::Embed {
                        ref contexts,
                        pop_count,
                        ..
                    } => Some(pre.saturating_sub(pop_count) + contexts.len()),
                    // Anything else leaves the arming where it is, which is
                    // upstream's "otherwise leave the state, e.g. non-consuming
                    // `set` could also result in a loop". Overwriting it here
                    // *narrows* the guard — a non-consuming `set:` between the
                    // push and the pop hid the push it was armed on — and hung
                    // `CFML Script (Tags)` and `JavaScript (for PHP
                    // double-quoted)`.
                    _ => None,
                };
                if let Some(reach) = reach {
                    // And never narrower than the one depth
                    // upstream arms. With a `pop: M` in front, the arithmetic
                    // above can put the reach at or below `pre` and leave the
                    // range empty — but the stack can still come back up
                    // through `pre + 1` under later pushes at this same byte,
                    // and a `Pop(1)` there lands on `pre` and loops. This guard
                    // may be widened and may not be narrowed.
                    *non_consuming_push_at = (match_end, pre, reach.max(pre + 1));
                }
            }

            *start = match_end;

            // A branch that ran out at a byte the cursor has
            // now passed is no longer anything to suppress. See
            // `skipped_branches`.
            self.skipped_branches.retain(|(at, _)| *at >= *start);

            // ignore `with_prototype`s below this if a context is pushed
            if reg_match.from_with_prototype {
                // use current height, since we're before the actual push
                self.proto_starts.push(self.stack.len());
            }

            let level_context = {
                let id = &self.stack[self.stack.len() - 1].context;
                syntax_set.get_context(id)?
            };
            self.exec_pattern(
                line,
                &reg_match,
                level_context,
                syntax_set,
                start,
                non_consuming_push_at,
                ops,
                search_cache,
            )?;

            Ok(true)
        } else if self.skipped_branches.iter().any(|(at, _)| *at == *start) {
            // Nothing matched at a position where a branch
            // point ran out, so the parent context has nothing to offer here
            // either. Drop the entry and advance one character — which is what
            // upstream does unconditionally, kept as the last resort. See
            // `skipped_branches`.
            self.skipped_branches.retain(|(at, _)| *at != *start);
            match line[*start..].char_indices().nth(1) {
                Some((i, _)) => {
                    *start += i;
                    search_cache.clear();
                    Ok(true)
                }
                // End of line: no character to advance onto.
                None => Ok(false),
            }
        } else {
            Ok(false)
        }
    }

    fn find_best_match<'a>(
        &self,
        line: &str,
        start: usize,
        syntax_set: &'a SyntaxSet,
        search_cache: &mut SearchCache,
        _regions: &mut Region,
        armed_pre: Option<usize>,
    ) -> Result<Option<RegexMatch<'a>>, ParsingError> {
        let cur_level = &self.stack[self.stack.len() - 1];
        let context = syntax_set.get_context(&cur_level.context)?;
        let prototype = if let Some(ref p) = context.prototype {
            Some(p)
        } else {
            None
        };

        // Build an iterator for the contexts we want to visit in order
        let context_chain = {
            let proto_start = self.proto_starts.last().cloned().unwrap_or(0);
            // Sublime applies with_prototypes from bottom to top
            let with_prototypes = self.stack[proto_start..].iter().flat_map(|lvl| {
                lvl.prototypes
                    .iter()
                    .map(move |ctx| (true, ctx, lvl.captures.as_ref()))
            });
            let cur_prototype = prototype.into_iter().map(|ctx| (false, ctx, None));
            let cur_context =
                Some((false, &cur_level.context, cur_level.captures.as_ref())).into_iter();
            with_prototypes.chain(cur_prototype).chain(cur_context)
        };

        // println!("{:#?}", cur_level);
        // println!("token at {} on {}", start, line.trim_right());

        // Check escape patterns first — they take strict precedence.
        // If an escape matches at `start`, return it immediately as a synthetic match.
        // If it matches later, truncate the search region for normal patterns.
        let mut search_end = line.len();
        let mut escape_match: Option<(usize, Region)> = None; // (escape_stack_index, region)

        for (ei, entry) in self.escape_stack.iter().enumerate() {
            let mut esc_regions = Region::new();
            if entry
                .regex
                .search(line, start, line.len(), Some(&mut esc_regions), true)
            {
                let (esc_start, _esc_end) = esc_regions.pos(0).unwrap();
                if esc_start < search_end {
                    search_end = esc_start;
                    escape_match = Some((ei, esc_regions));
                }
            }
        }

        // If escape matches right at `start`, it wins immediately — no need to
        // search normal patterns.
        if let Some((ei, ref esc_region)) = escape_match {
            let esc_start = esc_region.pos(0).unwrap().0;
            if esc_start == start {
                return Ok(Some(RegexMatch {
                    regions: esc_region.clone(),
                    context: syntax_set.get_context(&cur_level.context)?,
                    pat_index: usize::MAX, // sentinel for escape match
                    from_with_prototype: false,
                    would_loop: false,
                    escape_index: ei,
                }));
            }
        }

        let mut min_start = usize::MAX;
        let mut best_match: Option<RegexMatch<'_>> = None;
        let mut pop_would_loop = false;

        // Each flattened context is a replaceable ordered-pattern matcher. The
        // interpreter only arbitrates prototype priority, escapes and invalid branches.
        let haystack = &line[..search_end];
        for (from_with_proto, ctx, captures) in context_chain {
            let patterns = syntax_set.context_patterns(*ctx, captures, self.base_context)?;
            let mut excluded = Vec::new();
            if self.skipped_branches.iter().any(|(at, _)| *at == start) {
                for (index, (context_id, rule)) in patterns.rules.iter().enumerate() {
                    let pat = syntax_set.get_context(context_id)?.match_at(*rule)?;
                    if let MatchOperation::Branch { name, .. } = &pat.operation
                        && self
                            .skipped_branches
                            .iter()
                            .any(|(at, exhausted)| *at == start && exhausted == name)
                    {
                        excluded.push(index);
                    }
                }
            }
            while let Some(hit) =
                patterns
                    .matcher
                    .find_earliest_excluding(haystack, start, search_cache, &excluded)
            {
                let (context_id, pat_index) = patterns.rules[hit.pattern];
                let pat_context = syntax_set.get_context(&context_id)?;
                let match_pat = pat_context.match_at(pat_index)?;
                if let MatchOperation::Fail(name) = &match_pat.operation
                    && hit.start == hit.end
                    && !self.can_rewind_to(name)
                {
                    excluded.push(hit.pattern);
                    continue;
                }
                let looping = hit.end == start
                    && matches!((armed_pre,&match_pat.operation),(Some(pre),MatchOperation::Pop(n)) if self.stack.len().saturating_sub(*n)==pre);
                if hit.start < min_start || (hit.start == min_start && pop_would_loop) {
                    min_start = hit.start;
                    pop_would_loop = looping;
                    let push_too_deep = matches!(
                        match_pat.operation,
                        MatchOperation::Push { .. }
                            | MatchOperation::Branch { .. }
                            | MatchOperation::Embed { .. }
                    ) && self.stack.len() >= 100;
                    if push_too_deep {
                        return Ok(None);
                    }
                    best_match = Some(RegexMatch {
                        regions: hit.captures,
                        context: pat_context,
                        pat_index,
                        from_with_prototype: from_with_proto,
                        would_loop: looping,
                        escape_index: 0,
                    });
                    if min_start == start && !looping {
                        return Ok(best_match);
                    }
                }
                if looping {
                    excluded.push(hit.pattern);
                } else {
                    break;
                }
            }
        }

        // If no normal match was found before the escape position, or escape
        // position is earlier, use the escape match.
        if let Some((ei, esc_region)) = escape_match {
            let esc_start = esc_region.pos(0).unwrap().0;
            if esc_start < min_start || (esc_start == min_start && pop_would_loop) {
                return Ok(Some(RegexMatch {
                    regions: esc_region,
                    context: syntax_set.get_context(&cur_level.context)?,
                    pat_index: usize::MAX, // sentinel for escape match
                    from_with_prototype: false,
                    would_loop: false,
                    escape_index: ei,
                }));
            }
        }

        Ok(best_match)
    }

    /// Returns true if the stack was changed.
    /// For `Fail` operations, returns `Ok(true)` if backtracking was performed
    /// (caller should continue parsing from the rewound position).
    fn exec_pattern<'a>(
        &mut self,
        line: &str,
        reg_match: &RegexMatch<'a>,
        level_context: &'a Context,
        syntax_set: &'a SyntaxSet,
        start: &mut usize,
        non_consuming_push_at: &mut (usize, usize, usize),
        ops: &mut Vec<(usize, ScopeStackOp)>,
        search_cache: &mut SearchCache,
    ) -> Result<bool, ParsingError> {
        let (match_start, match_end) = reg_match.regions.pos(0).unwrap();
        let context = reg_match.context;
        let pat = context.match_at(reg_match.pat_index)?;
        // which version's rules the captures of this match
        // follow — the same question `push_meta_ops` asks, and asked the same
        // way, off the context on top of the stack. See `build_capture_ops`.
        let version = self.current_syntax_version(syntax_set);

        // Handle Fail: attempt backtracking.
        //
        // And only when there is somewhere to rewind to. The
        // other half of the change at the `Fail` check in `parse_next_token`,
        // which is where the reason is written down — a `fail` naming a branch
        // point nobody opened is a rule like any other, scope and all, not an
        // instruction to abandon the rest of the line.
        let unresolvable = match pat.operation {
            MatchOperation::Fail(ref name) => !self.can_rewind_to(name),
            _ => false,
        };
        if let MatchOperation::Fail(ref name) = pat.operation
            && !unresolvable
        {
            return self.handle_fail(
                name,
                line,
                start,
                non_consuming_push_at,
                ops,
                search_cache,
                syntax_set,
            );
        }

        // For Branch, we need to snapshot state before executing, then synthesize a Push.
        let is_branch = matches!(pat.operation, MatchOperation::Branch { .. });
        let synthetic_op;

        if is_branch {
            if let MatchOperation::Branch {
                ref name,
                ref alternatives,
                pop_count,
            } = pat.operation
            {
                // Snapshot current state.
                //
                // NOTE on field naming: `match_start` here stores the
                // position the parser should *resume* from on fail —
                // which is the branch match's end position (since the
                // parser has already consumed the match). `match_end`
                // and `pat_scope` carry the *real* match span plus the
                // keyword's own scopes so a same-line fail rewind can
                // re-emit them (they were truncated off `ops` along
                // with the alt[0]'s subsequent work).
                let bp = BranchPoint {
                    trigger_captures: (reg_match.regions.clone(), line.to_owned()),
                    name: name.clone(),
                    next_alternative: 1, // 0 is about to be pushed
                    alternatives: alternatives.clone(),
                    stack_snapshot: self.stack.clone(),
                    proto_starts_snapshot: self.proto_starts.clone(),
                    match_start: *start, // position before this match's advance
                    trigger_match_start: match_start,
                    pat_scope: pat.scope.clone(),
                    // the line this branch point is really
                    // on, which is not `line_number` inside a replay. See
                    // `replay_line`.
                    line_number: self.cur_line_number(),
                    ops_snapshot_len: ops.len(),
                    stack_depth: self.stack.len(),
                    non_consuming_push_at_snapshot: *non_consuming_push_at,
                    first_line_snapshot: self.first_line,
                    with_prototype: pat.with_prototype.clone(),
                    // see `pending_lines_mark`.
                    pending_lines_snapshot_len: self.pending_lines_mark(),
                    escape_stack_snapshot: self.escape_stack.clone(),
                    pop_count,
                    // Inside a replay, what this line has
                    // already been given comes first. See `Replay::prefix`.
                    prefix_ops: match &self.replay_line {
                        Some(replay) => {
                            let mut prefix = replay.prefix.clone();
                            prefix.extend(ops.iter().cloned());
                            prefix
                        }
                        None => ops.clone(),
                    },
                    capture_ops: pat
                        .captures
                        .as_ref()
                        .map(|m| build_capture_ops(m, &reg_match.regions, version))
                        .unwrap_or_default()
                        .into_vec(),
                };
                self.branch_points.push(bp);
                // `pop: N + branch:` is a lookahead, like
                // `pop: N + push:` and not like `set`. This built a synthetic
                // `Set`, which gave the trigger token the meta scopes of every
                // frame it was unwinding. Asked of ST with a three-frame probe,
                // `bgWQQQ` in `PopPush.sublime-syntax`, the trigger comes out
                // `source meta.tail.branch keyword.trigger.branch` — the target's
                // `meta_scope` alone. So the synthetic is a `Push` carrying the
                // count, which is where the lookahead unwinding lives.
                synthetic_op = if pop_count > 0 {
                    MatchOperation::Push {
                        ctx_refs: vec![alternatives[0].clone()],
                        pop_count,
                    }
                } else {
                    MatchOperation::Push {
                        ctx_refs: vec![alternatives[0].clone()],
                        pop_count: 0,
                    }
                };
            } else {
                unreachable!()
            }
        } else {
            synthetic_op = MatchOperation::None;
        }

        let op_to_use = if is_branch {
            &synthetic_op
        } else {
            &pat.operation
        };

        self.push_meta_ops(true, match_start, level_context, op_to_use, syntax_set, ops)?;
        for s in &pat.scope {
            ops.push((match_start, ScopeStackOp::Push(*s)));
        }
        let capture_ops = pat
            .captures
            .as_ref()
            .map(|m| build_capture_ops(m, &reg_match.regions, version))
            .unwrap_or_default();
        ops.extend(capture_ops.iter().cloned());
        if !pat.scope.is_empty() {
            ops.push((match_end, ScopeStackOp::Pop(pat.scope.len())));
        }
        self.push_meta_ops(false, match_end, level_context, op_to_use, syntax_set, ops)?;

        if is_branch {
            // Execute the synthetic Push through perform_op
            let synthetic_pat = MatchPattern::new(
                pat.has_captures,
                pat.regex.regex_str().to_string(),
                pat.scope.clone(),
                pat.captures.clone(),
                synthetic_op,
                pat.with_prototype.clone(),
            );
            self.perform_op(line, &reg_match.regions, &synthetic_pat, syntax_set)
        } else {
            self.perform_op(line, &reg_match.regions, pat, syntax_set)
        }
    }

    /// Handle a `fail` operation by rewinding to the named branch point.
    /// Returns Ok(true) if backtracking happened (caller should continue from rewound position).
    /// Returns Ok(false) if the fail had no effect.
    /// whether a `fail` naming this branch point has
    /// anywhere to rewind to.
    ///
    /// The same test `handle_fail` makes before it does anything, asked before
    /// the rule is routed there at all — see the site in `parse_next_token`
    /// that calls it, which is where the reason lives.
    fn can_rewind_to(&self, name: &str) -> bool {
        let stack_len = self.stack.len();
        self.branch_points
            .iter()
            .any(|bp| bp.name == name && branch_point_is_live(bp, stack_len))
    }

    fn handle_fail(
        &mut self,
        name: &str,
        line: &str,
        start: &mut usize,
        non_consuming_push_at: &mut (usize, usize, usize),
        ops: &mut Vec<(usize, ScopeStackOp)>,
        search_cache: &mut SearchCache,
        syntax_set: &SyntaxSet,
    ) -> Result<bool, ParsingError> {
        // Find the branch point by name (most recent first), skipping
        // records whose alternative's pushed frame is no longer on
        // the stack. The alternative lives at
        // `bp.stack_depth - bp.pop_count + 1`, so `stack.len() >
        // bp.stack_depth - bp.pop_count` means the frame is still
        // present. Without this skip, a nested `branch_point` with
        // the same name whose inner alternative popped cleanly would
        // shadow an enclosing record via `rposition`, rewinding to
        // the inner branch position instead of the outer one
        // (Haskell's raw-string QQ `[r|[a-zA-Z]|]`).
        let stack_len = self.stack.len();
        let bp_index = self
            .branch_points
            .iter()
            .rposition(|bp| bp.name == name && branch_point_is_live(bp, stack_len));
        let bp_index = match bp_index {
            Some(i) => i,
            None => return Ok(false), // No such branch point, fail is no-op
        };

        // see `replay_line`.
        let cur_line = self.cur_line_number();
        let bp = &self.branch_points[bp_index];

        // Check validity: not >128 lines old
        if cur_line.saturating_sub(bp.line_number) > 128 {
            let bp = self.branch_points.remove(bp_index);
            self.warnings.push(format!(
                "branch point '{}' expired (exceeded 128-line rewind limit)",
                bp.name
            ));
            return Ok(false);
        }

        // Check validity: the alternative's frame is still on the stack.
        // the fourth place this was asked and the second that
        // got it wrong — `stack.len() < stack_depth` ignores the `pop_count`
        // that the `rposition` above had just accounted for, so a `pop: N +
        // branch:` with N >= 2 found its branch point and then threw it away
        // one line later. Asked the one way now; see `branch_point_is_live`.
        if !branch_point_is_live(bp, self.stack.len()) {
            self.branch_points.remove(bp_index);
            return Ok(false);
        }

        // Check if there are more alternatives
        if bp.next_alternative >= bp.alternatives.len() {
            // All alternatives exhausted: restore parser state to the
            // pre-branch snapshot so the stuck alternative's pushed
            // contexts and emitted ops are discarded, then advance
            // past the branch_point match position by one character so
            // we don't immediately re-enter the same branch_point and
            // loop. Before this, the branch_point was silently removed
            // while its last alternative's contexts remained on the
            // stack — the cause of the "scope stack stays in
            // `meta.interpolation.brace.shell`" cascade in Zsh when
            // both `brace-interpolation-sequence` and
            // `brace-interpolation-series` failed and there was no
            // fallback alternative (Zsh explicitly excludes
            // `brace-interpolation-fallback`).
            //
            // Cross-line exhaustion takes the same shape, plus a replay
            // of the buffered lines under the pre-branch state so
            // callers see corrected ops for lines they've already been
            // handed. Without this, the unterminated TypeScript type
            // expression at `sublimehq/Packages#3598`
            // (`type x = { bar: (cb: (\n};`) left the inner
            // `ts-type-function-parameter-list-body` on the stack
            // forever, contaminating every subsequent line's scope
            // stack with `meta.type.js, meta.group.js` — 274 cascading
            // assertion failures in `syntax_test_typescript.ts`.
            let is_cross_line = bp.line_number < cur_line;
            let stack_snapshot = bp.stack_snapshot.clone();
            let proto_starts_snapshot = bp.proto_starts_snapshot.clone();
            let escape_stack_snapshot = bp.escape_stack_snapshot.clone();
            let first_line_snapshot = bp.first_line_snapshot;
            let non_consuming_push_at_snapshot = bp.non_consuming_push_at_snapshot;
            let ops_snapshot_len = bp.ops_snapshot_len;
            let match_start_pos = bp.match_start;
            let pending_lines_snapshot_len = bp.pending_lines_snapshot_len;
            let prefix_ops = bp.prefix_ops.clone();
            self.branch_points.remove(bp_index);

            self.stack = stack_snapshot;
            self.proto_starts = proto_starts_snapshot;
            self.escape_stack = escape_stack_snapshot;
            self.first_line = first_line_snapshot;
            *non_consuming_push_at = non_consuming_push_at_snapshot;
            ops.truncate(ops_snapshot_len.min(ops.len()));

            if is_cross_line {
                // Re-parse each buffered line under the restored (pre-branch)
                // state so `parse_line` can surface the corrected ops via
                // `ParseLineOutput::replayed`. The first buffered line is the
                // branch-creation line: emit its saved `prefix_ops` (the ops
                // emitted before the branch match) verbatim, then resume where
                // the branch matched.
                //
                // *where it matched*, with the branch's name
                // suppressed there — not one character past it, which is what
                // this did and what same-line arm stopped doing for
                // the reason `skipped_branches` gives: the character stepped
                // over is one the parent context never gets to match, so the
                // token the branch was opened on loses its scopes entirely.
                //
                // `package apple dot` in Java's corpus is the line: `dot` opens
                // a declaration whose branch runs out several lines later, and
                // the replay resumed at `ot`, which nothing matches. ST gives
                // the whole word `variable.other.java` — the parent's next
                // rule, which is exactly the fall-through the same-line arm
                // already does.
                let first = &self.pending_lines[pending_lines_snapshot_len];
                let resume_at = match_start_pos.min(first.len());
                let request = ReplayRequest {
                    from: pending_lines_snapshot_len,
                    prefix: prefix_ops,
                    resume_at,
                    skip: Some((resume_at, name.to_string())),
                };
                // inside a replay this does not replay. It
                // leaves the request for the driver and ends the line, so that
                // two loops cannot walk the same buffered lines and the reading
                // chosen last is the one that survives. See `replay_pending`.
                if self.replay_line.is_some() {
                    self.replay_pending = Some(request);
                    return Ok(false);
                }
                self.run_replays(request, syntax_set)?;

                // Restart the current line from the beginning under the
                // restored state. including what ran out on
                // it, which was exhausted under a reading this restart has
                // just replaced. See `skipped_branches`.
                self.skipped_branches.clear();
                ops.clear();
                *start = 0;
                *non_consuming_push_at = (0, 0, 0);
                search_cache.clear();
                return Ok(true);
            }

            // Same-line exhaustion. rewind to where the
            // branch point matched and record its name, so the parent
            // context's next rule is offered the position that the branch
            // took. Upstream advances one character past the lookahead
            // instead, which steps the cursor into the middle of the token the
            // parent was about to match. See `skipped_branches`.
            //
            // Unless the position is not a position in *this* string: a
            // cross-line replay parses an earlier line while `line_number`
            // still names the current one, so a branch point that really is
            // from another line reads as same-line here and its `match_start`
            // can be past the end of the line being replayed. There is nothing
            // on this line to rewind to then, so the line ends — which is what
            // the end-of-line arm did before.
            if match_start_pos <= line.len() {
                self.skipped_branches
                    .push((match_start_pos, name.to_string()));
                *start = match_start_pos;
            } else {
                *start = line.len();
            }
            search_cache.clear();
            return Ok(true);
        }

        // Determine if this is a cross-line fail (branch was created on a previous line).
        let is_cross_line = bp.line_number < cur_line;

        // Extract everything we need from bp before mutating self.
        let next_alt_index = bp.next_alternative;
        let next_alt = bp.alternatives[next_alt_index].clone();
        let match_start_pos = bp.match_start;
        let trigger_match_start = bp.trigger_match_start;
        let trigger_pat_scope = bp.pat_scope.clone();
        let trigger_capture_ops = bp.capture_ops.clone();
        let stack_snapshot = bp.stack_snapshot.clone();
        let proto_starts_snapshot = bp.proto_starts_snapshot.clone();
        let first_line_snapshot = bp.first_line_snapshot;
        let non_consuming_push_at_snapshot = bp.non_consuming_push_at_snapshot;
        let ops_snapshot_len = bp.ops_snapshot_len;
        let pending_lines_snapshot_len = bp.pending_lines_snapshot_len;
        let escape_stack_snapshot = bp.escape_stack_snapshot.clone();
        let prefix_ops = bp.prefix_ops.clone();
        // bp borrow ends here.

        let pop_count = self.branch_points[bp_index].pop_count;

        // Restore parser state to the snapshot.
        self.stack = stack_snapshot;
        self.proto_starts = proto_starts_snapshot;
        self.escape_stack = escape_stack_snapshot;
        self.first_line = first_line_snapshot;
        *non_consuming_push_at = non_consuming_push_at_snapshot;

        // Update the branch point record before popping/pushing
        // (must happen before the pop which may invalidate indices).
        self.branch_points[bp_index].next_alternative = next_alt_index + 1;

        // the ops for that unwinding, counted here because
        // this is the last moment the frames it is about are on the stack — the
        // re-emit below happens with the alternative already pushed over them.
        // See `lookahead_unwind_ops`.
        let mut unwind_ops = Vec::new();
        if pop_count > 0 {
            self.lookahead_unwind_ops(pop_count, trigger_match_start, syntax_set, &mut unwind_ops)?;
        }

        // For pop + branch: re-pop the contexts (snapshot was taken pre-pop).
        if pop_count > 0 {
            for _ in 0..pop_count {
                self.stack.pop();
            }
        }

        // Push the next alternative onto the stack.
        let with_prototype = self.branch_points[bp_index].with_prototype.clone();
        let context_id = next_alt.id_in(syntax_set)?;
        let context = syntax_set.get_context(&context_id)?;

        let proto_ids = match with_prototype {
            Some(ref p) => vec![p.id_in(syntax_set)?],
            None => Vec::new(),
        };

        let needs_captures = context.uses_backrefs
            || proto_ids
                .iter()
                .any(|id| syntax_set.get_context(id).is_ok_and(|c| c.uses_backrefs));
        let captures =
            needs_captures.then(|| self.branch_points[bp_index].trigger_captures.clone());
        self.has_while |= context.while_pattern.is_some();
        self.stack.push(StateLevel {
            context: context_id,
            prototypes: proto_ids,
            captures,
            // A branch alternative is pushed on its own, never as the guest of
            // an embed, and `handle_fail` pushes its `meta_content_scope`.
            mcs_skipped: 0,
        });

        if is_cross_line {
            // Cross-line fail: the ops for lines since the branch was created
            // have already been returned to callers.  Re-parse those lines under
            // the new alternative and store the corrected ops in `flushed_ops`
            // so that `parse_line` can surface them via `ParseLineOutput::replayed`.
            //
            // The first buffered line is the branch-creation line. Its
            // pre-branch prefix (cols 0..trigger_match_start) was correctly
            // parsed under the *pre-branch* state — not the new alternative.
            // Re-parsing it from column 0 with the new alternative on the
            // stack would misattribute that prefix to the new alternative's
            // rules (observed on multi-line SQL `LIKE … ESCAPE …`: every
            // non-whitespace before `LIKE` fires `else-pop` in the
            // escape-alternative, derailing the stack). Instead, reuse the
            // prefix_ops saved at branch-creation time, manually emit the
            // branch trigger's pat.scope and the new alternative's meta
            // scope ops, then resume parsing from match_end with the new
            // alternative on the stack via `parse_line_inner_from`.
            // Keep `pending_lines` intact (don't drain): if a second
            // branch_point on the current line also fails after this retry, its
            // own replay needs access to the same buffered lines. Nested
            // branches from the same earlier line share the buffer.
            //
            // Compose the first buffered line's prefix here, then hand the
            // whole thing to the driver. Appending each round's corrections is
            // safe because every one of them names its line: a later round for
            // a line already in the list simply wins, which is how the reading
            // chosen last is the one that survives.
            let mut prefix = prefix_ops;
            // the trigger's own unwinding comes first, exactly
            // as `push_meta_ops` emits it. See `lookahead_unwind_ops`.
            prefix.extend(unwind_ops.iter().cloned());
            if let Some(clear_amount) = context.clear_scopes {
                prefix.push((trigger_match_start, ScopeStackOp::Clear(clear_amount)));
            }
            for scope in context.meta_scope.iter() {
                prefix.push((trigger_match_start, ScopeStackOp::Push(*scope)));
            }
            for scope in &trigger_pat_scope {
                prefix.push((trigger_match_start, ScopeStackOp::Push(*scope)));
            }
            // See the matching comment in the same-line branch below — re-emit
            // the trigger match's captures inside the pat_scope brackets so
            // they survive the branch swap.
            prefix.extend(trigger_capture_ops.iter().cloned());
            if !trigger_pat_scope.is_empty() {
                prefix.push((match_start_pos, ScopeStackOp::Pop(trigger_pat_scope.len())));
            }
            for scope in context.meta_content_scope.iter() {
                prefix.push((match_start_pos, ScopeStackOp::Push(*scope)));
            }
            let request = ReplayRequest {
                from: pending_lines_snapshot_len,
                prefix,
                resume_at: match_start_pos,
                // The alternative that was just pushed is what resumes here;
                // nothing is being suppressed. See `ReplayRequest::skip`.
                skip: None,
            };
            // as in the exhaustion arm — a `fail` inside a
            // replay hands the request to the driver rather than running a
            // replay of its own. See `replay_pending`.
            if self.replay_line.is_some() {
                self.replay_pending = Some(request);
                return Ok(false);
            }
            self.run_replays(request, syntax_set)?;

            // Restart the current line from the beginning — and what ran out
            // on it goes with the reading that ran out. See
            // `skipped_branches`.
            self.skipped_branches.clear();
            ops.clear();
            *start = 0;
            *non_consuming_push_at = (0, 0, 0);

            // Guard: the replayed `parse_line_inner` calls above can
            // mutate `self.branch_points` (adding new branches,
            // removing expired or exhausted ones), which can shift or
            // invalidate `bp_index`. Indexing with the stale position
            // previously panicked outright on files that exercise
            // nested cross-line branching (observed on
            // `JavaScript/syntax_test_js.js` and
            // `syntax_test_typescript.ts`). Skip the bookkeeping if
            // the branch point has been removed — the replay already
            // completed, which is the essential work of the fail.
            if bp_index < self.branch_points.len() {
                self.branch_points[bp_index].ops_snapshot_len = 0;
            }
        } else {
            // Same-line fail: truncate ops back to the snapshot point and rewind.
            ops.truncate(ops_snapshot_len.min(ops.len()));
            // except at column 0 of an empty line, where the
            // rewind goes to the end of it instead.
            //
            // The alternative being swapped in there is typically a
            // `- match: '' pop: N` — Markdown's `link-def-attr-continuation`
            // failing into `immediately-pop2` — and running that pop at column
            // 0 emits the Pops on the empty line's only character, taking the
            // parent's `meta_scope` off before that line has been scoped at
            // all. Past the end they land on the next line's baseline instead,
            // which is where ST has them: the definition's pop straddles the
            // blank line and the paragraph after it, not the title's line and
            // the blank one. Upstream's #677.
            *start = match match_start_pos == 0 && line.len() <= 1 && line.trim().is_empty() {
                true => line.len(),
                false => match_start_pos,
            };

            // Keep `ops_snapshot_len` pointing at the pre-branch state.
            // Subsequent fails on the same branch_point must truncate
            // back to *here* — not to the position after the pat.scope
            // re-emit below — otherwise a second fail would preserve
            // the first re-emit's (Push at trigger_match_start) while
            // appending another re-emit, producing the disordered
            // sequence (trigger, Push), (match_end, Pop), (trigger,
            // Push), (match_end, Pop). `ScopeRegionIterator` then
            // panics in `easy.rs` because position goes backwards.
            self.branch_points[bp_index].ops_snapshot_len = ops.len();

            // the trigger's own unwinding comes first, exactly
            // as `push_meta_ops` emits it. See `lookahead_unwind_ops`.
            ops.extend(unwind_ops.iter().cloned());

            // Re-emit the branch_point match's own scopes over their
            // original span. Without this, keywords that trigger a
            // branch (e.g. `LIKE` with
            // `scope: keyword.operator.comparison.sql`) lose their
            // scope whenever alt[0] fails and alt[1..] succeeds,
            // because the original Push/Pop pair was truncated off
            // `ops` together with alt[0]'s subsequent work.
            //
            // The new alternative's `clear_scopes` and `meta_scope`
            // are emitted at `trigger_match_start` *before* the
            // trigger's `pat.scope`, mirroring the non-fail push path
            // in `push_meta_ops` (initial phase): meta_scope sits
            // below the match scope on the stack so the matched text
            // sees both. Placing them at `match_start_pos` would mean
            // the trigger character (e.g. `(` of `for (var i = 0; …)`)
            // never sees the alternative's `meta_scope`. The
            // `meta_content_scope` legitimately stays at
            // `match_start_pos` — mcs only applies after the matched
            // text.
            if let Some(clear_amount) = context.clear_scopes {
                ops.push((trigger_match_start, ScopeStackOp::Clear(clear_amount)));
            }
            for scope in context.meta_scope.iter() {
                ops.push((trigger_match_start, ScopeStackOp::Push(*scope)));
            }
            for scope in &trigger_pat_scope {
                ops.push((trigger_match_start, ScopeStackOp::Push(*scope)));
            }
            // Captures emitted alongside the original pat.scope (e.g.
            // `keyword.declaration.data.haskell` on the first capture of
            // `(data)(?:\s+(family|instance))?`) were truncated off with
            // alt[0]'s ops. Re-emit them inside the pat_scope brackets so
            // the keyword scope survives the branch swap.
            ops.extend(trigger_capture_ops.iter().cloned());
            if !trigger_pat_scope.is_empty() {
                ops.push((match_start_pos, ScopeStackOp::Pop(trigger_pat_scope.len())));
            }
            for scope in context.meta_content_scope.iter() {
                ops.push((match_start_pos, ScopeStackOp::Push(*scope)));
            }
        }

        // Clear search cache since we're rewinding.
        search_cache.clear();

        Ok(true)
    }

    /// Which line the parser is on, counting from 0.
    ///
    /// `line_number` is a count of `parse_line` calls, which
    /// a cross-line replay does not make. Everything that compares a branch
    /// point's line against "now" has to ask this instead. See `replay_line`.
    /// run cross-line replays until no more are asked for.
    ///
    /// One round re-reads `pending_lines[req.from..]` under whatever reading
    /// the caller has just put on the stack, and stops early if a `fail` on one
    /// of those lines asks for another round (`replay_pending`). The rounds are
    /// appended in order, so a later one wins the lines it covers — which is
    /// the point: the reading that survives is the last one chosen, not the
    /// last loop left running.
    ///
    /// An interrupted round keeps what it produced. The lines before the new
    /// round's `from` were read under a branch point that had not been created
    /// yet when they were parsed, so they are still right; the ones from there
    /// on are re-done by the round that follows.
    ///
    /// It terminates because every round is a branch point advancing to its
    /// next alternative or being removed, and neither can go backwards.
    fn run_replays(
        &mut self,
        mut req: ReplayRequest,
        syntax_set: &SyntaxSet,
    ) -> Result<(), ParsingError> {
        loop {
            let corrections = self.run_replay(&req, syntax_set)?;
            self.flushed_ops.extend(corrections);
            match self.replay_pending.take() {
                Some(next) => req = next,
                None => return Ok(()),
            }
        }
    }

    /// One round of the above: each buffered line from `req.from` on, under the
    /// state the caller has restored.
    ///
    /// `pending_lines` is not drained. A second branch point on the current
    /// line that fails after this one needs the same buffered lines, and so
    /// does the next round.
    fn run_replay(
        &mut self,
        req: &ReplayRequest,
        syntax_set: &SyntaxSet,
    ) -> Result<ReplayedOps, ParsingError> {
        let lines: Vec<String> = self.pending_lines[req.from..].to_vec();
        let mut out = Vec::with_capacity(lines.len());
        for (i, line) in lines.iter().enumerate() {
            // this buffered line's own number, for as long as
            // it is the one being parsed. Saved and put back, because a `fail`
            // on it reads `cur_line_number`. See `replay_line`.
            let outer = self.replay_line.replace(Replay {
                line_number: (self.line_number - self.pending_lines.len() + req.from + i)
                    .saturating_sub(1),
                pending_index: req.from + i,
                prefix: Vec::new(),
            });
            let line_ops = if i == 0 {
                let mut composed = req.prefix.clone();
                // what this line already has, for a branch
                // point created by the re-parse. See `Replay::prefix`.
                if let Some(replay) = self.replay_line.as_mut() {
                    replay.prefix = composed.clone();
                }
                let resume_at = req.resume_at.min(line.len());
                let tail =
                    self.parse_line_inner_from(line, syntax_set, resume_at, req.skip.clone())?;
                composed.extend(tail);
                composed
            } else {
                self.parse_line_inner(line, syntax_set)?
            };
            self.replay_line = outer;
            // The line `pending_lines[from + i]` is: the current line, less the
            // buffered ones before it, plus the offset into that buffer.
            out.push((
                self.line_number - self.pending_lines.len() + req.from + i,
                line_ops,
            ));
            if self.replay_pending.is_some() {
                break;
            }
        }
        Ok(out)
    }

    fn cur_line_number(&self) -> usize {
        match &self.replay_line {
            Some(replay) => replay.line_number,
            None => self.line_number.saturating_sub(1),
        }
    }

    /// Where a branch point created right now starts its replay from.
    ///
    /// at the top level that is "everything buffered after
    /// this point", which is `pending_lines.len()` — the line being parsed is
    /// not in the buffer yet. Inside a replay it is the buffer index of the
    /// line being replayed, which *is* in the buffer already: a branch point
    /// on it has to be able to replay it. Recording `len()` there said "there
    /// is nothing to replay", so the next `fail` restored the stack into the
    /// middle of a line it never re-read. See `replay_line`.
    fn pending_lines_mark(&self) -> usize {
        match &self.replay_line {
            Some(replay) => replay.pending_index,
            None => self.pending_lines.len(),
        }
    }

    /// Get the syntax version for the current parse state
    fn current_syntax_version(&self, syntax_set: &SyntaxSet) -> u32 {
        if let Some(level) = self.stack.last() {
            let syntax_index = level.context.syntax_index;
            syntax_set
                .syntaxes()
                .get(syntax_index)
                .map_or(1, |s| s.version)
        } else {
            1
        }
    }

    /// the ops a `pop: N + push:`/`branch:`/`embed:` emits
    /// for the frames it unwinds, before its trigger takes its own scope.
    ///
    /// The count is a *lookahead* — "the match will not receive the
    /// `meta_scope` of the contexts that are popped" — so the frames come off
    /// in the initial phase, where a `set` leaves the same work until after.
    /// Each cleared frame's Restore goes out before the totals are counted, for
    /// reason: `clear_scopes` "is applied before `meta_scope` and
    /// `meta_content_scope`", and what is counted here is what each frame
    /// pushed on the way in.
    ///
    /// Two callers, and the second is why this is a function: `push_meta_ops`
    /// emits it when the branch first fires, and `handle_fail` re-emits it when
    /// an alternative is swapped in, because the rewind truncates `ops` back to
    /// *before* the trigger and re-emits the trigger by hand. The unwind is the
    /// trigger's, not the alternative's — `handle_fail` re-pops the same
    /// `pop_count` off the restored stack — so leaving it out of the re-emit
    /// left the ops one reading and the parser's own stack another.
    fn lookahead_unwind_ops(
        &self,
        pop_count: usize,
        index: usize,
        syntax_set: &SyntaxSet,
        ops: &mut Vec<(usize, ScopeStackOp)>,
    ) -> Result<(), ParsingError> {
        let stack_len = self.stack.len();
        let mut atoms = 0usize;
        for depth in 0..pop_count.min(stack_len) {
            let ctx = syntax_set.get_context(&self.stack[stack_len - 1 - depth].context)?;
            if ctx.clear_scopes.is_some() {
                ops.push((index, ScopeStackOp::Restore));
            }
            atoms += ctx.meta_content_scope.len() + ctx.meta_scope.len();
        }
        if atoms > 0 {
            ops.push((index, ScopeStackOp::Pop(atoms)));
        }
        Ok(())
    }

    /// unwind the frames *under* the top one, a frame at a
    /// time, for a `pop: N` (or a `pop: N + set:`) that reaches past its own.
    ///
    /// Each frame's `meta_content_scope` comes off, then its `meta_scope`, then
    /// a Restore for its own `clear_scopes` — before the frames beneath it are
    /// touched. The atoms a `clear_scopes` took are the ones the frames below it
    /// pushed, so while it is in force they sit in the clear stack and not on
    /// the live one, and unwinding those frames first Pops that many *live*
    /// atoms from under them. Summing every frame's meta scopes into one `Pop`
    /// cannot express that.
    ///
    /// Three callers, one order: for a plain `pop: N`, for a
    /// `pop: N + set:` after its trigger, and for the same set
    /// *before* its trigger, when the target's `clear_scopes` is emitted early.
    fn unwind_frames_below_top(
        &self,
        count: usize,
        index: usize,
        syntax_set: &SyntaxSet,
        ops: &mut Vec<(usize, ScopeStackOp)>,
    ) -> Result<(), ParsingError> {
        let stack_len = self.stack.len();
        for depth in 1..count.min(stack_len) {
            let level_idx = stack_len - 1 - depth;
            let ctx = syntax_set.get_context(&self.stack[level_idx].context)?;
            let skipped = self.stack[level_idx].mcs_skipped;
            let atoms = ctx.meta_content_scope.len().saturating_sub(skipped);
            if atoms > 0 {
                ops.push((index, ScopeStackOp::Pop(atoms)));
            }
            if !ctx.meta_scope.is_empty() {
                ops.push((index, ScopeStackOp::Pop(ctx.meta_scope.len())));
            }
            if ctx.clear_scopes.is_some() {
                ops.push((index, ScopeStackOp::Restore));
            }
        }
        Ok(())
    }

    fn push_meta_ops(
        &self,
        initial: bool,
        index: usize,
        cur_context: &Context,
        match_op: &MatchOperation,
        syntax_set: &SyntaxSet,
        ops: &mut Vec<(usize, ScopeStackOp)>,
    ) -> Result<(), ParsingError> {
        let version = self.current_syntax_version(syntax_set);
        // println!("metas ops for {:?}, initial: {}",
        //          match_op,
        //          initial);
        // println!("{:?}", cur_context.meta_scope);
        match *match_op {
            MatchOperation::Pop(n) => {
                // For `pop: N` with N > 1, every context being popped
                // contributes scope atoms on the scope stack that must
                // be unwound in LIFO order. The TOP context's trigger
                // text must not see its own `meta_content_scope`, so
                // that one is popped in the initial phase; all other
                // scope unwinding (top context's `meta_scope`, then
                // each deeper context's `meta_content_scope` followed
                // by its `meta_scope`) happens in the non-initial
                // phase, immediately after the match text's own
                // scope has been popped.
                //
                // Before this fix only the top context's scopes were
                // ever popped, leaving the N-1 deeper contexts'
                // `meta_scope` / `meta_content_scope` atoms orphaned —
                // the cause of the "scope stack grows unboundedly"
                // cascade in Makefile and Zsh (Category A).
                let stack_len = self.stack.len();
                let pop_count = n.min(stack_len);
                if initial {
                    // v2: if the context immediately below the top has
                    // embed_scope_replaces, cur_context's meta_content_scope
                    // was never pushed, so don't generate a Pop for it.
                    // from the frame, not from the stack
                    // below it, and only what was actually pushed. See
                    // `StateLevel::mcs_skipped`.
                    let skipped = self
                        .stack
                        .last()
                        .map(|level| level.mcs_skipped)
                        .unwrap_or(0);
                    let atoms = cur_context.meta_content_scope.len().saturating_sub(skipped);
                    if atoms > 0 {
                        ops.push((index, ScopeStackOp::Pop(atoms)));
                    }
                } else {
                    // Top context's meta_scope comes off first (it sat
                    // immediately below the trigger text's scope on the
                    // stack).
                    if !cur_context.meta_scope.is_empty() {
                        ops.push((index, ScopeStackOp::Pop(cur_context.meta_scope.len())));
                    }
                    // Each deeper context's scopes are popped in
                    // top-to-bottom order: meta_content_scope first
                    // (pushed after its own meta_scope, hence above on
                    // the stack), then meta_scope, then any Restore
                    // paired with that context's own `clear_scopes`
                    // (mirrors the push order Clear → meta_scope → mcs
                    // in reverse). Without the Restore, a `pop: N`
                    // from the top that also unwinds a deeper context
                    // with `clear_scopes` leaves the originally cleared
                    // atoms orphaned — observed on Python f/t-string
                    // interpolation close, where `f-string-replacement-end`
                    // fires `pop: 2` that also pops
                    // `f-string-replacement-meta`.
                    // and cur's own `clear_scopes` is
                    // restored here, before the frames underneath are unwound —
                    // not after them, which is where upstream puts it.
                    //
                    // The atoms a `clear_scopes` took are the ones the frames
                    // below pushed, so while it is in force they are in the
                    // clear stack and not on the live one. Unwinding those
                    // frames first therefore Pops that many *live* atoms from
                    // under them, and the Restore then puts the cleared ones
                    // back on top of the damage.
                    //
                    // Haskell's string continuation is the file:
                    // `literal-string-continuation` has `clear_scopes: 2` over
                    // `literal-string-body`'s two-atom `meta_scope`, and its
                    // `- match: \" … pop: 2` emitted `Pop(1) Pop(2) Restore`
                    // where the live stack had one atom to give — the corpus's
                    // last scope-stack divergence, `pop of 2 with 1 on the
                    // stack`. The line after an unterminated continuation then
                    // stayed inside `meta.string.haskell`.
                    //
                    // This is ordering — "clear_scopes is applied
                    // before meta_scope and meta_content_scope", so it is undone
                    // after them — for a plain `pop: N`, where did it
                    // for `pop: N + set:`. `pop: 1` is unaffected either way:
                    // the loop below is empty, so the two positions coincide.
                    if pop_count > 1 && cur_context.clear_scopes.is_some() {
                        ops.push((index, ScopeStackOp::Restore));
                    }
                    // likewise — see above, and
                    // `unwind_frames_below_top`, which is this loop's one copy.
                    self.unwind_frames_below_top(pop_count, index, syntax_set, ops)?;
                }

                // Cleared scopes are restored after the scopes from match pattern that invoked the pop are applied
                // unless the unwinding above has already put
                // them back, which it does whenever there are frames under this
                // one whose atoms the clear is holding. See there.
                if !initial && pop_count <= 1 && cur_context.clear_scopes.is_some() {
                    ops.push((index, ScopeStackOp::Restore))
                }
            }
            // For some reason the ST3 behaviour of set is convoluted and is inconsistent with the docs and other ops
            // - the meta_content_scope of the current context is applied to the matched thing, unlike pop
            // - the clear_scopes are applied after the matched token, unlike push
            // - the interaction with meta scopes means that the token has the meta scopes of both the current scope and the new scope.
            MatchOperation::Push {
                ctx_refs: ref context_refs,
                ..
            }
            | MatchOperation::Set {
                ctx_refs: ref context_refs,
                ..
            } => {
                let is_set = matches!(*match_op, MatchOperation::Set { .. });
                let set_pop_count = match *match_op {
                    MatchOperation::Set { pop_count, .. } => pop_count.max(1),
                    _ => 1,
                };
                // `pop: N + push:` is a *lookahead*, so the
                // frames it unwinds are unwound **before** the matched text takes
                // its scope — all of it in the initial phase, where a `set` leaves
                // the same work until after.
                //
                // Sublime: "for `push`, `embed` and `branch` actions, the pop
                // treats the match as if it were a lookahead, which means the
                // match will not receive the `meta_scope` of the contexts that are
                // popped." Asked of ST directly, with three frames and a
                // `pop: 2 + push:` on the innermost, the trigger token comes out
                //
                //     source meta.tail keyword.trigger
                //
                // — the target's `meta_scope` and not one atom of the two frames
                // being popped. The same shape with `set:` comes out
                //
                //     source meta.outer meta.mid meta.tail keyword.trigger
                //
                // which is every frame plus the target, and is what this parser
                // already did for `set` — so the second sentence of that
                // paragraph ("any match will receive the `meta_scope` of the
                // context being popped *and* the context being pushed") is about
                // `set`, and upstream's #683, which would take the deeper frames
                // off a `pop: N + set:` trigger, is contradicted by the
                // application.
                let push_pop_count = match *match_op {
                    MatchOperation::Push { pop_count, .. } => pop_count,
                    _ => 0,
                };
                if initial && push_pop_count > 0 {
                    self.lookahead_unwind_ops(push_pop_count, index, syntax_set, ops)?;
                }
                // a match pattern that "set"s keeps the meta_content_scope and meta_scope from the previous context
                if initial {
                    // v2: pop parent's meta_content_scope so matched text does not see it
                    //
                    // unless the context below is an
                    // `embed_scope` wrapper, in which case this context is an
                    // embedded syntax's `main` and its `meta_content_scope`
                    // was *replaced* by the host's `embed_scope` — the push
                    // path skips it on `embed_scope_replaces` alone, so
                    // popping it here takes an atom off that nothing put on.
                    //
                    // Markdown embedding Bash is the file: the first `set:`
                    // inside a ```bash fence took `source.shell.bash` off, and
                    // from there every frame was one out — the fence's escape
                    // then popped `text.html.markdown`, the document's own
                    // root scope, and a second fence over-popped an empty
                    // stack. ST keeps `source.shell.bash` on the stack for the
                    // whole fence and `text.html.markdown` after it
                    // (the `mdfence.probe` probe).
                    let skipped = self
                        .stack
                        .last()
                        .map(|level| level.mcs_skipped)
                        .unwrap_or(0);
                    let atoms = cur_context.meta_content_scope.len().saturating_sub(skipped);
                    if is_set && version >= 2 && atoms > 0 {
                        ops.push((index, ScopeStackOp::Pop(atoms)));
                    }

                    // A `pop: N + set:` whose
                    // target clears carries its trigger on the *unwound* stack.
                    //
                    // The frames a `pop: N + set:` unwinds do reach its trigger
                    // token — ST answers `source meta.outer meta.mid meta.tail
                    // keyword.trigger` for three frames and a `pop: 2 + set:`
                    // (the `PopPush.sublime-syntax` probe),
                    // which is why they are unwound after the token and not
                    // before it. But when the *target* carries a `clear_scopes`
                    // that is emitted early (see `early_set_clear`), the clear
                    // and the frames it is counted against are on the same
                    // stack, and ST has unwound them first.
                    //
                    // `(?ix:` in `RegExp (Basic)` is the file, asked directly:
                    // `group-start` fires `pop: 3, set: [group-body-extended,
                    // maybe-unexpected-quantifiers]` and ST scopes the trigger
                    //
                    //     source.regexp meta.group.regexp meta.mode.extended.regexp
                    //         storage.modifier.mode.regexp
                    //
                    // — one `meta.group.regexp`, the target's. Ours had two:
                    // the target's `meta_scope` pushed over the leaving
                    // `group-body`'s, whose second atom the target's
                    // `clear_scopes: 1` ate instead of the one ST's clear
                    // reaches. Unwinding first puts the clear over the same
                    // atom ST's is over, and the column after the trigger — and
                    // every other column of
                    // the `regexp_modes.probe` probe — is
                    // where it already was.
                    //
                    // `early_set_clear` guarantees `cur_context` has no meta
                    // scopes of its own here, so nothing of the leaving context
                    // is being taken off the trigger; its `clear_scopes` is put
                    // back first, for the reason the unwinding is a frame at a
                    // time at all.
                    // Version 1 never emits a Clear before the trigger, so
                    // there is nothing early about it there.
                    let early_clear = if version >= 2 {
                        early_set_clear(is_set, cur_context, context_refs, syntax_set)?
                    } else {
                        None
                    };
                    let early_unwind = set_pop_count > 1 && early_clear.is_some();
                    if early_unwind {
                        if cur_context.clear_scopes.is_some() {
                            ops.push((index, ScopeStackOp::Restore));
                        }
                        self.unwind_frames_below_top(set_pop_count, index, syntax_set, ops)?;
                    }
                    // NOTE: cur_context.clear_scopes Restore is emitted in the
                    // non-initial phase below, AFTER Pop(cur.meta_scope + target.meta_scope)
                    // has run. Restoring here (pre-match) would place the cleared
                    // scopes on top of the stack above the target.meta_scope push,
                    // and the non-initial Pop would then remove the restored scopes
                    // instead of the intended meta_scopes — dropping cur's cleared
                    // state on the floor. Observed as duplicate
                    // `meta.mapping.value.json` atoms in nested JSON objects.
                    // add each context's meta scope
                    if version >= 2 {
                        // Push: emit Clear for every pushed context that has
                        // `clear_scopes`, at its own index position in the
                        // push order — same as v1. Sublime permits at most
                        // one `clear_scopes` per push list, but when it sits
                        // on a non-topmost entry (e.g. Python's
                        // `f-string-replacement-meta` at index 0 of a
                        // 3-context push) restricting to `i == last_idx`
                        // silently drops it, leaking the parent's
                        // `meta_content_scope` atoms into interpolation
                        // content.
                        //
                        // Single-context `set:` with clear_scopes on the
                        // target: emit Clear here — before target.meta_scope
                        // is pushed and before the trigger match scope — so
                        // the matched text sees the cleared stack.
                        // Observed on Lisp's `(defun fn (...)`: the
                        // parameter-list `(` otherwise kept the enclosing
                        // `meta.function.lisp` alongside
                        // `meta.function.parameters.lisp` because Clear
                        // fired only after the match in the non-initial
                        // phase. See
                        // `v2_set_to_target_with_clear_scopes_clears_parent_meta_content_scope`.
                        //
                        // Multi-context `set:` keeps Clear in the non-initial
                        // phase (emitted inline after preceding contexts'
                        // mcs pushes). Moving it to the initial phase here
                        // would strip atoms from below the outer mcs rather
                        // than from the top of the just-pushed inner mcs
                        // stack — Makefile's `set: [value-to-be-defined,
                        // eat-whitespace-then-pop]` relies on Clear eating
                        // the last-pushed mcs atom, which Restore then
                        // replaces when eat-whitespace-then-pop pops.
                        // not when the context being *left*
                        // carries meta scopes of its own.
                        //
                        // Emitting the target's Clear here is right when the
                        // visible top of the stack is the parent atom the Clear is
                        // meant to strip. When `cur_context` has a `meta_scope` or
                        // `meta_content_scope`, that is what sits on top instead,
                        // so the Clear hides *those*; the non-initial `Pop` below
                        // then takes its count from the atoms underneath them, and
                        // the trailing `Restore` puts cur's own meta scope back —
                        // exactly where the parent's atom should have been.
                        //
                        // Bash `: ~/` is the case. ST scopes the `/` as
                        // `meta.string.glob.shell string.unquoted.shell`; this
                        // emitted `meta.interpolation.tilde.shell
                        // string.unquoted.shell`, the tilde interpolation's own
                        // scope left behind by that swap. Asked of ST directly,
                        // the `bash_tilde.probe` probe.
                        //
                        // Deferring it to the non-initial phase is enough: the two
                        // phases share this predicate, and the one that does not
                        // emit here emits there. Upstream's #669.
                        //
                        // and a multi-context `set:` is the
                        // same question one target along — see
                        // `early_set_clear`, which is what decides both phases
                        // now. A single-context `set:` is the case where its
                        // answer is `Some(0)`.
                        // — a version 1 `push:`
                        // of several contexts clears their sum, once, first.
                        // See `summed_push_clear`.
                        let summed = if version < 2 && !is_set {
                            summed_push_clear(context_refs, syntax_set)?
                        } else {
                            None
                        };
                        if let Some(clear_amount) = summed {
                            ops.push((index, ScopeStackOp::Clear(clear_amount)));
                        }
                        for (i, r) in context_refs.iter().enumerate() {
                            let ctx = r.resolve(syntax_set)?;

                            let emit_clear_here =
                                (!is_set && version >= 2) || early_clear == Some(i);
                            if emit_clear_here && let Some(clear_amount) = ctx.clear_scopes {
                                ops.push((index, ScopeStackOp::Clear(clear_amount)));
                            }

                            for scope in ctx.meta_scope.iter() {
                                ops.push((index, ScopeStackOp::Push(*scope)));
                            }
                        }
                    } else {
                        // the other phase of it.
                        let summed = if version < 2 && !is_set {
                            summed_push_clear(context_refs, syntax_set)?
                        } else {
                            None
                        };
                        if let Some(clear_amount) = summed {
                            ops.push((index, ScopeStackOp::Clear(clear_amount)));
                        }
                        for r in context_refs.iter() {
                            let ctx = r.resolve(syntax_set)?;

                            if !is_set
                                && version >= 2
                                && let Some(clear_amount) = ctx.clear_scopes
                            {
                                ops.push((index, ScopeStackOp::Clear(clear_amount)));
                            }

                            for scope in ctx.meta_scope.iter() {
                                ops.push((index, ScopeStackOp::Push(*scope)));
                            }
                        }
                    }
                } else {
                    // — the summed clear belongs to
                    // the trigger token and to nothing after it.
                    //
                    // ST scopes `abc` in the documentation's example
                    // `meta.ctx2 meta.ctx3 identifier`, with the summed clear
                    // in force, and the column *after* it
                    // `source.lang meta.ctx3` — which is where version 2's
                    // interleaved clears land. So the two phases answer
                    // differently at version 1: the initial one clears the sum
                    // and pushes the meta scopes over it, and this one puts
                    // that back and applies them one context at a time, as
                    // version 2 does throughout. `multiclear_v1`'s last three
                    // columns are the record.
                    let summed = if version < 2 && !is_set {
                        summed_push_clear(context_refs, syntax_set)?
                    } else {
                        None
                    };
                    if summed.is_some() {
                        let pushed: usize = context_refs
                            .iter()
                            .map(|r| {
                                r.resolve(syntax_set)
                                    .map(|ctx| ctx.meta_scope.len())
                                    .unwrap_or(0)
                            })
                            .sum();
                        if pushed > 0 {
                            ops.push((index, ScopeStackOp::Pop(pushed)));
                        }
                        ops.push((index, ScopeStackOp::Restore));
                        for r in context_refs.iter() {
                            let ctx = r.resolve(syntax_set)?;
                            if let Some(clear_amount) = ctx.clear_scopes {
                                ops.push((index, ScopeStackOp::Clear(clear_amount)));
                            }
                            for scope in ctx.meta_scope.iter() {
                                ops.push((index, ScopeStackOp::Push(*scope)));
                            }
                            for scope in ctx.meta_content_scope.iter() {
                                ops.push((index, ScopeStackOp::Push(*scope)));
                            }
                        }
                        return Ok(());
                    }

                    // `pop: N + set:` (set_pop_count > 1) unwinds N-1 deeper
                    // contexts in addition to the usual set-replace semantics;
                    // their meta_scope / meta_content_scope atoms sitting on
                    // the scope stack must be popped off, so force repush to
                    // fire even if the immediate contexts had no mcs/ms.
                    let repush = (is_set
                        && (set_pop_count > 1
                            || !cur_context.meta_scope.is_empty()
                            || !cur_context.meta_content_scope.is_empty()
                            // cur has clear_scopes but no meta_scope/mcs: we still
                            // need to Pop the target.meta_scope pushed in initial,
                            // Restore cur.clear_scopes, and re-push target.meta_scope
                            // + target.meta_content_scope in the correct order.
                            || cur_context.clear_scopes.is_some()))
                        || context_refs.iter().any(|r| {
                            let ctx = r.resolve(syntax_set).unwrap();

                            !ctx.meta_content_scope.is_empty()
                                || (ctx.clear_scopes.is_some() && is_set)
                        });
                    if repush {
                        // remove previously pushed meta scopes, so that meta content scopes will be applied in the correct order
                        let mut num_to_pop: usize = context_refs
                            .iter()
                            .map(|r| {
                                let ctx = r.resolve(syntax_set).unwrap();
                                ctx.meta_scope.len()
                            })
                            .sum();

                        // also pop off the original context's meta scopes
                        if is_set {
                            if version >= 2 {
                                // v2: set excludes parent meta_content_scope from matched text
                                num_to_pop += cur_context.meta_scope.len();
                            } else {
                                num_to_pop += cur_context.meta_content_scope.len()
                                    + cur_context.meta_scope.len();
                            }
                            // For `pop: N + set:` (set_pop_count > 1) the N-1
                            // deeper contexts are unwound too — one frame at a
                            // time, below, rather than counted in here, because
                            // a frame with `clear_scopes` has to put its atoms
                            // back before the frames under it are touched. See
                            // there.
                        }

                        // for `pop: N + set:` with N > 1, the
                        // Restore comes *before* the Pop.
                        //
                        // Sublime documents the order: `clear_scopes` "is applied
                        // before `meta_scope` and `meta_content_scope`". So by the
                        // time the meta scopes of the frames being unwound are
                        // accounted for, the atoms `clear_scopes` took must be
                        // back where they were. With a `clear_scopes:` on the
                        // leaving context, part of what is about to be popped is
                        // sitting in `clear_stack` rather than on the live
                        // stack, so a Pop that counted it eats that many *live*
                        // atoms and takes them from frames below the ones being
                        // popped. (The frames *under* the replaced one are
                        // unwound one at a time further down, for the same
                        // reason taken one step further.)
                        //
                        // Batch File is where it shows: `cmd-set-quoted-value-
                        // inner-end` has `clear_scopes: 1` and fires
                        // `pop: 2, set: ignored-tail-outer`, and every
                        // `set "var"=...` line lost `meta.command.set.dosbatch`
                        // over its trailing content — 231 failing assertions in
                        // one file, the whole of that language's failures.
                        //
                        // Plain `set:` keeps Pop-then-Restore. There is nothing
                        // below its single frame for an over-Pop to reach, and the
                        // order there is load-bearing the other way: the restored
                        // atoms are meant to land *below* the target's upcoming
                        // meta_scope push, which is what
                        // `v2_set_to_target_with_clear_scopes_clears_parent_meta_content_scope`
                        // (the Lisp `defun` shape) pins.
                        //
                        // This is upstream's #646, taken alone.
                        // a target's `clear_scopes` emitted
                        // in the initial phase (see `early_set_clear`) has
                        // already eaten atoms, and when it sits past the first
                        // target those atoms are `meta_scope`s that
                        // `num_to_pop` above counts — so a Pop of the full
                        // count would take that many *live* atoms from frames
                        // below. Undo it first and let the re-push below emit
                        // it again in its place: one Clear is live either way,
                        // and the clear stack balances. `Some(0)` needs none of
                        // this, because a Clear before any of this set's own
                        // pushes ate nothing that `num_to_pop` is about.
                        let early_clear = if version >= 2 {
                            early_set_clear(is_set, cur_context, context_refs, syntax_set)?
                        } else {
                            None
                        };
                        // — what the initial phase
                        // has already done. See there.
                        let early_unwind = set_pop_count > 1 && early_clear.is_some();
                        //
                        // And `Some(0)` needs it too once the
                        // set is unwinding more than its own frame. The atom
                        // that Clear ate then belongs to one of the frames
                        // below — the one this set is replacing — and that
                        // frame's `meta_scope` is popped here, so the atom has
                        // to be back on the stack to be popped. `(?ix:` in a
                        // regex is the file: `group-start` fires
                        // `pop: 3, set: [group-body-extended, …]` into a target
                        // whose `clear_scopes: 1` had already taken the group
                        // body's `meta.mode.extended.regexp`.
                        let undo_early_clear = early_clear.is_some_and(|i| i > 0);
                        if undo_early_clear {
                            ops.push((index, ScopeStackOp::Restore));
                        }

                        // unless the initial phase restored
                        // it already, on its way to unwinding the frames under
                        // this one before the trigger — .
                        let restore = is_set && cur_context.clear_scopes.is_some() && !early_unwind;
                        let restore_first = restore && set_pop_count > 1;
                        if restore_first {
                            ops.push((index, ScopeStackOp::Restore));
                        }

                        // do all the popping as one operation
                        if num_to_pop > 0 {
                            ops.push((index, ScopeStackOp::Pop(num_to_pop)));
                        }

                        // then the frames a `pop: N + set:`
                        // unwinds under the one it replaces — one at a time,
                        // and each one's own `clear_scopes` put back before the
                        // frames beneath it are touched.
                        //
                        // This is ordering for the `set:` path: the
                        // atoms a `clear_scopes` took are the ones the frames
                        // below pushed, so while it is in force they sit in the
                        // clear stack and not on the live one, and unwinding
                        // those frames first Pops that many *live* atoms from
                        // under them. Counting every frame's `meta_scope` into
                        // one `Pop` — which is what this did — cannot express
                        // that, and in `RegExp (Basic)` it ate the embedded
                        // syntax's own `source.regexp` off the bottom: the
                        // corpus' last scope-stack divergence and Python's last
                        // failing assertion, both at the close of a `(?ix: … )`.
                        // unless the initial phase already
                        // did it, which it does when the target's own
                        // `clear_scopes` is emitted before the trigger — see
                        // `early_unwind` there, and .
                        if is_set && set_pop_count > 1 && !early_unwind {
                            self.unwind_frames_below_top(set_pop_count, index, syntax_set, ops)?;
                        }

                        // Restore scopes cleared by the leaving context, now that
                        // cur.meta_scope and the initial phase's target.meta_scope
                        // push have been popped off. The restored atoms land below
                        // the target's upcoming meta_scope / meta_content_scope push.
                        if restore && !restore_first {
                            ops.push((index, ScopeStackOp::Restore));
                        }

                        // now we push meta scope and meta context scope for each context pushed
                        if version >= 2 {
                            // v2: For multi-context `set:`, Clear is emitted
                            // here so it strips the topmost just-pushed mcs
                            // atom (as Sublime does for multi-context set).
                            // Single-context `set:` emitted its Clear earlier
                            // in the initial phase (so the trigger token sees
                            // the cleared stack); re-emitting here would
                            // double-push onto clear_stack and cause Pop
                            // underflow when the context unwinds.
                            //
                            // Clear is emitted per-context (not only for the
                            // topmost) because `clear_scopes` on a non-topmost
                            // context is a real pattern: Bash's
                            // `set: [def-function-body, def-function-params,
                            // def-function-name]` has `clear_scopes: 1` on
                            // def-function-params (middle). Each Clear is
                            // placed just before that context's own mcs/ms
                            // pushes so it strips the previous iteration's
                            // last-pushed atom, matching Sublime's semantics.
                            // The same predicate as the initial phase, so
                            // whichever phase does not emit the Clear, the other
                            // one does — see the reasoning there.
                            let mut prev_embed_scope_replaces = false;
                            for r in context_refs.iter() {
                                let id = r.id_in(syntax_set)?;
                                let ctx = syntax_set.get_context(&id)?;

                                // `Some(0)` is the one the
                                // initial phase emitted and left standing; any
                                // other early Clear was undone by the Restore
                                // above and is emitted here, in its place.
                                if is_set
                                    && (early_clear != Some(0) || undo_early_clear)
                                    && let Some(clear_amount) = ctx.clear_scopes
                                {
                                    ops.push((index, ScopeStackOp::Clear(clear_amount)));
                                }

                                for scope in ctx.meta_scope.iter() {
                                    ops.push((index, ScopeStackOp::Push(*scope)));
                                }
                                // v2: what the previous context's `embed_scope`
                                // replaces is not pushed — the embedded
                                // syntax's own scope, and only that. See
                                // `mcs_replaced_by_embed_scope`.
                                let skipped = mcs_replaced_by_embed_scope(
                                    prev_embed_scope_replaces,
                                    &id,
                                    ctx,
                                    syntax_set,
                                );
                                for scope in ctx.meta_content_scope.iter().skip(skipped) {
                                    ops.push((index, ScopeStackOp::Push(*scope)));
                                }
                                prev_embed_scope_replaces = ctx.embed_scope_replaces;
                            }
                        } else {
                            for r in context_refs {
                                let ctx = r.resolve(syntax_set)?;

                                // for some reason, contrary to my reading of the docs, set does this after the token
                                if is_set && let Some(clear_amount) = ctx.clear_scopes {
                                    ops.push((index, ScopeStackOp::Clear(clear_amount)));
                                }

                                for scope in ctx.meta_scope.iter() {
                                    ops.push((index, ScopeStackOp::Push(*scope)));
                                }
                                for scope in ctx.meta_content_scope.iter() {
                                    ops.push((index, ScopeStackOp::Push(*scope)));
                                }
                            }
                        }
                    }
                }
            }
            MatchOperation::Embed {
                ref contexts,
                pop_count,
                ..
            } => {
                // `pop: N + embed:` is a lookahead too — the
                // reference names all three, "for `push`, `embed` and `branch`
                // actions" — so the synthetic is a `Push` carrying the count and
                // not a `Set`. ST, on `efZVVV` in `PopPush.sublime-syntax`:
                // `source meta.tail.embed keyword.trigger.embed`, with neither
                // unwound frame's `meta_scope` on the trigger.
                let synthetic = if pop_count > 0 {
                    MatchOperation::Push {
                        ctx_refs: contexts.clone(),
                        pop_count,
                    }
                } else {
                    MatchOperation::Push {
                        ctx_refs: contexts.clone(),
                        pop_count: 0,
                    }
                };
                return self.push_meta_ops(
                    initial,
                    index,
                    cur_context,
                    &synthetic,
                    syntax_set,
                    ops,
                );
            }
            MatchOperation::None | MatchOperation::Fail(_) => (),
            MatchOperation::Branch {
                ref alternatives,
                pop_count,
                ..
            } => {
                // Branch acts like Push for meta ops purposes, with the count
                // when there is one (it used to become a `Set`
                // — see the exec-time site above for what ST says). At exec time
                // Branch is transformed into a synthetic Push before calling
                // push_meta_ops, so this arm is a safety fallback.
                let synthetic = if pop_count > 0 {
                    MatchOperation::Push {
                        ctx_refs: alternatives.clone(),
                        pop_count,
                    }
                } else {
                    MatchOperation::Push {
                        ctx_refs: alternatives.clone(),
                        pop_count: 0,
                    }
                };
                return self.push_meta_ops(
                    initial,
                    index,
                    cur_context,
                    &synthetic,
                    syntax_set,
                    ops,
                );
            }
        }

        Ok(())
    }

    /// Returns true if the stack was changed
    fn perform_op(
        &mut self,
        line: &str,
        regions: &Region,
        pat: &MatchPattern,
        syntax_set: &SyntaxSet,
    ) -> Result<bool, ParsingError> {
        // filled in by the `Embed` arm below.
        let mut embed_holder: Option<(ContextId, usize)> = None;
        let (ctx_refs, old_proto_ids, is_embed) = match pat.operation {
            MatchOperation::Push {
                ref ctx_refs,
                pop_count,
            } => {
                // `pop: N + push:`. The stack effect is a
                // `Set`'s — pop N, then push — and only the scopes the trigger
                // token sees differ (see `push_meta_ops`). The prototypes are
                // *not* carried forward the way `Set` carries them: a `push` is
                // entering a new context, not replacing the one being left.
                if pop_count > 0 {
                    for _ in 0..pop_count {
                        self.stack.pop();
                    }
                    let final_len = self.stack.len() + ctx_refs.len();
                    // see `branch_point_is_live`. This is
                    // where a `pop: N + branch:` threw away the branch point it
                    // had just created — makes that a synthetic `Push`,
                    // and `stack_depth <= final_len` reads the pre-pop depth as
                    // if it were the alternative's.
                    self.branch_points
                        .retain(|bp| branch_point_is_live(bp, final_len));
                    self.escape_stack.retain(|e| e.stack_depth < final_len);
                }
                (ctx_refs, None, false)
            }
            MatchOperation::Embed {
                ref contexts,
                pop_count,
                ..
            } => {
                if pop_count > 0 {
                    // Remember the frame this
                    // `embed:` was declared in before unwinding it: its meta
                    // scopes are on the escape, and nothing on the stack will
                    // be able to say so by then.
                    embed_holder = self.stack.last().map(|l| (l.context, l.mcs_skipped));
                    for _ in 0..pop_count {
                        self.stack.pop();
                    }
                    // see `branch_point_is_live`. Against the
                    // length *before* the contexts go on, as upstream has it —
                    // widening this to the post-push length the way the `push`
                    // and `set` arms count hangs `JavaScript (for PHP
                    // double-quoted)` outright, so what those two do is not
                    // simply right and this is not simply an inconsistency.
                    let stack_len = self.stack.len();
                    self.branch_points
                        .retain(|bp| branch_point_is_live(bp, stack_len));
                    self.escape_stack.retain(|e| e.stack_depth < stack_len);
                }
                (contexts, None, true)
            }
            MatchOperation::Set {
                ref ctx_refs,
                pop_count,
            } => {
                // a `with_prototype` stays active when the context is `set`
                // until the context layer in the stack (where the `with_prototype`
                // was initially applied) is popped off. With `pop: N + set:`
                // (pop_count > 1), the topmost popped frame's prototypes are
                // what carry forward onto the new push.
                let pops = pop_count.max(1);
                let old_proto_ids = self.stack.pop().map(|s| s.prototypes);
                for _ in 1..pops {
                    self.stack.pop();
                }
                // Prune branch_points / escape_stack against the *final* stack
                // length (after the common push loop below), so a branch_point
                // captured at the pre-set depth survives a pop-1 + push-1 set
                // — the depth the bp references is still valid after the push.
                let final_len = self.stack.len() + ctx_refs.len();
                // see `branch_point_is_live`.
                self.branch_points
                    .retain(|bp| branch_point_is_live(bp, final_len));
                self.escape_stack.retain(|e| e.stack_depth < final_len);
                (ctx_refs, old_proto_ids, false)
            }
            MatchOperation::Pop(n) => {
                for _ in 0..n {
                    self.stack.pop();
                }
                // Invalidate branch points whose alternative this pop took
                // away. see `branch_point_is_live`.
                let stack_len = self.stack.len();
                self.branch_points
                    .retain(|bp| branch_point_is_live(bp, stack_len));
                // Remove escape entries whose stack_depth >= current stack
                self.escape_stack.retain(|e| e.stack_depth < stack_len);
                return Ok(true);
            }
            MatchOperation::None => return Ok(false),
            MatchOperation::Branch { .. } | MatchOperation::Fail(_) => {
                // Branch and Fail are handled in exec_pattern, not here
                return Ok(false);
            }
        };

        // Record stack depth before pushing (for Embed escape entry)
        let stack_depth_before = self.stack.len();

        // mirrors the `prev_embed_scope_replaces` walk in
        // `push_meta_ops`, which is what decides whether a frame's
        // `meta_content_scope` is pushed at all. The two loops have to agree,
        // so the flag is set here from the same predicate rather than
        // reconstructed from the stack afterwards. See `StateLevel`.
        let mut prev_embed_scope_replaces = false;

        for (i, r) in ctx_refs.iter().enumerate() {
            let mut proto_ids = if i == 0 {
                // it is only necessary to preserve the old prototypes
                // at the first stack frame pushed
                old_proto_ids.clone().unwrap_or_else(Vec::new)
            } else {
                Vec::new()
            };
            if i == ctx_refs.len() - 1 {
                // if a with_prototype was specified, and multiple contexts were pushed,
                // then the with_prototype applies only to the last context pushed, i.e.
                // top most on the stack after all the contexts are pushed - this is also
                // referred to as the "target" of the push by sublimehq - see
                // https://forum.sublimetext.com/t/dev-build-3111/19240/17 for more info
                if let Some(ref p) = pat.with_prototype {
                    proto_ids.push(p.id_in(syntax_set)?);
                }
            }
            let context_id = r.id_in(syntax_set)?;
            let context = syntax_set.get_context(&context_id)?;
            let captures = {
                let mut uses_backrefs = context.uses_backrefs;
                if !proto_ids.is_empty() {
                    uses_backrefs = uses_backrefs
                        || proto_ids
                            .iter()
                            .any(|id| syntax_set.get_context(id).unwrap().uses_backrefs);
                }
                if uses_backrefs {
                    Some((regions.clone(), line.to_owned()))
                } else {
                    None
                }
            };
            let mcs_skipped = mcs_replaced_by_embed_scope(
                prev_embed_scope_replaces,
                &context_id,
                context,
                syntax_set,
            );
            prev_embed_scope_replaces = context.embed_scope_replaces;
            self.has_while |= context.while_pattern.is_some();
            self.stack.push(StateLevel {
                context: context_id,
                prototypes: proto_ids,
                captures,
                mcs_skipped,
            });
        }

        // For Embed: push an EscapeEntry with the resolved escape regex
        if is_embed && let MatchOperation::Embed { ref escape, .. } = pat.operation {
            let resolved_regex = if escape.has_captures {
                // Resolve backrefs in escape regex using the triggering match's captures
                let new_regex_str =
                    substitute_backrefs_in_regex(escape.escape_regex.regex_str(), |i| {
                        regions.pos(i).map(|(s, e)| escape_str(&line[s..e]))
                    });
                Regex::new(new_regex_str)
            } else {
                escape.escape_regex.clone()
            };
            self.escape_stack.push(EscapeEntry {
                regex: resolved_regex,
                captures: escape.escape_captures.clone(),
                stack_depth: stack_depth_before,
                // .
                holder: embed_holder,
            });
        }

        Ok(true)
    }

    /// Execute an escape match: apply escape_captures, pop stack down to
    /// the embed's stack_depth, and remove the escape entry.
    fn exec_escape(
        &mut self,
        escape_idx: usize,
        match_start: usize,
        match_end: usize,
        regions: &Region,
        syntax_set: &SyntaxSet,
        ops: &mut Vec<(usize, ScopeStackOp)>,
    ) -> Result<(), ParsingError> {
        let entry = &self.escape_stack[escape_idx];
        let target_depth = entry.stack_depth;
        let escape_captures = entry.captures.clone();
        // See below.
        let holder = entry.holder;

        // Pop all stack levels down to target_depth, emitting proper meta scope pops
        while self.stack.len() > target_depth {
            let level = &self.stack[self.stack.len() - 1];
            let ctx = syntax_set.get_context(&level.context)?;

            // Pop meta_content_scope.  If the context below has
            // embed_scope_replaces (it's a v2 embed_scope wrapper), the top
            // context is the embedded syntax's main — whose mcs was never
            // pushed on the way in — so skip the pop here too.  Gating this
            // on `current_syntax_version >= 2` would be wrong: the version
            // is read from the top context (the embedded syntax), but
            // embed_scope_replaces is set only by the v2 host syntax.  A v2
            // host embedding a v1 grammar (e.g. Rails HTML embedding Ruby)
            // would otherwise Pop a scope that was never pushed, misaligning
            // every scope below until the escape closes.
            {
                // the frame says how much of its
                // `meta_content_scope` was pushed; the stack underneath cannot.
                // See `StateLevel::mcs_skipped`.
                let atoms = ctx
                    .meta_content_scope
                    .len()
                    .saturating_sub(level.mcs_skipped);
                if atoms > 0 {
                    ops.push((match_start, ScopeStackOp::Pop(atoms)));
                }
            }

            // Pop meta_scope
            if !ctx.meta_scope.is_empty() {
                ops.push((match_start, ScopeStackOp::Pop(ctx.meta_scope.len())));
            }

            // Restore cleared scopes
            if ctx.clear_scopes.is_some() {
                ops.push((match_start, ScopeStackOp::Restore));
            }

            self.stack.pop();
        }

        // A `pop: N + embed:`'s escape still wears
        // the meta scopes of the context the `embed:` was written in.
        //
        // The reference's own example under "Using pop with another action" is
        // the case: a `paragraph` context carrying `meta_scope`, whose ```py
        // fence fires `pop: 1` and embeds Python. ST scopes the closing fence
        // `source.lang meta.para` and this had it bare
        // (`probes/reference/action_pop_embed.sublime-syntax`, and the columns
        // either side of it agree — the opening fence is a lookahead and wears
        // none of it, the embedded text is outside it, and the character after
        // the escape is back in `main`).
        //
        // So the frame is *gone* for everything except this one token. It is
        // not deferred: the embedded text is scoped `source.lang m.embedded`
        // with no `m.para` in it, which is what says the `pop:` really did
        // happen at the trigger. `holder` is the frame it took off, remembered
        // at the pop for exactly this, and version 1 wants none of it — change
        // 32 is that half, and there the holder is off the stack already.
        let v1 = self.current_syntax_version(syntax_set) < 2;
        let holder_metas = match (v1, holder) {
            (false, Some((context_id, mcs_skipped))) => {
                let ctx = syntax_set.get_context(&context_id)?;
                let mut pushed = 0;
                for scope in ctx.meta_scope.iter() {
                    ops.push((match_start, ScopeStackOp::Push(*scope)));
                    pushed += 1;
                }
                for scope in ctx.meta_content_scope.iter().skip(mcs_skipped) {
                    ops.push((match_start, ScopeStackOp::Push(*scope)));
                    pushed += 1;
                }
                pushed
            }
            _ => 0,
        };

        // In a version 1 syntax the escape's own
        // text is outside the context that carries the `embed:` — it gets
        // neither that context's `meta_scope` nor its `meta_content_scope` —
        // and this gave it both.
        //
        // Sublime lists it under Compatibility as "Embed Escape Match and Meta
        // Scopes": the closing `'` of `'abc'` should come out `meta.group
        // meta.content punctuation.end` and in version 1 comes out
        // `punctuation.end` alone. ST 4206 agrees at both versions
        // (the `VerEscapeV1.sublime-syntax` probe, the
        // `escape_v1` and `escape_v2` jobs), and it is only the token: the
        // column after it is back inside `meta.group meta.content`.
        //
        // So they come off before the escape's captures and go back on after
        // it — the shape a `set:`'s meta scopes already have around their
        // trigger — off the frame the `embed:` fired from, which is the one
        // the escape has just popped back to. `mcs_skipped` is what says how
        // much of its `meta_content_scope` was ever pushed: a v2
        // host embedding through an `embed_scope` never pushed all of it.
        let v1_host_metas = if v1 {
            match self.stack.last() {
                Some(level) => {
                    let ctx = syntax_set.get_context(&level.context)?;
                    let mcs = ctx
                        .meta_content_scope
                        .len()
                        .saturating_sub(level.mcs_skipped);
                    let ms = ctx.meta_scope.len();
                    if mcs > 0 {
                        ops.push((match_start, ScopeStackOp::Pop(mcs)));
                    }
                    if ms > 0 {
                        ops.push((match_start, ScopeStackOp::Pop(ms)));
                    }
                    Some((level.context, level.mcs_skipped))
                }
                None => None,
            }
        } else {
            None
        };

        // Apply escape_captures scopes
        if let Some(ref capture_map) = escape_captures {
            let mut map: Vec<((usize, i32), ScopeStackOp)> = Vec::new();
            for &(cap_index, ref scopes) in capture_map.iter() {
                if let Some((cap_start, cap_end)) = regions.pos(cap_index) {
                    if cap_start == cap_end {
                        continue;
                    }
                    for scope in scopes.iter() {
                        map.push((
                            (cap_start, -((cap_end - cap_start) as i32)),
                            ScopeStackOp::Push(*scope),
                        ));
                    }
                    map.push(((cap_end, i32::MIN), ScopeStackOp::Pop(scopes.len())));
                }
            }
            map.sort_by_key(|a| a.0);
            for ((index, _), op) in map.into_iter() {
                ops.push((index, op));
            }
        }

        // — and off again after the token.
        if holder_metas > 0 {
            ops.push((match_end, ScopeStackOp::Pop(holder_metas)));
        }

        // — and back on, after the escape's own
        // text, in the order they were pushed the first time.
        if let Some((context_id, mcs_skipped)) = v1_host_metas {
            let ctx = syntax_set.get_context(&context_id)?;
            for scope in ctx.meta_scope.iter() {
                ops.push((match_end, ScopeStackOp::Push(*scope)));
            }
            for scope in ctx.meta_content_scope.iter().skip(mcs_skipped) {
                ops.push((match_end, ScopeStackOp::Push(*scope)));
            }
        }

        // Remove this escape entry and any inner (later) escape entries
        self.escape_stack.truncate(escape_idx);

        // Invalidate branch points whose alternative this escape has just
        // popped away. asked the one way — see
        // `branch_point_is_live`.
        let stack_len = self.stack.len();
        self.branch_points
            .retain(|bp| branch_point_is_live(bp, stack_len));

        Ok(())
    }
}

/// Escape a string for use in regex substitution (re-export for use in escape resolution).
fn escape_str(s: &str) -> String {
    escape(s)
}

/// Context stack storage shared by checkpoints and branch snapshots.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct SharedStack(Arc<Vec<StateLevel>>);
impl Deref for SharedStack {
    type Target = Vec<StateLevel>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for SharedStack {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Arc::make_mut(&mut self.0)
    }
}
const STACK_SWEEP_INTERVAL: usize = 8_192;
struct StackPool {
    buckets: HashMap<u64, Vec<Weak<Vec<StateLevel>>>>,
    collect_after: usize,
}
impl Default for StackPool {
    fn default() -> Self {
        Self {
            buckets: HashMap::new(),
            collect_after: STACK_SWEEP_INTERVAL,
        }
    }
}
impl StackPool {
    fn sweep_if_needed(&mut self) -> bool {
        if self.buckets.len() <= self.collect_after {
            return false;
        }
        self.buckets.retain(|_, entries| {
            entries.retain(|entry| entry.strong_count() > 0);
            !entries.is_empty()
        });
        // Live checkpoints may exceed the initial threshold. Leave room for
        // another interval of new buckets instead of rescanning all live stacks
        // on every subsequent line. Weak entries never keep a stack alive.
        self.collect_after = self.buckets.len().saturating_add(STACK_SWEEP_INTERVAL);
        true
    }
}
static STACK_POOL: LazyLock<Mutex<StackPool>> = LazyLock::new(|| Mutex::new(StackPool::default()));
impl SharedStack {
    fn intern(&mut self) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        let key = h.finish();
        let mut pool = STACK_POOL.lock().unwrap_or_else(|e| e.into_inner());
        pool.sweep_if_needed();
        let bucket = pool.buckets.entry(key).or_default();
        bucket.retain(|p| p.strong_count() > 0);
        for weak in bucket.iter() {
            if let Some(shared) = weak.upgrade()
                && (Arc::ptr_eq(&shared, &self.0) || *shared == *self.0)
            {
                self.0 = shared;
                return;
            }
        }
        bucket.push(Arc::downgrade(&self.0));
    }
}
impl InterpreterState {
    #[cold]
    #[inline(never)]
    fn check_while_patterns(
        &mut self,
        line: &str,
        syntax_set: &SyntaxSet,
        match_start: &mut usize,
        res: &mut ScopeOperations,
    ) -> Result<(), ParsingError> {
        // TextMate while rules are tested outermost first, even when an inner
        // begin/end rule is still open. Failure unwinds that entire region.
        let mut depth = 0;
        while depth < self.stack.len() {
            let level = &self.stack[depth];
            let context = syntax_set.get_context(&level.context)?;
            if let Some(pattern) = &context.while_pattern {
                let regex = match &level.captures {
                    Some((captures, text)) if pattern.has_captures => {
                        pattern.regex_with_refs(captures, text)
                    }
                    _ => pattern.regex.clone(),
                };
                let mut captures = Region::new();
                if !regex.search(line, *match_start, line.len(), Some(&mut captures), true)
                    || captures.pos(0).is_none_or(|span| span.0 != *match_start)
                {
                    while self.stack.len() > depth {
                        let level = self.stack.pop().expect("while frame");
                        let context = syntax_set.get_context(&level.context)?;
                        let count = context
                            .meta_content_scope
                            .len()
                            .saturating_sub(level.mcs_skipped)
                            + context.meta_scope.len();
                        if count > 0 {
                            res.push((*match_start, ScopeStackOp::Pop(count)));
                        }
                        if context.clear_scopes.is_some() {
                            res.push((*match_start, ScopeStackOp::Restore));
                        }
                    }
                    self.escape_stack.retain(|e| e.stack_depth < depth);
                    self.branch_points
                        .retain(|bp| branch_point_is_live(bp, depth));
                    break;
                }
                if let Some(map) = &pattern.captures {
                    res.extend(build_capture_ops(map, &captures, 2));
                }
                *match_start = captures.pos(0).expect("matched while").1;
            }
            depth += 1;
        }
        Ok(())
    }
    pub(crate) fn semantic_eq(&self, other: &Self) -> bool {
        if self.branch_points.is_empty() && other.branch_points.is_empty() {
            self.base_context == other.base_context
                && self.first_line == other.first_line
                && self.stack == other.stack
                && self.proto_starts == other.proto_starts
                && self.escape_stack == other.escape_stack
        } else {
            self == other
        }
    }
    pub(crate) fn fingerprint(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.base_context.hash(&mut h);
        self.first_line.hash(&mut h);
        self.stack.hash(&mut h);
        self.proto_starts.hash(&mut h);
        self.escape_stack.hash(&mut h);
        if !self.branch_points.is_empty() {
            self.line_number.hash(&mut h);
            self.branch_points.hash(&mut h);
            self.pending_lines.hash(&mut h);
        }
        h.finish()
    }
}

#[cfg(test)]
mod stack_pool_tests {
    use super::*;

    #[test]
    fn live_checkpoints_do_not_trigger_a_full_sweep_on_every_line() {
        let live = Arc::new(Vec::<StateLevel>::new());
        let mut pool = StackPool::default();
        for key in 0..=STACK_SWEEP_INTERVAL as u64 {
            pool.buckets.insert(key, vec![Arc::downgrade(&live)]);
        }
        assert!(pool.sweep_if_needed());
        for key in STACK_SWEEP_INTERVAL as u64 + 1..STACK_SWEEP_INTERVAL as u64 + 1_001 {
            pool.buckets.insert(key, vec![Weak::new()]);
            assert!(
                !pool.sweep_if_needed(),
                "live stacks must not force repeated full sweeps"
            );
        }
        drop(live);
        let mut key = STACK_SWEEP_INTERVAL as u64 + 1_001;
        while !pool.sweep_if_needed() {
            pool.buckets.insert(key, vec![Weak::new()]);
            key += 1;
            assert!(key <= 2 * STACK_SWEEP_INTERVAL as u64 + 2);
        }
        assert!(
            pool.buckets.is_empty(),
            "dead metadata must eventually be reclaimed"
        );
    }
}
