import AppKit
import Metal
import QuartzCore
let t0 = CACurrentMediaTime()
func ms() -> String { String(format: "%.1f", (CACurrentMediaTime() - t0) * 1000) }
var marks: [String] = []
func mark(_ s: String) { marks.append("\(s)=\(ms())") }
// process start time via sysctl
var kp = kinfo_proc(); var sz = MemoryLayout<kinfo_proc>.size
var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
sysctl(&mib, 4, &kp, &sz, nil, 0)
let st = kp.kp_proc.p_un.__p_starttime
let startEpoch = Double(st.tv_sec) + Double(st.tv_usec) / 1e6
let preMain = (Date().timeIntervalSince1970 - startEpoch) * 1000
class D: NSObject, NSApplicationDelegate {
  var w: NSWindow!; var layer: CAMetalLayer!; var dev: MTLDevice!; var q: MTLCommandQueue!
  func applicationDidFinishLaunching(_ n: Notification) {
    mark("didFinishLaunching")
    w = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 1200, height: 800), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    let v = NSView(frame: w.contentLayoutRect); v.wantsLayer = true
    layer = CAMetalLayer(); dev = MTLCreateSystemDefaultDevice(); q = dev.makeCommandQueue()
    layer.device = dev; layer.pixelFormat = .bgra8Unorm; layer.contentsScale = w.backingScaleFactor
    layer.drawableSize = CGSize(width: 1200 * w.backingScaleFactor, height: 800 * w.backingScaleFactor)
    v.layer = layer; w.contentView = v
    mark("window+layer")
    w.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
    mark("ordered_front")
    guard let d = layer.nextDrawable() else { print("no drawable"); exit(1) }
    mark("drawable")
    let rp = MTLRenderPassDescriptor(); rp.colorAttachments[0].texture = d.texture
    rp.colorAttachments[0].loadAction = .clear; rp.colorAttachments[0].clearColor = MTLClearColor(red: 0.15, green: 0.16, blue: 0.2, alpha: 1)
    let cb = q.makeCommandBuffer()!; cb.makeRenderCommandEncoder(descriptor: rp)!.endEncoding()
    cb.present(d); cb.addCompletedHandler { _ in }
    d.addPresentedHandler { _ in DispatchQueue.main.async { mark("presented(scanout)"); print(String(format: "premain=%.1f ", preMain) + marks.joined(separator: " ")); exit(0) } }
    cb.commit(); mark("committed")
  }
}
let app = NSApplication.shared; let del = D(); app.delegate = del
app.setActivationPolicy(.regular)
mark("nsapp")
app.run()
