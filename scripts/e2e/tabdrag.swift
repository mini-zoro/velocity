// Scenario: tab drag smoothness and drop outcomes.
//
// A pressed tab is dragged with synthetic HID events at 120 Hz on a diagonal out of
// the strip and back to another slot. One thread posts the pointer events on fixed
// deadlines; another samples, at 240 Hz, where the dragged tab is drawn: the
// floating drag window from CGWindowList (Velocity's own window at the floating
// level), else the tab's AXFrame in the strip. Neither thread catches up in bursts
// when a call blocks, so samples stay evenly spaced in time.
//
// Reported per drag: the distance between the tab and "pointer - grab offset"
// (median, p95); the largest jump between displayed frames (judged): how much the
// tab's offset from the pointer event a frame shows changes between frames; the
// literal largest difference between the tab's and the pointer's step over 120 Hz
// samples at the measured latency (on a 60 Hz display that one cannot go below one
// pointer step, since each frame moves two steps at once); the jump where the
// floating window first shows the tab; the share of 120 Hz steps in which the
// pointer moved and the tab did not; frame intervals, late frames and latency. The footprint is taken after a warm-up drag, before and after the
// measured drags, once it settles.
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
/// The sampler runs at twice the pointer rate, to time each displayed frame.
private let sampleTick = tick / 2
/// THRESHOLD in app_tab_drag.rs: a press stays a click until it moves this far.
private let clickThreshold = 4.0
private let floatingLevel = Int(CGWindowLevelForKey(.floatingWindow))
/// Judged limits for the "after" build.
private let maxJumpLimit = 2.0
private let footprintSlack: Int64 = 2 * 1_048_576

private enum TabSource: String { case none = "-", floating = "F", strip = "S" }

/// The scenario's App Nap exemption.
private var napGuard: NSObjectProtocol?

/// One pointer event as posted.
private struct Posted {
    var t: Double
    var point: CGPoint
    var phase: Int
    var cost: Double
    /// How long after its deadline the event was posted.
    var late: Double
}

