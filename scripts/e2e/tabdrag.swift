// Scenario: tab drag smoothness and drop outcomes.
//
// A pressed tab is dragged with synthetic HID events at 120 Hz on a diagonal out of
// the strip and back to another slot. One thread posts the pointer events on fixed
// deadlines; another samples, also at 120 Hz, where the dragged tab is drawn: the
// floating drag window from CGWindowList (Velocity's own window at the floating
// level), else the tab's AXFrame in the strip. Neither thread catches up in bursts
// when a call blocks, so samples stay evenly spaced in time.
//
// Reported per drag: the distance between the tab and "pointer - grab offset"
// (median, p95), the largest step in which the tab moved differently from the
// pointer (compared at the measured input-to-screen latency, so a constant delay is
// not a jump), the step where the floating window first shows the tab, the share of
// steps in which the pointer moved and the tab did not, and the latency. The process
// footprint is taken after a warm-up drag, before and after the measured drags.
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
/// THRESHOLD in app_tab_drag.rs: a press stays a click until it moves this far.
private let clickThreshold = 4.0
private let floatingLevel = Int(CGWindowLevelForKey(.floatingWindow))
/// Judged limits for the "after" build.
private let maxJumpLimit = 2.0
private let footprintSlack: Int64 = 2 * 1_048_576

private enum TabSource: String { case none = "-", floating = "F", strip = "S" }

/// One pointer event as posted.
private struct Posted {
    var t: Double
    var point: CGPoint
    var phase: Int
    var cost: Double
}

private struct DragSample {
    var t: Double
    var tab: CGPoint?
    var source: TabSource
    var cost: Double
    // Filled in after the run from the post log.
    var pointer = CGPoint.zero
    var phase = 0
}

private struct DragMetrics {
    var moving = 0
    var floatingShare = 0.0
    var errorMedian = Double.nan
    var errorP95 = Double.nan
    var maxJump = Double.nan
    var maxJumpRaw = Double.nan
    var entryJump = Double.nan
    /// How far the tab itself moved on screen at that step.
    var entryShift = Double.nan
    var stallShare = 0.0
    var lagMs = Double.nan
    var sampleP95Ms = Double.nan
    var postP95Ms = Double.nan

    var line: String {
        String(format: "moving=%d floating=%.1f%% err_med=%.2f err_p95=%.2f max_jump=%.2f max_jump_raw=%.2f entry_jump=%.2f entry_shift=%.2f stall=%.1f%% lag_ms=%.1f sample_dt_p95_ms=%.1f post_p95_ms=%.2f",
               moving, floatingShare * 100, errorMedian, errorP95, maxJump, maxJumpRaw, entryJump, entryShift,
               stallShare * 100, lagMs, sampleP95Ms, postP95Ms)
    }
}

private final class MachClock: @unchecked Sendable {
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

private struct Leg {
    let to: CGPoint
    let ticks: Int
}

/// Posts the gesture on its own thread at fixed 120 Hz deadlines.
private final class Poster: @unchecked Sendable {
    let clock: MachClock
    let press: CGPoint
    let legs: [Leg]
    private(set) var log: [Posted] = []
    let done = DispatchSemaphore(value: 0)

    init(clock: MachClock, press: CGPoint, legs: [Leg]) {
        self.clock = clock
        self.press = press
        self.legs = legs
        log.reserveCapacity(1024)
    }

    func start() {
        let thread = Thread { [self] in run() }
        thread.qualityOfService = .userInteractive
        thread.start()
    }

