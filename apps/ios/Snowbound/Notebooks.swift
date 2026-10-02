import Foundation
import Security
import UIKit

/// A string the Rust library allocated, taken and freed.
func take(_ text: UnsafeMutablePointer<CChar>?) -> String? {
    guard let text else { return nil }
    defer { sb_string_free(text) }
    return String(cString: text)
}

func decode<T: Decodable>(_ type: T.Type, _ json: UnsafeMutablePointer<CChar>?) -> T? {
    take(json).flatMap { try? JSONDecoder().decode(type, from: Data($0.utf8)) }
}

/// A page's title date and time, as OneNote writes them: the long date and short time.
func titleDate(_ date: Date = Date()) -> (date: String, time: String) {
    let formatter = DateFormatter()
    formatter.dateStyle = .full
    formatter.timeStyle = .none
    let day = formatter.string(from: date)
    formatter.dateStyle = .none
    formatter.timeStyle = .short
    return (day, formatter.string(from: date))
}

/// The user name edits are stored under, OneNote's Personalize setting; asked once.
enum Author {
    private static let key = "author"

    static var name: String? {
        get { UserDefaults.standard.string(forKey: key) }
        set {
            UserDefaults.standard.set(newValue, forKey: key)
            // Open sections name it in their edits from now on.
            for section in Section.all { sb_section_set_author(section.handle, newValue ?? "") }
        }
    }

    /// Asks for the user name OneNote shows beside changes, then runs `then`.
    static func ask(from controller: UIViewController, then: @escaping () -> Void) {
        let alert = UIAlertController(
            title: "Personalize",
            message: "OneNote shows your user name beside the pages and changes you make.",
            preferredStyle: .alert)
        let save = UIAlertAction(title: "Continue", style: .default) { [weak alert] _ in
            name = alert?.textFields?.first?.text?.trimmingCharacters(in: .whitespaces)
            then()
        }
        alert.addTextField { field in
            field.text = name
            field.placeholder = "User name"
            field.textContentType = .name
            field.autocapitalizationType = .words
            save.isEnabled = !(field.text ?? "").isEmpty
            NotificationCenter.default.addObserver(
                forName: UITextField.textDidChangeNotification, object: field, queue: .main
            ) { _ in save.isEnabled = !(field.text ?? "").trimmingCharacters(in: .whitespaces).isEmpty }
        }
        alert.addAction(save)
        alert.preferredAction = save
        controller.present(alert, animated: true)
    }
}

/// An SMB share and the folder of a notebook on it.
struct Server: Codable, Equatable {
    /// `host` or `host:port`.
    var host: String
    var share: String
    /// Empty for a guest.
    var user: String
    /// The notebook folder, `/`-separated from the share's top.
    var root: String

    var hostName: String { host.split(separator: ":").first.map(String.init) ?? host }
    var port: Int { host.split(separator: ":").dropFirst().first.flatMap { Int($0) } ?? 445 }
}

/// The password of an SMB account, kept in the Keychain as Files keeps a server's.
enum Keychain {
    private static func query(_ server: Server) -> [String: Any] {
        [
            kSecClass as String: kSecClassInternetPassword,
            kSecAttrServer as String: server.hostName,
            kSecAttrPort as String: server.port,
            kSecAttrProtocol as String: kSecAttrProtocolSMB,
            kSecAttrAccount as String: server.user,
        ]
    }

