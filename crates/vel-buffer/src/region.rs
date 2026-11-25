use std::{ops::Range, sync::Arc};
use vel_text::Text;

/// A byte region with an anchor (`a`), caret (`b`) and optional vertical goal.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Region {
    /// Anchor, in UTF-8 bytes.
    pub a: usize,
    /// Caret, in UTF-8 bytes.
    pub b: usize,
    /// Goal supplied by a vertical layout; `None` after horizontal movement.
    pub xpos: Option<f64>,
}
impl Region {
    /// Makes a region without a vertical goal.
    pub const fn new(a: usize, b: usize) -> Self {
        Self { a, b, xpos: None }
    }
    /// Makes an empty caret.
    pub const fn caret(point: usize) -> Self {
        Self::new(point, point)
    }
    /// Smaller endpoint.
    pub fn begin(self) -> usize {
        self.a.min(self.b)
    }
    /// Larger endpoint.
    pub fn end(self) -> usize {
        self.a.max(self.b)
    }
    /// Size in bytes.
    pub fn size(self) -> usize {
        self.a.abs_diff(self.b)
    }
    /// Whether both endpoints coincide.
    pub fn empty(self) -> bool {
        self.a == self.b
    }
    /// Whether a point or a complete region is enclosed (including endpoints).
    pub fn contains(self, other: impl Into<Region>) -> bool {
        // ST-BEHAVIOR: unverified [R1]: point containment includes both endpoints; cover/intersection return forward regions.
        let other = other.into();
        self.begin() <= other.begin() && other.end() <= self.end()
    }
    /// Smallest forward region enclosing both regions.
    pub fn cover(self, other: Region) -> Region {
        Self::new(self.begin().min(other.begin()), self.end().max(other.end()))
    }
    /// Intersection, or an empty region at zero when disjoint.
    pub fn intersection(self, other: Region) -> Region {
        // ST-BEHAVIOR: unverified [R1]: disjoint intersection is (0, 0); touching is empty at the touch.
        let a = self.begin().max(other.begin());
        let b = self.end().min(other.end());
        if a > b {
            Self::caret(0)
        } else {
            Self::new(a, b)
        }
    }
    /// Whether a non-empty interval is shared; carets intersect enclosed endpoints.
    pub fn intersects(self, other: Region) -> bool {
        // ST-BEHAVIOR: unverified [R1]: caret intersections include either boundary.
        if self.empty() {
            other.contains(self.a)
        } else if other.empty() {
            self.contains(other.a)
        } else {
            self.begin() < other.end() && other.begin() < self.end()
        }
    }
    /// Half-open range in forward order.
    pub fn to_range(self) -> Range<usize> {
        self.begin()..self.end()
    }
    pub(crate) fn directed(self, a: usize, b: usize) -> Self {
        let mut r = if self.a <= self.b {
            Self::new(a, b)
        } else {
            Self::new(b, a)
        };
        r.xpos = self.xpos;
        r
    }
}
impl From<usize> for Region {
    fn from(value: usize) -> Self {
        Self::caret(value)
    }
}
impl From<Range<usize>> for Region {
    fn from(value: Range<usize>) -> Self {
        Self::new(value.start, value.end)
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Regions {
    // Goals are a separate allocation, absent on the typing path: 16 bytes/caret.
    points: Vec<usize>,
    anchors: Vec<usize>,
    goals: Vec<Option<f64>>,
}
/// Sorted, normalized regions with an independently tracked primary region.
/// `clear()` permits a temporary empty set; `normalize()` restores a caret at zero.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Selection {
    data: Arc<Regions>,
    primary: usize,
}
impl Default for Selection {
    fn default() -> Self {
        Self::from(Region::caret(0))
    }
}
impl From<Region> for Selection {
    fn from(r: Region) -> Self {
        Self::from_regions([r])
    }
}
impl Selection {
    /// Whether two immutable snapshots share their regions and primary index, in O(1).
    /// Useful for observers which must not compare every caret on each input event.
    pub fn same_snapshot(&self, other: &Self) -> bool {
        self.primary == other.primary && Arc::ptr_eq(&self.data, &other.data)
    }
    pub(crate) fn snapshot_relative(&mut self, base: usize, decode: bool) {
        let data = Arc::make_mut(&mut self.data);
        for p in data.points.iter_mut().chain(&mut data.anchors) {
            *p = if decode {
                base.wrapping_add((*p >> 1) ^ 0usize.wrapping_sub(*p & 1))
            } else {
                let delta = p.wrapping_sub(base);
                (delta << 1) ^ ((delta as isize >> (usize::BITS - 1)) as usize)
            };
        }
    }
    pub(crate) fn snapshot_valid(&self, text: Option<&Text>) -> bool {
        let n = self.data.points.len();
        if n == 0
            || self.primary >= n
            || (!self.data.anchors.is_empty() && self.data.anchors.len() != n)
            || (!self.data.goals.is_empty() && self.data.goals.len() != n)
        {
            return false;
        }
        let mut last: Option<Region> = None;
        for r in self.iter() {
            if r.xpos.is_some_and(|x| !x.is_finite())
                || text.is_some_and(|t| !t.is_char_boundary(r.a) || !t.is_char_boundary(r.b))
                || last.is_some_and(|p| !precedes(p, r))
            {
                return false;
            }
            last = Some(r);
        }
        true
    }

