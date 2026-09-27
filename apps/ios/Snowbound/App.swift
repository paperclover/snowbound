import UIKit
import UniformTypeIdentifiers

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        configurationForConnecting session: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        let configuration = UISceneConfiguration(name: nil, sessionRole: session.role)
        configuration.delegateClass = SceneDelegate.self
        return configuration
    }
}

/// Notebook, section and page columns as Notes lays out folders, notes and a note: side by
/// side on a wide screen, a navigation stack on a phone.
final class SceneDelegate: UIResponder, UIWindowSceneDelegate, UISplitViewControllerDelegate {
    var window: UIWindow?
    private let split = UISplitViewController(style: .tripleColumn)
    private let sections = SectionsViewController()
    private let pages = PagesViewController()
    private var showingPage = false

    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options: UIScene.ConnectionOptions) {
        guard let scene = scene as? UIWindowScene else { return }
        let window = UIWindow(windowScene: scene)
        split.delegate = self
        split.preferredDisplayMode = .oneBesideSecondary
        split.preferredSplitBehavior = .tile
        split.setViewController(sections, for: .primary)
        split.setViewController(pages, for: .supplementary)
        split.setViewController(UIViewController(), for: .secondary)
        sections.onOpen = { [weak self] section in self?.show(section: section) }
        pages.onOpen = { [weak self] section, page in self?.show(section: section, page: page) }
        window.rootViewController = split
        window.makeKeyAndVisible()
        self.window = window
        sections.load(Library.opening())
        let environment = ProcessInfo.processInfo.environment
        // The split view collapses into a stack once the window is on screen.
        DispatchQueue.main.async { [self] in
            if let index = environment["SNOWBOUND_SECTION"].flatMap(Int.init), let notebook = sections.notebook,
                index < notebook.sections.count
            {
                show(section: notebook.sections[index])
                if let page = environment["SNOWBOUND_PAGE"].flatMap(Int.init), let section = pages.section,
                    page < section.headings.count
                {
                    show(section: section, page: page)
                }
            }
        }
    }

    private func show(section tab: Tab) {
        pages.load(tab)
        split.show(.supplementary)
    }

    private func show(section: Section, page: Int) {
        showingPage = true
        let controller = PageViewController(section: section, page: page)
        split.setViewController(UINavigationController(rootViewController: controller), for: .secondary)
        split.show(.secondary)
    }

    /// A phone opens on the notebook's sections, as Notes opens on its folders.
    func splitViewController(
        _ svc: UISplitViewController, topColumnForCollapsingToProposedTopColumn proposedTopColumn: UISplitViewController.Column
    ) -> UISplitViewController.Column {
        showingPage ? proposedTopColumn : .primary
    }
}

/// A notebook's listing from `sb_notebook`.
struct Notebook: Decodable {
    let name: String
    let sections: [Tab]
}

struct Tab: Decodable {
    let name: String
    let path: String
    /// The section group holding it, `/`-separated; empty at the notebook's top.
    let group: String
    let color: [UInt8]
    let readable: Bool

    var uiColor: UIColor {
        UIColor(red: CGFloat(color[0]) / 255, green: CGFloat(color[1]) / 255, blue: CGFloat(color[2]) / 255, alpha: 1)
    }
}

/// Where the notebook shown comes from: the one last opened from Files, kept as a bookmark,
/// or the bundled sample.
enum Library {
    private static let bookmarkKey = "notebook"
    /// The Files location being read, held open while it is shown.
    private static var accessed: URL?

    static func opening() -> Notebook? {
        if let path = ProcessInfo.processInfo.environment["SNOWBOUND_NOTEBOOK"] { return read(path) }
        var stale = false
        if let data = UserDefaults.standard.data(forKey: bookmarkKey),
            let url = try? URL(resolvingBookmarkData: data, bookmarkDataIsStale: &stale),
            let notebook = open(url)
        {
            return notebook
        }
        // The bundled folder keeps its corpus name.
        return Bundle.main.path(forResource: "notebook", ofType: nil).flatMap(read).map {
            Notebook(name: "Sample", sections: $0.sections)
        }
    }

    /// A notebook folder or section file chosen in Files, remembered for the next launch.
    static func open(_ url: URL) -> Notebook? {
        accessed?.stopAccessingSecurityScopedResource()
        accessed = url.startAccessingSecurityScopedResource() ? url : nil
        guard let notebook = read(url.path) else { return nil }
        if let data = try? url.bookmarkData() { UserDefaults.standard.set(data, forKey: bookmarkKey) }
        return notebook
    }

    private static func read(_ path: String) -> Notebook? {
        guard let json = sb_notebook(path) else { return nil }
        defer { sb_string_free(json) }
        return try? JSONDecoder().decode(Notebook.self, from: Data(String(cString: json).utf8))
    }
}

/// A parsed .one section; pages are read-only copies until saving lands.
final class Section {
    let handle: OpaquePointer?
    let name: String
    let color: UIColor
    /// Each page's title and level in the page list, 1 at the top.
    let headings: [(title: String, level: Int)]

    init(_ tab: Tab) {
        handle = sb_section_open(tab.path)
        name = tab.name
        color = tab.uiColor
        headings = handle.map { handle in
            (0..<sb_section_count(handle)).map {
                (String(cString: sb_section_title(handle, $0)), Int(sb_section_level(handle, $0)))
            }
        } ?? []
    }

    deinit { if let handle { sb_section_free(handle) } }
}

final class SectionsViewController: UITableViewController, UIDocumentPickerDelegate {
    private(set) var notebook: Notebook?
    /// Sections by group, in the notebook's order.
    private var groups: [(name: String, sections: [Tab])] = []
    var onOpen: ((Tab) -> Void)?