    static func password(_ server: Server) -> String? {
        var query = query(server)
        query[kSecReturnData as String] = true
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let data = result as? Data
        else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func save(_ password: String, for server: Server) {
        SecItemDelete(query(server) as CFDictionary)
        var item = query(server)
        item[kSecValueData as String] = Data(password.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        SecItemAdd(item as CFDictionary, nil)
    }
}

/// Where a notebook lives, as the notebook list keeps it.
enum Source: Codable, Equatable {
    /// A folder or section file chosen in Files, as a security-scoped bookmark.
    case files(bookmark: Data)
    /// A folder in the app's Documents, which Files shows as Snowbound's.
    case documents(path: String)
    /// A folder in Snowbound's folder in iCloud Drive (`ICloud.documents`).
    case icloud(path: String)
    /// A notebook folder on an SMB share, opened through Snowbound's own client.
    case server(Server)
    /// A folder or section at an absolute path, for scripted runs.
    case path(String)
}

/// A section tab from `sb_library_sections`.
struct Tab: Decodable, Equatable {
    let name: String
    /// The catalog path `sb_section_open` takes.
    let path: String
    /// The section group holding it, `/`-separated; empty at the notebook's top.
    let group: String
    let color: [UInt8]
    let readable: Bool
    /// Password protected and not unlocked: `Notebook.unlock` opens it.
    let locked: Bool
    /// Not on this device yet; `Notebook` asks iCloud Drive for it.
    let downloading: Bool
    /// Why the file could not be read, where retrying or repair may help.
    let problem: String?

    var uiColor: UIColor { UIColor(rgb: color) }
}

extension UIImage {
    /// A notebook's binder, the desktop's art, `side` points square: its cover in `color`, or
    /// the art's own orange.
    static func notebook(_ color: UIColor?, side: CGFloat = 28) -> UIImage {
        let frame = CGRect(x: 0, y: 0, width: side, height: side)
        let cover = color ?? UIColor(rgb: [0xf3, 0x9c, 0x28])
        return UIGraphicsImageRenderer(size: frame.size).image { _ in
            UIImage(named: "NotebookBack")?.draw(in: frame)
            UIImage(named: "NotebookCover")?.withTintColor(cover, renderingMode: .alwaysOriginal).draw(in: frame)
            UIImage(named: "Notebook")?.draw(in: frame)
        }.withRenderingMode(.alwaysOriginal)
    }
}

extension UIViewController {
    /// Tells the reader `title` and `message` in an alert they dismiss with OK.
    func alert(_ title: String, _ message: String) {
        let alert = UIAlertController(title: title, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default))
        present(alert, animated: true)
    }
}

/// Runs `work` off the main thread, then `done` with its result on it.
func background<T>(_ work: @escaping () -> T, done: @escaping (T) -> Void) {
    DispatchQueue.global(qos: .userInitiated).async {
        let result = work()
        DispatchQueue.main.async { done(result) }
    }
}

/// Where section replicas live: kept, as they hold edits not yet published.
let cacheDirectory: URL = {
    let url = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("Sections", isDirectory: true)
    try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}()

let documentsDirectory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]

/// An open notebook: its Rust library and the sections it lists.
final class Notebook {
    /// A UUID, or `Documents/` and the folder's name for a notebook On My iPhone.
    let id: String
    let source: Source
    private(set) var name: String
    private(set) var handle: OpaquePointer?
    private(set) var tabs: [Tab] = []
    /// The notebook's colour, which its binder's cover takes in the list; none where it has none.
    private(set) var color: UIColor?
    /// Why the notebook could not be opened, while it cannot.
    private(set) var problem: String?
    /// The Files location being read, held open while the notebook is.
    private var accessed: URL?
    /// Reports other apps' changes to a folder on this device to the library.
    private var presenter: FolderPresenter?
    /// Lists the sections again while iCloud Drive downloads some.
    private var recheck: DispatchWorkItem?
    /// Posted on the main thread when the sections list again on their own.
    static let listed = Notification.Name("NotebookListed")

    init(id: String = UUID().uuidString, name: String, source: Source) {
        self.id = id
        self.name = name
        self.source = source
    }

    deinit {
        presenter?.stop()
        if let handle { sb_library_free(handle) }
        accessed?.stopAccessingSecurityScopedResource()
    }