    private func run() {
        let source = CGEventSource(stateID: .hidSystemState)
        var deadline = clock.now()
        func post(_ type: CGEventType, _ point: CGPoint, phase: Int) {
            let start = clock.now()
            if let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point,
                                   mouseButton: .left) {
                event.setIntegerValueField(.mouseEventClickState, value: 1)
                event.flags = []
                event.post(tap: .cghidEventTap)
            }
            let end = clock.now()
            log.append(Posted(t: end, point: point, phase: phase, cost: end - start))
        }
        func step() {
            deadline += tick
            clock.wait(until: deadline)
        }
        post(.leftMouseDown, press, phase: 0)
        // Pressed and still: the press frame renders and the image is prepared.
        for _ in 0..<18 { step() }
        var from = press
        for (index, leg) in legs.enumerated() {
            for i in 1...leg.ticks {
                step()
                let f = CGFloat(i) / CGFloat(leg.ticks)
                post(.leftMouseDragged, CGPoint(x: from.x + (leg.to.x - from.x) * f,
                                                y: from.y + (leg.to.y - from.y) * f), phase: index + 1)
            }
            from = leg.to
        }
        for _ in 0..<15 { step() }
        post(.leftMouseUp, from, phase: legs.count + 1)
        done.signal()
    }
}

