//! Public NSAccessibility getters/actions on virtual elements. No native objects in the model.
use super::*;
use crate::accessibility::{Id, Node, Preferences, Request, Tree};
use objc2_foundation::{NSDictionary, NSNumber, NSValue};

pub(super) fn preferences() -> Preferences {
    let workspace = NSWorkspace::sharedWorkspace();
    Preferences {
        reduce_motion: workspace.accessibilityDisplayShouldReduceMotion(),
        increase_contrast: workspace.accessibilityDisplayShouldIncreaseContrast(),
    }
}
fn tree(host: &Host) -> Rc<Tree> {
    host.ax_queried.set(Some(Instant::now()));
    snapshot(host)
}
fn snapshot(host: &Host) -> Rc<Tree> {
    host.ax_cache.get(|| {
        host.app
            .try_borrow()
            .ok()
            .and_then(|a| a.as_ref().map(|a| a.accessibility_tree()))
    })
}
fn object(host: &Rc<Host>, id: Id) -> Retained<AnyObject> {
    let mut elements = host.ax_elements.borrow_mut();
    let element = elements
        .entry(id)
        .or_insert_with(|| {
            let element = AxElement::alloc(host.mtm).set_ivars(ElementIvars {
                host: Rc::downgrade(host),
                id,
            });
            // SAFETY: the NSObject initializer runs after every Rust ivar is installed.
            unsafe { msg_send![super(element), init] }
        })
        .clone();
    // SAFETY: every Objective-C object is an AnyObject; the retained reference transfers unchanged.
    unsafe { Retained::cast_unchecked(element) }
}
pub(super) fn children(host: &Rc<Host>, parent: Option<Id>) -> Retained<NSArray<AnyObject>> {
    let tree = tree(host);
    host.ax_elements
        .borrow_mut()
        .retain(|id, _| tree.node(*id).is_some());
    let children = tree
        .nodes
        .iter()
        .filter(|n| n.parent == parent)
        .map(|n| object(host, n.id))
        .collect::<Vec<_>>();
    NSArray::from_retained_slice(&children)
}
pub(super) fn focused(host: &Rc<Host>) -> Option<Retained<AnyObject>> {
    tree(host).focused().map(|id| object(host, id))
}
pub(super) fn hit(host: &Rc<Host>, point: NSPoint) -> Option<Retained<AnyObject>> {
    let view = host.view.get()?;
    let window = host.window.get()?;
    let p = view.convertPoint_fromView(window.convertPointFromScreen(point), None);
    tree(host)
        .hit(Point { x: p.x, y: p.y })
        .map(|id| object(host, id))
}
fn screen_rect(host: &Host, rect: Rect) -> NSRect {
    let rect = NSRect::new(
        NSPoint::new(rect.x, rect.y),
        NSSize::new(rect.width, rect.height),
    );
    let Some(view) = host.view.get() else {
        return rect;
    };
    let window_rect = view.convertRect_toView(rect, None);
    host.window
        .get()
        .map_or(window_rect, |w| w.convertRectToScreen(window_rect))
}
/// Post notifications only to queried accessibility clients. Ordinary editing keeps O(1) snapshots.
pub(super) fn notify(host: &Rc<Host>, previous: &TextInputState, current: &TextInputState) {
    host.ax_cache.invalidate();
    if !host
        .ax_queried
        .get()
        .is_some_and(|at| at.elapsed() < Duration::from_secs(5))
    {
        host.ax_queried.set(None);
        return;
    }
    let current_tree = snapshot(host);
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for node in &current_tree.nodes {
        node.id.hash(&mut hash);
        node.parent.hash(&mut hash);
        node.selected.hash(&mut hash);
        node.checked.hash(&mut hash);
        node.role.native().hash(&mut hash);
        if node.role != crate::accessibility::Role::Status {
            node.label.hash(&mut hash);
        }
        for coordinate in [
            node.frame.x,
            node.frame.y,
            node.frame.width,
            node.frame.height,
        ] {
            coordinate.to_bits().hash(&mut hash);
        }
    }
    let revision = hash.finish();
    let layout_changed = host.ax_layout.replace(revision) != revision;
    let focused = current_tree.focused();
    let offset = focused.and_then(|id| {
        current_tree
            .node(id)?
            .text
            .as_ref()
            .map(|model| (id, model.document_offset))
    });
    let window_moved = host.ax_window_offset.replace(offset) != offset;
    let announcements = host.ax_announcements.borrow_mut().changes(&current_tree);
    if let Some(id) = focused {
        let element = object(host, id);
        // SAFETY: each constant is an immutable AppKit AX notification name; elements are retained.
        unsafe {
            if host.ax_focus.replace(focused) != focused {
                NSAccessibilityPostNotification(
                    &element,
                    NSAccessibilityFocusedUIElementChangedNotification,
                );
            }
            if window_moved
                || previous.selected != current.selected
                || previous.text != current.text
            {
                NSAccessibilityPostNotification(
                    &element,
                    NSAccessibilitySelectedTextChangedNotification,
                );
            }
            if window_moved || previous.text != current.text {
                NSAccessibilityPostNotification(&element, NSAccessibilityValueChangedNotification);
            }
            if layout_changed {
                NSAccessibilityPostNotification(&element, NSAccessibilityLayoutChangedNotification);
                for node in &current_tree.nodes {
                    if matches!(
                        node.role,
                        crate::accessibility::Role::List
                            | crate::accessibility::Role::Outline
                            | crate::accessibility::Role::TabGroup
                    ) {
                        let container = object(host, node.id);
                        NSAccessibilityPostNotification(
                            &container,
                            NSAccessibilitySelectedChildrenChangedNotification,
                        );
                        if node.role != crate::accessibility::Role::TabGroup {
                            NSAccessibilityPostNotification(
                                &container,
                                NSAccessibilitySelectedRowsChangedNotification,
                            );
                        }
                    }
                }
            }
        }
    }
    if let Some(view) = host.view.get() {
        for message in announcements {
            // https://developer.apple.com/documentation/appkit/nsaccessibility-swift.struct/notification/announcementrequested
            // SAFETY: notification constants and keys are immutable AppKit exports.
            unsafe {
                let text = NSString::from_str(&message);
                let priority = NSNumber::new_isize(NSAccessibilityPriorityLevel::Medium.0);
                let info = NSDictionary::from_slices(
                    &[NSAccessibilityAnnouncementKey, NSAccessibilityPriorityKey],
                    &[text.as_ref() as &AnyObject, priority.as_ref() as &AnyObject],
                );
                NSAccessibilityPostNotificationWithUserInfo(
                    view,
                    NSAccessibilityAnnouncementRequestedNotification,
                    Some(&info),
                );
            }
        }
    }
}
#[derive(Debug)]
pub(super) struct ElementIvars {
    host: Weak<Host>,
    id: Id,
}
define_class!(
    #[unsafe(super(NSAccessibilityElement))]
    #[name = "VelocityAccessibilityElement"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ElementIvars]
    pub(super) struct AxElement;
    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for AxElement {}
    impl AxElement {
        #[unsafe(method(isAccessibilityElement))] fn is_element(&self) -> bool { self.node().is_some() }
        #[unsafe(method_id(accessibilityRole))] fn role(&self) -> Option<Retained<NSString>> { self.node().map(|n| NSString::from_str(n.role.native()))  }
        #[unsafe(method_id(accessibilityRoleDescription))] fn role_description(&self) -> Option<Retained<NSString>> { self.node().filter(|n| n.role == crate::accessibility::Role::Tab).map(|_| NSString::from_str("tab"))  }
        #[unsafe(method_id(accessibilityIdentifier))] fn identifier(&self) -> Retained<NSString> { NSString::from_str(&format!("velocity-{}",self.ivars().id))  }
        #[unsafe(method_id(accessibilityLabel))] fn label(&self) -> Option<Retained<NSString>> { self.node().map(|n| NSString::from_str(&n.label))  }
        #[unsafe(method_id(accessibilityHelp))] fn help(&self) -> Option<Retained<NSString>> { self.node().map(|n| NSString::from_str(&n.help))  }
        #[unsafe(method(accessibilityFrame))] fn frame(&self) -> NSRect { self.ivars().host.upgrade().map_or(NSRect::ZERO, |h| screen_rect(&h,self.node().map_or(Rect::default(),|n| n.frame))) }
        #[unsafe(method_id(accessibilityParent))] fn parent(&self) -> Option<Retained<AnyObject>> { self.parent_object() }
        #[unsafe(method_id(accessibilityWindow))] fn window(&self) -> Option<Retained<AnyObject>> {
            // SAFETY: NSWindow is an Objective-C object; cloning retains it.
            self.window_object()
         }
        #[unsafe(method_id(accessibilityTopLevelUIElement))] fn top_level(&self) -> Option<Retained<AnyObject>> { self.window_object()  }
        #[unsafe(method_id(accessibilityChildren))] fn children(&self) -> Option<Retained<NSArray<AnyObject>>> { self.ivars().host.upgrade().map(|h| children(&h,Some(self.ivars().id)))  }
        #[unsafe(method_id(accessibilityChildrenInNavigationOrder))] fn navigation(&self) -> Option<Retained<NSArray<AnyObject>>> { self.child_objects()  }
        #[unsafe(method_id(accessibilityRows))] fn rows(&self) -> Option<Retained<NSArray<AnyObject>>> { self.child_objects()  }
        #[unsafe(method_id(accessibilitySelectedChildren))] fn selected_children(&self) -> Option<Retained<NSArray<AnyObject>>> { self.selected_row_objects() }
        #[unsafe(method_id(accessibilitySelectedRows))] fn selected_rows(&self) -> Option<Retained<NSArray<AnyObject>>> { self.selected_row_objects() }
        #[unsafe(method(isAccessibilityEnabled))] fn enabled(&self) -> bool { self.node().is_some_and(|n| n.enabled) }
        #[unsafe(method(isAccessibilitySelected))] fn selected(&self) -> bool { self.node().is_some_and(|n| n.selected) }
        #[unsafe(method(setAccessibilitySelected:))] fn select(&self, selected: bool) { if selected { self.perform(Request::Press); } }
        #[unsafe(method(isAccessibilityFocused))] fn focused(&self) -> bool { self.node().is_some_and(|n| n.focused) }
        #[unsafe(method(setAccessibilityFocused:))] fn focus(&self, focus: bool) { if focus { self.perform(Request::Focus); } }
        #[unsafe(method(isAccessibilityExpanded))] fn expanded(&self) -> bool { self.node().is_some_and(|n| n.expanded==Some(true)) }
        #[unsafe(method(setAccessibilityExpanded:))] fn expand(&self, expanded: bool) { if self.node().is_some_and(|n| n.expanded.is_some_and(|v| v!=expanded)) { self.perform(Request::Press); } }
        #[unsafe(method(accessibilityDisclosureLevel))] fn level(&self) -> isize { self.node().map_or(0,|n| n.level as isize) }
        #[unsafe(method(accessibilityPerformPress))] fn press(&self) -> bool { self.perform(Request::Press) }
        #[unsafe(method_id(accessibilityValue))] fn value(&self) -> Option<Retained<AnyObject>> { self.value_object() }
        #[unsafe(method(setAccessibilityValue:))] fn set_value(&self, value: &AnyObject) { if let Some(text)=value.downcast_ref::<NSString>() { self.perform(Request::SetValue(text.to_string())); } }
        #[unsafe(method_id(accessibilitySelectedText))] fn selected_text(&self) -> Option<Retained<NSString>> { self.node().and_then(|n| n.text).and_then(|t| t.string(t.selected)).map(|s| NSString::from_str(&s))  }
        #[unsafe(method(accessibilitySelectedTextRange))] fn selected_range(&self) -> NSRange { to_range(self.node().and_then(|n| n.text.map(|t|t.selected))) }
        #[unsafe(method_id(accessibilitySelectedTextRanges))] fn selected_ranges(&self) -> Retained<NSArray<NSValue>> {
            self.node().and_then(|n|n.text).map_or_else(|| NSArray::from_slice(&[]), |t| NSArray::from_retained_slice(&[{
                // SAFETY: NSRange is a valid value type; the selected range is scalar-aligned.
                unsafe { NSValue::valueWithRange(to_range(Some(t.selected))) }
            }]))
         }
        #[unsafe(method(setAccessibilitySelectedTextRange:))] fn set_selected_range(&self, range: NSRange) { if let Some(range)=from_range(range) { self.perform(Request::Select(range)); } }
        #[unsafe(method(setAccessibilitySelectedTextRanges:))] fn set_selected_ranges(&self, ranges: &NSArray<NSValue>) {
            if let Some(range)=ranges.firstObject() && let Some(range)=value_range(&range) { self.perform(Request::Select(range)); }
        }
        #[unsafe(method(accessibilityNumberOfCharacters))] fn count(&self) -> isize { self.node().and_then(|n|n.text).map_or(0,|t|t.text.len_utf16() as isize) }
        #[unsafe(method(accessibilityVisibleCharacterRange))] fn visible_range(&self) -> NSRange { to_range(self.node().and_then(|n|n.text.map(|t|t.visible))) }
        #[unsafe(method(accessibilityInsertionPointLineNumber))] fn insertion_line(&self) -> isize { self.node().and_then(|n|n.text).map_or(0,|t|t.insertion_line as isize) }
        #[unsafe(method_id(accessibilityStringForRange:))] fn string(&self, range: NSRange) -> Option<Retained<NSString>> { from_range(range).and_then(|range|self.node().and_then(|n|n.text).and_then(|t|t.string(range))).map(|s|NSString::from_str(&s))  }
        #[unsafe(method(accessibilityLineForIndex:))] fn line_for_index(&self,index: isize) -> isize { if index<0 { return -1; } self.node().and_then(|n|n.text?.line(index as usize)).map_or(-1,|i|i as isize) }
        #[unsafe(method(accessibilityRangeForLine:))] fn range_for_line(&self,line: isize) -> NSRange { to_range(if line<0 { None } else { self.node().and_then(|n|n.text?.range_for_line(line as usize)) }) }
        #[unsafe(method(accessibilityFrameForRange:))] fn range_frame(&self,range: NSRange) -> NSRect {
            let Some(range)=from_range(range) else{return NSRect::ZERO;};
            let Some(h)=self.ivars().host.upgrade() else { return NSRect::ZERO; };
            let Some(model)=tree(&h).node(self.ivars().id).and_then(|n| n.text.clone()) else { return NSRect::ZERO; };
            let rect=h.app.try_borrow().ok().and_then(|a|a.as_ref().map(|a| a.accessibility_range_rect(self.ivars().id,model.document_range(range)))).unwrap_or_default();
            screen_rect(&h,rect)
        }
        #[unsafe(method(isAccessibilitySelectorAllowed:))] fn allowed(&self,selector: Sel) -> bool {
            let name=selector.name(); let name=name.to_string_lossy();
            if name.starts_with("setAccessibilityValue") { self.node().is_some_and(|n|n.editable) }
            else if name.starts_with("setAccessibilitySelectedText") { self.node().is_some_and(|n|n.text.is_some()) }
            else if name=="accessibilityPerformPress" { self.node().is_some_and(|n|n.enabled && n.activation.is_some()) }
            else { true }

        }
    }
);
impl AxElement {
    fn value_object(&self) -> Option<Retained<AnyObject>> {
        let node = self.node()?;
        if node.role == crate::accessibility::Role::Tab || node.checked.is_some() {
            // SAFETY: NSNumber inherits AnyObject; ownership is transferred without changing retain count.
            Some(unsafe {
                Retained::cast_unchecked(NSNumber::new_bool(node.checked.unwrap_or(node.selected)))
            })
        } else {
            let value = node.text.map_or(node.label, |text| text.value());
            // SAFETY: NSString inherits AnyObject; ownership is transferred unchanged.
            Some(unsafe { Retained::cast_unchecked(NSString::from_str(&value)) })
        }
    }
    fn parent_object(&self) -> Option<Retained<AnyObject>> {
        let host = self.ivars().host.upgrade()?;
        if let Some(id) = self.node()?.parent {
            Some(object(&host, id))
        } else {
            host.view.get().map(|view| {
                // SAFETY: EditorView inherits AnyObject and cloning retains it.
                unsafe { Retained::cast_unchecked(view.clone()) }
            })
        }
    }
    fn selected_row_objects(&self) -> Option<Retained<NSArray<AnyObject>>> {
        let host = self.ivars().host.upgrade()?;
        let tree = tree(&host);
        let rows = tree
            .nodes
            .iter()
            .filter(|n| n.parent == Some(self.ivars().id) && n.selected)
            .map(|n| object(&host, n.id))
            .collect::<Vec<_>>();
        Some(NSArray::from_retained_slice(&rows))
    }
    fn window_object(&self) -> Option<Retained<AnyObject>> {
        self.ivars().host.upgrade().and_then(|h| {
            h.window.get().map(|w| {
                // SAFETY: NSWindow inherits AnyObject; cloning retains it.
                unsafe { Retained::cast_unchecked(w.clone()) }
            })
        })
    }
    fn child_objects(&self) -> Option<Retained<NSArray<AnyObject>>> {
        self.ivars()
            .host
            .upgrade()
            .map(|h| children(&h, Some(self.ivars().id)))
    }
    fn node(&self) -> Option<Node> {
        self.ivars()
            .host
            .upgrade()
            .and_then(|h| tree(&h).node(self.ivars().id).cloned())
    }
    fn perform(&self, request: Request) -> bool {
        let Some(host) = self.ivars().host.upgrade() else {
            return false;
        };
        if self.node().is_none_or(|n| !n.enabled) {
            return false;
        }
        host.dispatch(Event::Accessibility(self.ivars().id, request));
        host.flush();
        true
    }
}

fn value_range(value: &NSValue) -> Option<Utf16Range> {
    use objc2::Encode;
    // SAFETY: NSValue returns a non-null NUL-terminated type encoding valid for its lifetime.
    let encoding = unsafe { std::ffi::CStr::from_ptr(value.objCType().as_ptr()) };
    if encoding.to_bytes() != NSRange::ENCODING.to_string().as_bytes() {
        return None;
    }
    // SAFETY: objCType was checked to be precisely an NSRange encoding.
    from_range(unsafe { value.rangeValue() })
}
