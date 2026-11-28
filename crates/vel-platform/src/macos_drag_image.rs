//! One non-activating, click-through panel on the AppKit thread.
use crate::{DragImage, Point};
use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::AnyObject,
};
use objc2_app_kit::*;
use objc2_core_foundation::CFData;
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CALayer, CATransaction};
use std::cell::RefCell;

define_class!(
    // SAFETY: No extra AppKit state is required by these overrides.
    #[unsafe(super = NSPanel)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct DragPanel;
    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for DragPanel {}
    impl DragPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool { false }
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { false }
        #[unsafe(method(constrainFrameRect:toScreen:))]
        fn unconstrained_frame(&self, frame: NSRect, _screen: Option<&NSScreen>) -> NSRect {
            frame
        }
    }
);

struct Popup {
    panel: Retained<DragPanel>,
    layer: Retained<CALayer>,
    grab: Point,
    loaded: bool,
    visible: bool,
}

thread_local! {
    static POPUP: RefCell<Option<Popup>> = const { RefCell::new(None) };
}

impl Popup {
    fn new(mtm: MainThreadMarker) -> Self {
        let bounds = NSRect::new(NSPoint::new(0., 0.), NSSize::new(1., 1.));
        let panel = DragPanel::alloc(mtm).set_ivars(());
        // SAFETY: AppKit's designated initializer, called with its exact signature.
        let panel: Retained<DragPanel> = unsafe {
            msg_send![super(panel), initWithContentRect: bounds,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered, defer: false]
        };
        // SAFETY: Popup retains the panel until application teardown.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setIgnoresMouseEvents(true);
        panel.setExcludedFromWindowsMenu(true);
        panel.setHidesOnDeactivate(true);
        panel.setLevel(NSFloatingWindowLevel);
        panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Transient
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        let view = NSView::initWithFrame(NSView::alloc(mtm), bounds);
        let layer = CALayer::new();
        view.setLayer(Some(&layer));
        view.setWantsLayer(true);
        panel.setContentView(Some(&view));
        Self {
            panel,
            layer,
            grab: Point::default(),
            loaded: false,
            visible: false,
        }
    }

    fn follow(&mut self, point: Point) {
        // The AppKit screen origin is bottom-left; grab uses top-left image coordinates.
        self.panel
            .setFrameTopLeftPoint(NSPoint::new(point.x - self.grab.x, point.y + self.grab.y));
        if !self.visible {
            self.panel.orderFrontRegardless();
            self.panel.invalidateShadow();
            self.visible = true;
        }
    }
}

/// Uploads the image while the panel stays hidden; the next `follow` only moves
/// and orders it front, so the first visible frame does no image work.
pub(super) fn load(mtm: MainThreadMarker, image: &DragImage, grab: Point) {
    let (width, height) = (image.width as usize, image.height as usize);
    if width.checked_mul(height).and_then(|n| n.checked_mul(4)) != Some(image.rgba.len())
        || width == 0
        || height == 0
        || !image.scale.is_finite()
        || image.scale <= 0.
    {
        return;
    }
    let data = CFData::from_bytes(&image.rgba);
    let Some(provider) = CGDataProvider::with_cf_data(Some(&data)) else {
        return;
    };
    // SAFETY: This is Core Graphics' immutable sRGB color-space name.
    let Some(space) = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })) else {
        return;
    };
    let bitmap =
        CGBitmapInfo(CGImageByteOrderInfo::Order32Big.0 | CGImageAlphaInfo::PremultipliedLast.0);
    // SAFETY: The provider retains the verified RGBA storage; no decode array is needed.
    let Some(native) = (unsafe {
        CGImage::new(
            width,
            height,
            8,
            32,
            width * 4,
            Some(&space),
            bitmap,
            Some(&provider),
            std::ptr::null(),
            true,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }) else {
        return;
    };
    POPUP.with(|slot| {
        let mut slot = slot.borrow_mut();
        let popup = slot.get_or_insert_with(|| Popup::new(mtm));
        if popup.visible {
            popup.panel.orderOut(None);
            popup.visible = false;
        }
        let size = NSSize::new(width as f64 / image.scale, height as f64 / image.scale);
        popup.panel.setContentSize(size);
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        popup
            .layer
            .setFrame(NSRect::new(NSPoint::new(0., 0.), size));
        popup.layer.setContentsScale(image.scale);
        // SAFETY: CALayer.contents accepts and retains a bridged CGImageRef.
        unsafe {
            let contents = &*std::ptr::from_ref(&*native).cast::<AnyObject>();
            popup.layer.setContents(Some(contents));
        }
        CATransaction::commit();
        popup.grab = grab;
        popup.loaded = true;
    });
}

pub(super) fn follow(point: Point) {
    POPUP.with(|slot| {
        if let Some(popup) = slot.borrow_mut().as_mut().filter(|popup| popup.loaded) {
            popup.follow(point);
        }
    });
}

pub(super) fn hide(clear: bool) {
    POPUP.with(|slot| {
        if let Some(popup) = slot.borrow_mut().as_mut().filter(|popup| popup.loaded) {
            if popup.visible {
                popup.panel.orderOut(None);
                popup.visible = false;
            }
            if clear {
                CATransaction::begin();
                CATransaction::setDisableActions(true);
                // SAFETY: Nil clears the layer's retained contents.
                unsafe { popup.layer.setContents(None) };
                CATransaction::commit();
                popup.loaded = false;
            }
        }
    });
}

pub(super) fn window_at(mtm: MainThreadMarker, point: Point) -> Option<usize> {
    let point = NSPoint::new(point.x, point.y);
    let number = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(point, 0, mtm);
    let number = POPUP.with(|slot| {
        if slot
            .borrow()
            .as_ref()
            .is_some_and(|popup| popup.panel.windowNumber() == number && popup.loaded)
        {
            NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(point, number, mtm)
        } else {
            number
        }
    });
    (number > 0).then_some(number as usize)
}
