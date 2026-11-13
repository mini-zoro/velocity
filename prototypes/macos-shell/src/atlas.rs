//! Pure shelf packing. One-pixel transparent padding prevents texture bleed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Debug)]
pub struct Shelf {
    side: u32,
    x: u32,
    y: u32,
    row_height: u32,
}
impl Shelf {
    pub fn new(side: u32) -> Self {
        Self {
            side,
            x: 0,
            y: 0,
            row_height: 0,
        }
    }
    pub fn allocate(&mut self, width: u32, height: u32) -> Option<Slot> {
        let w = width.checked_add(2)?;
        let h = height.checked_add(2)?;
        if w > self.side || h > self.side {
            return None;
        }
        let wrap = self.x.checked_add(w).is_none_or(|end| end > self.side);
        let y = if wrap {
            self.y.checked_add(self.row_height)?
        } else {
            self.y
        };
        if y.checked_add(h)? > self.side {
            return None;
        }
        if wrap {
            self.x = 0;
            self.y = y;
            self.row_height = 0;
        }
        let slot = Slot {
            x: self.x + 1,
            y: self.y + 1,
            width,
            height,
        };
        self.x += w;
        self.row_height = self.row_height.max(h);
        Some(slot)
    }
}
/// Physical x origin and quarter-pixel phase; floor also works for negative scrolling.
pub fn subpixel_x(x: f32) -> (f32, u8) {
    let origin = x.floor();
    (origin, ((x - origin) * 4.0).floor().clamp(0.0, 3.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packed_slots_do_not_overlap_and_have_padding() {
        let mut p = Shelf::new(64);
        let mut slots: Vec<Slot> = Vec::new();
        for i in 0..300 {
            if let Some(a) = p.allocate(i % 9 + 1, i % 7 + 1) {
                assert!(a.x >= 1 && a.y >= 1 && a.x + a.width < 64 && a.y + a.height < 64);
                for b in &slots {
                    assert!(
                        a.x + a.width + 1 < b.x
                            || b.x + b.width + 1 < a.x
                            || a.y + a.height + 1 < b.y
                            || b.y + b.height + 1 < a.y
                    );
                }
                slots.push(a);
            }
        }
        assert!(slots.len() > 10);
    }
    #[test]
    fn failed_allocation_does_not_corrupt_packer() {
        let mut p = Shelf::new(10);
        assert!(p.allocate(u32::MAX, 1).is_none());
        assert!(p.allocate(9, 9).is_none());
        assert_eq!(
            p.allocate(8, 8),
            Some(Slot {
                x: 1,
                y: 1,
                width: 8,
                height: 8
            })
        );
        assert!(p.allocate(1, 1).is_none());
    }
    #[test]
    fn subpixel_phase_with_negative_x() {
        assert_eq!(subpixel_x(3.74), (3.0, 2));
        assert_eq!(subpixel_x(-0.1), (-1.0, 3));
        assert_eq!(subpixel_x(0.0), (0.0, 0));
    }
}