    /// Opens the library off the main thread and lists its sections; `done` runs on the main
    /// thread either way.
    func open(done: @escaping () -> Void) {
        let source = source
        // The library, the Files location held open, why it failed, and a local folder to watch.
        background({ () -> (OpaquePointer?, URL?, String?, URL?) in
            var error: UnsafeMutablePointer<CChar>?
            switch source {
            case .server(let server):
                let library = sb_library_server(
                    server.host, server.share, server.user, Keychain.password(server) ?? "", "", server.root,
                    cacheDirectory.path, &error)
                return (library, nil, take(error), nil)
            case .files(let bookmark):
                var stale = false
                guard let url = try? URL(resolvingBookmarkData: bookmark, bookmarkDataIsStale: &stale) else {
                    return (nil, nil, "The notebook was moved or deleted.", nil)
                }
                let accessed = url.startAccessingSecurityScopedResource() ? url : nil
                let local = onThisDevice(url)
                let (library, problem) = coordinatedOpen(url, local: local)
                return (library, accessed, problem, local ? url : nil)
            case .documents(let path):
                let url = documentsDirectory.appendingPathComponent(path)
                let library = sb_library_open(url.path, cacheDirectory.path, true, &error)
                return (library, nil, take(error), url)
            case .icloud(let path):
                guard let url = ICloud.documents?.appendingPathComponent(path) else {
                    return (nil, nil, "Turn on iCloud Drive in Settings to open this notebook.", nil)
                }
                let (library, problem) = coordinatedOpen(url, local: false)
                return (library, nil, problem, url)
            case .path(let path):
                let url = URL(fileURLWithPath: path)
                let local = onThisDevice(url)
                return (sb_library_open(path, cacheDirectory.path, local, &error), nil, take(error), local ? url : nil)
            }
        }) { [self] (library, accessed, problem, watched) in
            self.accessed?.stopAccessingSecurityScopedResource()
            self.accessed = accessed
            presenter?.stop()
            presenter = nil
            if let handle { sb_library_free(handle) }
            handle = library
            if let library, let watched { presenter = FolderPresenter(watching: watched, for: library) }
            self.problem = library == nil ? (problem ?? "The notebook can’t be read.") : nil
            follow()
            reload(done: done)
        }
    }

    /// Works offline or online, as Work Offline stands.
    func follow() {
        if let handle { sb_library_set_offline(handle, Sync.offline) }
    }

    /// Where the notebook is kept, as the sync sheet names it.
    var location: String {
        switch source {
        case .files: "In Files"
        case .documents: "On My \(UIDevice.current.model), in Snowbound’s folder"
        case .icloud: "In iCloud Drive"
        case .server(let server):
            "smb://\(server.host)/\([server.share, server.root].filter { !$0.isEmpty }.joined(separator: "/"))"
        case .path(let path): path
        }
    }

    /// Unlocks the protected section at `path` with `password` off the main thread, then lists
    /// the sections again; `done` learns whether the password matched.
    func unlock(_ path: String, password: String, done: @escaping (Bool) -> Void) {
        guard let handle else { return done(false) }
        let pointer = Int(bitPattern: handle)
        background({ sb_library_unlock(OpaquePointer(bitPattern: pointer), path, password) }) { [self] unlocked in
            reload { done(unlocked) }
        }
    }

    /// Locks every protected section, as OneNote's Lock All does.
    func lockAll() {
        if let handle { sb_library_lock_all(handle) }
    }

    /// Lists the sections again, off the main thread.
    func reload(done: @escaping () -> Void) {
        guard let handle else {
            tabs = []
            return done()
        }
        let pointer = Int(bitPattern: handle)
        background({ () -> ([Tab]?, Int32) in
            let library = OpaquePointer(bitPattern: pointer)
            return (decode([Tab].self, sb_library_sections(library)), sb_library_color(library))
        }) { [self] (tabs, color) in
            if let tabs { self.tabs = tabs }
            self.color = color < 0 ? nil : UIColor(rgb: [16, 8, 0].map { UInt8(truncatingIfNeeded: color >> $0) })
            download()
            done()
        }
    }

