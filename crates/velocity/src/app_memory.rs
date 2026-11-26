use super::App;
use std::collections::HashSet;
use vel_platform::MemoryCacheBytes;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Trim {
    Idle,
    Pressure,
    Critical,
}

impl App {
    pub(super) fn loaded_document_bytes(&self) -> u64 {
        let mut seen = HashSet::new();
        self.workspace
            .tabs
            .iter()
            .filter(|tab| tab.lazy.is_none() && seen.insert(tab.session_document))
            .map(|tab| {
                tab.editor.binary.as_ref().map_or_else(
                    || tab.editor.buffer.text().len_bytes() as u64,
                    |binary| binary.view.len_bytes(),
                )
            })
            .fold(0, u64::saturating_add)
    }

    pub(super) fn trim_memory(&mut self, level: Trim) -> MemoryCacheBytes {
        let pressure = level != Trim::Idle;
        let mut sizes = MemoryCacheBytes::default();
        if let Some(renderer) = &mut self.renderer {
            (sizes.atlas, sizes.instances) = renderer.trim_memory(pressure);
        }
        if let Some(fonts) = &mut self.fonts {
            // Shared: subsequent windows see zero, and the resolved cascade
            // stays warm so the first frame after a trim is still complete.
            sizes.fonts = fonts.trim_shaping() as u64;
        }

        self.prune_closed_sheet_views();
        let groups = &self.workspace.workbench.groups;
        // A parked view may still be displayed in another split. Even on
        // Critical, keep the view itself: it owns scroll and reflow anchors.
        for (&sheet, view) in &mut self.sheet_views {
            sizes.images += view
                .image_preview
                .image()
                .map_or(0, |p| p.rgba.len() as u64);
            if !groups.iter().any(|group| group.selected.contains(&sheet)) {
                view.image_preview.release_inactive();
                self.inactive_image_decodes.remove(&sheet);
            }
        }
        sizes.images += self
            .view
            .image_preview
            .image()
            .map_or(0, |p| p.rgba.len() as u64);
        sizes.images += self.workspace.themes.ui.borrow_mut().trim_caches() as u64;
        sizes.find = self.panels.trim_find_caches(&self.workspace, pressure) as u64;
        if pressure {
            vel_syntax::trim_regex_caches();
        }
        sizes
    }
}
