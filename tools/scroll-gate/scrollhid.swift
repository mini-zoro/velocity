// Native pixel-scroll source. No application internals or timer-driven render calls.
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import QuartzCore

func abort(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(2)
}
func emit(_ record: [String: Any]) {
    do {
        let data = try JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([10]))
    } catch { abort("JSON: \(error)") }
}
let args = Array(CommandLine.arguments.dropFirst())
if args.count == 2 && args[0] == "--quit-pid" {
    guard let pid = Int32(args[1]), let app = NSRunningApplication(processIdentifier: pid), app.terminate() else {
        abort("Cannot request application termination")
    }
    exit(0)
}
if args == ["--help"] {
    print("scrollhid --pid PID [--duration 10 --rate 144 --delta 8 --x X --y Y --momentum]")
    exit(0)
}
var values: [String: String] = [:]
var momentum = false
var index = 0
while index < args.count {
    let key = args[index]
    if key == "--momentum" { momentum = true; index += 1; continue }
    guard ["--pid", "--duration", "--rate", "--delta", "--x", "--y"].contains(key),
          index + 1 < args.count, values[key] == nil else { abort("Invalid argument: \(key)") }
    values[key] = args[index + 1]
    index += 2
}
guard let pidText = values["--pid"], let pid = Int32(pidText), pid > 0,
      let app = NSRunningApplication(processIdentifier: pid) else { abort("--pid must name a running app") }
func number(_ key: String, _ fallback: Double) -> Double {
    guard let raw = values[key] else { return fallback }
    guard let value = Double(raw), value.isFinite else { abort("Invalid \(key)") }
    return value
}
let duration = number("--duration", 10)
let rate = number("--rate", 144)
let delta = number("--delta", 8)
guard duration >= 5, duration <= 600, rate >= 1, rate <= 1000, delta >= 1, delta <= 100 else {
    abort("duration must be 5..600 s; rate 1..1000 Hz; delta 1..100 pixels")
}
guard CGPreflightPostEventAccess(), AXIsProcessTrusted() else {
    abort("Grant Accessibility to scrollhid (and the invoking terminal); enable Input Monitoring if macOS requires it")
}
func windows() -> [[String: Any]] {
    guard let info = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]] else { abort("Cannot inspect on-screen windows") }
    return info
}
func rect(_ window: [String: Any]) -> CGRect? {
    guard let bounds = window[kCGWindowBounds as String] as? [String: Any] else { return nil }
    return CGRect(dictionaryRepresentation: bounds as CFDictionary)
}
func owner(_ window: [String: Any]) -> Int32? {
    (window[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value
}
app.activate(options: [.activateAllWindows])
let activationDeadline = CACurrentMediaTime() + 3
while NSWorkspace.shared.frontmostApplication?.processIdentifier != pid && CACurrentMediaTime() < activationDeadline {
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
}
guard let target = windows().first(where: {
    owner($0) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 && (rect($0)?.height ?? 0) > 100
}), let bounds = rect(target) else { abort("No normal visible window belongs to pid \(pid)") }
let point = CGPoint(x: number("--x", bounds.minX + bounds.width * 0.45),
                    y: number("--y", bounds.minY + bounds.height * 0.5))
func checkTarget() {
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid,
          let top = windows().first(where: { window in
              (window[kCGWindowAlpha as String] as? Double ?? 1) > 0 && (rect(window)?.contains(point) ?? false)
          }), owner(top) == pid else {
        abort("pid \(pid) does not own the frontmost window under (\(point.x), \(point.y)); no further events posted")
    }
}
checkTarget()
guard let source = CGEventSource(stateID: .hidSystemState) else { abort("Cannot create HID event source") }
func post(_ pixels: Int32, phase: Int64, inertia: Int64) {
    // Check before EVERY event so a stolen focus never redirects the workload.
    checkTarget()
    guard let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 2,
                              wheel1: pixels, wheel2: 0, wheel3: 0) else { abort("Cannot create scroll event") }
    event.location = point
    event.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1)
    event.setIntegerValueField(.scrollWheelEventScrollPhase, value: phase)
    event.setIntegerValueField(.scrollWheelEventMomentumPhase, value: inertia)
    event.post(tap: .cghidEventTap)
}
let count = Int((duration * rate).rounded())
let start = CACurrentMediaTime() + 0.1
let wallStart = Date().timeIntervalSince1970 + (start - CACurrentMediaTime())
emit(["kind": "poster_start", "pid": pid, "start_uptime_s": start, "start_unix_s": wallStart,
      "duration_s": duration, "rate_hz": rate, "momentum": momentum, "x": point.x, "y": point.y])
var maxLateness = 0.0
var lateSlots = 0
var fingerEnded = false
for i in 0..<count {
    let due = start + Double(i) / rate
    let remaining = due - CACurrentMediaTime()
    if remaining > 0 { Thread.sleep(forTimeInterval: remaining) }
    let lateness = max(0, CACurrentMediaTime() - due)
    maxLateness = max(maxLateness, lateness)
    if lateness > 1 / rate { lateSlots += 1 }
    // Repeated bidirectional gestures keep the 200-line workload away from EOF.
    // Each one-second cycle has an explicit boundary. Momentum occupies its last 35%.
    let slot = i % max(2, Int(rate.rounded()))
    let cycle = i / max(2, Int(rate.rounded()))
    let inMomentum = momentum && Double(slot) / rate >= 0.65
    let direction: Double = cycle % 2 == 0 ? -1 : 1
    let pixels = Int32((delta * direction * (inMomentum ? max(0.15, (1 - Double(slot) / rate) / 0.35) : 1)).rounded())
    if slot == 0 {
        if i > 0 {
            post(0, phase: fingerEnded ? 0 : 4, inertia: fingerEnded ? 3 : 0)
        }
        fingerEnded = false
        post(pixels, phase: 1, inertia: 0)
    } else if inMomentum {
        if !fingerEnded {
            post(0, phase: 4, inertia: 0)
            fingerEnded = true
            post(pixels, phase: 0, inertia: 1)
        } else { post(pixels, phase: 0, inertia: 2) }
    } else { post(pixels, phase: 2, inertia: 0) }
}
let remaining = start + duration - CACurrentMediaTime()
if remaining > 0 { Thread.sleep(forTimeInterval: remaining) }
post(0, phase: fingerEnded ? 0 : 4, inertia: fingerEnded ? 3 : 0)
emit(["kind": "poster_end", "pid": pid, "start_unix_s": wallStart,
      "end_unix_s": Date().timeIntervalSince1970, "duration_s": duration, "rate_hz": rate,
      "movement_events": count, "late_slots": lateSlots, "max_lateness_ms": maxLateness * 1000,
      "elapsed_s": CACurrentMediaTime() - start, "complete": true])