    /// Asks iCloud Drive for the notebook files it keeps elsewhere, which iOS lists as
    /// `.Name.icloud` or dataless under their names, and lists the sections again until they
    /// arrive.
    private func download() {
        let folder: URL? =
            switch source {
            case .files: accessed
            case .documents(let path): documentsDirectory.appendingPathComponent(path)
            case .icloud(let path): ICloud.documents?.appendingPathComponent(path)
            case .path(let path): URL(fileURLWithPath: path)
            case .server: nil
            }
        guard let folder, recheck == nil,
            let files = FileManager.default.enumerator(
                at: folder, includingPropertiesForKeys: [.ubiquitousItemDownloadingStatusKey])
        else { return }
        var waiting = false
        for case let file as URL in files {
            let stub = file.lastPathComponent.hasPrefix(".") && file.pathExtension == "icloud"
            let status = try? file.resourceValues(forKeys: [.ubiquitousItemDownloadingStatusKey])
            guard stub || status?.ubiquitousItemDownloadingStatus == .notDownloaded else { continue }
            let real = file.deletingLastPathComponent().appendingPathComponent(
                String(file.deletingPathExtension().lastPathComponent.dropFirst()))
            try? FileManager.default.startDownloadingUbiquitousItem(at: stub ? real : file)
            waiting = true
        }
        guard waiting else { return }
        let recheck = DispatchWorkItem { [weak self] in
            self?.recheck = nil
            self?.reload { NotificationCenter.default.post(name: Self.listed, object: self) }
        }
        self.recheck = recheck
        DispatchQueue.main.asyncAfter(deadline: .now() + 5, execute: recheck)
    }

    /// Stops hearing other apps' writes while in the background, where a suspended presenter
    /// can deadlock their coordinated writes.
    func pause() { presenter?.stop() }

    /// Hears other apps' writes again, and checks the sections and their list for those
    /// missed meanwhile.
    func resume() {
        presenter?.resume()
        reload { NotificationCenter.default.post(name: Self.listed, object: self) }
    }
}

/// Opens the library at `url` within a coordinated read, which brings a cloud folder's files
/// down before they are read; nil with why where it cannot.
private func coordinatedOpen(_ url: URL, local: Bool) -> (OpaquePointer?, String?) {
    var library: OpaquePointer?
    var error: UnsafeMutablePointer<CChar>?
    var coordination: NSError?
    NSFileCoordinator().coordinate(readingItemAt: url, options: [], error: &coordination) { url in
        library = sb_library_open(url.path, cacheDirectory.path, local, &error)
    }
    return (library, take(error) ?? coordination?.localizedDescription)
}

/// Whether `url` is kept on this device, rather than by a file provider that keeps it elsewhere
/// too, as iCloud Drive does.
private func onThisDevice(_ url: URL) -> Bool {
    let values = try? url.resourceValues(forKeys: [.volumeIsLocalKey, .isUbiquitousItemKey])
    return values?.volumeIsLocal == true && values?.isUbiquitousItem != true
}

/// Tells a library which files of its folder on this device changed, as file coordination
/// reports other apps' writes; the library checks just those sections.
final class FolderPresenter: NSObject, NSFilePresenter {
    let presentedItemURL: URL?
    let presentedItemOperationQueue: OperationQueue = {
        let queue = OperationQueue()
        queue.maxConcurrentOperationCount = 1
        return queue
    }()
    private let root: String
    private let library: OpaquePointer

    init(watching url: URL, for library: OpaquePointer) {
        presentedItemURL = url
        root = url.standardizedFileURL.resolvingSymlinksInPath().path
        self.library = library
        super.init()
        NSFileCoordinator.addFilePresenter(self)
    }

    func resume() {
        NSFileCoordinator.addFilePresenter(self)
        touched("")
    }

    /// Stops reporting, before the library goes or the app leaves the foreground.
    func stop() {
        NSFileCoordinator.removeFilePresenter(self)
        presentedItemOperationQueue.waitUntilAllOperationsAreFinished()
    }

