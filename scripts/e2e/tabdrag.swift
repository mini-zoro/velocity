// Scenario: tab drag smoothness and drop outcomes.
//
// A pressed tab is dragged with synthetic HID events at 120 Hz on a diagonal out of
// the strip and back to another slot. Before every event the harness reads where the
// dragged tab is drawn: the floating drag window from CGWindowList (Velocity's own
// window above the normal level), else the tab's AXFrame in the strip. It reports
// how far the tab is from "pointer - grab offset", how much it jumps beyond the
// pointer's own motion, the jump when the drag starts, how often the pointer moves
// while the tab does not, and the process footprint before and after.
//
// E2E_TABDRAG_LAUNCHERS="before=/a/velocity,after=/b/velocity" measures two builds on
// the same runner, alternating; only "after" (or the single default build) is judged.
// Raw samples go to E2E_ARTIFACTS as CSV; nothing is written next to the sources.
import AppKit
import ApplicationServices
import CoreGraphics
import Darwin
import Foundation

/// IMAGE_PAD in crates/velocity/src/app_tab_drag.rs: transparent margin around the tab.
private let tabImagePad: CGFloat = 12
private let tick = 1.0 / 120
private let floatingLevel = Int(CGWindowLevelForKey(.floatingWindow))
/// Judged limits for the "after" build.
private let maxJumpLimit = 2.0
private let footprintSlack: Int64 = 2 * 1_048_576

private enum TabSource: String { case none = "-", floating = "F", strip = "S" }

private struct DragSample {
    var t: Double
    var pointer: CGPoint
    var tab: CGPoint?
    var source: TabSource
    var phase: Int
}

private struct DragMetrics {
    var moving = 0
    var floatingShare = 0.0
    var errorMedian = Double.nan
    var errorP95 = Double.nan
    var maxJump = Double.nan
    var entryJump = Double.nan
    var stallShare = 0.0
    var lagMs = Double.nan

    var line: String {
        String(format: "moving=%d floating=%.1f%% err_med=%.2f err_p95=%.2f max_jump=%.2f entry_jump=%.2f stall=%.1f%% lag_ms=%.1f",
               moving, floatingShare * 100, errorMedian, errorP95, maxJump, entryJump, stallShare * 100, lagMs)
    }
}

private final class MachClock {
    private let base = mach_absolute_time()
    private let numer: UInt64
    private let denom: UInt64
    init() {
        var info = mach_timebase_info_data_t()
        mach_timebase_info(&info)
        numer = UInt64(info.numer)
        denom = UInt64(info.denom)
    }
    func now() -> Double {
        Double((mach_absolute_time() - base) * numer / denom) / 1e9
    }
    func wait(until seconds: Double) {
        mach_wait_until(base + UInt64(max(0, seconds) * 1e9) * denom / numer)
    }
}

/// Velocity's floating drag window: owned by the app, at the floating level, tab-sized.
private final class FloatingTab {
    let pid: pid_t
    private var id: CGWindowID?
    private var scans = 0
    init(pid: pid_t) { self.pid = pid }

    /// Top-left of the tab inside the visible floating window, or nil while it is hidden.
    func tabOrigin() -> CGPoint? {
        if id == nil {
            // A full on-screen scan is the expensive call; while nothing is found,
            // do it on every other sample only.
            scans += 1
            if scans % 2 == 1 { id = find() }
        }
        guard let id else { return nil }
        guard let list = CGWindowListCopyWindowInfo([.optionIncludingWindow], id) as? [[String: Any]],
              let entry = list.first else {
            self.id = nil
            return nil
        }
        guard (entry[kCGWindowIsOnscreen as String] as? Bool) == true,
              let bounds = entry[kCGWindowBounds as String] as? [String: Any],
              let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary) else { return nil }
        return CGPoint(x: rect.minX + tabImagePad, y: rect.minY + tabImagePad)
    }

    private func find() -> CGWindowID? {
        let raw = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]]) ?? []
        for entry in raw {
            guard entry[kCGWindowOwnerPID as String] as? pid_t == pid,
                  entry[kCGWindowLayer as String] as? Int == floatingLevel,
                  let bounds = entry[kCGWindowBounds as String] as? [String: Any],
                  let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary),
                  (40...800).contains(rect.width), (30...160).contains(rect.height),
                  let number = entry[kCGWindowNumber as String] as? Int else { continue }
            return CGWindowID(number)
        }
        return nil
    }
}

