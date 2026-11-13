// Clear-only native lower bound, adapted from the MIT/Apache P0 floor.swift.
// Unlike P0's console marks, record drawable.presentedTime as well as receipt.
import AppKit
import Metal
import QuartzCore
let velocityFloorEntry = CACurrentMediaTime()
let velocityFloorWall = Date().timeIntervalSince1970
var velocityFloorMarks: [[String: Any]] = []
func mark(_ phase: String) {
    velocityFloorMarks.append(["phase": phase, "main_ms": (CACurrentMediaTime() - velocityFloorEntry) * 1000])
}
func emit(_ record: [String: Any]) {
    let bytes = try! JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
    print(String(data: bytes, encoding: .utf8)!)
}
var velocityFloorInfo = kinfo_proc()
var velocityFloorSize = MemoryLayout<kinfo_proc>.size
var velocityFloorMib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
let velocityFloorBirthOK = sysctl(&velocityFloorMib, 4, &velocityFloorInfo, &velocityFloorSize, nil, 0) == 0
let velocityFloorStart = velocityFloorInfo.kp_proc.p_un.__p_starttime
let velocityFloorPremain: Double? = velocityFloorBirthOK ? (velocityFloorWall - Double(velocityFloorStart.tv_sec) - Double(velocityFloorStart.tv_usec) / 1e6) * 1000 : nil
DispatchQueue.global().asyncAfter(deadline: .now() + 30) {
    emit(["schema": 1, "kind": "error", "complete": false, "message": "native floor watchdog expired"])
    exit(2)
}
class VelocityFloorDelegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var layer: CAMetalLayer!
    var queue: MTLCommandQueue!
    func applicationDidFinishLaunching(_ notification: Notification) {
        mark("did_finish_launching")
        window = NSWindow(contentRect: NSRect(x: 160, y: 160, width: 1200, height: 800), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        let view = NSView(frame: NSRect(origin: .zero, size: window.contentLayoutRect.size))
        layer = CAMetalLayer()
        let device = MTLCreateSystemDefaultDevice()!
        queue = device.makeCommandQueue()!
        layer.device = device
        layer.pixelFormat = .bgra8Unorm
        layer.contentsScale = window.backingScaleFactor
        layer.drawableSize = CGSize(width: 1200 * window.backingScaleFactor, height: 800 * window.backingScaleFactor)
        layer.displaySyncEnabled = true
        view.layer = layer
        view.wantsLayer = true
        window.contentView = view
        mark("window_layer_ready")
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        mark("ordered_front")
        guard let drawable = layer.nextDrawable() else {
            emit(["schema": 1, "kind": "error", "complete": false, "message": "no drawable"])
            exit(1)
        }
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = drawable.texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0.075, green: 0.085, blue: 0.105, alpha: 1)
        let commands = queue.makeCommandBuffer()!
        commands.makeRenderCommandEncoder(descriptor: pass)!.endEncoding()
        drawable.addPresentedHandler { presented in
            let actualScanout = presented.presentedTime
            DispatchQueue.main.async {
                let callback = CACurrentMediaTime()
                guard actualScanout > 0 else {
                    emit(["schema": 1, "kind": "error", "complete": false, "message": "drawable skipped"])
                    exit(1)
                }
                var record: [String: Any] = ["schema": 1, "kind": "native_floor", "mode": "native_floor", "complete": true,
                    "scanout_main_ms": (actualScanout - velocityFloorEntry) * 1000,
                    "presented_callback_main_ms": (callback - velocityFloorEntry) * 1000,
                    "scanout_to_callback_ms": (callback - actualScanout) * 1000,
                    "milestones": velocityFloorMarks, "workload": "clear only; no fonts, file, atlas, shaders or pipeline",
                    "boundary": "MTLDrawable.presentedTime (same timebase as CACurrentMediaTime)"]
                if let premain = velocityFloorPremain {
                    record["before_main_ms"] = premain
                    record["scanout_process_ms"] = premain + (actualScanout - velocityFloorEntry) * 1000
                }
                emit(record)
                exit(0)
            }
        }
        commands.present(drawable)
        commands.commit()
        mark("present_returned")
    }
}
let velocityFloorApplication = NSApplication.shared
let velocityFloorDelegate = VelocityFloorDelegate()
velocityFloorApplication.delegate = velocityFloorDelegate
velocityFloorApplication.setActivationPolicy(.regular)
mark("nsapplication_ready")
velocityFloorApplication.run()