    func presentedItemDidChange() { touched("") }
    func presentedSubitemDidChange(at url: URL) { touched(url) }
    func presentedSubitemDidAppear(at url: URL) { touched(url) }
    /// Another device's commit that lost in iCloud Drive, which changes no file.
    func presentedSubitem(at url: URL, didGain version: NSFileVersion) { touched(url) }
    func presentedSubitem(at oldURL: URL, didMoveTo newURL: URL) {
        touched(oldURL)
        touched(newURL)
    }
    func accommodatePresentedSubitemDeletion(at url: URL, completionHandler: @escaping (Error?) -> Void) {
        touched(url)
        completionHandler(nil)
    }

    private func touched(_ url: URL) {
        let path = url.standardizedFileURL.resolvingSymlinksInPath().path
        touched(path.hasPrefix(root + "/") ? String(path.dropFirst(root.count + 1)) : "")
    }

    private func touched(_ path: String) {
        sb_library_touched(library, path)
    }
}

/// The notebooks the list shows: those in Documents, and those opened from elsewhere, kept
/// across launches.
enum Notebooks {
    private struct Entry: Codable {
        let id: String
        let name: String
        let source: Source
    }

    private static let key = "notebooks"
    private static let hidden = "hidesOnDevice"
    /// The Snowbound Guide's folder, in the app and in Documents once copied there.
    static let guide = "Snowbound Guide"
    /// Whether the list offers the guide; off until Clover has read it through.
    static let guideOffered = false
    private static let scripted = ProcessInfo.processInfo.environment["SNOWBOUND_NOTEBOOK"]
    /// The folders and sections in Documents, as On My iPhone lists them.
    private(set) static var onDevice: [Notebook] = []
    /// The folders in Snowbound's folder in iCloud Drive.
    private(set) static var inCloud: [Notebook] = []
    private(set) static var elsewhere: [Notebook] = []
    static var all: [Notebook] { inCloud + onDevice + elsewhere }

    /// Whether the list shows On My iPhone, as Files lets a location be hidden.
    static var showsOnDevice: Bool {
        get { scripted == nil && !UserDefaults.standard.bool(forKey: hidden) }
        set { UserDefaults.standard.set(!newValue, forKey: hidden) }
    }

    /// The kept notebooks and those in Documents.
    static func load() {
        if let scripted {
            elsewhere = [
                Notebook(
                    name: URL(fileURLWithPath: scripted).deletingPathExtension().lastPathComponent,
                    source: .path(scripted))
            ]
            return
        }
        if let data = UserDefaults.standard.data(forKey: key),
            let entries = try? JSONDecoder().decode([Entry].self, from: data)
        {
            elsewhere = entries.compactMap { entry in
                switch entry.source {
                case .documents, .icloud: return nil
                default: break
                }
                return Notebook(id: entry.id, name: entry.name, source: entry.source)
            }
        } else {
            // The notebook the first version remembered from Files.
            if let bookmark = UserDefaults.standard.data(forKey: "notebook") {
                var stale = false
                let name = (try? URL(resolvingBookmarkData: bookmark, bookmarkDataIsStale: &stale))?
                    .deletingPathExtension().lastPathComponent
                elsewhere.append(Notebook(name: name ?? "Notebook", source: .files(bookmark: bookmark)))
            }
            save()
        }
        scan()
    }

    /// Lists Documents again, keeping the notebooks still there; returns those new to the list.
    @discardableResult
    static func scan() -> [Notebook] {
        let folder = showsOnDevice ? documentsDirectory : nil
        return rescan(folder, "Documents/", &onDevice) { .documents(path: $0) }
    }

    /// Lists Snowbound's folder in iCloud Drive again, as `scan` lists Documents; none while
    /// iCloud Drive is off.
    @discardableResult
    static func scanICloud() -> [Notebook] {
        rescan(scripted == nil ? ICloud.documents : nil, "iCloud/", &inCloud) { .icloud(path: $0) }
    }

