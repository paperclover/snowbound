import UIKit

/// Runs the library's file access under `NSFileCoordinator`, so file providers such as
/// iCloud Drive download before it reads and learn of what it writes.
private let coordinate: sb_coordinator = { path, write, body, context in
    guard let path, let body else { return }
    let url = URL(fileURLWithPath: String(cString: path))
    var error: NSError?
    if write {
        NSFileCoordinator().coordinate(writingItemAt: url, options: .forMerging, error: &error) { _ in body(context) }
    } else {
        NSFileCoordinator().coordinate(readingItemAt: url, options: [], error: &error) { _ in body(context) }
    }
}

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(
        _ application: UIApplication, didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        sb_set_coordinator(coordinate)
        #if DEBUG
        Presenter.watch()
        #endif
        return true
    }

    func application(
        _ application: UIApplication,
        configurationForConnecting session: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        let configuration = UISceneConfiguration(name: nil, sessionRole: session.role)
        configuration.delegateClass = SceneDelegate.self
        return configuration
    }

    /// Edits are in the cache once stored; this gives publishing a moment before the app goes.
    func applicationWillTerminate(_ application: UIApplication) {
        NotificationCenter.default.post(name: PageViewController.leaving, object: nil)
        for section in Section.all { _ = section.flush(2) }
    }
}

/// Where the reader was, reopened on the next launch as Notes reopens its last note.
private struct Place: Codable {
    let notebook: UUID
    let section: String
    let page: String?

    static let key = "place"

    static var saved: Place? {
        get { UserDefaults.standard.data(forKey: key).flatMap { try? JSONDecoder().decode(Place.self, from: $0) } }
        set { UserDefaults.standard.set(try? JSONEncoder().encode(newValue), forKey: key) }
    }
}