/// Synthetic HID pointer without the fixed pauses of `mouseEvent`.
private final class Pointer {
    private let source = CGEventSource(stateID: .hidSystemState)
    private(set) var position = CGPoint.zero
    func post(_ type: CGEventType, _ point: CGPoint) {
        position = point
        guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point,
                                  mouseButton: .left) else { return }
        event.setIntegerValueField(.mouseEventClickState, value: 1)
        event.flags = []
        event.post(tap: .cghidEventTap)
    }
}

private struct Leg {
    let to: CGPoint
    let ticks: Int
}

/// Presses at `press`, follows `legs` one event per tick, holds, releases, and keeps
/// sampling through the drop. Each sample is taken just before the next event.
private func runDrag(pid: pid_t, press: CGPoint, legs: [Leg], stripTab: AXUIElement?) -> [DragSample] {
    let floating = FloatingTab(pid: pid)
    let pointer = Pointer()
    let clock = MachClock()
    var samples: [DragSample] = []
    samples.reserveCapacity(1024)
    var n = 0
    func sample(_ phase: Int) {
        var tab = floating.tabOrigin()
        var source = tab == nil ? TabSource.none : .floating
        // AX makes the app build its tree; read it at 60 Hz and only while no
        // floating window shows the tab.
        if tab == nil, n % 2 == 0, let stripTab,
           let rect = AXReadErrors.shared.quietly({ axRect(stripTab) }), rect.width > 1, rect.height > 1 {
            tab = rect.origin
            source = .strip
        }
        samples.append(DragSample(t: clock.now(), pointer: pointer.position, tab: tab, source: source, phase: phase))
        n += 1
    }
    mouseMove(to: press)
    usleep(150_000)
    var deadline = clock.now()
    func step() {
        deadline += tick
        clock.wait(until: deadline)
    }
    pointer.post(.leftMouseDown, press)
    // Phase 0: pressed and still; the press frame renders and the image is prepared.
    for _ in 0..<18 { step(); sample(0) }
    var from = press
    for (index, leg) in legs.enumerated() {
        for i in 1...leg.ticks {
            step()
            sample(index + 1)
            let f = CGFloat(i) / CGFloat(leg.ticks)
            pointer.post(.leftMouseDragged, CGPoint(x: from.x + (leg.to.x - from.x) * f,
                                                    y: from.y + (leg.to.y - from.y) * f))
        }
        from = leg.to
    }
    let hold = legs.count + 1
    for _ in 0..<15 { step(); sample(hold) }
    pointer.post(.leftMouseUp, from)
    // The landing.
    for _ in 0..<36 { step(); sample(hold + 1) }
    return samples
}

private func percentile(_ values: [Double], _ p: Double) -> Double {
    guard !values.isEmpty else { return .nan }
    let sorted = values.sorted()
    let rank = min(sorted.count - 1, max(0, Int((p * Double(sorted.count - 1)).rounded())))
    return sorted[rank]
}

private func distance(_ a: CGPoint, _ b: CGPoint) -> Double {
    Double(hypot(a.x - b.x, a.y - b.y))
}

