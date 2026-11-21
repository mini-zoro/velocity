use std::sync::Arc;
use vel_scope::{CompactScopeOp, ScopeStackOp};

/// Shared byte-indexed parser operations. The compact representation is exact;
/// unusually large offsets and counts retain the full-width representation.
#[derive(Clone, Debug)]
pub struct SharedScopeOperations(Storage);

#[derive(Clone, Debug)]
enum Storage {
    Compact(Arc<[CompactScopeOp]>),
    Wide(Arc<[(usize, ScopeStackOp)]>),
}

#[derive(Debug, Default)]
pub(crate) struct PackingScratch(Vec<CompactScopeOp>);

impl Default for SharedScopeOperations {
    fn default() -> Self {
        Self(Storage::Compact(Arc::from([])))
    }
}

impl SharedScopeOperations {
    /// Retains an immutable copy of ordered operations.
    pub fn new(ops: &[(usize, ScopeStackOp)]) -> Self {
        let mut scratch = PackingScratch::default();
        Self::with_scratch(ops, &mut scratch)
    }

    pub(crate) fn with_scratch(
        ops: &[(usize, ScopeStackOp)],
        scratch: &mut PackingScratch,
    ) -> Self {
        let scratch = &mut scratch.0;
        scratch.clear();
        for &(at, op) in ops {
            let Some(packed) = CompactScopeOp::pack(at, op) else {
                return Self(Storage::Wide(Arc::from(ops)));
            };
            scratch.push(packed);
        }
        Self(Storage::Compact(Arc::from(scratch.as_slice())))
    }

    pub(crate) fn matches(&self, operations: &[(usize, ScopeStackOp)]) -> bool {
        match &self.0 {
            Storage::Wide(ops) => ops.as_ref() == operations,
            Storage::Compact(ops) => {
                ops.len() == operations.len()
                    && ops
                        .iter()
                        .copied()
                        .map(CompactScopeOp::unpack)
                        .eq(operations.iter().copied())
            }
        }
    }

    pub(crate) fn write_to(&self, scratch: &mut Vec<(usize, ScopeStackOp)>) {
        scratch.clear();
        match &self.0 {
            Storage::Wide(ops) => scratch.extend_from_slice(ops),
            Storage::Compact(ops) => {
                scratch.extend(ops.iter().copied().map(CompactScopeOp::unpack))
            }
        }
    }

    /// Borrows a wide slice, expanding small operations into reusable scratch.
    pub fn expanded<'a>(
        &'a self,
        scratch: &'a mut Vec<(usize, ScopeStackOp)>,
    ) -> &'a [(usize, ScopeStackOp)] {
        if let Storage::Wide(ops) = &self.0 {
            ops
        } else {
            self.write_to(scratch);
            scratch
        }
    }

    /// Identity token valid while at least one handle to the allocation is kept.
    pub fn address(&self) -> usize {
        match &self.0 {
            Storage::Compact(ops) => ops.as_ptr() as usize,
            Storage::Wide(ops) => ops.as_ptr() as usize,
        }
    }

    /// Bytes in the allocation's slice, excluding the Arc header.
    pub fn payload_bytes(&self) -> usize {
        match &self.0 {
            Storage::Compact(ops) => size_of_val(ops.as_ref()),
            Storage::Wide(ops) => size_of_val(ops.as_ref()),
        }
    }

    /// Whether all positions belong to scalar boundaries in this line.
    pub fn matches_boundaries(&self, line: &str) -> bool {
        match &self.0 {
            Storage::Compact(ops) => ops.iter().all(|op| line.is_char_boundary(op.byte())),
            Storage::Wide(ops) => ops.iter().all(|(at, _)| line.is_char_boundary(*at)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vel_scope::{ClearAmount, Scope};

    #[test]
    fn stored_operations_round_trip_all_variants_and_large_values() {
        let original = [
            (0, ScopeStackOp::Push(Scope::new("source.test").unwrap())),
            (1, ScopeStackOp::Pop(2)),
            (1, ScopeStackOp::Clear(ClearAmount::All)),
            (1, ScopeStackOp::Restore),
            (70_000, ScopeStackOp::Clear(ClearAmount::TopN(71_000))),
            (70_000, ScopeStackOp::Noop),
        ];
        let mut scratch = Vec::new();
        let packed = SharedScopeOperations::new(&original);
        assert!(packed.matches(&original));
        assert_eq!(packed.expanded(&mut scratch), original);
        assert_eq!(packed.payload_bytes(), 8 * original.len());
        let original = [(2, ScopeStackOp::Pop(usize::MAX))];
        let wide = SharedScopeOperations::new(&original);
        assert_eq!(wide.expanded(&mut scratch), original);
        if usize::BITS > 32 {
            assert!(matches!(wide.0, Storage::Wide(_)));
        }
        let original = [((u32::MAX >> 3) as usize + 1, ScopeStackOp::Noop)];
        let wide = SharedScopeOperations::new(&original);
        assert_eq!(wide.expanded(&mut scratch), original);
        assert!(matches!(wide.0, Storage::Wide(_)));
    }
}