    /// A temporary empty selection, for building a replacement.
    pub fn new() -> Self {
        Self {
            data: Arc::new(Regions::default()),
            primary: 0,
        }
    }
    /// Normalizes regions, with the last input region primary.
    pub fn from_regions(regions: impl IntoIterator<Item = Region>) -> Self {
        Self::from_regions_impl(regions, false)
    }
    /// Builds a selection from search results, retaining each nonempty match as
    /// a separate editing target even when two matches share an endpoint.
    pub fn from_match_regions(regions: impl IntoIterator<Item = Region>) -> Self {
        Self::from_regions_impl(regions, true)
    }
    fn from_regions_impl(regions: impl IntoIterator<Item = Region>, keep_adjacent: bool) -> Self {
        let mut items: Vec<_> = regions.into_iter().enumerate().collect();
        items.sort_by_key(|(_, r)| r.begin());
        let mut result: Vec<(usize, Region)> = Vec::with_capacity(items.len());
        for (rank, r) in items {
            if let Some((prev_rank, prev)) = result.last_mut()
                && if keep_adjacent {
                    prev.intersects(r)
                } else {
                    mergeable(*prev, r)
                }
            {
                let last = if rank >= *prev_rank { r } else { *prev };
                *prev = last.directed(prev.begin().min(r.begin()), prev.end().max(r.end()));
                *prev_rank = (*prev_rank).max(rank);
            } else {
                result.push((rank, r));
            }
        }
        let primary = result
            .iter()
            .enumerate()
            .max_by_key(|(_, (rank, _))| *rank)
            .map_or(0, |(i, _)| i);
        let mut s = Self::new();
        Arc::make_mut(&mut s.data)
            .points
            .reserve_exact(result.len());
        for (_, r) in result {
            s.push(r);
        }
        s.primary = primary;
        s.normalize();
        s
    }
    /// Adds one region, merging overlaps and touching non-empty intervals.
    pub fn add(&mut self, region: impl Into<Region>) {
        let r = region.into();
        let mut lo = 0;
        let mut hi = self.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.get(mid).expect("in bounds").end() < r.begin() {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let first = lo;
        let mut last = first;
        let mut merged = r;
        while last < self.len() {
            let old = self.get(last).expect("in bounds");
            if old.begin() > merged.end() {
                break;
            }
            if mergeable(old, merged) {
                merged = r.directed(old.begin().min(merged.begin()), old.end().max(merged.end()));
                last += 1;
            } else if old.end() == merged.begin() {
                last += 1;
            } else {
                break;
            }
        }
        // Empty/nonempty boundary pairs merge too, avoiding ambiguous edit ownership.
        // ST-BEHAVIOR: unverified [S1]: last-added direction and goal win, including boundary carets.
        let data = Arc::make_mut(&mut self.data);
        let anchors = !data.anchors.is_empty() || !merged.empty();
        if data.anchors.is_empty() && anchors {
            data.anchors = data.points.clone();
        }
        data.points.splice(first..last, [merged.b]);
        if anchors {
            data.anchors.splice(first..last, [merged.a]);
        }
        if !data.goals.is_empty() {
            data.goals.splice(first..last, [merged.xpos]);
        } else if merged.xpos.is_some() {
            data.goals = vec![None; data.points.len()];
            data.goals[first] = merged.xpos;
        }
        self.primary = first;
    }
    /// Adds regions in interaction order.
    pub fn add_all(&mut self, regions: impl IntoIterator<Item = Region>) {
        for r in regions {
            self.add(r);
        }
        self.normalize();
    }
    /// Removes the covered portion, preserving the direction of surviving pieces.
    pub fn subtract(&mut self, cut: Region) {
        // ST-BEHAVIOR: unverified [S2]: split pieces keep direction; removing all restores a caret at cut.begin().
        let primary = self.primary();
        let mut out = Vec::new();
        for r in self.iter() {
            if cut.empty() {
                if !(r.empty() && r.b == cut.b) {
                    out.push(r);
                }
            } else if !r.intersects(cut) {
                out.push(r);
            } else {
                if r.begin() < cut.begin() {
                    out.push(r.directed(r.begin(), cut.begin()));
                }
                if r.end() > cut.end() {
                    out.push(r.directed(cut.end(), r.end()));
                }
            }
        }
        if out.is_empty() {
            out.push(Region::caret(cut.begin()));
        }
        *self = Self::from_regions(out);
        if let Some(r) = primary {
            let index = self
                .iter()
                .position(|p| p.contains(r.b))
                .unwrap_or(self.len() - 1);
            self.primary = index;
        }
    }
    /// Removes all regions until the next normalization.
    pub fn clear(&mut self) {
        self.data = Arc::new(Regions::default());
        self.primary = 0;
    }
    /// Restores a caret at zero if temporarily empty.
    pub fn normalize(&mut self) {
        // ST-BEHAVIOR: unverified [S3]: empty normalization restores a caret at zero; clear/new may temporarily be empty.
        if self.is_empty() {
            self.push(Region::caret(0));
            self.primary = 0;
        }
    }
    /// Whether one region contains the point or region.
    pub fn contains(&self, region: impl Into<Region>) -> bool {
        let r = region.into();
        let mut lo = 0;
        let mut hi = self.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.get(mid).expect("in bounds").begin() <= r.begin() {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let i = lo;
        i > 0 && self.get(i - 1).is_some_and(|old| old.contains(r))
    }
    /// Regions, in document order. Values are copied without allocation.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = Region> + DoubleEndedIterator + '_ {
        (0..self.len()).map(|i| self.get(i).expect("in bounds"))
    }
    /// Number of regions.
    pub fn len(&self) -> usize {
        self.data.points.len()
    }
    /// Whether temporarily empty.
    pub fn is_empty(&self) -> bool {
        self.data.points.is_empty()
    }
    /// Last interactively added region, independent of document order.
    pub fn primary(&self) -> Option<Region> {
        self.get(self.primary)
    }
    /// Index of the primary region.
    pub fn primary_index(&self) -> usize {
        self.primary
    }
    /// Region by sorted index.
    pub fn get(&self, i: usize) -> Option<Region> {
        self.data.points.get(i).map(|&b| Region {
            a: self.data.anchors.get(i).copied().unwrap_or(b),
            b,
            xpos: self.data.goals.get(i).copied().flatten(),
        })
    }
    /// Regions touching the given LF line (a terminating LF belongs to that line).
    pub fn regions_on_line<'a>(
        &'a self,
        text: &'a Text,
        line: usize,
    ) -> impl Iterator<Item = Region> + 'a {
        let a = text.line_to_byte(line);
        let b = if line + 1 < text.len_lines() {
            text.line_to_byte(line + 1)
        } else {
            text.len_bytes() + 1
        };
        // ST-BEHAVIOR: unverified [L1]: a nonempty end at BOL belongs to the preceding line.
        self.iter()
            .filter(move |r| r.begin() < b && if r.empty() { r.end() >= a } else { r.end() > a })
    }
    /// Storage in region arrays, excluding shared Arc headers.
    pub fn storage_bytes(&self) -> usize {
        (self.data.points.capacity() + self.data.anchors.capacity()) * std::mem::size_of::<usize>()
            + self.data.goals.capacity() * std::mem::size_of::<Option<f64>>()
    }
    pub(crate) fn header_bytes() -> usize {
        std::mem::size_of::<Regions>() + 2 * std::mem::size_of::<usize>()
    }
    pub(crate) fn history_allocation(&self) -> (usize, usize) {
        (
            Arc::as_ptr(&self.data) as usize,
            self.storage_bytes() + Self::header_bytes(),
        )
    }
    pub(crate) fn push(&mut self, r: Region) {
        let d = Arc::make_mut(&mut self.data);
        if d.anchors.is_empty() && !r.empty() {
            d.anchors = d.points.clone();
        }
        if !d.anchors.is_empty() || !r.empty() {
            d.anchors.push(r.a);
        }
        d.points.push(r.b);
        if !d.goals.is_empty() {
            d.goals.push(r.xpos);
        } else if r.xpos.is_some() {
            d.goals = vec![None; d.points.len()];
            *d.goals.last_mut().expect("one region") = r.xpos;
        }
    }
    /// Maps regions, normalizing overlaps while retaining the mapped primary region.
    pub fn map(&self, mut f: impl FnMut(Region) -> Region) -> Self {
        let mut primary = Region::caret(0);
        let regions: Vec<_> = self
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let mapped = f(r);
                if i == self.primary {
                    primary = mapped;
                }
                mapped
            })
            .collect();
        Self::from_regions(regions).with_primary_region(primary)
    }
    pub(crate) fn with_primary_region(mut self, r: Region) -> Self {
        let mut lo = 0;
        let mut hi = self.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.get(mid).expect("in bounds").begin() <= r.begin() {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        self.primary = lo.saturating_sub(1);
        if let Some(old) = self.get(self.primary) {
            let merged = r.directed(old.begin(), old.end());
            if (old.a > old.b) != (merged.a > merged.b) || old.xpos != merged.xpos {
                let data = Arc::make_mut(&mut self.data);
                if data.anchors.is_empty() {
                    data.anchors = data.points.clone();
                }
                data.points[self.primary] = merged.b;
                data.anchors[self.primary] = merged.a;
                if !data.goals.is_empty() {
                    data.goals[self.primary] = merged.xpos;
                } else if merged.xpos.is_some() {
                    data.goals = vec![None; data.points.len()];
                    data.goals[self.primary] = merged.xpos;
                }
            }
        }
        self
    }
    pub(crate) fn reset_xpos(&mut self) {
        if !self.data.goals.is_empty() {
            Arc::make_mut(&mut self.data).goals.clear();
        }
    }
    pub(crate) fn with_primary(mut self, i: usize) -> Self {
        self.primary = i.min(self.len().saturating_sub(1));
        self
    }
}
fn precedes(a: Region, b: Region) -> bool {
    // Two nonempty ranges can share an endpoint without an intersection.
    a.end() < b.begin() || (a.end() == b.begin() && !a.empty() && !b.empty())
}
fn mergeable(a: Region, b: Region) -> bool {
    a.begin() <= b.end() && b.begin() <= a.end()
}
