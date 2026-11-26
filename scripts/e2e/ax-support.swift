// Shared support for the GUI end-to-end harness (scripts/e2e).
//
// Everything here posts real HID events (CGEventPost to .cghidEventTap) and reads
// the public accessibility tree, so it exercises the same path a person's
// keyboard, mouse and input method use. It never needs to be inside Velocity.
import AppKit
import ApplicationServices
import Carbon.HIToolbox
import CoreGraphics
import Foundation

// MARK: - Reporting

final class Report: @unchecked Sendable {
    private(set) var failures = 0
    private(set) var passes = 0
    private var lines: [String] = []
    private let started = Date()
    let name: String

    init(_ name: String) {
        self.name = name
    }

    func check(_ ok: Bool, _ label: String, detail: String = "") {
        if ok {
            passes += 1
            emit("PASS", label, detail)
        } else {
            failures += 1
            emit("FAIL", label, detail)
        }
    }

    func info(_ label: String, detail: String = "") {
        emit("INFO", label, detail)
    }

    func note(_ label: String, detail: String = "") {
        emit("NOTE", label, detail)
    }

    private func emit(_ kind: String, _ label: String, _ detail: String) {
        let line = "\(kind) [\(name)] \(label)\(detail.isEmpty ? "" : ": \(detail)")"
        lines.append(line)
        print(line)
        fflush(stdout)
    }

    /// Summary line every scenario ends with; the workflow greps it.
    func finish() -> Never {
        let seconds = String(format: "%.1f", Date().timeIntervalSince(started))
        let verdict = failures == 0 ? "PASS" : "FAIL"
        let line = "SUMMARY [\(name)] \(verdict) \(passes) passed, \(failures) failed, \(seconds)s"
        print(line)
        fflush(stdout)
        if let path = ProcessInfo.processInfo.environment["E2E_SUMMARY"] {
            let text = (lines + [line]).joined(separator: "\n") + "\n"
            try? text.write(toFile: path, atomically: true, encoding: .utf8)
        }
        exit(failures == 0 ? 0 : 1)
    }
}

func fail(_ message: String) -> Never {
    print("ERROR \(message)")
    fflush(stdout)
    exit(2)
}

func log(_ message: String) {
    print("LOG \(message)")
    fflush(stdout)
}

// MARK: - Shell helpers

@discardableResult
func shell(_ command: String) -> (status: Int32, out: String) {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/zsh")
    process.arguments = ["-c", command]
    let pipe = Pipe()
    process.standardOutput = pipe
    process.standardError = pipe
    do {
        try process.run()
    } catch {
        return (-1, "spawn failed: \(error)")
    }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    return (process.terminationStatus, String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines))
}

/// Runs a command and returns its trimmed output, or nil when it failed.
func output(_ command: String) -> String? {
    let (status, text) = shell(command)
    return status == 0 ? text : nil
}

// MARK: - Accessibility

func axAttr(_ element: AXUIElement, _ name: String) -> AnyObject? {
    var value: AnyObject?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
    return value
}

func axString(_ element: AXUIElement, _ name: String) -> String? {
    guard let value = axAttr(element, name) else { return nil }
    if let text = value as? String { return text }
    if let number = value as? NSNumber { return number.stringValue }
    return nil
}

func axBool(_ element: AXUIElement, _ name: String) -> Bool? {
    (axAttr(element, name) as? NSNumber)?.boolValue
}

func axChildren(_ element: AXUIElement) -> [AXUIElement] {
    (axAttr(element, kAXChildrenAttribute) as? [AXUIElement]) ?? []
}

func axRange(_ element: AXUIElement, _ name: String) -> NSRange? {
    guard let value = axAttr(element, name), CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    var range = CFRange()
    guard AXValueGetValue(value as! AXValue, .cfRange, &range) else { return nil }
    return NSRange(location: range.location, length: range.length)
}

func axRect(_ element: AXUIElement) -> CGRect? {
    guard let value = axAttr(element, "AXFrame"), CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    var rect = CGRect.zero
    guard AXValueGetValue(value as! AXValue, .cgRect, &rect) else { return nil }
    return rect
}

func axRole(_ element: AXUIElement) -> String {
    axString(element, kAXRoleAttribute) ?? ""
}