/// Metrics over the moving legs (phases 1...legs). `grab` is pointer minus tab origin
/// at the press.
private func measure(_ samples: [DragSample], legs: Int, grab: CGVector) -> DragMetrics {
    var metrics = DragMetrics()
    let moving = samples.indices.filter { (1...legs).contains(samples[$0].phase) }
    metrics.moving = moving.count
    guard !moving.isEmpty else { return metrics }
    metrics.floatingShare = Double(moving.filter { samples[$0].source == .floating }.count) / Double(moving.count)
    let expected = { (s: DragSample) in CGPoint(x: s.pointer.x - grab.dx, y: s.pointer.y - grab.dy) }
    var errors: [Double] = []
    var lags: [Double] = []
    for i in moving {
        guard let tab = samples[i].tab else { continue }
        errors.append(distance(tab, expected(samples[i])))
        if samples[i].source == .floating {
            // Which recent event does the tab show? Its age is the input-to-window lag.
            var best = (lag: 0, error: Double.infinity)
            for k in stride(from: i, through: max(0, i - 12), by: -1) {
                let e = distance(tab, expected(samples[k]))
                if e < best.error { best = (i - k, e) }
            }
            if best.error < 0.75 { lags.append(Double(best.lag) * tick * 1000) }
        }
    }
    metrics.errorMedian = percentile(errors, 0.5)
    metrics.errorP95 = percentile(errors, 0.95)
    metrics.lagMs = percentile(lags, 0.5)
    // Jumps: tab motion between neighbouring known samples beyond the pointer's own.
    var jumps: [Double] = []
    var previous: Int?
    var floatingSeen = false
    for i in samples.indices where samples[i].phase <= legs {
        guard let tab = samples[i].tab else { continue }
        if let j = previous, let last = samples[j].tab {
            let moved = CGVector(dx: tab.x - last.x, dy: tab.y - last.y)
            let led = CGVector(dx: samples[i].pointer.x - samples[j].pointer.x,
                               dy: samples[i].pointer.y - samples[j].pointer.y)
            let jump = Double(hypot(moved.dx - led.dx, moved.dy - led.dy))
            // Neighbours only: a gap of unknown samples is not one visible step.
            if samples[i].phase >= 1, i - j <= 3 { jumps.append(jump) }
            // The step where the floating window first shows the tab, from wherever
            // the tab was drawn before it (the strip).
            if samples[i].source == .floating, !floatingSeen { metrics.entryJump = jump }
        }
        if samples[i].source == .floating { floatingSeen = true }
        previous = i
    }
    metrics.maxJump = jumps.max() ?? .nan
    // Stalls: the pointer moved since the previous sample, the tab did not (or is not shown).
    var stalls = 0
    var moves = 0
    for i in moving where i > 0 {
        guard distance(samples[i].pointer, samples[i - 1].pointer) > 0.5 else { continue }
        moves += 1
        if let a = samples[i].tab, let b = samples[i - 1].tab, distance(a, b) >= 0.05 { continue }
        stalls += 1
    }
    metrics.stallShare = moves == 0 ? 0 : Double(stalls) / Double(moves)
    return metrics
}

private func writeCSV(_ samples: [DragSample], name: String) -> String {
    var text = "t,phase,pointer_x,pointer_y,tab_x,tab_y,source\n"
    for s in samples {
        let tab = s.tab.map { String(format: "%.2f,%.2f", $0.x, $0.y) } ?? ","
        text += String(format: "%.5f,%d,%.2f,%.2f,", s.t, s.phase, s.pointer.x, s.pointer.y) + tab + ",\(s.source.rawValue)\n"
    }
    let path = artifactPath("tabdrag-\(name).csv")
    try? text.write(toFile: path, atomically: true, encoding: .utf8)
    return path
}

private func tabElement(_ window: AXUIElement, named name: String) -> AXUIElement? {
    tabNodes(window).first {
        (axString($0, kAXTitleAttribute) ?? axString($0, kAXDescriptionAttribute) ?? "").hasPrefix(name)
    }
}

private func labels(_ window: AXUIElement) -> [String] {
    tabs(window).sorted { $0.frame.minX < $1.frame.minX }
        .map { String($0.label.split(separator: ".").first ?? "") }
}

private struct VariantResult {
    var name: String
    var reps: [DragMetrics] = []
    var footprintBefore: Int64 = 0
    var footprintAfter: Int64 = 0
    var footprintEnd: Int64 = 0
}

