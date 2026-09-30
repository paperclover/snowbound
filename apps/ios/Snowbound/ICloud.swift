import Foundation

/// Lists the conflict versions iCloud Drive keeps beside a section file, as it keeps another
/// device's commit that lost; the library merges each into the file.
private let listVersions: sb_versions = { path, found, context in
    guard let path, let found else { return }
    let url = URL(fileURLWithPath: String(cString: path))
    for version in NSFileVersion.unresolvedConflictVersionsOfItem(at: url) ?? [] {
        version.url.path.withCString { id in
            guard let device = version.localizedNameOfSavingComputer else { return found(context, id, nil) }
            device.withCString { found(context, id, $0) }
        }
    }
}

/// Resolves and removes a version the library merged, first keeping it beside the file as
/// “Name (Device).one” when it is another section. A version already gone, as another device
/// resolved it, is done.
private let retireVersion: sb_retire = { path, id, keep in
    guard let path, let id else { return false }
    let url = URL(fileURLWithPath: String(cString: path))
    let named = String(cString: id)
    guard let version = NSFileVersion.unresolvedConflictVersionsOfItem(at: url)?.first(where: { $0.url.path == named })
    else { return true }
    var retired = false
    var error: NSError?
    NSFileCoordinator().coordinate(writingItemAt: url, options: .forMerging, error: &error) { url in
        if keep {
            let device = version.localizedNameOfSavingComputer ?? "another device"
            let stem = url.deletingPathExtension().lastPathComponent
            let folder = url.deletingLastPathComponent()
            let kept = (1...).lazy.map { n in
                folder.appendingPathComponent(n == 1 ? "\(stem) (\(device)).one" : "\(stem) (\(device) \(n)).one")
            }.first { !FileManager.default.fileExists(atPath: $0.path) }
            guard let kept, (try? FileManager.default.copyItem(at: version.url, to: kept)) != nil else { return }
        }
        version.isResolved = true
        retired = (try? version.remove()) != nil
    }
    return retired
}

/// Snowbound's folder in iCloud Drive: its container's Documents, which Files shows as iCloud
/// Drive's Snowbound folder.
enum ICloud {
    /// Sent on the main thread when the folder was looked up again.
    static let changed = Notification.Name("ICloudChanged")
    private static let identifier = "iCloud.net.paperclover.snowbound"

    /// The folder, once looked up; nil while iCloud Drive is off, signed out, or the app lacks
    /// the container.
    private(set) static var documents: URL?

    /// Whether iCloud is signed in with iCloud Drive on, container or not.
    static var signedIn: Bool { FileManager.default.ubiquityIdentityToken != nil }

    /// Has the library merge conflict versions, follows the account, and hears of notebooks
    /// another device adds or removes; call once at launch.
    static func start() {
        sb_set_versions(listVersions, retireVersion)
        NotificationCenter.default.addObserver(
            forName: .NSUbiquityIdentityDidChange, object: nil, queue: .main
        ) { _ in lookUp() }
        lookUp()
        query.searchScopes = [NSMetadataQueryUbiquitousDocumentsScope]
        query.predicate = NSPredicate(format: "%K LIKE '*'", NSMetadataItemFSNameKey)
        for name in [Notification.Name.NSMetadataQueryDidFinishGathering, .NSMetadataQueryDidUpdate] {
            NotificationCenter.default.addObserver(forName: name, object: query, queue: .main) { _ in
                if Notebooks.inCloudChanged { NotificationCenter.default.post(name: changed, object: nil) }
            }
        }
        query.start()
    }

    /// Reports what iCloud Drive syncs of the container, as files come from other devices.
    private static let query = NSMetadataQuery()

    /// Looks the folder up off the main thread, as asking iCloud can take a while.
    static func lookUp() {
        background({ () -> URL? in
            guard
                let container = FileManager.default.url(forUbiquityContainerIdentifier: identifier)?
                    .appendingPathComponent("Documents", isDirectory: true)
            else { return nil }
            try? FileManager.default.createDirectory(at: container, withIntermediateDirectories: true)
            return container
        }) { found in
            documents = found
            NotificationCenter.default.post(name: changed, object: nil)
        }
    }

}