/// Depth-first search for the first element whose role equals `role` and whose
/// label matches `label` exactly (or by prefix when `prefix` is set).
func axFind(_ root: AXUIElement, role: String? = nil, label: String? = nil, prefix: Bool = false, parent: AXUIElement? = nil, depth: Int = 0) -> AXUIElement? {
    guard depth < 40 else { return nil }
    for child in axChildren(root) {
        let r = axRole(child)
        if r == "AXMenuBar" { continue }
        if role == nil || r == role {
            let l = axString(child, kAXTitleAttribute) ?? axString(child, kAXDescriptionAttribute)
            if label == nil || l == label || (prefix && (l?.hasPrefix(label!) ?? false)) {
                return child
            }
            // AXTextField tab-rename field has a generic title; its description carries "Rename".
            if role == "AXTextField", let want = label,
               (axString(child, kAXDescriptionAttribute) ?? axString(child, kAXHelpAttribute))?.contains(want) == true {
                return child
            }
        }
        if let found = axFind(child, role: role, label: label, prefix: prefix, parent: child, depth: depth + 1) {
            return found
        }
    }
    return nil
}

func axFindAll(_ root: AXUIElement, role: String, depth: Int = 0) -> [AXUIElement] {
    guard depth < 40 else { return [] }
    var found: [AXUIElement] = []
    for child in axChildren(root) where axRole(child) != "AXMenuBar" {
        if axRole(child) == role { found.append(child) }
        found.append(contentsOf: axFindAll(child, role: role, depth: depth + 1))
    }
    return found
}

func axApp(_ pid: pid_t) -> AXUIElement {
    AXUIElementCreateApplication(pid)
}

func axWindows(_ pid: pid_t) -> [AXUIElement] {
    (axAttr(axApp(pid), kAXWindowsAttribute) as? [AXUIElement]) ?? []
}

func axWindowTitle(_ window: AXUIElement) -> String {
    axString(window, kAXTitleAttribute) ?? ""
}

/// Compact description of an element for logs and AX dumps.
func axDescribe(_ element: AXUIElement) -> String {
    var parts: [String] = ["role=\(axRole(element))"]
    for (key, attr) in [("sub", kAXSubroleAttribute), ("title", kAXTitleAttribute), ("desc", kAXDescriptionAttribute),
                        ("value", kAXValueAttribute), ("id", kAXIdentifierAttribute), ("help", kAXHelpAttribute)] {
        if let text = axString(element, attr) {
            var shown = text.replacingOccurrences(of: "\n", with: "⏎")
            if shown.count > 200 { shown = String(shown.prefix(200)) + "…(\(text.count))" }
            parts.append("\(key)=\(shown)")
        }
    }
    if let selected = axBool(element, kAXSelectedAttribute) { parts.append("sel=\(selected)") }
    if let focused = axBool(element, kAXFocusedAttribute), focused { parts.append("focus") }
    if let rect = axRect(element) { parts.append("frame=\(Int(rect.minX)),\(Int(rect.minY)) \(Int(rect.width))x\(Int(rect.height))") }
    return parts.joined(separator: " ")
}

func axDump(_ element: AXUIElement, depth: Int = 0, maxDepth: Int = 12, into lines: inout [String]) {
    lines.append(String(repeating: "  ", count: depth) + axDescribe(element))
    guard depth < maxDepth else { return }
    for child in axChildren(element) {
        if axRole(child) == "AXMenuBar" {
            lines.append(String(repeating: "  ", count: depth + 1) + "AXMenuBar (skipped)")
            continue
        }
        axDump(child, depth: depth + 1, maxDepth: maxDepth, into: &lines)
    }
}

// MARK: - Windows (CoreGraphics)

struct WindowInfo {
    let id: Int
    let pid: pid_t
    let owner: String
    let layer: Int
    let name: String
    let bounds: CGRect
}