    init() {
        super.init(style: .insetGrouped)
        navigationItem.largeTitleDisplayMode = .always
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "section")
        let open = UIBarButtonItem(
            title: "Open Notebook", image: UIImage(systemName: "folder"), target: self, action: #selector(open))
        navigationItem.rightBarButtonItem = open
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        navigationController?.navigationBar.prefersLargeTitles = true
    }

    func load(_ notebook: Notebook?) {
        self.notebook = notebook
        title = notebook?.name
        var groups: [(name: String, sections: [Tab])] = []
        for tab in notebook?.sections ?? [] {
            if groups.last?.name == tab.group {
                groups[groups.count - 1].sections.append(tab)
            } else {
                groups.append((tab.group, [tab]))
            }
        }
        self.groups = groups
        tableView.reloadData()
        var empty = UIContentUnavailableConfiguration.empty()
        empty.text = notebook == nil ? "No Notebook Open" : "No Sections"
        empty.secondaryText = notebook == nil ? "Open a notebook folder or section from Files." : nil
        contentUnavailableConfiguration = groups.isEmpty ? empty : nil
    }

    @objc private func open() {
        let picker = UIDocumentPickerViewController(
            forOpeningContentTypes: [.folder, UTType(filenameExtension: "one") ?? .data])
        picker.delegate = self
        present(picker, animated: true)
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard let url = urls.first else { return }
        guard let notebook = Library.open(url) else {
            let alert = UIAlertController(
                title: "Can’t Open Notebook", message: "Choose a OneNote notebook folder or a section file.",
                preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "OK", style: .default))
            present(alert, animated: true)
            return
        }
        load(notebook)
    }

    override func numberOfSections(in tableView: UITableView) -> Int { groups.count }

    override func tableView(_ tableView: UITableView, titleForHeaderInSection section: Int) -> String? {
        groups[section].name.isEmpty ? nil : groups[section].name.replacingOccurrences(of: "/", with: " › ")
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        groups[section].sections.count
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let tab = groups[indexPath.section].sections[indexPath.row]
        let cell = tableView.dequeueReusableCell(withIdentifier: "section", for: indexPath)
        var content = cell.defaultContentConfiguration()
        content.text = tab.name
        content.image = UIImage(systemName: tab.readable ? "rectangle.portrait.fill" : "lock.fill")
        content.imageProperties.tintColor = tab.uiColor
        if !tab.readable {
            content.secondaryText = "Can’t be opened here"
            content.textProperties.color = .secondaryLabel
        }
        cell.contentConfiguration = content
        cell.accessoryType = tab.readable ? .disclosureIndicator : .none
        cell.selectionStyle = tab.readable ? .default : .none
        return cell
    }

    override func tableView(_ tableView: UITableView, willSelectRowAt indexPath: IndexPath) -> IndexPath? {
        groups[indexPath.section].sections[indexPath.row].readable ? indexPath : nil
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        onOpen?(groups[indexPath.section].sections[indexPath.row])
    }
}

final class PagesViewController: UITableViewController {
    private(set) var section: Section?
    var onOpen: ((Section, Int) -> Void)?

    init() {
        super.init(style: .plain)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "page")
    }

    func load(_ tab: Tab) {
        let section = Section(tab)
        self.section = section
        title = section.name
        tableView.reloadData()
        var empty = UIContentUnavailableConfiguration.empty()
        empty.text = section.handle == nil ? "Can’t Open Section" : "No Pages"
        contentUnavailableConfiguration = section.headings.isEmpty ? empty : nil
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        self.section?.headings.count ?? 0
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let heading = section!.headings[indexPath.row]
        let cell = tableView.dequeueReusableCell(withIdentifier: "page", for: indexPath)
        var content = cell.defaultContentConfiguration()
        content.text = heading.title.isEmpty ? "Untitled Page" : heading.title
        if heading.level > 1 { content.textProperties.color = .secondaryLabel }
        // Subpages sit under their page, indented as in OneNote's page list.
        content.directionalLayoutMargins.leading += CGFloat(heading.level - 1) * 20
        cell.contentConfiguration = content
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        guard let section else { return }
        onOpen?(section, indexPath.row)
    }
}

final class PageViewController: UIViewController {
    private let section: Section
    private let page: Int
    private lazy var canvas = CanvasView(section: section, page: page)
    private lazy var done = UIBarButtonItem(
        systemItem: .done, primaryAction: UIAction { [weak self] _ in _ = self?.canvas.resignFirstResponder() })
    private lazy var undo = UIBarButtonItem(
        systemItem: .undo, primaryAction: UIAction { [weak self] _ in self?.canvas.undo(redo: false) })
    private lazy var redo = UIBarButtonItem(
        systemItem: .redo, primaryAction: UIAction { [weak self] _ in self?.canvas.undo(redo: true) })

    init(section: Section, page: Int) {
        self.section = section
        self.page = page
        super.init(nibName: nil, bundle: nil)
        title = section.headings[page].title
        navigationItem.largeTitleDisplayMode = .never
    }

    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        view = canvas
        canvas.onChange = { [weak self] in self?.editingChanged() }
    }

    /// Notes' checkmark ends editing and puts the keyboard away; undo and redo sit beside it.
    private func editingChanged() {
        guard canvas.isFirstResponder else {
            navigationItem.rightBarButtonItems = nil
            return
        }
        undo.isEnabled = canvas.canUndo(redo: false)
        redo.isEnabled = canvas.canUndo(redo: true)
        if navigationItem.rightBarButtonItems?.first !== done {
            navigationItem.rightBarButtonItems = [done, redo, undo]
        }
    }
}
