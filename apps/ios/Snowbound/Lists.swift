import UIKit
import UniformTypeIdentifiers

/// Every open notebook with its sections, as Notes lists each account's folders.
final class NotebooksViewController: UITableViewController, UIDocumentPickerDelegate {
    private enum Item {
        case group(String, depth: Int)
        case section(Tab, depth: Int)
        case opening
        case problem(String)
    }

    private var items: [[Item]] = []
    private var selected: (notebook: UUID, path: String)?
    var onOpen: ((Tab, Notebook) -> Void)?

    init() {
        super.init(style: .insetGrouped)
        title = "Notebooks"
        navigationItem.largeTitleDisplayMode = .always
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "row")
        let add = UIMenu(children: [
            UIAction(title: "Open from Files…", image: UIImage(systemName: "folder")) { [weak self] _ in
                self?.openFromFiles()
            },
            UIAction(title: "Connect to Server…", image: UIImage(systemName: "server.rack")) { [weak self] _ in
                self?.connect()
            },
            UIAction(title: "Change Your Name…", image: UIImage(systemName: "person.crop.circle")) { [weak self] _ in
                guard let self else { return }
                Author.ask(from: self) {}
            },
        ])
        navigationItem.rightBarButtonItem = UIBarButtonItem(
            title: "Add Notebook", image: UIImage(systemName: "plus"), menu: add)
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        navigationController?.navigationBar.prefersLargeTitles = true
    }

    func reload() {
        items = Notebooks.all.map { notebook in
            if notebook.handle == nil {
                return [notebook.problem.map(Item.problem) ?? .opening]
            }
            var rows: [Item] = []
            var group = ""
            for tab in notebook.tabs {
                if tab.group != group {
                    // Each group level not yet shown gets its own row.
                    let parts = tab.group.split(separator: "/").map(String.init)
                    let shown = group.split(separator: "/").map(String.init)
                    let common = zip(parts, shown).prefix { $0 == $1 }.count
                    for depth in common..<parts.count { rows.append(.group(parts[depth], depth: depth)) }
                    group = tab.group
                }
                rows.append(.section(tab, depth: tab.group.split(separator: "/").count))
            }
            return rows
        }
        tableView.reloadData()
        showSelection()
        var empty = UIContentUnavailableConfiguration.empty()
        empty.text = "No Notebooks"
        empty.secondaryText = "Open a OneNote notebook from Files or a server."
        contentUnavailableConfiguration = items.isEmpty ? empty : nil
    }

    func select(_ tab: Tab, of notebook: Notebook) {
        selected = (notebook.id, tab.path)
        showSelection()
    }

    /// Marks the open section, as the sidebar of a wide window keeps it.
    private func showSelection() {
        guard let selected, let section = Notebooks.all.firstIndex(where: { $0.id == selected.notebook }),
            let row = items[section].firstIndex(where: {
                if case .section(let tab, _) = $0 { tab.path == selected.path } else { false }
            })
        else { return }
        tableView.selectRow(at: IndexPath(row: row, section: section), animated: false, scrollPosition: .none)
    }

    private func add(_ notebook: Notebook) {
        Notebooks.add(notebook)
        reload()
        notebook.open { [weak self] in
            self?.reload()
            // A notebook just added opens at its first section.
            if let tab = notebook.tabs.first(where: \.readable) { self?.onOpen?(tab, notebook) }
        }
    }

    private func openFromFiles() {
        let picker = UIDocumentPickerViewController(
            forOpeningContentTypes: [.folder, UTType(filenameExtension: "one") ?? .data])
        picker.delegate = self
        present(picker, animated: true)
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard let url = urls.first else { return }
        let accessing = url.startAccessingSecurityScopedResource()
        defer { if accessing { url.stopAccessingSecurityScopedResource() } }
        guard let bookmark = try? url.bookmarkData() else { return }
        add(Notebook(name: url.deletingPathExtension().lastPathComponent, source: .files(bookmark: bookmark)))
    }

    private func connect() {
        let server = ServerViewController()
        server.onOpen = { [weak self] notebook in
            self?.dismiss(animated: true)
            self?.add(notebook)
        }
        present(UINavigationController(rootViewController: server), animated: true)
    }

    private func newSection(in notebook: Notebook) {
        let alert = UIAlertController(title: "New Section", message: nil, preferredStyle: .alert)
        alert.addTextField { field in
            field.placeholder = "Name"
            field.autocapitalizationType = .words
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Create", style: .default) { [weak self, weak alert] _ in
            let name = alert?.textFields?.first?.text?.trimmingCharacters(in: .whitespaces) ?? ""
            guard !name.isEmpty, let handle = notebook.handle else { return }
            let (date, time) = titleDate()
            let path = take(sb_library_new_section(handle, "", name, Author.name ?? "", date, time))
            notebook.reload {
                self?.reload()
                if let tab = notebook.tabs.first(where: { $0.path == path }) { self?.onOpen?(tab, notebook) }
            }
        })
        present(alert, animated: true)
    }

    override func numberOfSections(in tableView: UITableView) -> Int { items.count }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        items[section].count
    }

    override func tableView(_ tableView: UITableView, viewForHeaderInSection section: Int) -> UIView? {
        let notebook = Notebooks.all[section]
        var title = UIButton.Configuration.plain()
        title.title = notebook.name
        title.image = UIImage(systemName: "ellipsis.circle")
        title.imagePlacement = .trailing
        title.imagePadding = 6
        title.contentInsets = NSDirectionalEdgeInsets(top: 8, leading: 4, bottom: 4, trailing: 4)
        title.baseForegroundColor = .label
        title.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer {
            var attributes = $0
            attributes.font = UIFont.preferredFont(forTextStyle: .title3).withTraits(.traitBold)
            return attributes
        }
        let button = UIButton(configuration: title)
        button.contentHorizontalAlignment = .leading
        button.showsMenuAsPrimaryAction = true
        button.menu = UIMenu(children: [
            UIAction(title: "New Section…", image: UIImage(systemName: "plus.rectangle.portrait")) {
                [weak self] _ in self?.newSection(in: notebook)
            },
            UIAction(title: "Close Notebook", image: UIImage(systemName: "xmark.circle"), attributes: .destructive) {
                [weak self] _ in
                Notebooks.remove(notebook)
                self?.reload()
            },
        ])
        return button
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "row", for: indexPath)
        var content = cell.defaultContentConfiguration()
        cell.accessoryType = .none
        cell.selectionStyle = .none
        cell.accessoryView = nil
        switch items[indexPath.section][indexPath.row] {
        case .group(let name, let depth):
            content.text = name
            content.image = UIImage(systemName: "folder")
            content.textProperties.color = .secondaryLabel
            content.directionalLayoutMargins.leading += CGFloat(depth) * 20
        case .section(let tab, let depth):
            content.text = tab.name
            content.image = UIImage(systemName: tab.readable ? "rectangle.portrait.fill" : "lock.fill")
            content.imageProperties.tintColor = tab.uiColor
            content.directionalLayoutMargins.leading += CGFloat(depth) * 20
            if tab.readable {
                cell.accessoryType = .disclosureIndicator
                cell.selectionStyle = .default
            } else {
                content.secondaryText = "Can’t be opened here"
                content.textProperties.color = .secondaryLabel
            }

        case .opening:
            content.text = "Opening…"
            content.textProperties.color = .secondaryLabel
            let spinner = UIActivityIndicatorView(style: .medium)
            spinner.startAnimating()
            cell.accessoryView = spinner
        case .problem(let problem):
            content.text = "Can’t Open Notebook"
            content.secondaryText = problem
            content.image = UIImage(systemName: "exclamationmark.triangle")
            content.imageProperties.tintColor = .systemOrange
            cell.accessoryView = UIImageView(image: UIImage(systemName: "arrow.clockwise"))
            cell.selectionStyle = .default
        }
        cell.contentConfiguration = content
        return cell
    }

    override func tableView(_ tableView: UITableView, willSelectRowAt indexPath: IndexPath) -> IndexPath? {
        switch items[indexPath.section][indexPath.row] {
        case .section(let tab, _): tab.readable ? indexPath : nil
        case .problem: indexPath
        default: nil
        }
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        let notebook = Notebooks.all[indexPath.section]
        switch items[indexPath.section][indexPath.row] {
        case .section(let tab, _): onOpen?(tab, notebook)
        case .problem:
            items[indexPath.section] = [.opening]
            tableView.reloadSections([indexPath.section], with: .none)
            notebook.open { [weak self] in self?.reload() }
        default: break
        }
    }
}