    /// Lists `folder`'s notebooks into `list`, keeping those still there and naming new ones by
    /// `prefix` and their `source`; returns the new ones.
    private static func rescan(
        _ folder: URL?, _ prefix: String, _ list: inout [Notebook], source: (String) -> Source
    ) -> [Notebook] {
        guard let folder else {
            list = []
            return []
        }
        var added: [Notebook] = []
        list = notebooks(in: folder).map { [list] name in
            if let kept = list.first(where: { $0.id == prefix + name }) { return kept }
            let notebook = Notebook(
                id: prefix + name, name: (name as NSString).deletingPathExtension, source: source(name))
            added.append(notebook)
            return notebook
        }
        return added
    }

    /// Whether Snowbound's folder in iCloud Drive holds other notebooks than the list shows, as
    /// when another device added or removed one.
    static var inCloudChanged: Bool {
        guard let folder = ICloud.documents, scripted == nil else { return false }
        return notebooks(in: folder).map { "iCloud/" + $0 } != inCloud.map(\.id)
    }

    /// The notebook folders and section files in `folder`, by name, as Files orders them.
    private static func notebooks(in folder: URL) -> [String] {
        let listed = try? FileManager.default.contentsOfDirectory(
            at: folder, includingPropertiesForKeys: [.isDirectoryKey], options: .skipsHiddenFiles)
        return (listed ?? []).filter { url in
            (try? url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
                || url.pathExtension.lowercased() == "one"
        }
        .map(\.lastPathComponent)
        .sorted { $0.localizedStandardCompare($1) == .orderedAscending }
    }

    static func add(_ notebook: Notebook) {
        elsewhere.append(notebook)
        save()
    }

    static func remove(_ notebook: Notebook) {
        elsewhere.removeAll { $0 === notebook }
        save()
    }

    private static func save() {
        guard scripted == nil else { return }
        let entries = elsewhere.map { Entry(id: $0.id, name: $0.name, source: $0.source) }
        UserDefaults.standard.set(try? JSONEncoder().encode(entries), forKey: key)
    }
}

/// A page row from `sb_section_pages`.
struct Row: Decodable, Equatable {
    struct Version: Decodable, Equatable {
        let id: String
        let user: String
        /// Seconds since 1970.
        let created: Double?
    }

    let id: String
    var title: String
    /// 1 at the top, 2 for a subpage and so on.
    let level: Int
    /// Conflict pages: versions with changes a merge could not take.
    let versions: [Version]
}

/// Saving as the section reports it, from `sb_section_poll`.
enum SaveStatus: UInt8 {
    case unknown, saved, saving, offline, notSaving
}

private let wakeSection: sb_wake = { token in
    DispatchQueue.main.async { Section.open[Int(token)]?.value?.poll() }
}

private struct Weak<T: AnyObject> { weak var value: T? }

/// A section open for editing: pages listed and saved through its replica.
final class Section {
    /// Sent on the main thread with `flags` from `sb_section_poll` after each poll.
    static let changed = Notification.Name("SectionChanged")
    static let listed: UInt32 = 1
    static let remote: UInt32 = 2
    static let rejected: UInt32 = 4

    fileprivate static var open: [Int: Weak<Section>] = [:]
    private static var tokens = 0

    let notebook: Notebook
    let tab: Tab
    let handle: OpaquePointer
    private let token: Int
    private(set) var rows: [Row] = []
    private(set) var status = SaveStatus.unknown

    static var all: [Section] { open.values.compactMap(\.value) }

    private init(notebook: Notebook, tab: Tab, handle: OpaquePointer, token: Int) {
        self.notebook = notebook
        self.tab = tab
        self.handle = handle
        self.token = token
        reloadRows()
    }

    deinit {
        Self.open[token] = nil
        sb_section_free(handle)
    }