func windowList(owner: String? = nil, layers: Set<Int> = [0]) -> [WindowInfo] {
    let raw = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]) ?? []
    return raw.compactMap { entry in
        guard let pid = entry[kCGWindowOwnerPID as String] as? pid_t,
              let layer = entry[kCGWindowLayer as String] as? Int,
              layers.contains(layer),
              let boundsDict = entry[kCGWindowBounds as String] as? [String: Any],
              let id = entry[kCGWindowNumber as String] as? Int else { return nil }
        let name = entry[kCGWindowOwnerName as String] as? String ?? ""
        if let owner, name != owner { return nil }
        let rect = CGRect(x: boundsDict["X"] as? Double ?? 0, y: boundsDict["Y"] as? Double ?? 0,
                          width: boundsDict["Width"] as? Double ?? 0, height: boundsDict["Height"] as? Double ?? 0)
        return WindowInfo(id: id, pid: pid, owner: name, layer: layer,
                          name: entry[kCGWindowName as String] as? String ?? "", bounds: rect)
    }
}

func onScreenWindows(_ pid: pid_t) -> [WindowInfo] {
    windowList().filter { $0.pid == pid && $0.bounds.width > 40 && $0.bounds.height > 40 }
}

// MARK: - Process helpers

func runningApplications(matching path: String) -> [NSRunningApplication] {
    let target = URL(fileURLWithPath: path).resolvingSymlinksInPath().standardizedFileURL.path
    return NSWorkspace.shared.runningApplications.filter { app in
        guard let exe = app.executableURL?.resolvingSymlinksInPath().standardizedFileURL.path else { return false }
        return exe == target
    }
}

func frontmostPid() -> pid_t {
    NSWorkspace.shared.frontmostApplication?.processIdentifier ?? 0
}

func frontmostName() -> String {
    NSWorkspace.shared.frontmostApplication?.localizedName ?? "none"
}

func activate(_ pid: pid_t) -> Bool {
    guard let app = NSRunningApplication(processIdentifier: pid) else { return false }
    let ok = app.activate(options: [.activateAllWindows])
    let element = axApp(pid)
    AXUIElementSetAttributeValue(element, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
    if let window = axWindows(pid).first {
        AXUIElementPerformAction(window, kAXRaiseAction as CFString)
        AXUIElementSetAttributeValue(window, kAXMainAttribute as CFString, kCFBooleanTrue)
    }
    return ok
}

func footprintBytes(_ pid: pid_t) -> UInt64? {
    var info = rusage_info_v4()
    let result = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V4, $0) }
    }
    return result == 0 ? info.ri_phys_footprint : nil
}

/// Velocity processes running `executable`; the GUI child is the one whose command
/// line carries the harness's --width flag (the CLI forwarder has none).
func topVelocityProcesses(executable: String) -> [(pid: pid_t, footprint: UInt64)] {
    let target = URL(fileURLWithPath: executable).resolvingSymlinksInPath().standardizedFileURL.path
    var pids: [pid_t] = []
    if let ps = output("ps -axo pid=,command=") {
        for line in ps.split(separator: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard let space = trimmed.firstIndex(of: " "), let pid = pid_t(trimmed[..<space]) else { continue }
            let command = String(trimmed[trimmed.index(after: space)...])
            let runsTarget = command == target || command.hasSuffix(" " + target) || command.hasPrefix(target + " ")
            if runsTarget && command.contains("--width") {
                pids.append(pid)
            }
        }
    }
    return pids.compactMap { pid in footprintBytes(pid).map { (pid, $0) } }
        .sorted { $0.1 > $1.1 }
}

// MARK: - Waiting

func sleep(seconds: Double) {
    Thread.sleep(forTimeInterval: seconds)
}

/// Polls `probe` until it returns a value or the deadline passes.
func waitUntil(_ timeout: Double, interval: Double = 0.1, _ probe: () -> Bool) -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    repeat {
        if probe() { return true }
        Thread.sleep(forTimeInterval: interval)
    } while Date() < deadline
    return false
}

// MARK: - HID events

private let modifierFlags: [String: CGEventFlags] = [
    "cmd": .maskCommand, "command": .maskCommand, "shift": .maskShift,
    "opt": .maskAlternate, "alt": .maskAlternate, "ctrl": .maskControl, "control": .maskControl,
]

func modifierFlags(_ spec: String) -> CGEventFlags {
    var flags = CGEventFlags()
    for part in spec.split(separator: "+") where !part.isEmpty {
        if let flag = modifierFlags[part.lowercased()] { flags.insert(flag) }
    }
    return flags
}