/// Notebooks and sections, pages and a page, as Notes lays out folders, notes and a note:
/// side by side on a wide screen, a navigation stack on a phone.
final class SceneDelegate: UIResponder, UIWindowSceneDelegate, UISplitViewControllerDelegate {
    var window: UIWindow?
    private let split = RootViewController(style: .tripleColumn)
    private let notebooks = NotebooksViewController()
    private let pages = PagesViewController()
    private var showingPage = false
    private var background = UIBackgroundTaskIdentifier.invalid

    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options: UIScene.ConnectionOptions) {
        guard let scene = scene as? UIWindowScene else { return }
        let window = UIWindow(windowScene: scene)
        split.delegate = self
        split.scene = self
        split.preferredDisplayMode = .oneBesideSecondary
        split.preferredSplitBehavior = .tile
        split.setViewController(notebooks, for: .primary)
        split.setViewController(pages, for: .supplementary)
        split.setViewController(UIViewController(), for: .secondary)
        notebooks.onOpen = { [weak self] tab, notebook in self?.open(tab, of: notebook) }
        pages.onOpen = { [weak self] section, id, find in self?.show(section: section, page: id, find: find) }
        pages.onOpenSection = { [weak self] notebook, path, id, find in
            guard let tab = notebook.tabs.first(where: { $0.path == path }) else { return }
            self?.open(tab, of: notebook, page: id, find: find)
        }
        window.rootViewController = split
        window.makeKeyAndVisible()
        self.window = window
        Notebooks.load()
        notebooks.reload()
        for notebook in Notebooks.all {
            notebook.open { [weak self] in
                self?.notebooks.reload()
                self?.restore(notebook)
            }
        }
    }

    /// Reopens the section and page shown last, or the scripted one.
    private func restore(_ notebook: Notebook) {
        let environment = ProcessInfo.processInfo.environment
        if let index = environment["SNOWBOUND_SECTION"].flatMap(Int.init) {
            let readable = notebook.tabs.filter(\.readable)
            guard index < readable.count else { return }
            open(readable[index], of: notebook) { [weak self] section in
                guard let page = environment["SNOWBOUND_PAGE"].flatMap(Int.init), page < section.rows.count else {
                    return
                }
                self?.show(section: section, page: section.rows[page].id)
            }
            return
        }
        guard let place = Place.saved, place.notebook == notebook.id,
            let tab = notebook.tabs.first(where: { $0.path == place.section })
        else { return }
        open(tab, of: notebook, page: place.page)
    }

    func open(
        _ tab: Tab, of notebook: Notebook, page: String? = nil, find: String? = nil,
        then: ((Section) -> Void)? = nil
    ) {
        guard Author.name != nil else {
            return Author.ask(from: split) { [weak self] in self?.open(tab, of: notebook, page: page, find: find, then: then) }
        }
        pages.loading(tab)
        split.show(.supplementary)
        Section.open(tab, of: notebook) { [weak self] section, problem in
            guard let self else { return }
            guard let section else {
                pages.failed(tab, problem)
                return
            }
            pages.load(section)
            notebooks.select(tab, of: notebook)
            Place.saved = Place(notebook: notebook.id, section: tab.path, page: page)
            if let page, section.row(of: page) != nil {
                show(section: section, page: page, find: find)
            }
            then?(section)
        }
    }

    func show(section: Section, page: String, find: String? = nil, titleFocus: Bool = false) {
        showingPage = true
        let controller = PageViewController(section: section, page: page, find: find, titleFocus: titleFocus)
        controller.onOpen = { [weak self] id in self?.show(section: section, page: id) }
        split.setViewController(UINavigationController(rootViewController: controller), for: .secondary)
        split.show(.secondary)
        pages.select(page)
        Place.saved = Place(notebook: section.notebook.id, section: section.tab.path, page: page)
    }

    /// Adds a page to the open section and opens it, its title ready for typing.
    func newPage(subpage: Bool = false) {
        guard let section = pages.section else { return }
        let parent = subpage ? (Place.saved?.page).flatMap { section.row(of: $0)?.row.id } : nil
        guard let id = section.newPage(under: parent) else { return }
        show(section: section, page: id, titleFocus: true)
    }

    func search() {
        split.show(.supplementary)
        pages.startSearching()
    }

    /// A phone opens on the notebooks, as Notes opens on its folders.
    func splitViewController(
        _ svc: UISplitViewController, topColumnForCollapsingToProposedTopColumn proposedTopColumn: UISplitViewController.Column
    ) -> UISplitViewController.Column {
        showingPage ? proposedTopColumn : .primary
    }

    /// Composition ends and every edit is stored and published while the system allows.
    func sceneDidEnterBackground(_ scene: UIScene) {
        NotificationCenter.default.post(name: PageViewController.leaving, object: nil)
        let sections = Section.all
        guard !sections.isEmpty, background == .invalid else { return }
        background = UIApplication.shared.beginBackgroundTask(withName: "Saving") { [weak self] in
            self?.finishBackground()
        }
        DispatchQueue.global(qos: .userInitiated).async {
            for section in sections { _ = section.flush(20) }
            DispatchQueue.main.async { [weak self] in self?.finishBackground() }
        }
    }

    private func finishBackground() {
        guard background != .invalid else { return }
        UIApplication.shared.endBackgroundTask(background)
        background = .invalid
    }

    /// Changes made elsewhere while away show at once.
    func sceneWillEnterForeground(_ scene: UIScene) {
        for section in Section.all { section.wake() }
    }
}

/// Takes the app's keyboard shortcuts wherever focus is.
final class RootViewController: UISplitViewController {
    weak var scene: SceneDelegate?

    override var keyCommands: [UIKeyCommand]? {
        [
            UIKeyCommand(title: "New Page", action: #selector(newPage), input: "n", modifierFlags: .command),
            UIKeyCommand(
                title: "New Subpage", action: #selector(newSubpage), input: "n", modifierFlags: [.command, .shift]),
            UIKeyCommand(title: "Search", action: #selector(search), input: "f", modifierFlags: [.command, .alternate]),
        ]
    }

    @objc private func newPage() { scene?.newPage() }
    @objc private func newSubpage() { scene?.newPage(subpage: true) }
    @objc private func search() { scene?.search() }
}