/// Presses at `press`, follows `legs`, holds, releases, and keeps sampling through
/// the drop.
private func runDrag(pid: pid_t, press: CGPoint, legs: [Leg], stripTab: AXUIElement?) -> ([DragSample], [Posted]) {
    let clock = MachClock()
    let floating = FloatingTab(pid: pid)
    mouseMove(to: press)
    usleep(150_000)
    let poster = Poster(clock: clock, press: press, legs: legs)
    var samples: [DragSample] = []
    samples.reserveCapacity(1536)
    // Half a tick after the posts, so a sample sees the latest event's effect.
    var deadline = clock.now() + tick / 2
    poster.start()
    var released: Double?
    while released.map({ clock.now() < $0 + 0.32 }) ?? true {
        clock.wait(until: deadline)
        let start = clock.now()
        var tab = floating.tabOrigin()
        var source = tab == nil ? TabSource.none : .floating
        // AX makes the app answer on its main thread; read it only while no
        // floating window shows the tab.
        if tab == nil, let stripTab,
           let rect = AXReadErrors.shared.quietly({ axRect(stripTab) }), rect.width > 1, rect.height > 1 {
            tab = rect.origin
            source = .strip
        }
        let end = clock.now()
        samples.append(DragSample(t: (start + end) / 2, tab: tab, source: source, cost: end - start))
        // A blocked read delays the next sample; it never bunches samples together.
        deadline = max(deadline + tick, clock.now())
        if released == nil, poster.done.wait(timeout: .now()) == .success { released = clock.now() }
    }
    // Attach the pointer that was in effect at each sample.
    let log = poster.log
    var k = 0
    for i in samples.indices {
        while k + 1 < log.count, log[k + 1].t <= samples[i].t { k += 1 }
        if let posted = log.indices.contains(k) ? log[k] : nil, posted.t <= samples[i].t {
            samples[i].pointer = posted.point
            samples[i].phase = posted.phase
        } else {
            samples[i].pointer = press
            samples[i].phase = 0
        }
    }
    return (samples, log)
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

/// Where the pointer was `lag` seconds before sample `i`: the last event posted by then.
private func pointerAt(_ samples: [DragSample], _ i: Int, lag: Double, log: [Posted], press: CGPoint) -> CGPoint {
    let t = samples[i].t - lag
    var low = 0, high = log.count
    while low < high {
        let mid = (low + high) / 2
        if log[mid].t <= t { low = mid + 1 } else { high = mid }
    }
    return low == 0 ? press : log[low - 1].point
}

/// Metrics over the moving legs (phases 1...legs) once the pointer has passed the
/// click threshold. `grab` is pointer minus tab origin at the press.
private func measure(_ samples: [DragSample], log: [Posted], press: CGPoint, legs: Int, grab: CGVector) -> DragMetrics {
    var metrics = DragMetrics()
    let dts = zip(samples, samples.dropFirst()).map { ($1.t - $0.t) * 1000 }
    metrics.sampleP95Ms = percentile(dts, 0.95)
    metrics.postP95Ms = percentile(log.map { $0.cost * 1000 }, 0.95)
    func pointer(_ i: Int, lag: Double) -> CGPoint {
        pointerAt(samples, i, lag: lag, log: log, press: press)
    }
    guard let first = samples.firstIndex(where: {
        (1...legs).contains($0.phase) && distance($0.pointer, press) >= clickThreshold
    }) else { return metrics }
    let moving = samples.indices.filter { $0 >= first && (1...legs).contains(samples[$0].phase) }
    metrics.moving = moving.count
    guard !moving.isEmpty else { return metrics }
    metrics.floatingShare = Double(moving.filter { samples[$0].source == .floating }.count) / Double(moving.count)
    func expected(_ p: CGPoint) -> CGPoint { CGPoint(x: p.x - grab.dx, y: p.y - grab.dy) }
    let errors = moving.compactMap { i in samples[i].tab.map { distance($0, expected(samples[i].pointer)) } }
    metrics.errorMedian = percentile(errors, 0.5)
    metrics.errorP95 = percentile(errors, 0.95)
    // Latency: the delay that best explains where the floating tab is drawn.
    var best = (lag: 0.0, error: Double.infinity)
    for ms in stride(from: 0.0, through: 50.0, by: 1.0) {
        let lag = ms / 1000
        let e = percentile(moving.compactMap { i -> Double? in
            guard samples[i].source == .floating, let tab = samples[i].tab else { return nil }
            return distance(tab, expected(pointer(i, lag: lag)))
        }, 0.5)
        if e < best.error { best = (lag, e) }
    }
    let lag = best.error.isFinite ? best.lag : 0
    metrics.lagMs = lag * 1000
    // Jumps: the tab's step between neighbouring samples beyond the pointer's step
    // (the pointer as the screen shows it, `lag` earlier). Raw: without the lag.
    var jumps: [Double] = []
    var raw: [Double] = []
    var stalls = 0
    var moves = 0
    for (a, b) in zip(moving, moving.dropFirst()) where b == a + 1 {
        guard let ta = samples[a].tab, let tb = samples[b].tab else { continue }
        let moved = CGVector(dx: tb.x - ta.x, dy: tb.y - ta.y)
        func step(_ shift: Double) -> CGVector {
            let pa = pointer(a, lag: shift), pb = pointer(b, lag: shift)
            return CGVector(dx: pb.x - pa.x, dy: pb.y - pa.y)
        }
        let led = step(lag)
        jumps.append(Double(hypot(moved.dx - led.dx, moved.dy - led.dy)))
        let direct = step(0)
        raw.append(Double(hypot(moved.dx - direct.dx, moved.dy - direct.dy)))
        if hypot(led.dx, led.dy) >= 1 {
            moves += 1
            if hypot(moved.dx, moved.dy) < 0.5 { stalls += 1 }
        }
    }
    metrics.maxJump = jumps.max() ?? .nan
    metrics.maxJumpRaw = raw.max() ?? .nan
    metrics.stallShare = moves == 0 ? 0 : Double(stalls) / Double(moves)
    // Entry: the first sample that shows the floating window, against the last
    // sample that knew where the tab was drawn before it.
    if let entry = samples.indices.first(where: { samples[$0].source == .floating && samples[$0].phase >= 1 }),
       let before = samples[..<entry].lastIndex(where: { $0.tab != nil }),
       let ta = samples[before].tab, let tb = samples[entry].tab {
        let moved = CGVector(dx: tb.x - ta.x, dy: tb.y - ta.y)
        let pa = pointer(before, lag: lag), pb = pointer(entry, lag: lag)
        let led = CGVector(dx: pb.x - pa.x, dy: pb.y - pa.y)
        metrics.entryJump = Double(hypot(moved.dx - led.dx, moved.dy - led.dy))
        metrics.entryShift = Double(hypot(moved.dx, moved.dy))
    }
    return metrics
}

private func writeCSV(_ samples: [DragSample], name: String) -> String {
    var text = "t,phase,pointer_x,pointer_y,tab_x,tab_y,source,read_ms\n"
    for s in samples {
        let tab = s.tab.map { String(format: "%.2f,%.2f", $0.x, $0.y) } ?? ","
        text += String(format: "%.5f,%d,%.2f,%.2f,", s.t, s.phase, s.pointer.x, s.pointer.y) + tab
            + ",\(s.source.rawValue)," + String(format: "%.3f", s.cost * 1000) + "\n"
    }
    let path = artifactPath("tabdrag-\(name).csv")
    try? text.write(toFile: path, atomically: true, encoding: .utf8)
    return path
}

private func labels(_ window: AXUIElement) -> [String] {
    tabs(window).sorted { $0.frame.minX < $1.frame.minX }
        .map { String($0.label.split(separator: ".").first ?? "") }
}

private func tabElement(_ window: AXUIElement, named name: String) -> AXUIElement? {
    tabNodes(window).first {
        (axString($0, kAXTitleAttribute) ?? axString($0, kAXDescriptionAttribute) ?? "").hasPrefix(name)
    }
}

/// The footprint once it stops moving (two reads 250 ms apart within 128 KiB), or
/// the last read after five seconds.
private func settledFootprint(_ pid: pid_t) -> Int64 {
    var last = Int64(footprintBytes(pid) ?? 0)
    for _ in 0..<20 {
        usleep(250_000)
        let now = Int64(footprintBytes(pid) ?? 0)
        if abs(now - last) < 128 * 1024 { return now }
        last = now
    }
    return last
}

private struct VariantResult {
    var name: String
    var reps: [DragMetrics] = []
    var footprintLaunch: Int64 = 0
    var footprintBefore: Int64 = 0
    var footprintAfter: Int64 = 0
    var footprintEnd: Int64 = 0
}

/// The middle tab out on a 300 x 150 diagonal and back to slot 0, at 1.86 points
/// per event. Returns the metrics and the order the drop should produce, or nil when
/// the strip was not as expected.
private func measuredDrag(_ window: AXUIElement, pid: pid_t, label: String) -> (DragMetrics, [String])? {
    let order = labels(window)
    guard order.count == 3, let dragged = tab(window, named: order[1] + "."),
          let first = tab(window, named: order[0] + ".") else { return nil }
    let press = CGPoint(x: dragged.frame.minX + dragged.frame.width * 0.4, y: dragged.frame.midY)
    let grab = CGVector(dx: press.x - dragged.frame.minX, dy: press.y - dragged.frame.minY)
    let out = CGPoint(x: press.x + 300, y: press.y + 150)
    let back = CGPoint(x: first.frame.minX + min(12, first.frame.width / 4), y: first.frame.midY)
    let backTicks = Int((distance(out, back) / 1.86).rounded(.up))
    let (samples, log) = runDrag(pid: pid, press: press,
                                 legs: [Leg(to: out, ticks: 180), Leg(to: back, ticks: backTicks)],
                                 stripTab: tabElement(window, named: order[1] + "."))
    let metrics = measure(samples, log: log, press: press, legs: 2, grab: grab)
    _ = writeCSV(samples, name: label)
    return (metrics, [order[1], order[0], order[2]])
}

/// One build: a warm-up drag, three measured drags in one window, then a merge into
/// a second window and a tear-off. Checks are verdicts only when `judged`.
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
    sleep(seconds: 1.0)
    result.footprintLaunch = settledFootprint(pid)

    // 0. Warm-up: the first drag in a process creates the drag window.
    if let (_, expected) = measuredDrag(window, pid: pid, label: "\(name)-round\(round)-warmup") {
        _ = waitUntil(3) { labels(window) == expected }
    } else {
        verdict(false, "three tabs before the warm-up drag", detail: tabSummary(window))
    }
    sleep(seconds: 0.3)
    result.footprintBefore = settledFootprint(pid)

    // 1. Three measured drags.
    for rep in 1...3 {
        guard let (metrics, expected) = measuredDrag(window, pid: pid, label: "\(name)-round\(round)-rep\(rep)") else {
            verdict(false, "rep \(rep): three tabs before the drag", detail: tabSummary(window))
            break
        }
        result.reps.append(metrics)
        report.info("METRIC variant=\(name) round=\(round) rep=\(rep)", detail: metrics.line)
        _ = waitUntil(3) { labels(window) == expected }
        verdict(labels(window) == expected, "rep \(rep): the dropped tab lands in slot 0",
                detail: "\(labels(window)) expected \(expected)")
        if judged {
            report.check(metrics.maxJump <= maxJumpLimit, "[\(name)] rep \(rep): largest jump <= \(maxJumpLimit) points",
                         detail: String(format: "%.2f (latency %.0f ms, raw %.2f)", metrics.maxJump, metrics.lagMs,
                                        metrics.maxJumpRaw))
        }
        sleep(seconds: 0.3)
    }
    result.footprintAfter = settledFootprint(pid)
    let grown = result.footprintAfter - result.footprintBefore
    verdict(abs(grown) <= footprintSlack, "footprint after the drags is within 2 MiB of before",
            detail: String(format: "launch %.1f MiB, after warm-up %.1f MiB, after 3 drags %.1f MiB (%+.2f MiB)",
                           Double(result.footprintLaunch) / 1_048_576, Double(result.footprintBefore) / 1_048_576,
                           Double(result.footprintAfter) / 1_048_576, Double(grown) / 1_048_576))

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
            let (samples, _) = runDrag(pid: pid, press: press,
                                  legs: [Leg(to: below, ticks: 80),
                                         Leg(to: drop, ticks: Int((distance(below, drop) / 1.86).rounded(.up)))],
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
            _ = runDrag(pid: pid, press: press,
                        legs: [Leg(to: out, ticks: Int((distance(press, out) / 1.86).rounded(.up)))], stripTab: nil)
            _ = waitUntil(5) { onScreenWindows(pid).contains { !before.contains($0.id) } }
            let added = onScreenWindows(pid).filter { !before.contains($0.id) }
            verdict(!added.isEmpty, "a drop outside every window opens a new window",
                    detail: "\(added.count) new window(s)")
        }
    } else {
        verdict(false, "a second window opened", detail: "\(windows.count) window(s)")
    }
    result.footprintEnd = settledFootprint(pid)
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
    report.info("screen", detail: NSScreen.screens.map {
        "\($0.frame.width)x\($0.frame.height)@\($0.backingScaleFactor)x \($0.maximumFramesPerSecond) Hz"
    }.joined(separator: ", "))
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
        func worst(_ key: (DragMetrics) -> Double) -> Double { reps.map(key).filter { !$0.isNaN }.max() ?? .nan }
        let mib = { (bytes: Int64) in String(format: "%.1f", Double(bytes) / 1_048_576) }
        let grown = mine.map { String(format: "%+.2f", Double($0.footprintAfter - $0.footprintBefore) / 1_048_576) }
        report.info("COMPARE \(name)", detail: String(
            format: "reps=%d err_med=%.2f err_p95=%.2f max_jump(median)=%.2f max_jump(worst)=%.2f max_jump_raw(worst)=%.2f entry_jump=%.2f entry_shift=%.2f stall=%.1f%% floating=%.1f%% lag_ms=%.1f",
            reps.count, median(\.errorMedian), median(\.errorP95), median(\.maxJump), worst(\.maxJump),
            worst(\.maxJumpRaw), median(\.entryJump), median(\.entryShift), median(\.stallShare) * 100,
            median(\.floatingShare) * 100, median(\.lagMs))
            + " footprint launch=" + mine.map { mib($0.footprintLaunch) }.joined(separator: "/")
            + " before=" + mine.map { mib($0.footprintBefore) }.joined(separator: "/")
            + " after=" + mine.map { mib($0.footprintAfter) }.joined(separator: "/")
            + " grown=" + grown.joined(separator: "/")
            + " end(2+ windows)=" + mine.map { mib($0.footprintEnd) }.joined(separator: "/") + " MiB")
    }
    report.finish()
}