/// US ANSI virtual key codes (HID usage is the layout's business; the IME and the
/// layout both see these, which is exactly what a physical key sends).
let keyCodes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
    "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19,
    "3": 20, "4": 21, "6": 22, "5": 23, "9": 25, "7": 26, "8": 28, "0": 29, "o": 31,
    "u": 32, "i": 34, "p": 35, "return": 36, "enter": 36, "l": 37, "j": 38, "k": 40,
    "n": 45, "m": 46, "tab": 48, "space": 49, "`": 50, "delete": 51, "escape": 53,
    "esc": 53, "left": 123, "right": 124, "down": 125, "up": 126, "home": 115,
    "end": 119, "pageup": 116, "pagedown": 121, "forwarddelete": 117,
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97,
    "-": 27, "=": 24, "[": 33, "]": 30, ";": 41, "'": 39, ",": 43, ".": 47, "/": 44,
    "\\": 42, "keypad_enter": 76,
]
private let shiftedCharacters: [Character: String] = [
    "A": "a", "S": "s", "D": "d", "F": "f", "H": "h", "G": "g", "Z": "z", "X": "x", "C": "c", "V": "v",
    "B": "b", "Q": "q", "W": "w", "E": "e", "R": "r", "Y": "y", "T": "t", "O": "o", "U": "u", "I": "i",
    "P": "p", "L": "l", "J": "j", "K": "k", "N": "n", "M": "m", "!": "1", "@": "2", "#": "3", "$": "4",
    "%": "5", "^": "6", "&": "7", "*": "8", "(": "9", ")": "0", "_": "-", "+": "=", "{": "[", "}": "]",
    ":": ";", "\"": "'", "<": ",", ">": ".", "?": "/", "|": "\\", "~": "`",
]

func postKey(_ code: CGKeyCode, flags: CGEventFlags = [], hold: UInt32 = 12_000) {
    let source = CGEventSource(stateID: .hidSystemState)
    guard let down = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false) else { return }
    down.flags = flags
    up.flags = flags
    down.post(tap: .cghidEventTap)
    usleep(hold)
    up.post(tap: .cghidEventTap)
    usleep(20_000)
}

func postKey(_ name: String, flags: CGEventFlags = []) {
    let lower = name.lowercased()
    guard let code = keyCodes[lower] else { fail("unknown key \(name)") }
    postKey(code, flags: flags)
}

/// HID key events for the US layout: letters go through the active input source,
/// which is what the IME needs to see.
func typeKeys(_ text: String, hold: UInt32 = 10_000, gap: UInt32 = 30_000) {
    for character in text {
        let key: String
        var flags = CGEventFlags()
        if let shifted = shiftedCharacters[character] {
            key = shifted
            flags.insert(.maskShift)
        } else if character == " " {
            key = "space"
        } else {
            key = String(character)
        }
        guard let code = keyCodes[key] else { fail("no keycode for \(character.debugDescription)") }
        let source = CGEventSource(stateID: .hidSystemState)
        guard let down = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false) else { continue }
        down.flags = flags
        up.flags = flags
        down.post(tap: .cghidEventTap)
        usleep(hold)
        up.post(tap: .cghidEventTap)
        usleep(gap)
    }
}

/// Unicode key events: bypass the layout and the input method (used for fixtures
/// such as setting a document name).
func typeUnicode(_ text: String, gap: UInt32 = 20_000) {
    let source = CGEventSource(stateID: .hidSystemState)
    for character in text.utf16 {
        var unit = character
        guard let down = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: false) else { continue }
        down.keyboardSetUnicodeString(stringLength: 1, unicodeString: &unit)
        up.keyboardSetUnicodeString(stringLength: 1, unicodeString: &unit)
        down.post(tap: .cghidEventTap)
        usleep(8_000)
        up.post(tap: .cghidEventTap)
        usleep(gap)
    }
}

/// HID pointer events, top-left screen coordinates (the same space as AXFrame and
/// CGWindowBounds). A move precedes every press so hover state and the window under
/// the pointer are live, as with a hand on the mouse.
func mouseMove(to point: CGPoint) {
    let source = CGEventSource(stateID: .hidSystemState)
    CGEvent(mouseEventSource: source, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left)?
        .post(tap: .cghidEventTap)
    usleep(15_000)
}

