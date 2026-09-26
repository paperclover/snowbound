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

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?

    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options: UIScene.ConnectionOptions) {
        guard let scene = scene as? UIWindowScene else { return }
        let window = UIWindow(windowScene: scene)
        let path = ProcessInfo.processInfo.environment["SNOWBOUND_SECTION"]
            ?? Bundle.main.path(forResource: "Features", ofType: "one")!
        let pages = PagesViewController(section: Section(path: path))
        window.rootViewController = UINavigationController(rootViewController: pages)
        window.makeKeyAndVisible()
        self.window = window
        if let page = ProcessInfo.processInfo.environment["SNOWBOUND_PAGE"].flatMap(Int.init) {
            pages.show(page: page, animated: false)
        }
    }
}

/// A parsed .one section; pages are read-only copies until saving lands.
final class Section {
    let handle: OpaquePointer?
    let name: String

    init(path: String) {
        handle = sb_section_open(path)
        name = (path as NSString).lastPathComponent.replacingOccurrences(of: ".one", with: "")
    }

    deinit { if let handle { sb_section_free(handle) } }

    var titles: [String] {
        guard let handle else { return [] }
        return (0..<sb_section_count(handle)).map { String(cString: sb_section_title(handle, $0)) }
    }
}

final class PagesViewController: UITableViewController, UIDocumentPickerDelegate {
    private var section: Section
    private var titles: [String]

    init(section: Section) {
        self.section = section
        titles = section.titles
        super.init(style: .insetGrouped)
        title = section.name
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "page")
        navigationItem.rightBarButtonItem = UIBarButtonItem(
            title: "Open", image: UIImage(systemName: "folder"), target: self, action: #selector(open))
    }

    @objc private func open() {
        let picker = UIDocumentPickerViewController(
            forOpeningContentTypes: [UTType(filenameExtension: "one") ?? .data], asCopy: true)
        picker.delegate = self
        present(picker, animated: true)
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard let url = urls.first else { return }
        section = Section(path: url.path)
        titles = section.titles
        title = section.name
        tableView.reloadData()
    }

    func show(page: Int, animated: Bool) {
        guard page < titles.count else { return }
        navigationController?.pushViewController(
            PageViewController(section: section, page: page, title: titles[page]), animated: animated)
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        titles.count
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "page", for: indexPath)
        var content = cell.defaultContentConfiguration()
        content.text = titles[indexPath.row].isEmpty ? "Untitled page" : titles[indexPath.row]
        cell.contentConfiguration = content
        cell.accessoryType = .disclosureIndicator
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        show(page: indexPath.row, animated: true)
    }
}

final class PageViewController: UIViewController {
    private let section: Section
    private let page: Int

    init(section: Section, page: Int, title: String) {
        self.section = section
        self.page = page
        super.init(nibName: nil, bundle: nil)
        self.title = title
        navigationItem.largeTitleDisplayMode = .never
    }

    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        view = CanvasView(section: section, page: page)
    }
}