    /// Opens `tab` of `notebook` off the main thread, or hands back the section already open
    /// there, as its replica takes one owner at a time.
    static func open(_ tab: Tab, of notebook: Notebook, done: @escaping (Section?, String?) -> Void) {
        if let section = all.first(where: { $0.notebook === notebook && $0.tab.path == tab.path }) {
            return done(section, nil)
        }
        guard let library = notebook.handle else { return done(nil, notebook.problem) }
        tokens += 1
        let token = tokens
        let pointer = Int(bitPattern: library)
        let author = Author.name ?? ""
        background({ () -> (Int, String?) in
            var error: UnsafeMutablePointer<CChar>?
            let handle = sb_section_open(
                OpaquePointer(bitPattern: pointer), tab.path, author, wakeSection, UInt(token), &error)
            return (Int(bitPattern: handle), take(error))
        }) { handle, error in
            guard let handle = OpaquePointer(bitPattern: handle) else { return done(nil, error) }
            let section = Section(notebook: notebook, tab: tab, handle: handle, token: token)
            open[token] = Weak(value: section)
            done(section, nil)
        }
    }

    func reloadRows() {
        rows = decode([Row].self, sb_section_pages(handle)) ?? rows
    }

    /// The row listing page or conflict page `id`, and the version when it is one.
    func row(of id: String) -> (row: Row, version: Row.Version?)? {
        for row in rows {
            if row.id == id { return (row, nil) }
            if let version = row.versions.first(where: { $0.id == id }) { return (row, version) }
        }
        return nil
    }

    fileprivate func poll() {
        var status: UInt8 = 0
        let flags = sb_section_poll(handle, &status)
        let changed = SaveStatus(rawValue: status) ?? .unknown
        guard flags != 0 || changed != self.status else { return }
        self.status = changed
        if flags & Self.listed != 0 { reloadRows() }
        NotificationCenter.default.post(name: Self.changed, object: self, userInfo: ["flags": flags])
    }

    /// A page's title as typed, shown in the list before the section reports it.
    func retitle(_ id: String, _ title: String) {
        guard let index = rows.firstIndex(where: { $0.id == id }), rows[index].title != title else { return }
        rows[index].title = title
        NotificationCenter.default.post(name: Self.changed, object: self, userInfo: ["flags": UInt32(0)])
    }

    /// Adds a page, or a subpage of `parent`; returns its id.
    func newPage(under parent: String? = nil) -> String? {
        let (date, time) = titleDate()
        let id = take(sb_section_new_page(handle, parent, date, time))
        reloadRows()
        NotificationCenter.default.post(name: Self.changed, object: self, userInfo: ["flags": Self.listed])
        return id
    }

    func delete(_ id: String, done: @escaping () -> Void = {}) {
        let (date, time) = titleDate()
        let pointer = Int(bitPattern: handle)
        background({ sb_section_delete_page(OpaquePointer(bitPattern: pointer), id, date, time) }) { [self] _ in
            reloadRows()
            NotificationCenter.default.post(name: Self.changed, object: self, userInfo: ["flags": Self.listed])
            done()
        }
    }

    /// The notebook's themes, and the one page `id`, its section and its notebook each name.
    struct Themes: Decodable {
        struct Listed: Decodable {
            let id: String
            let name: String
        }
        let themes: [Listed]
        let page: String?
        let section: String?
        let notebook: String?
    }

    func themes(of id: String) -> Themes? { decode(Themes.self, sb_section_themes(handle, id)) }

    /// Gives `theme`, or none of its own, to page `id` (scope 0), its section (1) or its
    /// notebook (2).
    func setTheme(_ theme: String?, scope: UInt8, of id: String, done: @escaping (Bool) -> Void) {
        let pointer = Int(bitPattern: handle)
        background({ sb_section_set_theme(OpaquePointer(bitPattern: pointer), id, scope, theme) }, done: done)
    }

    /// Stores every edit and publishes them within `seconds`; call off the main thread.
    func flush(_ seconds: Double) -> Bool { sb_section_flush(handle, seconds) }

    func wake() { sb_section_wake(handle) }
}
