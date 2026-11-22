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

    /// The folder the new file goes into: the selected folder when exactly one folder is
    /// selected, otherwise the parent of the first selected item, otherwise the folder whose
    /// background was clicked.
    private func targetDirectory() -> URL? {
        let controller = FIFinderSyncController.default()
        if let selection = controller.selectedItemURLs(), let first = selection.first, first.isFileURL {
            if selection.count == 1 && (first.hasDirectoryPath
                || (try? first.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true) {
                return first
            }
            return first.deletingLastPathComponent()
        }
        return controller.targetedURL().flatMap { $0.isFileURL ? $0 : nil }
    }

    override func menu(for kind: FIMenuKind) -> NSMenu? {
        guard kind == .contextualMenuForContainer || kind == .contextualMenuForItems,
              targetDirectory() != nil else { return nil }
        let menu = NSMenu()
        let item = NSMenuItem(title: NSLocalizedString("New Text File", comment: "Finder context menu"),
                              action: #selector(newTextFile(_:)), keyEquivalent: "")
        item.target = self
        menu.addItem(item)
        return menu
    }

    @objc private func newTextFile(_ item: NSMenuItem) {
        // Finder copies the menu into its own process, so the item carries no
        // representedObject here; ask the controller for the target again.
        guard let directory = targetDirectory() else {
            logger.error("No target folder for New Text File")
            return
        }
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