func mouseEvent(_ type: CGEventType, _ point: CGPoint, clicks: Int = 1, flags: CGEventFlags = []) {
    let source = CGEventSource(stateID: .hidSystemState)
    guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: .left) else { return }
    event.setIntegerValueField(.mouseEventClickState, value: Int64(clicks))
    event.flags = flags
    event.post(tap: .cghidEventTap)
    usleep(25_000)
}

func click(at point: CGPoint, clicks: Int = 1, flags: CGEventFlags = []) {
    mouseMove(to: point)
    for clickState in 1...max(1, clicks) {
        mouseEvent(.leftMouseDown, point, clicks: clickState, flags: flags)
        mouseEvent(.leftMouseUp, point, clicks: clickState, flags: flags)
        usleep(clicks > 1 ? 40_000 : 80_000)
    }
}

func drag(from start: CGPoint, to end: CGPoint, steps: Int = 12, pause: Double = 0.05) {
    mouseMove(to: start)
    mouseEvent(.leftMouseDown, start)
    let dx = (end.x - start.x) / CGFloat(steps)
    let dy = (end.y - start.y) / CGFloat(steps)
    for step in 1...steps {
        let point = CGPoint(x: start.x + dx * CGFloat(step), y: start.y + dy * CGFloat(step))
        mouseEvent(.leftMouseDragged, point)
        Thread.sleep(forTimeInterval: pause)
    }
    // Hold still at the destination so a drop target can latch.
    Thread.sleep(forTimeInterval: 0.15)
    mouseEvent(.leftMouseUp, end)
    usleep(120_000)
}

func pointerLocation() -> CGPoint {
    CGEvent(source: nil)?.location ?? .zero
}

// MARK: - Input sources

func stringProperty(_ source: TISInputSource, _ key: String) -> String {
    guard let raw = TISGetInputSourceProperty(source, key as CFString) else { return "" }
    return String(describing: Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue())
}

func boolProperty(_ source: TISInputSource, _ key: String) -> Bool {
    (TISGetInputSourceProperty(source, key as CFString).map { Unmanaged<AnyObject>.fromOpaque($0).takeUnretainedValue() } as? NSNumber)?.boolValue ?? false
}

func inputSources(includeDisabled: Bool = true) -> [TISInputSource] {
    (TISCreateInputSourceList(nil, includeDisabled).takeRetainedValue() as? [TISInputSource]) ?? []
}

func currentInputSourceId() -> String {
    stringProperty(TISCopyCurrentKeyboardInputSource().takeRetainedValue(), kTISPropertyInputSourceID as String)
}

func findInputSource(_ id: String) -> TISInputSource? {
    let properties = [kTISPropertyInputSourceID as String: id] as CFDictionary
    let list = TISCreateInputSourceList(properties, true).takeRetainedValue() as? [TISInputSource]
    return list?.first
}

func enableInputSource(_ id: String) -> OSStatus {
    guard let source = findInputSource(id) else { return -1 }
    return TISEnableInputSource(source)
}

func selectInputSource(_ id: String) -> OSStatus {
    guard let source = findInputSource(id) else { return -1 }
    if !boolProperty(source, kTISPropertyInputSourceIsEnabled as String) {
        _ = TISEnableInputSource(source)
    }
    return TISSelectInputSource(source)
}

let pinyinIds = [
    "com.apple.inputmethod.SCIM.ITABC",
    "com.apple.inputmethod.SCIM",
    "com.apple.inputmethod.SCIM.SimplifiedPinyin",
]
let abcIds = [
    "com.apple.keylayout.ABC",
    "com.apple.keylayout.US",
]

/// Switches the system input source and waits for every client to see it.
@discardableResult
func useInputSource(_ ids: [String]) -> String {
    for id in ids where findInputSource(id) != nil {
        let status = selectInputSource(id)
        if status == 0 {
            _ = waitUntil(3) { currentInputSourceId() == id }
            return id
        }
        log("select \(id) failed status=\(status)")
    }
    return ""
}
