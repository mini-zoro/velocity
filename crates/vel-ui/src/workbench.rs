//! Normalized tiled layouts and ordered sheet selections, independent of rendering.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use vel_platform::{Point, Rect};

/// Public `set_layout` coordinates. Cells contain left, top, right, bottom indices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// Strictly increasing normalized horizontal coordinates, including 0 and 1.
    pub cols: Vec<f64>,
    /// Strictly increasing normalized vertical coordinates, including 0 and 1.
    pub rows: Vec<f64>,
    /// Nonoverlapping rectangular groups covering the whole unit square.
    pub cells: Vec<[usize; 4]>,
}
impl Default for Layout {
    fn default() -> Self {
        Self::grid(1, 1)
    }
}
impl Layout {
    /// Equal-sized preset, in row-major order.
    pub fn grid(columns: usize, rows: usize) -> Self {
        let columns = columns.clamp(1, 64);
        let rows = rows.clamp(1, 64 / columns);
        Self {
            cols: (0..=columns).map(|n| n as f64 / columns as f64).collect(),
            rows: (0..=rows).map(|n| n as f64 / rows as f64).collect(),
            cells: (0..rows)
                .flat_map(|y| (0..columns).map(move |x| [x, y, x + 1, y + 1]))
                .collect(),
        }
    }
    /// Rejects malformed, nonfinite, overlapping or incomplete layouts before mutation.
    pub fn validate(&self) -> Result<(), String> {
        for coordinates in [&self.cols, &self.rows] {
            if coordinates.len() < 2
                || coordinates.len() > 65
                || coordinates.first() != Some(&0.)
                || coordinates.last() != Some(&1.)
                || coordinates.iter().any(|n| !n.is_finite())
                || coordinates.windows(2).any(|p| p[0] >= p[1])
            {
                return Err("layout coordinates must increase from 0 to 1".into());
            }
        }
        if self.cells.is_empty() || self.cells.len() > 64 {
            return Err("layout requires 1..64 groups".into());
        }
        let width = self.cols.len() - 1;
        let mut covered = vec![false; width * (self.rows.len() - 1)];
        for &[l, t, r, b] in &self.cells {
            if l >= r || t >= b || r >= self.cols.len() || b >= self.rows.len() {
                return Err("invalid layout cell".into());
            }
            for y in t..b {
                for x in l..r {
                    if std::mem::replace(&mut covered[y * width + x], true) {
                        return Err("overlapping layout cells".into());
                    }
                }
            }
        }
        if covered.iter().any(|v| !v) {
            return Err("layout has a gap".into());
        }
        Ok(())
    }
    /// Projects a cell into logical content coordinates.
    pub fn rect(&self, group: usize, bounds: Rect) -> Option<Rect> {
        let &[l, t, r, b] = self.cells.get(group)?;
        Some(Rect {
            x: bounds.x + self.cols[l] * bounds.width,
            y: bounds.y + self.rows[t] * bounds.height,
            width: (self.cols[r] - self.cols[l]) * bounds.width,
            height: (self.rows[b] - self.rows[t]) * bounds.height,
        })
    }
    /// Hit-tests an internal boundary, with tolerance in logical points.
    pub fn splitter(&self, point: Point, bounds: Rect, tolerance: f64) -> Option<Splitter> {
        if !crate::contains(bounds, point) {
            return None;
        }
        for (splitter, rect) in self.dividers(bounds, tolerance * 2.) {
            if crate::contains(rect, point) {
                return Some(splitter);
            }
        }
        None
    }
    /// Visible divider segments. A spanning cell never gains an artificial interior line.
    pub fn dividers(&self, bounds: Rect, thickness: f64) -> Vec<(Splitter, Rect)> {
        let mut out = Vec::new();
        for (vertical, coordinates) in [(true, &self.cols), (false, &self.rows)] {
            for (index, coordinate) in coordinates
                .iter()
                .enumerate()
                .skip(1)
                .take(coordinates.len().saturating_sub(2))
            {
                let mut segments = Vec::<(f64, f64)>::new();
                for &[l, t, r, b] in &self.cells {
                    if vertical && (l == index || r == index) {
                        segments.push((self.rows[t], self.rows[b]));
                    }
                    if !vertical && (t == index || b == index) {
                        segments.push((self.cols[l], self.cols[r]));
                    }
                }
                segments.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut merged = Vec::<(f64, f64)>::new();
                for (start, end) in segments {
                    if let Some(last) = merged.last_mut()
                        && start <= last.1
                    {
                        last.1 = last.1.max(end);
                    } else {
                        merged.push((start, end));
                    }
                }
                for (start, end) in merged {
                    let rect = if vertical {
                        Rect {
                            x: bounds.x + coordinate * bounds.width - thickness / 2.,
                            y: bounds.y + start * bounds.height,
                            width: thickness,
                            height: (end - start) * bounds.height,
                        }
                    } else {
                        Rect {
                            x: bounds.x + start * bounds.width,
                            y: bounds.y + coordinate * bounds.height - thickness / 2.,
                            width: (end - start) * bounds.width,
                            height: thickness,
                        }
                    };
                    out.push((Splitter { vertical, index }, rect));
                }
            }
        }
        out
    }
    /// Drags a divider while retaining strict order and a minimum adjacent extent.
    pub fn drag(&mut self, splitter: Splitter, position: f64) -> bool {
        let coords = if splitter.vertical {
            &mut self.cols
        } else {
            &mut self.rows
        };
        let i = splitter.index;
        if i == 0 || i + 1 >= coords.len() || !position.is_finite() {
            return false;
        }
        let margin = 0.04_f64.min((coords[i + 1] - coords[i - 1]) / 3.);
        coords[i] = position.clamp(coords[i - 1] + margin, coords[i + 1] - margin);
        true
    }
    /// Drags a divider with a minimum logical size for every touching cell.
    pub fn drag_with_minimum(
        &mut self,
        splitter: Splitter,
        position: f64,
        bounds: Rect,
        minimum: (f64, f64),
    ) -> bool {
        if !position.is_finite() {
            return false;
        }
        let (lo, hi, coords, extent, pixels) = if splitter.vertical {
            (0, 2, &mut self.cols, bounds.width, minimum.0)
        } else {
            (1, 3, &mut self.rows, bounds.height, minimum.1)
        };
        let i = splitter.index;
        if i == 0 || i + 1 >= coords.len() || extent <= 0. {
            return false;
        }
        let mut low = coords[i - 1] + f64::EPSILON;
        let mut high = coords[i + 1] - f64::EPSILON;
        for cell in &self.cells {
            if cell[hi] == i {
                low = low.max(coords[cell[lo]] + pixels / extent);
            }
            if cell[lo] == i {
                high = high.min(coords[cell[hi]] - pixels / extent);
            }
        }
        if low > high {
            return false;
        }
        coords[i] = position.clamp(low, high);
        true
    }
    /// Finds the closest group in a cardinal direction, preferring overlapping edges.
    pub fn neighbor(&self, group: usize, direction: Direction) -> Option<usize> {
        let unit = Rect {
            x: 0.,
            y: 0.,
            width: 1.,
            height: 1.,
        };
        let a = self.rect(group, unit)?;
        self.cells
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != group)
            .filter_map(|(i, _)| {
                let b = self.rect(i, unit)?;
                let dx = b.x + b.width / 2. - a.x - a.width / 2.;
                let dy = b.y + b.height / 2. - a.y - a.height / 2.;
                let (forward, lateral) = match direction {
                    Direction::Right => (dx, dy.abs()),
                    Direction::Left => (-dx, dy.abs()),
                    Direction::Down => (dy, dx.abs()),
                    Direction::Up => (-dy, dx.abs()),
                };
                (forward > 0.00001).then_some((i, forward + lateral * 4.))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }
    /// Splits one cell without changing the other groups.
    pub fn split(&mut self, group: usize, vertical: bool) -> Result<(), String> {
        if self.cells.len() >= 64 {
            return Err("too many groups".into());
        }
        let cell = *self.cells.get(group).ok_or("unknown group")?;
        let (lo, hi) = if vertical { (0, 2) } else { (1, 3) };
        let coords = if vertical {
            &mut self.cols
        } else {
            &mut self.rows
        };
        let middle = (coords[cell[lo]] + coords[cell[hi]]) / 2.;
        let index = coords.partition_point(|n| *n < middle);
        if coords.get(index) != Some(&middle) {
            coords.insert(index, middle);
            for c in &mut self.cells {
                for n in [lo, hi] {
                    if c[n] >= index {
                        c[n] += 1;
                    }
                }
            }
        }
        let mut next = self.cells[group];
        next[lo] = index;
        self.cells[group][hi] = index;
        self.cells.push(next);
        self.validate()
    }
    /// Removes a cell, expanding a matching neighbor; complex tilings fall back to columns.
    pub fn remove(&mut self, group: usize) -> Result<(), String> {
        if self.cells.len() == 1 || group >= self.cells.len() {
            return Err("cannot remove group".into());
        }
        let [l, t, r, b] = self.cells[group];
        let neighbor = self.cells.iter().enumerate().find_map(|(i, c)| {
            if i == group {
                return None;
            }
            let [cl, ct, cr, cb] = *c;
            if (ct == t && cb == b && (cr == l || cl == r))
                || (cl == l && cr == r && (cb == t || ct == b))
            {
                Some(i)
            } else {
                None
            }
        });
        if let Some(i) = neighbor {
            self.cells[i] = [
                l.min(self.cells[i][0]),
                t.min(self.cells[i][1]),
                r.max(self.cells[i][2]),
                b.max(self.cells[i][3]),
            ];
            self.cells.remove(group);
        } else {
            *self = Self::grid(self.cells.len() - 1, 1);
        }
        self.validate()
    }
}
/// An internal row/column divider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Splitter {
    /// True for a vertical divider (horizontal movement).
    pub vertical: bool,
    /// Coordinate index, excluding endpoints.
    pub index: usize,
}
/// Cardinal navigation direction.
#[derive(Clone, Copy, Debug)]
pub enum Direction {
    /// Left.
    Left,
    /// Right.
    Right,
    /// Up.
    Up,
    /// Down.
    Down,
}
/// Per-group ordered tabs, selection and focus. IDs remain stable during reorder/movement.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Group {
    /// Tab order.
    pub sheets: Vec<u64>,
    /// Simultaneously displayed sheets.
    pub selected: BTreeSet<u64>,
    /// Keyboard focus; must belong to `selected` unless empty.
    pub focused: Option<u64>,
    /// Shift-selection anchor.
    pub anchor: Option<u64>,
    /// Horizontal tab-bar scroll offset.
    pub tab_scroll: f64,
    /// Most recently focused sheets first; older sessions start with an empty stack.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mru: Vec<u64>,
}
impl Group {
    /// Applies plain, primary-modifier toggle, or shift-range selection.
    pub fn click(&mut self, id: u64, toggle: bool, extend: bool) {
        let Some(index) = self.sheets.iter().position(|s| *s == id) else {
            return;
        };
        if extend {
            let anchor = self
                .anchor
                .and_then(|a| self.sheets.iter().position(|s| *s == a))
                .unwrap_or(index);
            if !toggle {
                self.selected.clear();
            }
            self.selected
                .extend(self.sheets[index.min(anchor)..=index.max(anchor)].iter());
            self.focused = Some(id);
        } else if toggle && self.selected.contains(&id) {
            if self.selected.len() > 1 {
                self.selected.remove(&id);
            }
            if !self.selected.contains(&self.focused.unwrap_or(id)) {
                self.focused = self.selected.first().copied();
            }
            self.anchor = self.focused;
        } else {
            if !toggle {
                self.selected.clear();
            }
            self.selected.insert(id);
            self.focused = Some(id);
            self.anchor = Some(id);
        }
        self.touch_focus();
    }
    /// Focuses a tab in place of the focused member without deselecting its peers.
    pub fn replace_focused(&mut self, id: u64) {
        if !self.sheets.contains(&id) {
            return;
        }
        if self.selected.insert(id)
            && let Some(previous) = self.focused
        {
            self.selected.remove(&previous);
        }
        self.focused = Some(id);
        self.anchor = Some(id);
        self.touch_focus();
    }
    /// Commits the focused sheet as most recently used.
    pub fn touch_focus(&mut self) {
        if let Some(focus) = self.focused
            && self.mru.first() != Some(&focus)
        {
            self.mru.retain(|id| *id != focus);
            self.mru.insert(0, focus);
        }
    }
    /// Resolves a stack step, including sheets not yet visited in a restored session.
    pub fn stack_neighbor(&self, reverse: bool) -> Option<u64> {
        let focus = self.focused;
        let mut seen = self
            .mru
            .iter()
            .copied()
            .filter(|id| Some(*id) != focus && self.sheets.contains(id));
        let mut rest = self
            .sheets
            .iter()
            .copied()
            .filter(|id| Some(*id) != focus && !self.mru.contains(id));
        if reverse {
            rest.next_back().or_else(|| seen.next_back())
        } else {
            seen.chain(rest).next()
        }
    }
    /// Adds a sheet and focuses it, optionally retaining the selection.
    pub fn add(&mut self, id: u64, additive: bool) {
        if !self.sheets.contains(&id) {
            self.sheets.push(id);
        }
        self.click(id, additive, false);
    }
    /// Removes a sheet, choosing a neighboring focus when needed.
    pub fn remove(&mut self, id: u64) {
        let at = self.sheets.iter().position(|s| *s == id).unwrap_or(0);
        self.sheets.retain(|s| *s != id);
        self.mru.retain(|s| *s != id);
        self.selected.remove(&id);
        if self.focused == Some(id) {
            self.focused = self.selected.first().copied().or_else(|| {
                self.sheets
                    .get(at.min(self.sheets.len().saturating_sub(1)))
                    .copied()
            });
        }
        if let Some(f) = self.focused {
            self.selected.insert(f);
        }
        if self.anchor == Some(id) {
            self.anchor = self.focused;
        }
        self.touch_focus();
    }
    /// Selected IDs in display order.
    pub fn visible(&self) -> Vec<u64> {
        self.sheets
            .iter()
            .copied()
            .filter(|s| self.selected.contains(s))
            .collect()
    }
}
/// Window-local workbench state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workbench {
    /// Valid tiled layout.
    pub layout: Layout,
    /// One group for each layout cell.
    pub groups: Vec<Group>,
    /// Focused group index.
    pub active: usize,
    /// Window-local limit used by automatic new panes before adding rows.
    #[serde(default = "default_max_columns")]
    pub max_columns: usize,
}
fn default_max_columns() -> usize {
    2
}
impl Default for Workbench {
    fn default() -> Self {
        Self {
            layout: Layout::default(),
            groups: vec![Group::default()],
            active: 0,
            max_columns: default_max_columns(),
        }
    }
}
impl Workbench {
    /// Atomically changes layout, moving sheets from removed groups to the last survivor.
    pub fn set_layout(&mut self, layout: Layout) -> Result<(), String> {
        layout.validate()?;
        let count = layout.cells.len();
        if self.groups.len() > count {
            let removed = self.groups.split_off(count);
            let target = &mut self.groups[count - 1];
            for group in removed {
                target.sheets.extend(group.sheets);
            }
            // A layout merge retains the receiving group's own active view.
            // It must not turn unrelated groups into a multi-selection.
            target.focused = target.focused.or_else(|| target.sheets.first().copied());
            target.selected.clear();
            if let Some(focus) = target.focused {
                target.selected.insert(focus);
            }
            target.anchor = target.focused;
        }
        self.groups.resize_with(count, Group::default);
        self.active = self.active.min(count - 1);
        self.layout = layout;
        Ok(())
    }
    /// Finds the owning group of a stable sheet.
    pub fn group_of(&self, id: u64) -> Option<usize> {
        self.groups.iter().position(|g| g.sheets.contains(&id))
    }
    /// Moves only the focused view, as documented for move_to_group.
    /// <https://docs.sublimetext.io/reference/commands.html#move-to-group>
    pub fn move_active(&mut self, target: usize) -> Result<(), String> {
        if target >= self.groups.len() {
            return Err("unknown group".into());
        }
        if target != self.active
            && let Some(id) = self.groups[self.active].focused
        {
            self.groups[self.active].remove(id);
            self.groups[target].add(id, false);
            self.active = target;
        }
        Ok(())
    }
    /// Moves all selected sheets in display order and focuses the destination.
    pub fn move_selected(&mut self, target: usize) -> Result<(), String> {
        if target >= self.groups.len() {
            return Err("unknown group".into());
        }
        if target == self.active {
            return Ok(());
        }
        let ids = self.groups[self.active].visible();
        if ids.is_empty() {
            return Ok(());
        }
        let focus = self.groups[self.active].focused;
        self.groups[target].selected.clear();
        for id in ids {
            self.groups[self.active].remove(id);
            self.groups[target].add(id, true);
        }
        self.groups[target].focused = focus;
        self.groups[target].anchor = focus;
        self.active = target;
        Ok(())
    }
    /// Creates an empty neighboring pane.
    pub fn new_pane(&mut self, vertical: bool) -> Result<(), String> {
        self.layout.split(self.active, vertical)?;
        self.groups.push(Group::default());
        Ok(())
    }
    /// Closes the focused pane and moves its sheets to a survivor.
    pub fn close_pane(&mut self) -> Result<(), String> {
        let index = self.active;
        // Route sheets to the cell that actually absorbs this cell's area.
        // Layout::remove chooses a full-edge neighbor or retessellates columns.
        let bounds = Rect {
            x: 0.,
            y: 0.,
            width: 1.,
            height: 1.,
        };
        let closed = self.layout.rect(index, bounds).ok_or("unknown group")?;
        self.layout.remove(index)?;
        let removed = self.groups.remove(index);
        self.active = (0..self.groups.len())
            .max_by(|a, b| {
                let overlap = |i| {
                    let r = self.layout.rect(i, bounds).expect("validated cell");
                    ((r.x + r.width).min(closed.x + closed.width) - r.x.max(closed.x)).max(0.)
                        * ((r.y + r.height).min(closed.y + closed.height) - r.y.max(closed.y))
                            .max(0.)
                };
                overlap(*a).total_cmp(&overlap(*b))
            })
            .expect("surviving group");
        let target = &mut self.groups[self.active];
        target.sheets.extend(removed.sheets);
        if removed.focused.is_some() {
            target.selected = removed.selected;
            target.focused = removed.focused;
            target.anchor = removed.anchor;
        }
        Ok(())
    }
    /// Repairs persisted focus/selection and sheet membership without dropping views.
    /// Unknown or duplicate references are discarded; orphan sheets join the active group.
    pub fn normalize(&mut self, ids: &BTreeSet<u64>) -> bool {
        if self.validate(ids).is_ok() {
            return false;
        }
        self.max_columns = self.max_columns.clamp(1, 64);
        if self.groups.is_empty() {
            self.groups.push(Group::default());
        }
        if self.groups.len() > 64 {
            let removed = self.groups.split_off(64);
            for group in removed {
                self.groups[63].sheets.extend(group.sheets);
            }
        }
        if self.layout.validate().is_err() || self.layout.cells.len() != self.groups.len() {
            self.layout = Layout::grid(self.groups.len(), 1);
        }
        self.active = self.active.min(self.groups.len() - 1);
        let mut seen = BTreeSet::new();
        for group in &mut self.groups {
            group
                .sheets
                .retain(|id| ids.contains(id) && seen.insert(*id));
        }
        self.groups[self.active]
            .sheets
            .extend(ids.difference(&seen).copied());
        for group in &mut self.groups {
            group.selected.retain(|id| group.sheets.contains(id));
            group.focused = group
                .focused
                .filter(|id| group.sheets.contains(id))
                .or_else(|| group.selected.first().copied())
                .or_else(|| group.sheets.first().copied());
            if let Some(focus) = group.focused {
                group.selected.insert(focus);
            }
            group.anchor = group
                .anchor
                .filter(|id| group.sheets.contains(id))
                .or(group.focused);
            if !group.tab_scroll.is_finite() {
                group.tab_scroll = 0.;
            }
        }
        true
    }
    /// Validates persisted sheet references and selection invariants.
    pub fn validate(&self, ids: &BTreeSet<u64>) -> Result<(), String> {
        self.layout.validate()?;
        if !(1..=64).contains(&self.max_columns) {
            return Err("invalid maximum columns".into());
        }
        if self.groups.len() != self.layout.cells.len() || self.active >= self.groups.len() {
            return Err("invalid group count/focus".into());
        }
        let mut seen = BTreeSet::new();
        for g in &self.groups {
            for id in &g.sheets {
                if !ids.contains(id) || !seen.insert(*id) {
                    return Err("unknown/duplicate sheet".into());
                }
            }
            if !g.selected.iter().all(|s| g.sheets.contains(s))
                || g.focused.is_some_and(|f| !g.selected.contains(&f))
                || g.anchor.is_some_and(|id| !g.sheets.contains(&id))
                || (!g.sheets.is_empty() && (g.focused.is_none() || g.selected.is_empty()))
                || !g.tab_scroll.is_finite()
            {
                return Err("invalid sheet selection".into());
            }
        }
        if seen != *ids {
            return Err("unassigned sheet".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layouts_cover_cells_and_drag_with_nonzero_origin() {
        let mut l = Layout::grid(2, 2);
        l.validate().unwrap();
        let bounds = Rect {
            x: 220.,
            y: 50.,
            width: 1000.,
            height: 600.,
        };
        assert_eq!(
            l.rect(3, bounds).unwrap(),
            Rect {
                x: 720.,
                y: 350.,
                width: 500.,
                height: 300.
            }
        );
        let s = l.splitter(Point { x: 720., y: 100. }, bounds, 4.).unwrap();
        assert!(l.drag(s, 0.7));
        assert_eq!(l.rect(0, bounds).unwrap().width, 700.);
        assert!(l.drag(s, 2.));
        l.validate().unwrap();
        assert_eq!(l.neighbor(0, Direction::Right), Some(1));
        l.cells.push(l.cells[0]);
        assert!(l.validate().is_err());
        l = Layout::grid(2, 1);
        l.cols[1] = f64::NAN;
        assert!(l.validate().is_err());
    }
    #[test]
    fn selections_range_toggle_movement_and_pane_merge() {
        let mut w = Workbench::default();
        for i in 1..=5 {
            w.groups[0].add(i, false);
        }
        w.groups[0].click(2, false, false);
        w.groups[0].click(4, false, true);
        assert_eq!(w.groups[0].visible(), vec![2, 3, 4]);
        w.groups[0].click(3, true, false);
        assert_eq!(w.groups[0].visible(), vec![2, 4]);
        w.new_pane(true).unwrap();
        w.move_selected(1).unwrap();
        assert_eq!(w.groups[0].sheets, vec![1, 3, 5]);
        assert_eq!(w.groups[1].sheets, vec![2, 4]);
        w.close_pane().unwrap();
        w.validate(&(1..=5).collect()).unwrap();
        w.new_pane(false).unwrap();
        w.set_layout(Layout::grid(1, 1)).unwrap();
        w.validate(&(1..=5).collect()).unwrap();
    }
}
