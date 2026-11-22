// Live-check driver: accessibility tree, menus, keyboard events, input sources, window shots.
import AppKit
import ApplicationServices
import Carbon
import CoreGraphics
import Foundation

let args = Array(CommandLine.arguments.dropFirst())
func die(_ s: String) -> Never { FileHandle.standardError.write((s + "\n").data(using: .utf8)!); exit(1) }
guard let cmd = args.first else { die("usage: ax <cmd> ...") }

func attr(_ e: AXUIElement, _ name: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success ? v : nil
}
func str(_ v: AnyObject?) -> String? {
    guard let v else { return nil }
    if let s = v as? String { return s }
    if let n = v as? NSNumber { return n.stringValue }
    if CFGetTypeID(v) == AXValueGetTypeID() {
        let a = v as! AXValue
        switch AXValueGetType(a) {
        case .cfRange: var r = CFRange(); AXValueGetValue(a, .cfRange, &r); return "{\(r.location),\(r.length)}"
        case .cgRect: var r = CGRect.zero; AXValueGetValue(a, .cgRect, &r); return "\(Int(r.minX)),\(Int(r.minY)) \(Int(r.width))x\(Int(r.height))"
        default: return "\(a)"
        }
    }
    if CFGetTypeID(v) == AXUIElementGetTypeID() { return "<element>" }
    return "\(v)"
}
func children(_ e: AXUIElement) -> [AXUIElement] { (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] }
func describe(_ e: AXUIElement) -> String {
    var parts: [String] = []
    for (k, n) in [("role", kAXRoleAttribute), ("sub", kAXSubroleAttribute), ("rdesc", kAXRoleDescriptionAttribute),
                   ("title", kAXTitleAttribute), ("label", kAXDescriptionAttribute), ("value", kAXValueAttribute),
                   ("help", kAXHelpAttribute), ("sel", kAXSelectedAttribute), ("focus", kAXFocusedAttribute),
                   ("enabled", kAXEnabledAttribute), ("seltext", kAXSelectedTextAttribute),
                   ("selrange", kAXSelectedTextRangeAttribute), ("line", kAXInsertionPointLineNumberAttribute),
                   ("nchars", kAXNumberOfCharactersAttribute), ("id", kAXIdentifierAttribute),
                   ("frame", "AXFrame")] {
        if let s = str(attr(e, n)) {
            var t = s.replacingOccurrences(of: "\n", with: "⏎")
            if t.count > 120 { t = String(t.prefix(120)) + "…(\(s.count))" }
            if k == "enabled" && t == "1" { continue }
            if (k == "sel" || k == "focus") && t == "0" { continue }
            parts.append("\(k)=\(t)")
        }
    }
    return parts.joined(separator: " ")
}
func dump(_ e: AXUIElement, _ depth: Int, _ max: Int, _ skipMenu: Bool) {
    let role = str(attr(e, kAXRoleAttribute)) ?? ""
    if skipMenu && role == "AXMenuBar" { print(String(repeating: "  ", count: depth) + "AXMenuBar (skipped)"); return }
    print(String(repeating: "  ", count: depth) + describe(e))
    if depth < max { for c in children(e) { dump(c, depth + 1, max, skipMenu) } }
}
func app(_ pidArg: String) -> (AXUIElement, pid_t) {
    guard let pid = pid_t(pidArg) else { die("bad pid") }
    return (AXUIElementCreateApplication(pid), pid)
}
func findMenu(_ appEl: AXUIElement, _ path: [String]) -> AXUIElement? {
    guard let bar = attr(appEl, kAXMenuBarAttribute) else { return nil }
    var cur = bar as! AXUIElement
    for (i, name) in path.enumerated() {
        var next: AXUIElement?
        var items = children(cur)
        // menu bar item -> AXMenu -> items
        if items.count == 1, str(attr(items[0], kAXRoleAttribute)) == "AXMenu" { items = children(items[0]) }
        for it in items where (str(attr(it, kAXTitleAttribute)) ?? "") == name { next = it; break }
        if next == nil {
            for it in items where (str(attr(it, kAXTitleAttribute)) ?? "").hasPrefix(name) { next = it; break }
        }
        guard let n = next else { FileHandle.standardError.write("menu item not found: \(name) at level \(i); have: \(items.map { str(attr($0, kAXTitleAttribute)) ?? "?" })\n".data(using: .utf8)!); return nil }
        cur = n
    }
    return cur
}
let mods: [String: CGEventFlags] = ["cmd": .maskCommand, "shift": .maskShift, "alt": .maskAlternate, "ctrl": .maskControl]
func flags(_ s: String) -> CGEventFlags {
    var f = CGEventFlags()
    for m in s.split(separator: "+") where !m.isEmpty { if let v = mods[String(m)] { f.insert(v) } }
    return f
}
func postKey(_ code: CGKeyCode, _ f: CGEventFlags, pid: pid_t?) {
    let src = CGEventSource(stateID: .hidSystemState)
    let d = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: true)!
    let u = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: false)!
    d.flags = f; u.flags = f
    if let pid { d.postToPid(pid); usleep(15000); u.postToPid(pid) } else { d.post(tap: .cghidEventTap); usleep(15000); u.post(tap: .cghidEventTap) }
    usleep(40000)
}