/// One build: three measured drags in one window, then a merge into a second window
/// and a tear-off. Checks are verdicts only when `judged`.
private func runVariant(_ name: String, launcher: URL, work: URL, round: Int, judged: Bool,
                        report: Report) -> VariantResult {
    func verdict(_ ok: Bool, _ label: String, detail: String = "") {
        if judged {
            report.check(ok, "[\(name)] \(label)", detail: detail)
        } else {
            report.note("[\(name)] \(label): \(ok ? "yes" : "NO")", detail: detail)
        }
    }
    var result = VariantResult(name: name)
    let base = work.appendingPathComponent("\(name)-\(round)")
    let environment = Environment(work: base, dataDir: base.appendingPathComponent("data"), launcher: launcher)
    environment.reset()
    let files = ["a", "b", "c", "d"].map { environment.fixture("\($0).txt", contents: "\($0)\n") }
    let pid = launchVelocity(environment, files: Array(files.prefix(3)))
    dismissAlerts(report: report)
    guard let window = velocityWindow(pid) else {
        verdict(false, "window opened")
        return result
    }
    _ = moveWindow(window, to: CGRect(x: 20, y: 30, width: 900, height: 620))
    bringToFront(pid, report: report)
    if let frame = axRect(window) {
        _ = guardedClick(CGPoint(x: frame.midX, y: frame.minY + 160), pid: pid, report: report, label: "focus")
    }
    _ = waitUntil(10) { tabs(window).count == 3 }
    sleep(seconds: 1.5)
    result.footprintBefore = Int64(footprintBytes(pid) ?? 0)

    // 1. Three measured drags: the middle tab out on a 300 x 150 diagonal, back to slot 0.
    for rep in 1...3 {
        let order = labels(window)
        guard order.count == 3, let dragged = tab(window, named: order[1] + "."),
              let first = tab(window, named: order[0] + ".") else {
            verdict(false, "rep \(rep): three tabs before the drag", detail: tabSummary(window))
            break
        }
        let press = CGPoint(x: dragged.frame.minX + dragged.frame.width * 0.4, y: dragged.frame.midY)
        let grab = CGVector(dx: press.x - dragged.frame.minX, dy: press.y - dragged.frame.minY)
        let out = CGPoint(x: press.x + 300, y: press.y + 150)
        let back = CGPoint(x: first.frame.minX + min(12, first.frame.width / 4), y: first.frame.midY)
        // 1.86 points per event in both legs.
        let backTicks = Int((distance(out, back) / 1.86).rounded(.up))
        let samples = runDrag(pid: pid, press: press, legs: [Leg(to: out, ticks: 180), Leg(to: back, ticks: backTicks)],
                              stripTab: tabElement(window, named: order[1] + "."))
        let metrics = measure(samples, legs: 2, grab: grab)
        result.reps.append(metrics)
        let csv = writeCSV(samples, name: "\(name)-round\(round)-rep\(rep)")
        report.info("METRIC variant=\(name) round=\(round) rep=\(rep)", detail: metrics.line + " csv=\(csv)")
        sleep(seconds: 0.5)
        let expected = [order[1], order[0], order[2]]
        _ = waitUntil(3) { labels(window) == expected }
        verdict(labels(window) == expected, "rep \(rep): the dropped tab lands in slot 0",
                detail: "\(labels(window)) expected \(expected)")
        if judged {
            report.check(metrics.maxJump <= maxJumpLimit, "[\(name)] rep \(rep): largest jump <= \(maxJumpLimit) points",
                         detail: String(format: "%.2f", metrics.maxJump))
        }
    }
    sleep(seconds: 2.5)
    result.footprintAfter = Int64(footprintBytes(pid) ?? 0)
    let grown = result.footprintAfter - result.footprintBefore
    verdict(abs(grown) <= footprintSlack, "footprint after the drags is within 2 MiB of before",
            detail: String(format: "before %.1f MiB after %.1f MiB (%+.2f MiB)",
                           Double(result.footprintBefore) / 1_048_576, Double(result.footprintAfter) / 1_048_576,
                           Double(grown) / 1_048_576))

    // 2. Merge into a second window's strip.
    _ = shell("\(launcher.path.shellQuoted) --data-dir \(environment.dataDir.path.shellQuoted) -n \(files[3].shellQuoted)")
    _ = waitUntil(10) { velocityWindows(pid).count >= 2 }
    sleep(seconds: 0.8)
    let windows = velocityWindows(pid)
    if windows.count >= 2, let second = windows.first(where: { tabs($0).contains { $0.label.hasPrefix("d.") } }),
       let main = windows.first(where: { !sameWindow($0, second) }) {
        arrangeSideBySide(main, second, report: report)
        _ = waitUntil(5) { !tabs(main).isEmpty && !tabs(second).isEmpty }
        sleep(seconds: 0.5)
        let moving = labels(main).last ?? "c"
        if let source = tab(main, named: moving + "."), let target = tabs(second).first {
            let press = CGPoint(x: source.frame.midX, y: source.frame.midY)
            let below = CGPoint(x: press.x + 40, y: press.y + 140)
            let drop = CGPoint(x: target.frame.minX + 6, y: target.frame.midY)
            let samples = runDrag(pid: pid, press: press,
                                  legs: [Leg(to: below, ticks: 80), Leg(to: drop, ticks: Int((distance(below, drop) / 1.86).rounded(.up)))],
                                  stripTab: nil)
            _ = writeCSV(samples, name: "\(name)-round\(round)-merge")
            _ = waitUntil(3) { labels(second).first == moving }
            verdict(labels(second).first == moving && !labels(main).contains(moving),
                    "a drop on the second window's strip inserts the tab there",
                    detail: "main \(labels(main)) second \(labels(second))")
        } else {
            verdict(false, "tabs for the merge drag", detail: "main \(labels(main)) second \(labels(second))")
        }

        // 3. Tear-off: release outside every window.
        let before = Set(onScreenWindows(pid).map(\.id))
        if let torn = tab(main, named: (labels(main).first ?? "a") + "."), let frame = axRect(main) {
            let press = CGPoint(x: torn.frame.midX, y: torn.frame.midY)
            let screenHeight = CGDisplayBounds(CGMainDisplayID()).height
            let out = CGPoint(x: frame.midX, y: min(frame.maxY + 120, screenHeight - 40))
            _ = runDrag(pid: pid, press: press, legs: [Leg(to: out, ticks: Int((distance(press, out) / 1.86).rounded(.up)))],
                        stripTab: nil)
            _ = waitUntil(5) { onScreenWindows(pid).contains { !before.contains($0.id) } }
            let added = onScreenWindows(pid).filter { !before.contains($0.id) }
            verdict(!added.isEmpty, "a drop outside every window opens a new window",
                    detail: "\(added.count) new window(s)")
        }
    } else {
        verdict(false, "a second window opened", detail: "\(windows.count) window(s)")
    }
    sleep(seconds: 1.5)
    result.footprintEnd = Int64(footprintBytes(pid) ?? 0)
    if report.failures > 0 && judged { dumpOnFailure(report, pid: pid, label: "tabdrag-\(name)") }
    killVelocity(at: launcher)
    return result
}