/// Gives the calling thread a real-time (time-constraint) schedule, as audio and
/// video threads use, so a 120 Hz deadline is not stretched by timer coalescing.
private func makeRealtime(period: Double) -> Bool {
    var info = mach_timebase_info_data_t()
    mach_timebase_info(&info)
    func ticks(_ seconds: Double) -> UInt32 {
        UInt32(seconds * 1e9 * Double(info.denom) / Double(info.numer))
    }
    var policy = thread_time_constraint_policy_data_t(period: ticks(period), computation: ticks(0.0015),
                                                      constraint: ticks(0.003), preemptible: 1)
    let count = mach_msg_type_number_t(MemoryLayout<thread_time_constraint_policy_data_t>.size
        / MemoryLayout<integer_t>.size)
    let status = withUnsafeMutablePointer(to: &policy) { pointer in
        pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            thread_policy_set(pthread_mach_thread_np(pthread_self()), thread_policy_flavor_t(THREAD_TIME_CONSTRAINT_POLICY),
                              $0, count)
        }
    }
    return status == KERN_SUCCESS
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
    /// Per displayed frame: how much the tab's offset from the pointer event it
    /// shows changes from one frame to the next.
    var maxJump = Double.nan
    /// Per 120 Hz sample pair, as literally defined; on a 60 Hz display its floor
    /// is one pointer step (the frame moves two at once).
    var maxJump120 = Double.nan
    var maxJumpRaw = Double.nan
    var entryJump = Double.nan
    /// How far the tab itself moved on screen at that step.
    var entryShift = Double.nan
    var stallShare = 0.0
    var frameMs = Double.nan
    var lateFrameShare = 0.0
    var frameLatencyMs = Double.nan
    var lagMs = Double.nan
    var sampleP95Ms = Double.nan
    var postP95Ms = Double.nan
    var postLateP95Ms = Double.nan
    var postLateMaxMs = Double.nan
    var realtime = false

    var line: String {
        String(format: "moving=%d floating=%.1f%% err_med=%.2f err_p95=%.2f max_jump_frame=%.2f max_jump_120hz=%.2f max_jump_raw=%.2f entry_jump=%.2f entry_shift=%.2f stall_120hz=%.1f%% frame_ms=%.1f late_frames=%.1f%% frame_latency_ms=%.1f lag_ms=%.1f sample_dt_p95_ms=%.1f post_p95_ms=%.2f post_late_p95_ms=%.1f post_late_max_ms=%.1f realtime=%@",
               moving, floatingShare * 100, errorMedian, errorP95, maxJump, maxJump120, maxJumpRaw, entryJump,
               entryShift, stallShare * 100, frameMs, lateFrameShare * 100, frameLatencyMs, lagMs, sampleP95Ms, postP95Ms,
               postLateP95Ms, postLateMaxMs, realtime ? "yes" : "no")
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
    private(set) var realtime = false
    let done = DispatchSemaphore(value: 0)

    init(clock: MachClock, press: CGPoint, legs: [Leg]) {
        self.clock = clock
        self.press = press
        self.legs = legs
        log.reserveCapacity(1024)
    }

    func start() {
        let thread = Thread { [self] in
            realtime = makeRealtime(period: tick)
            run()
        }
        thread.qualityOfService = .userInteractive
        thread.start()
    }

    private func run() {
        let source = CGEventSource(stateID: .hidSystemState)
        var deadline = clock.now()
        func post(_ type: CGEventType, _ point: CGPoint, phase: Int) {
            let start = clock.now()
            let late = start - deadline
            if let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point,
                                   mouseButton: .left) {
                event.setIntegerValueField(.mouseEventClickState, value: 1)
                event.flags = []
                event.post(tap: .cghidEventTap)
            }
            let end = clock.now()
            log.append(Posted(t: end, point: point, phase: phase, cost: end - start, late: late))
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

/// Samples the dragged tab on its own real-time thread at 240 Hz.
private final class Sampler: @unchecked Sendable {
    let clock: MachClock
    let floating: FloatingTab
    let stripTab: AXUIElement?
    let poster: Poster
    private(set) var samples: [DragSample] = []
    let done = DispatchSemaphore(value: 0)

    init(clock: MachClock, pid: pid_t, stripTab: AXUIElement?, poster: Poster) {
        self.clock = clock
        floating = FloatingTab(pid: pid)
        self.stripTab = stripTab
        self.poster = poster
        samples.reserveCapacity(1536)
    }

    func start() {
        let thread = Thread { [self] in
            _ = makeRealtime(period: sampleTick)
            run()
            done.signal()
        }
        thread.qualityOfService = .userInteractive
        thread.start()
    }

    private func run() {
        // Offset from the posts, so a sample sees the latest event's effect.
        var deadline = clock.now() + sampleTick / 2
        var released: Double?
        var n = 0
        while released.map({ clock.now() < $0 + 0.32 }) ?? true {
            clock.wait(until: deadline)
            let start = clock.now()
            var tab = floating.tabOrigin()
            var source = tab == nil ? TabSource.none : .floating
            // AX makes the app answer on its main thread; read it at 120 Hz and only
            // while no floating window shows the tab.
            n += 1
            if tab == nil, n % 2 == 0, let stripTab,
               let rect = AXReadErrors.shared.quietly({ axRect(stripTab) }), rect.width > 1, rect.height > 1 {
                tab = rect.origin
                source = .strip
            }
            let end = clock.now()
            samples.append(DragSample(t: (start + end) / 2, tab: tab, source: source, cost: end - start))
            // A blocked read delays the next sample; it never bunches samples together.
            deadline = max(deadline + sampleTick, clock.now())
            if released == nil, poster.done.wait(timeout: .now()) == .success { released = clock.now() }
        }
    }
}

/// Presses at `press`, follows `legs`, holds, releases, and keeps sampling through
/// the drop.
private func runDrag(pid: pid_t, press: CGPoint, legs: [Leg], stripTab: AXUIElement?) -> ([DragSample], [Posted], Bool) {
    let clock = MachClock()
    mouseMove(to: press)
    usleep(150_000)
    let poster = Poster(clock: clock, press: press, legs: legs)
    let sampler = Sampler(clock: clock, pid: pid, stripTab: stripTab, poster: poster)
    sampler.start()
    poster.start()
    sampler.done.wait()
    var samples = sampler.samples
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
    return (samples, log, poster.realtime)
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

/// Metrics over the moving legs (phases 1...legs) once the pointer has passed the
/// click threshold. `grab` is pointer minus tab origin at the press.
private func measure(_ samples: [DragSample], log: [Posted], press: CGPoint, legs: Int, grab: CGVector) -> DragMetrics {
    var metrics = DragMetrics()
    let dts = zip(samples, samples.dropFirst()).map { ($1.t - $0.t) * 1000 }
    metrics.sampleP95Ms = percentile(dts, 0.95)
    metrics.postP95Ms = percentile(log.map { $0.cost * 1000 }, 0.95)
    metrics.postLateP95Ms = percentile(log.map { $0.late * 1000 }, 0.95)
    metrics.postLateMaxMs = log.map { $0.late * 1000 }.max() ?? .nan
    func pointer(at t: Double) -> CGPoint {
        var low = 0, high = log.count
        while low < high {
            let mid = (low + high) / 2
            if log[mid].t <= t { low = mid + 1 } else { high = mid }
        }
        return low == 0 ? press : log[low - 1].point
    }
    func step(_ from: Double, _ to: Double, lag: Double) -> CGVector {
        let a = pointer(at: from - lag), b = pointer(at: to - lag)
        return CGVector(dx: b.x - a.x, dy: b.y - a.y)
    }
    func gap(_ a: CGVector, _ b: CGVector) -> Double { Double(hypot(a.dx - b.dx, a.dy - b.dy)) }
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
        let e = percentile(moving.compactMap { i -> Double? in
            guard samples[i].source == .floating, let tab = samples[i].tab else { return nil }
            return distance(tab, expected(pointer(at: samples[i].t - ms / 1000)))
        }, 0.5)
        if e < best.error { best = (ms / 1000, e) }
    }
    let lag = best.error.isFinite ? best.lag : 0
    metrics.lagMs = lag * 1000

    // Displayed frames: each change of the tab's position, seen first at sample `i`.
    // A frame shows some recent pointer event: the one whose "pointer - grab" is
    // nearest, among the last seven posted (58 ms) before the frame was seen. A
    // jump is a change of the residual (tab minus that event's position) between
    // frames. Unlike a fixed time lag this does not depend on where 60 Hz frames
    // fall between 120 Hz events, so a tab that shows events exactly scores ~0.
    var frames: [(t: Double, tab: CGPoint, source: TabSource)] = []
    var previous: Int?
    for i in samples.indices where samples[i].phase <= legs {
        guard let tab = samples[i].tab else { continue }
        if let j = previous, let old = samples[j].tab, old != tab, i >= first {
            if frames.isEmpty { frames.append((samples[j].t, old, samples[j].source)) }
            frames.append((samples[i].t, tab, samples[i].source))
        }
        previous = i
    }
    var residuals: [CGVector] = []
    var latencies: [Double] = []
    for frame in frames {
        var low = 0, high = log.count
        while low < high {
            let mid = (low + high) / 2
            if log[mid].t <= frame.t { low = mid + 1 } else { high = mid }
        }
        var best: (d: Double, e: CGVector, k: Int)?
        for k in max(0, low - 7)..<low {
            let e = CGVector(dx: frame.tab.x - (log[k].point.x - grab.dx), dy: frame.tab.y - (log[k].point.y - grab.dy))
            let d = Double(hypot(e.dx, e.dy))
            if best.map({ d < $0.d }) ?? true { best = (d, e, k) }
        }
        let fallback = CGVector(dx: frame.tab.x - (press.x - grab.dx), dy: frame.tab.y - (press.y - grab.dy))
        residuals.append(best?.e ?? fallback)
        if let best { latencies.append((frame.t - log[best.k].t) * 1000) }
    }
    let frameJumps = zip(residuals, residuals.dropFirst()).map { gap($1, $0) }
    metrics.maxJump = frameJumps.max() ?? .nan
    metrics.frameLatencyMs = percentile(latencies, 0.5)
    if let entry = frames.indices.first(where: { $0 > 0 && frames[$0].source == .floating }) {
        metrics.entryJump = gap(residuals[entry], residuals[entry - 1])
        metrics.entryShift = distance(frames[entry].tab, frames[entry - 1].tab)
    }
    let intervals = zip(frames, frames.dropFirst()).map { ($1.t - $0.t) * 1000 }
    metrics.frameMs = percentile(intervals, 0.5)
    if !intervals.isEmpty {
        metrics.lateFrameShare = Double(intervals.filter { $0 > metrics.frameMs * 1.5 }.count) / Double(intervals.count)
    }

    // The literal 120 Hz definition: every other sample, neighbour to neighbour, on
    // the samples that also read the strip (the sampler reads AX on odd samples).
    let grid = moving.filter { $0 % 2 == 1 }
    var jumps: [Double] = []
    var raw: [Double] = []
    var stalls = 0
    var moves = 0
    for (a, b) in zip(grid, grid.dropFirst()) where b == a + 2 {
        guard let ta = samples[a].tab, let tb = samples[b].tab else { continue }
        let moved = CGVector(dx: tb.x - ta.x, dy: tb.y - ta.y)
        let led = step(samples[a].t, samples[b].t, lag: lag)
        jumps.append(gap(moved, led))
        raw.append(gap(moved, step(samples[a].t, samples[b].t, lag: 0)))
        if hypot(led.dx, led.dy) >= 1 {
            moves += 1
            if hypot(moved.dx, moved.dy) < 0.5 { stalls += 1 }
        }
    }
    metrics.maxJump120 = jumps.max() ?? .nan
    metrics.maxJumpRaw = raw.max() ?? .nan
    metrics.stallShare = moves == 0 ? 0 : Double(stalls) / Double(moves)

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

/// The footprint once it stops moving (three reads 250 ms apart within 128 KiB),
/// or the last read after eight seconds. `series` gets every read, in MiB.
private func settledFootprint(_ pid: pid_t, series: inout [String]) -> Int64 {
    var reads = [Int64(footprintBytes(pid) ?? 0)]
    for _ in 0..<32 {
        usleep(250_000)
        reads.append(Int64(footprintBytes(pid) ?? 0))
        let tail = reads.suffix(3)
        if tail.count == 3, tail.max()! - tail.min()! < 128 * 1024 { break }
    }
    series = reads.map { String(format: "%.2f", Double($0) / 1_048_576) }
    return reads.last!
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
    let (samples, log, realtime) = runDrag(pid: pid, press: press,
                                           legs: [Leg(to: out, ticks: 180), Leg(to: back, ticks: backTicks)],
                                           stripTab: tabElement(window, named: order[1] + "."))
    var metrics = measure(samples, log: log, press: press, legs: 2, grab: grab)
    metrics.realtime = realtime
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
    var series: [String] = []
    result.footprintLaunch = settledFootprint(pid, series: &series)

    // 0. Warm-up: the first drag in a process creates the drag window.
    if let (_, expected) = measuredDrag(window, pid: pid, label: "\(name)-round\(round)-warmup") {
        _ = waitUntil(3) { labels(window) == expected }
    } else {
        verdict(false, "three tabs before the warm-up drag", detail: tabSummary(window))
    }
    sleep(seconds: 0.3)
    result.footprintBefore = settledFootprint(pid, series: &series)
    report.info("footprint settle [\(name)] after warm-up", detail: series.joined(separator: " "))

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
            report.check(metrics.maxJump <= maxJumpLimit,
                         "[\(name)] rep \(rep): largest jump between displayed frames <= \(maxJumpLimit) points",
                         detail: String(format: "%.2f per frame (%.1f ms frames, %.0f ms from event to frame); per 120 Hz sample %.2f",
                                        metrics.maxJump, metrics.frameMs, metrics.frameLatencyMs, metrics.maxJump120))
        }
        sleep(seconds: 0.3)
    }
    result.footprintAfter = settledFootprint(pid, series: &series)
    report.info("footprint settle [\(name)] after 3 drags", detail: series.joined(separator: " "))
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
            let (samples, _, _) = runDrag(pid: pid, press: press,
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
    result.footprintEnd = settledFootprint(pid, series: &series)
    if report.failures > 0 && judged { dumpOnFailure(report, pid: pid, label: "tabdrag-\(name)") }
    killVelocity(at: launcher)
    return result
}

func scenarioTabDrag(_ options: ArraySlice<String>) -> Never {
    let report = Report("tabdrag")
    // No App Nap or timer coalescing for the harness while it keeps 120 Hz deadlines.
    // Held until the process exits.
    napGuard = ProcessInfo.processInfo.beginActivity(
        options: [.userInitiated, .latencyCritical, .idleSystemSleepDisabled], reason: "tab drag timing")
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
            format: "reps=%d err_med=%.2f err_p95=%.2f max_jump_frame(median)=%.2f max_jump_frame(worst)=%.2f max_jump_120hz(median)=%.2f max_jump_120hz(worst)=%.2f entry_jump(median)=%.2f entry_jump(worst)=%.2f stall_120hz=%.1f%% frame_ms=%.1f late_frames=%.1f%% frame_latency_ms=%.1f floating=%.1f%%",
            reps.count, median(\.errorMedian), median(\.errorP95), median(\.maxJump), worst(\.maxJump),
            median(\.maxJump120), worst(\.maxJump120), median(\.entryJump), worst(\.entryJump),
            median(\.stallShare) * 100, median(\.frameMs), median(\.lateFrameShare) * 100,
            median(\.frameLatencyMs), median(\.floatingShare) * 100)
            + " footprint launch=" + mine.map { mib($0.footprintLaunch) }.joined(separator: "/")
            + " before=" + mine.map { mib($0.footprintBefore) }.joined(separator: "/")
            + " after=" + mine.map { mib($0.footprintAfter) }.joined(separator: "/")
            + " grown=" + grown.joined(separator: "/")
            + " end(2+ windows)=" + mine.map { mib($0.footprintEnd) }.joined(separator: "/") + " MiB")
    }
    report.finish()
}