switch cmd {
case "dump":  // dump <pid> [depth] [menus]
    let (a, _) = app(args[1])
    dump(a, 0, args.count > 2 ? Int(args[2])! : 12, !(args.count > 3 && args[3] == "menus"))
case "focused":
    let (a, _) = app(args[1])
    if let f = attr(a, kAXFocusedUIElementAttribute) { print(describe(f as! AXUIElement)) } else { print("no focused element") }
case "string":  // string <pid> loc len : string for range of focused element
    let (a, _) = app(args[1])
    guard let f = attr(a, kAXFocusedUIElementAttribute) else { die("no focus") }
    var r = CFRange(location: Int(args[2])!, length: Int(args[3])!)
    let rv = AXValueCreate(.cfRange, &r)!
    var out: AnyObject?
    let err = AXUIElementCopyParameterizedAttributeValue(f as! AXUIElement, kAXStringForRangeParameterizedAttribute as CFString, rv, &out)
    print("err=\(err.rawValue) string=\(str(out) ?? "nil")")
case "menu":  // menu <pid> "Top>Item>Sub"
    let (a, _) = app(args[1])
    guard let item = findMenu(a, args[2].components(separatedBy: ">")) else { exit(2) }
    let e = AXUIElementPerformAction(item, kAXPressAction as CFString)
    print("press=\(e.rawValue)")
case "menus":  // menus <pid> [top-filter] : list items with shortcut attributes
    let (a, _) = app(args[1])
    guard let bar = attr(a, kAXMenuBarAttribute) else { die("no menubar") }
    func walk(_ e: AXUIElement, _ depth: Int) {
        for c in children(e) {
            let role = str(attr(c, kAXRoleAttribute)) ?? ""
            if role == "AXMenu" { walk(c, depth); continue }
            let t = str(attr(c, kAXTitleAttribute)) ?? ""
            let ch = str(attr(c, "AXMenuItemCmdChar")) ?? ""
            let vk = str(attr(c, "AXMenuItemCmdVirtualKey")) ?? ""
            let md = str(attr(c, "AXMenuItemCmdModifiers")) ?? ""
            let en = str(attr(c, kAXEnabledAttribute)) ?? ""
            let mark = str(attr(c, "AXMenuItemMarkChar")) ?? ""
            if depth == 0 && args.count > 2 && t != args[2] { continue }
            if !t.isEmpty || !ch.isEmpty {
                print(String(repeating: "  ", count: depth) + "\(t)" + (ch.isEmpty && vk.isEmpty ? "" : "  [char=\(ch) vk=\(vk) mods=\(md)]") + (en == "0" ? "  (disabled)" : "") + (mark.isEmpty ? "" : " mark=\(mark)"))
            }
            walk(c, depth + 1)
        }
    }
    walk(bar as! AXUIElement, 0)
case "press":  // press <pid> <identifier-or-label-substring> : AXPress the first matching element
    let (a, _) = app(args[1])
    var found: AXUIElement?
    func search(_ e: AXUIElement, _ d: Int) {
        if found != nil || d > 30 { return }
        let id = str(attr(e, kAXIdentifierAttribute)) ?? ""
        let label = str(attr(e, kAXDescriptionAttribute)) ?? ""
        let title = str(attr(e, kAXTitleAttribute)) ?? ""
        if id == args[2] || label == args[2] || title == args[2] { found = e; return }
        for c in children(e) where str(attr(c, kAXRoleAttribute)) != "AXMenuBar" { search(c, d + 1) }
    }
    search(a, 0)
    guard let f = found else { die("element not found") }
    print("press=\(AXUIElementPerformAction(f, kAXPressAction as CFString).rawValue) \(describe(f))")
case "key":  // key <pid|hid> <keycode> [mods]
    let pid = args[1] == "hid" ? nil : pid_t(args[1])
    postKey(CGKeyCode(args[2])!, flags(args.count > 3 ? args[3] : ""), pid: pid)
case "type":  // type <pid|hid> text  (unicode string events, bypass layout/IME)
    let pid = args[1] == "hid" ? nil : pid_t(args[1])
    let src = CGEventSource(stateID: .hidSystemState)
    for ch in args[2].utf16 {
        var c = ch
        let d = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: true)!
        let u = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: false)!
        d.keyboardSetUnicodeString(stringLength: 1, unicodeString: &c)
        u.keyboardSetUnicodeString(stringLength: 1, unicodeString: &c)
        if let pid { d.postToPid(pid); u.postToPid(pid) } else { d.post(tap: .cghidEventTap); u.post(tap: .cghidEventTap) }
        usleep(20000)
    }