extension UIFont {
    func withTraits(_ traits: UIFontDescriptor.SymbolicTraits) -> UIFont {
        fontDescriptor.withSymbolicTraits(traits).map { UIFont(descriptor: $0, size: 0) } ?? self
    }
}

/// A section's pages, subpages indented, each page's conflict pages beneath it.
final class PagesViewController: UITableViewController, UISearchResultsUpdating {
    private struct Item {
        let row: Row
        let version: Row.Version?
        var id: String { version?.id ?? row.id }
    }

    private(set) var section: Section?
    private var items: [Item] = []
    private var selected: String?
    private let results = SearchViewController()
    private lazy var search = UISearchController(searchResultsController: results)
    private var searching: DispatchWorkItem?
    /// Opens a page of the section.
    var onOpen: ((Section, String, String?) -> Void)?
    /// Opens a page of another section of the notebook, found by search.
    var onOpenSection: ((Notebook, String, String, String?) -> Void)?

    init() {
        super.init(style: .plain)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "page")
        search.searchResultsUpdater = self
        search.searchBar.placeholder = "Search Notebook"
        navigationItem.searchController = search
        navigationItem.hidesSearchBarWhenScrolling = false
        results.onOpen = { [weak self] found, query in
            guard let self, let section, let notebook = Optional(section.notebook) else { return }
            if found.section == section.tab.path {
                onOpen?(section, found.page, query)
            } else {
                onOpenSection?(notebook, found.section, found.page, query)
            }
        }
        let compose = UIBarButtonItem(
            title: "New Page", image: UIImage(systemName: "square.and.pencil"),
            primaryAction: UIAction { [weak self] _ in self?.newPage(under: nil) })
        toolbarItems = [.flexibleSpace(), compose]
        NotificationCenter.default.addObserver(
            self, selector: #selector(changed), name: Section.changed, object: nil)
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        navigationController?.setToolbarHidden(section == nil, animated: false)
    }