func scenarioTabDrag(_ options: ArraySlice<String>) -> Never {
    let report = Report("tabdrag")
    let environment = Environment.from(options)
    var variants: [(String, URL)] = []
    for pair in (ProcessInfo.processInfo.environment["E2E_TABDRAG_LAUNCHERS"] ?? "").split(separator: ",") {
        let parts = pair.split(separator: "=", maxSplits: 1)
        if parts.count == 2 { variants.append((String(parts[0]), URL(fileURLWithPath: String(parts[1])))) }
    }
    if variants.isEmpty { variants = [("after", environment.launcher)] }
    let judged = variants.contains { $0.0 == "after" } ? "after" : variants.last!.0
    report.info("variants", detail: variants.map { "\($0.0)=\($0.1.path)" }.joined(separator: ", ") + " judged=\(judged)")
    report.info("screen", detail: NSScreen.screens.map { "\($0.frame.width)x\($0.frame.height)@\($0.backingScaleFactor)x" }
        .joined(separator: ", "))
    let rounds = max(1, Int(ProcessInfo.processInfo.environment["E2E_TABDRAG_ROUNDS"] ?? "") ?? 2)
    var results: [VariantResult] = []
    for round in 1...rounds {
        // Alternate which build goes first.
        let ordered = round % 2 == 1 ? variants : variants.reversed()
        for (name, launcher) in ordered {
            results.append(runVariant(name, launcher: launcher, work: environment.work, round: round,
                                      judged: name == judged, report: report))
        }
    }
    for (name, _) in variants {
        let mine = results.filter { $0.name == name }
        let reps = mine.flatMap(\.reps)
        guard !reps.isEmpty else { continue }
        func median(_ key: (DragMetrics) -> Double) -> Double { percentile(reps.map(key).filter { !$0.isNaN }, 0.5) }
        let worst = reps.map(\.maxJump).filter { !$0.isNaN }.max() ?? .nan
        let grown = mine.map { Double($0.footprintAfter - $0.footprintBefore) / 1_048_576 }
        report.info("COMPARE \(name)", detail: String(
            format: "reps=%d err_med=%.2f err_p95=%.2f max_jump(median)=%.2f max_jump(worst)=%.2f entry_jump=%.2f stall=%.1f%% floating=%.1f%% lag_ms=%.1f footprint_before=%.1fMiB footprint_after=%.1fMiB grown=%@MiB end=%.1fMiB",
            reps.count, median(\.errorMedian), median(\.errorP95), median(\.maxJump), worst, median(\.entryJump),
            median(\.stallShare) * 100, median(\.floatingShare) * 100, median(\.lagMs),
            Double(mine.map(\.footprintBefore).max() ?? 0) / 1_048_576, Double(mine.map(\.footprintAfter).max() ?? 0) / 1_048_576,
            grown.map { String(format: "%+.2f", $0) }.joined(separator: "/"),
            Double(mine.map(\.footprintEnd).max() ?? 0) / 1_048_576))
    }
    report.finish()
}