case "activate":
    guard let pid = pid_t(args[1]), let r = NSRunningApplication(processIdentifier: pid) else { die("no app") }
    let ok = r.activate(options: [.activateAllWindows])
    // Cooperative activation can be refused; ask the app through accessibility as well.
    let a = AXUIElementCreateApplication(pid)
    AXUIElementSetAttributeValue(a, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
    if let wins = attr(a, kAXWindowsAttribute) as? [AXUIElement], let w = wins.first {
        AXUIElementPerformAction(w, kAXRaiseAction as CFString)
        AXUIElementSetAttributeValue(w, kAXMainAttribute as CFString, kCFBooleanTrue)
    }
    usleep(300_000)
    print("activate=\(ok) front=\(NSWorkspace.shared.frontmostApplication?.processIdentifier == pid)")
case "resize":  // resize <pid> x y w h : move/resize the first standard window (points, top-left origin)
    let (a, _) = app(args[1])
    guard let wins = attr(a, kAXWindowsAttribute) as? [AXUIElement], let w = wins.first else { die("no window") }
    var p = CGPoint(x: Double(args[2])!, y: Double(args[3])!)
    var sz = CGSize(width: Double(args[4])!, height: Double(args[5])!)
    let e1 = AXUIElementSetAttributeValue(w, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &p)!)
    let e2 = AXUIElementSetAttributeValue(w, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &sz)!)
    print("move=\(e1.rawValue) size=\(e2.rawValue)")
case "mouse":  // current pointer location
    let l = CGEvent(source: nil)!.location
    print("\(Int(l.x)),\(Int(l.y))")
case "frontpid":
    print(NSWorkspace.shared.frontmostApplication.map { "\($0.processIdentifier)" } ?? "0")
case "front":
    print(NSWorkspace.shared.frontmostApplication.map { "\($0.processIdentifier) \($0.localizedName ?? "")" } ?? "none")
case "inputs":  // list enabled + selectable keyboard input sources
    let all = TISCreateInputSourceList(nil, true).takeRetainedValue() as! [TISInputSource]
    func prop(_ s: TISInputSource, _ k: CFString) -> String {
        guard let p = TISGetInputSourceProperty(s, k) else { return "" }
        let o = Unmanaged<AnyObject>.fromOpaque(p).takeUnretainedValue()
        return "\(o)"
    }
    let cur = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
    print("current=\(prop(cur, kTISPropertyInputSourceID))")
    for s in all {
        let id = prop(s, kTISPropertyInputSourceID)
        if args.count > 1 && !id.localizedCaseInsensitiveContains(args[1]) { continue }
        print("\(id) enabled=\(prop(s, kTISPropertyInputSourceIsEnabled)) selectable=\(prop(s, kTISPropertyInputSourceIsSelectCapable)) cat=\(prop(s, kTISPropertyInputSourceCategory))")
    }
case "input":  // input enable|disable|select <id>
    let props = [kTISPropertyInputSourceID as String: args[2]] as CFDictionary
    let list = TISCreateInputSourceList(props, true).takeRetainedValue() as! [TISInputSource]
    guard let s = list.first else { die("no such input source \(args[2])") }
    let r: OSStatus
    switch args[1] {
    case "enable": r = TISEnableInputSource(s)
    case "disable": r = TISDisableInputSource(s)
    default: r = TISSelectInputSource(s)
    }
    print("status=\(r)")
case "windows":  // windows <owner> : list on-screen windows with ids
    let list = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]]) ?? []
    for w in list where (w[kCGWindowOwnerName as String] as? String) == args[1] {
        let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
        print("id=\(w[kCGWindowNumber as String]!) pid=\(w[kCGWindowOwnerPID as String]!) layer=\(w[kCGWindowLayer as String]!) name=\(w[kCGWindowName as String] ?? "") bounds=\(b["X"]!),\(b["Y"]!) \(b["Width"]!)x\(b["Height"]!)")
    }
case "click":  // click x y [mods] [count]
    let p = CGPoint(x: Double(args[1])!, y: Double(args[2])!)
    let f = flags(args.count > 3 ? args[3] : "")
    let n = args.count > 4 ? Int(args[4])! : 1
    let src = CGEventSource(stateID: .hidSystemState)
    for i in 1...n {
        let d = CGEvent(mouseEventSource: src, mouseType: .leftMouseDown, mouseCursorPosition: p, mouseButton: .left)!
        let u = CGEvent(mouseEventSource: src, mouseType: .leftMouseUp, mouseCursorPosition: p, mouseButton: .left)!
        d.setIntegerValueField(.mouseEventClickState, value: Int64(i)); u.setIntegerValueField(.mouseEventClickState, value: Int64(i))
        d.flags = f; u.flags = f
        d.post(tap: .cghidEventTap); usleep(30000); u.post(tap: .cghidEventTap); usleep(60000)
    }
case "rclick":  // rclick x y
    let p = CGPoint(x: Double(args[1])!, y: Double(args[2])!)
    let src = CGEventSource(stateID: .hidSystemState)
    CGEvent(mouseEventSource: src, mouseType: .rightMouseDown, mouseCursorPosition: p, mouseButton: .right)!.post(tap: .cghidEventTap)
    usleep(30000)
    CGEvent(mouseEventSource: src, mouseType: .rightMouseUp, mouseCursorPosition: p, mouseButton: .right)!.post(tap: .cghidEventTap)
default:
    die("unknown command \(cmd)")
}
