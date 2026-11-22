import AppKit
import FinderSync
import os

@objc(VelocityFinderNewFile)
final class FinderNewFile: FIFinderSync {
    private let logger = Logger(subsystem: "dev.velocity.Velocity.FinderNewFile", category: "Finder")

    override init() {
        super.init()
        FIFinderSyncController.default().directoryURLs = [URL(fileURLWithPath: "/")]
    }

    override func menu(for kind: FIMenuKind) -> NSMenu? {
        let controller = FIFinderSyncController.default()
        let directory: URL?
        switch kind {
        case .contextualMenuForContainer:
            directory = controller.targetedURL()
        case .contextualMenuForItems:
            guard let selection = controller.selectedItemURLs(),
                  let first = selection.first, first.isFileURL else { return nil }
            if selection.count == 1 && (first.hasDirectoryPath
                || (try? first.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true) {
                directory = first
            } else {
                directory = first.deletingLastPathComponent()
            }
        default:
            return nil
        }
        guard let directory, directory.isFileURL else { return nil }

        let menu = NSMenu()
        let item = NSMenuItem(title: NSLocalizedString("New Text File", comment: "Finder context menu"),
                              action: #selector(newTextFile(_:)), keyEquivalent: "")
        item.target = self
        item.representedObject = directory
        menu.addItem(item)
        return menu
    }

    @objc private func newTextFile(_ item: NSMenuItem) {
        guard let directory = item.representedObject as? URL else { return }
        var request = URLComponents()
        request.scheme = "velocity"
        request.host = "new-text-file"
        request.queryItems = [URLQueryItem(name: "dir", value: directory.path)]
        guard let url = request.url else { return }

        let plugins = Bundle.main.bundleURL.deletingLastPathComponent()
        let contents = plugins.deletingLastPathComponent()
        let app = contents.deletingLastPathComponent()
        guard plugins.lastPathComponent == "PlugIns", contents.lastPathComponent == "Contents",
              app.pathExtension == "app" else { return }

        let configuration = NSWorkspace.OpenConfiguration()
        configuration.allowsRunningApplicationSubstitution = false
        configuration.promptsUserIfNeeded = false
        configuration.addsToRecentItems = false
        NSWorkspace.shared.open([url], withApplicationAt: app, configuration: configuration) { _, error in
            if let error {
                self.logger.error("Could not open Velocity: \(error.localizedDescription)")
            }
        }
    }
}