    func loading(_ tab: Tab) {
        section = nil
        items = []
        title = tab.name
        tableView.reloadData()
        var loading = UIContentUnavailableConfiguration.loading()
        loading.text = "Opening…"
        contentUnavailableConfiguration = loading
        navigationController?.setToolbarHidden(true, animated: false)
    }

    func failed(_ tab: Tab, _ problem: String?) {
        var empty = UIContentUnavailableConfiguration.empty()
        empty.text = "Can’t Open Section"
        empty.secondaryText = problem
        contentUnavailableConfiguration = empty
    }

    func load(_ section: Section) {
        self.section = section
        title = section.tab.name
        navigationController?.setToolbarHidden(false, animated: false)
        reload()
    }

    func select(_ id: String) {
        selected = id
        if let index = items.firstIndex(where: { $0.id == id }) {
            tableView.selectRow(at: IndexPath(row: index, section: 0), animated: false, scrollPosition: .none)
        }
    }

    func startSearching() {
        search.isActive = true
        DispatchQueue.main.async { self.search.searchBar.becomeFirstResponder() }
    }

    private func reload() {
        items = (section?.rows ?? []).flatMap { row in
            [Item(row: row, version: nil)] + row.versions.map { Item(row: row, version: $0) }
        }
        tableView.reloadData()
        if let selected { select(selected) }
        var empty = UIContentUnavailableConfiguration.empty()
        empty.text = "No Pages"
        contentUnavailableConfiguration = section != nil && items.isEmpty ? empty : nil
    }

    @objc private func changed(_ notification: Notification) {
        guard notification.object as AnyObject? === section else { return }
        reload()
    }

    private func newPage(under parent: String?) {
        guard let section, let id = section.newPage(under: parent) else { return }
        (view.window?.windowScene?.delegate as? SceneDelegate)?.show(section: section, page: id, titleFocus: true)
    }

    private func delete(_ item: Item) {
        section?.delete(item.id) { _ in }
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { items.count }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let item = items[indexPath.row]
        let cell = tableView.dequeueReusableCell(withIdentifier: "page", for: indexPath)
        var content = cell.defaultContentConfiguration()
        let level = CGFloat(item.row.level - 1)
        if let version = item.version {
            // OneNote lists a conflict page by whose version it is and when the merge made it.
            content.text = version.user.isEmpty ? "Conflicting Version" : version.user
            content.secondaryText = version.created.map {
                DateFormatter.localizedString(
                    from: Date(timeIntervalSince1970: $0), dateStyle: .short, timeStyle: .short)
            }
            content.image = UIImage(systemName: "doc.on.doc")
            content.imageProperties.tintColor = .systemOrange
            content.textProperties.color = .secondaryLabel
            content.directionalLayoutMargins.leading += (level + 1) * 20
        } else {
            content.text = item.row.title.isEmpty ? "Untitled Page" : item.row.title
            if item.row.title.isEmpty || item.row.level > 1 { content.textProperties.color = .secondaryLabel }
            content.directionalLayoutMargins.leading += level * 20
            if !item.row.versions.isEmpty {
                content.image = UIImage(systemName: "exclamationmark.triangle.fill")
                content.imageProperties.tintColor = .systemOrange
            }
        }
        cell.contentConfiguration = content
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        guard let section else { return }
        selected = items[indexPath.row].id
        onOpen?(section, items[indexPath.row].id, nil)
    }

    override func tableView(
        _ tableView: UITableView, trailingSwipeActionsConfigurationForRowAt indexPath: IndexPath
    ) -> UISwipeActionsConfiguration? {
        let item = items[indexPath.row]
        let delete = UIContextualAction(style: .destructive, title: "Delete") { [weak self] _, _, done in
            self?.delete(item)
            done(true)
        }
        delete.image = UIImage(systemName: "trash")
        return UISwipeActionsConfiguration(actions: [delete])
    }

    override func tableView(
        _ tableView: UITableView, contextMenuConfigurationForRowAt indexPath: IndexPath, point: CGPoint
    ) -> UIContextMenuConfiguration? {
        let item = items[indexPath.row]
        return UIContextMenuConfiguration(actionProvider: { [weak self] _ in
            var actions: [UIMenuElement] = []
            if item.version == nil {
                actions.append(UIAction(title: "New Subpage", image: UIImage(systemName: "text.badge.plus")) { _ in
                    self?.newPage(under: item.row.id)
                })
            }
            actions.append(
                UIAction(
                    title: item.version == nil ? "Delete Page" : "Delete Conflict Page",
                    image: UIImage(systemName: "trash"), attributes: .destructive
                ) { _ in self?.delete(item) })
            return UIMenu(children: actions)
        })
    }

    func updateSearchResults(for controller: UISearchController) {
        searching?.cancel()
        let query = controller.searchBar.text?.trimmingCharacters(in: .whitespaces) ?? ""
        guard let section, let library = section.notebook.handle, !query.isEmpty else {
            results.show([], query: query)
            return
        }
        // Each keystroke waits for the next before the notebook is read.
        let work = DispatchWorkItem { [weak self] in
            let (library, open, path) = (Int(bitPattern: library), Int(bitPattern: section.handle), section.tab.path)
            self?.results.searching(query)
            background({
                decode(
                    [Found].self,
                    sb_library_search(
                        OpaquePointer(bitPattern: library), OpaquePointer(bitPattern: open), path, query)) ?? []
            }) { [weak self] found in
                guard controller.searchBar.text?.trimmingCharacters(in: .whitespaces) == query else { return }
                self?.results.show(
                    found, query: query,
                    sections: Dictionary(
                        section.notebook.tabs.map { ($0.path, $0.name) }, uniquingKeysWith: { first, _ in first }))
            }
        }
        searching = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: work)
    }
}

/// A page `sb_library_search` found.
struct Found: Decodable {
    let section: String
    let page: String
    let title: String
    let snippet: String
}

final class SearchViewController: UITableViewController {
    private var found: [Found] = []
    private var sections: [String: String] = [:]
    private var query = ""
    var onOpen: ((Found, String) -> Void)?

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "found")
    }

    func searching(_ query: String) {
        guard found.isEmpty else { return }
        var loading = UIContentUnavailableConfiguration.loading()
        loading.text = "Searching…"
        contentUnavailableConfiguration = loading
    }

    func show(_ found: [Found], query: String, sections: [String: String] = [:]) {
        self.found = found
        self.query = query
        self.sections = sections
        tableView.reloadData()
        contentUnavailableConfiguration = found.isEmpty && !query.isEmpty ? UIContentUnavailableConfiguration.search() : nil
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { found.count }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let found = found[indexPath.row]
        let cell = tableView.dequeueReusableCell(withIdentifier: "found", for: indexPath)
        var content = UIListContentConfiguration.subtitleCell()
        content.text = found.title.isEmpty ? "Untitled Page" : found.title
        let section = sections[found.section] ?? ""
        content.secondaryAttributedText = highlighted(
            [section, found.snippet].filter { !$0.isEmpty }.joined(separator: " · "))
        content.secondaryTextProperties.numberOfLines = 2
        cell.contentConfiguration = content
        return cell
    }

    /// `text` with the query in bold where it occurs.
    private func highlighted(_ text: String) -> NSAttributedString {
        let font = UIFont.preferredFont(forTextStyle: .subheadline)
        let attributed = NSMutableAttributedString(
            string: text, attributes: [.font: font, .foregroundColor: UIColor.secondaryLabel])
        var range = text.startIndex..<text.endIndex
        while let match = text.range(of: query, options: .caseInsensitive, range: range) {
            attributed.addAttributes(
                [.font: font.withTraits(.traitBold), .foregroundColor: UIColor.label], range: NSRange(match, in: text))
            range = match.upperBound..<text.endIndex
        }
        return attributed
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        onOpen?(found[indexPath.row], query)
    }
}
