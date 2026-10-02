import UIKit
import UniformTypeIdentifiers

/// The places notebooks are kept, each listing its notebooks with their sections, as Files
/// lists its locations and Notes each account's folders.
final class NotebooksViewController: UICollectionViewController, UIDocumentPickerDelegate {
    enum Location: Hashable {
        /// Snowbound's folder in iCloud Drive, where new notebooks go while iCloud Drive is on.
        case icloud
        /// Snowbound's Documents, which Files shows as Snowbound's folder.
        case onDevice
        /// Notebooks opened from Files or a server.
        case elsewhere
    }

    enum Item: Hashable {
        case location(Location)
        case notebook(String)
        /// A section group, by its `/`-separated path.
        case group(notebook: String, path: String)
        case section(notebook: String, path: String)
        /// Opening, or why the notebook cannot be.
        case status(notebook: String)
    }

    var dataSource: UICollectionViewDiffableDataSource<Location, Item>!
    private var collapsed: Set<Item> = []
    /// What each notebook's row shows of its sync status, by notebook id.
    private var attention: [String: Sync.Attention] = [:]
    private var selected: Item?
    var onOpen: ((Tab, Notebook) -> Void)?

    private static let device = "On My \(UIDevice.current.model)"

    init() {
        super.init(collectionViewLayout: UICollectionViewFlowLayout())
        title = "Notebooks"
        navigationItem.largeTitleDisplayMode = .always
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        collectionView.collectionViewLayout = UICollectionViewCompositionalLayout { [weak self] _, environment in
            let compact = environment.traitCollection.horizontalSizeClass == .compact
            var list = UICollectionLayoutListConfiguration(appearance: compact ? .insetGrouped : .sidebar)
            list.headerMode = .firstItemInSection
            list.trailingSwipeActionsConfigurationProvider = { indexPath in
                guard let self, let item = self.dataSource.itemIdentifier(for: indexPath) else { return nil }
                if case .notebook(let id) = item, let notebook = self.notebook(id) {
                    return UISwipeActionsConfiguration(actions: self.swipeActions(for: notebook, at: indexPath))
                }
                guard item == .location(.onDevice) else { return nil }
                let hide = UIContextualAction(style: .normal, title: "Hide") { [weak self] _, _, done in
                    self?.showOnDevice(false)
                    done(true)
                }
                hide.image = UIImage(systemName: "eye.slash")
                return UISwipeActionsConfiguration(actions: [hide])
            }
            return NSCollectionLayoutSection.list(using: list, layoutEnvironment: environment)
        }
        let cell = UICollectionView.CellRegistration<UICollectionViewListCell, Item> { [weak self] cell, _, item in
            self?.configure(cell, item)
            self?.arranging(cell, item)
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { view, indexPath, item in
            view.dequeueConfiguredReusableCell(using: cell, for: indexPath, item: item)
        }
        dataSource.sectionSnapshotHandlers.willCollapseItem = { [weak self] in self?.collapsed.insert($0) }
        dataSource.sectionSnapshotHandlers.willExpandItem = { [weak self] in self?.collapsed.remove($0) }

        let add = UIMenu(children: [
            UIDeferredMenuElement.uncached { [weak self] provide in
                guard let self else { return provide([]) }
                var places: [UIMenuElement] = []
                if ICloud.documents != nil {
                    places.append(
                        UIAction(title: "iCloud Drive", image: UIImage(systemName: "icloud")) { [weak self] _ in
                            self?.newNotebook(inCloud: true)
                        })
                } else if !ICloud.signedIn {
                    places.append(
                        UIAction(title: "Turn On iCloud Drive", image: UIImage(systemName: "icloud")) { _ in
                            if let settings = URL(string: UIApplication.openSettingsURLString) {
                                UIApplication.shared.open(settings)
                            }
                        })
                }
                places.append(
                    UIAction(title: Self.device, image: UIImage(systemName: "iphone")) { [weak self] _ in
                        self?.newNotebook(inCloud: false)
                    })
                var actions: [UIMenuElement] = [
                    UIMenu(title: "New Notebook", options: .displayInline, children: places),
                    UIAction(title: "Open from Server…", image: UIImage(systemName: "server.rack")) { [weak self] _ in
                        self?.connect()
                    },
                    UIAction(title: "Open Folder…", image: UIImage(systemName: "folder")) { [weak self] _ in
                        self?.openFolder()
                    },
                ]
                let copied = Notebooks.onDevice.contains { $0.source == .documents(path: Notebooks.guide) }
                if Notebooks.guideOffered && !copied {
                    actions.append(
                        UIAction(title: "Open the Snowbound Guide", image: UIImage(systemName: "book")) { [weak self] _ in
                            self?.openGuide()
                        })
                }
                provide(actions)
            }
        ])
        let more = UIMenu(children: [
            UIDeferredMenuElement.uncached { [weak self] provide in
                var actions: [UIMenuElement] = [
                    UIAction(title: "Sync Status…", image: UIImage(systemName: "arrow.triangle.2.circlepath")) {
                        [weak self] _ in self.map(SyncViewController.present)
                    }
                ]
                if !Notebooks.showsOnDevice {
                    actions.append(
                        UIAction(title: "Show \(Self.device)", image: UIImage(systemName: "eye")) { [weak self] _ in
                            self?.showOnDevice(true)
                        })
                }
                actions.append(
                    UIAction(title: "Personalize…", image: UIImage(systemName: "person.crop.circle")) { [weak self] _ in
                        guard let self else { return }
                        Author.ask(from: self) {}
                    })
                actions.append(Appearance.menu())
                actions.append(Editing.fontMenu())
                provide(actions)
            }
        ])
        navigationItem.rightBarButtonItems = [
            UIBarButtonItem(title: "Add Notebook", image: UIImage(systemName: "plus"), menu: add),
            UIBarButtonItem(title: "More", image: UIImage(systemName: "ellipsis"), menu: more),
        ]
        navigationItem.searchController = SearchViewController.controller()
        setUpArranging()
        let corner = Prototype.quickNote ? [QuickNote.item()] : []
        if #available(iOS 26, *) {
            // Search sits in the bottom toolbar, as Notes and Files put it.
            navigationItem.preferredSearchBarPlacement = .integrated
            toolbarItems = [navigationItem.searchBarPlacementBarButtonItem] + (corner.isEmpty ? [] : [.fixedSpace()] + corner)
        } else {
            toolbarItems = corner.isEmpty ? [] : [.flexibleSpace()] + corner
        }
        for name in [Sync.changed, Section.changed] {
            NotificationCenter.default.addObserver(self, selector: #selector(refreshSync), name: name, object: nil)
        }
        NotificationCenter.default.addObserver(forName: Notebook.listed, object: nil, queue: .main) { [weak self] _ in
            guard let self else { return }
            reload(animated: true)
            var shown = dataSource.snapshot()
            shown.reconfigureItems(shown.itemIdentifiers)
            dataSource.apply(shown, animatingDifferences: false)
        }
        reload()
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        navigationController?.navigationBar.prefersLargeTitles = true
        navigationController?.setToolbarHidden(toolbarItems?.isEmpty != false, animated: false)
    }

    func notebook(_ id: String) -> Notebook? { Notebooks.all.first { $0.id == id } }

    func reload(animated: Bool = false) {
        guard isViewLoaded else { return }
        if Prototype.welcome {
            let welcome = Prototype.welcome(
                new: { [weak self] in self?.newNotebook(inCloud: ICloud.documents != nil) },
                openFolder: { [weak self] in self?.openFolder() }, connect: { [weak self] in self?.connect() })
            contentUnavailableConfiguration = welcome
            if welcome != nil { return dataSource.apply(.init(), animatingDifferences: false) }
        }
        // A place with no notebooks is left out; the Add Notebook menu reaches every place.
        let locations: [(Location, [Notebook])] = [
            (.icloud, Notebooks.inCloud), (.onDevice, Notebooks.onDevice), (.elsewhere, Notebooks.elsewhere),
        ].filter { !$0.1.isEmpty }
        if locations.isEmpty {
            var empty = UIContentUnavailableConfiguration.empty()
            empty.text = "No Notebooks"
            empty.secondaryText = "Create a notebook, or open one from a server or folder."
            var button = UIButton.Configuration.borderedProminent()
            button.title = "New Notebook"
            empty.button = button
            empty.buttonProperties.primaryAction = UIAction { [weak self] _ in
                self?.newNotebook(inCloud: ICloud.documents != nil)
            }
            contentUnavailableConfiguration = empty
        } else {
            contentUnavailableConfiguration = nil
        }
        // Locations kept keep their rows, so a move animates.
        if dataSource.snapshot().sectionIdentifiers != locations.map(\.0) {
            var sections = NSDiffableDataSourceSnapshot<Location, Item>()
            sections.appendSections(locations.map(\.0))
            dataSource.apply(sections, animatingDifferences: false)
        }
        for (location, notebooks) in locations {
            var list = NSDiffableDataSourceSectionSnapshot<Item>()
            let header = Item.location(location)
            list.append([header])
            list.append(notebooks.map { .notebook($0.id) }, to: header)
            for notebook in notebooks {
                let parent = Item.notebook(notebook.id)
                guard notebook.handle != nil else {
                    list.append([.status(notebook: notebook.id)], to: parent)
                    continue
                }
                for tab in notebook.tabs {
                    var parent = parent
                    var path = ""
                    for part in tab.group.split(separator: "/") {
                        path += (path.isEmpty ? "" : "/") + part
                        let group = Item.group(notebook: notebook.id, path: path)
                        if !list.contains(group) { list.append([group], to: parent) }
                        parent = group
                    }
                    list.append([.section(notebook: notebook.id, path: tab.path)], to: parent)
                }
            }
            list.expand(list.items.filter { !collapsed.contains($0) })
            dataSource.apply(list, to: location, animatingDifferences: animated)
        }
        showSelection()
        refreshSync()
    }

    /// Reads every notebook's sync status, marks the rows it changed, and tells of a notebook
    /// that has just stopped syncing with edits waiting.
    @objc private func refreshSync() {
        Sync.status { [weak self] notebooks in
            guard let self else { return }
            var changed: [Item] = []
            for (notebook, sections) in notebooks {
                let before = attention[notebook.id]
                let after = Sync.attention(sections)
                guard before != after else { continue }
                attention[notebook.id] = after
                changed.append(.notebook(notebook.id))
                // The list is off screen on a phone while a page shows.
                if let toast = Sync.toast(notebook.name, from: before, to: after),
                    let window = splitViewController?.view.window ?? view.window
                {
                    SyncToast.show(toast.title, toast.detail, in: window) { [weak self] in
                        self.map(SyncViewController.present)
                    }
                }
            }
            var shown = dataSource.snapshot()
            let items = changed.filter { shown.indexOfItem($0) != nil }
            guard !items.isEmpty else { return }
            shown.reconfigureItems(items)
            dataSource.apply(shown, animatingDifferences: false)
        }
    }

    /// Lists Documents and iCloud Drive again and opens the notebooks new there; `opened` runs
    /// as each opens.
    func rescan(opened: ((Notebook) -> Void)? = nil) {
        for notebook in Notebooks.scanICloud() + Notebooks.scan() {
            notebook.open { [weak self] in
                self?.reload()
                opened?(notebook)
            }
        }
        reload()
    }

    /// Lists the notebooks again and opens the first section of the one from `source`.
    private func rescan(opening source: Source) {
        rescan { [weak self] notebook in
            guard notebook.source == source, let tab = notebook.tabs.first(where: \.readable) else { return }
            self?.onOpen?(tab, notebook)
        }
    }

    private func showOnDevice(_ shown: Bool) {
        Notebooks.showsOnDevice = shown
        rescan()
    }

    func select(_ tab: Tab, of notebook: Notebook) {
        selected = .section(notebook: notebook.id, path: tab.path)
        showSelection()
    }

    /// Marks the open section, as the sidebar of a wide window keeps it.
    private func showSelection() {
        guard let selected, let indexPath = dataSource.indexPath(for: selected) else { return }
        collectionView.selectItem(at: indexPath, animated: false, scrollPosition: [])
    }

    private func configure(_ cell: UICollectionViewListCell, _ item: Item) {
        var content = cell.defaultContentConfiguration()
        var accessories: [UICellAccessory] = []
        switch item {
        case .location(let location):
            content.text =
                switch location {
                case .icloud: "iCloud Drive"
                case .onDevice: Self.device
                case .elsewhere: "Other Locations"
                }
            accessories = [.outlineDisclosure(options: .init(style: .header))]
        case .notebook(let id):
            guard let notebook = notebook(id) else { break }
            content.text = notebook.name
            content.image = .notebook(notebook.color)
            var details: [String] = []
            if case .server(let server) = notebook.source { details.append(server.host) }
            if let state = attention[id]?.state {
                details.append(Sync.label(state))
                let mark = UIImageView(image: UIImage(systemName: Sync.symbol(state)))
                mark.tintColor = state == .notConnected || Sync.offline ? .secondaryLabel : .systemOrange
                mark.preferredSymbolConfiguration = UIImage.SymbolConfiguration(textStyle: .body)
                accessories.append(.customView(configuration: .init(customView: mark, placement: .trailing(displayed: .always))))
            }
            content.secondaryText = details.isEmpty ? nil : details.joined(separator: " · ")
            content.secondaryTextProperties.color = .secondaryLabel
            let more = UIButton(type: .system)
            more.setImage(UIImage(systemName: "ellipsis.circle"), for: .normal)
            more.accessibilityLabel = "More"
            more.showsMenuAsPrimaryAction = true
            more.menu = menu(for: notebook)
            accessories += [
                .customView(configuration: .init(customView: more, placement: .trailing(displayed: .always))),
                .outlineDisclosure(options: .init(style: .cell)),
            ]
        case .group(_, let path):
            content.text = path.split(separator: "/").last.map(String.init)
            content.image = UIImage(systemName: "folder")
            content.textProperties.color = .secondaryLabel
            accessories = [.outlineDisclosure(options: .init(style: .cell))]
        case .section(let id, let path):
            guard let tab = notebook(id)?.tabs.first(where: { $0.path == path }) else { break }
            content.text = tab.name
            content.image = UIImage(
                systemName: tab.readable ? "rectangle.portrait.fill" : tab.downloading ? "icloud.and.arrow.down" : "lock.fill")
            content.imageProperties.tintColor = tab.uiColor
            if tab.locked {
                content.secondaryText = "Password protected"
            } else if !tab.readable {
                content.secondaryText = tab.downloading ? "Downloading…" : tab.problem ?? "Can’t be opened here"
                content.textProperties.color = .secondaryLabel
            }
        case .status(let id):
            if let problem = notebook(id)?.problem {
                content.text = "Can’t Open Notebook"
                content.secondaryText = problem
                content.image = UIImage(systemName: "exclamationmark.triangle")
                content.imageProperties.tintColor = .systemOrange
                let retry = UIImageView(image: UIImage(systemName: "arrow.clockwise"))
                accessories = [.customView(configuration: .init(customView: retry, placement: .trailing()))]
            } else {
                content.text = "Opening…"
                content.textProperties.color = .secondaryLabel
                let spinner = UIActivityIndicatorView(style: .medium)
                spinner.startAnimating()
                accessories = [.customView(configuration: .init(customView: spinner, placement: .trailing()))]
            }
        }
        cell.contentConfiguration = content
        cell.accessories = accessories
    }

    private func menu(for notebook: Notebook) -> UIMenu {
        // Why the notebook isn't syncing and when it last did, while it isn't.
        let status: [UIMenuElement] =
            attention[notebook.id].map { attention in
                let details = UIAction(
                    title: Sync.label(attention.state), subtitle: attention.detail.isEmpty ? nil : attention.detail,
                    image: UIImage(systemName: Sync.symbol(attention.state))
                ) { [weak self] _ in self.map(SyncViewController.present) }
                let now = UIAction(title: "Sync Now", image: UIImage(systemName: "arrow.clockwise")) { _ in
                    if let handle = notebook.handle { sb_library_sync_now(handle) }
                }
                return [UIMenu(options: .displayInline, children: [details, now])]
            } ?? []
        let actions = [
            UIAction(title: "New Section…", image: UIImage(systemName: "plus.rectangle.portrait")) {
                [weak self] _ in self?.newSection(in: notebook)
            },
            UIAction(title: "Tags Summary", image: UIImage(systemName: "tag")) { [weak self] _ in
                guard let self else { return }
                (view.window?.windowScene?.delegate as? SceneDelegate)?.showTags(of: notebook, from: self)
            },
        ]
        // A notebook On My iPhone is its folder there, handled as Files handles a folder.
        guard let folder = folder(of: notebook) else {
            let image = UIImage(systemName: "xmark.circle")
            let close = UIAction(title: "Close Notebook", image: image, attributes: .destructive) { [weak self] _ in
                Notebooks.remove(notebook)
                self?.reload()
            }
            return UIMenu(children: status + actions + [close])
        }
        let file = [
            UIAction(title: "Rename…", image: UIImage(systemName: "pencil")) { [weak self] _ in
                self?.rename(folder, of: notebook)
            },
            UIAction(title: "Duplicate", image: UIImage(systemName: "plus.square.on.square")) { [weak self] _ in
                self?.duplicate(folder, of: notebook)
            },
            UIAction(title: "Share…", image: UIImage(systemName: "square.and.arrow.up")) { [weak self] _ in
                self?.share(folder, from: notebook)
            },
            UIAction(title: "Show in Files", image: UIImage(systemName: "folder")) { _ in
                // Files opens the folder a `shareddocuments` URL names.
                if let url = URL(string: "shareddocuments://" + folder.path) { UIApplication.shared.open(url) }
            },
        ]
        let delete = UIAction(title: "Delete", image: UIImage(systemName: "trash"), attributes: .destructive) {
            [weak self] _ in self?.delete(folder, of: notebook)
        }
        return UIMenu(children: status + [
            UIMenu(options: .displayInline, children: actions), UIMenu(options: .displayInline, children: file), delete,
        ])
    }

    private func swipeActions(for notebook: Notebook, at indexPath: IndexPath) -> [UIContextualAction] {
        guard let folder = folder(of: notebook) else {
            let close = UIContextualAction(style: .destructive, title: "Close") { [weak self] _, _, done in
                Notebooks.remove(notebook)
                self?.reload()
                done(true)
            }
            close.image = UIImage(systemName: "xmark.circle")
            return [close]
        }
        let delete = UIContextualAction(style: .destructive, title: "Delete") { [weak self] _, _, done in
            self?.delete(folder, of: notebook)
            done(true)
        }
        delete.image = UIImage(systemName: "trash")
        let share = UIContextualAction(style: .normal, title: "Share") { [weak self] _, _, done in
            self?.share(folder, from: notebook)
            done(true)
        }
        share.image = UIImage(systemName: "square.and.arrow.up")
        share.backgroundColor = .systemBlue
        return [delete, share]
    }

    /// The notebook's folder or section file On My iPhone; nil for one opened elsewhere.
    private func folder(of notebook: Notebook) -> URL? {
        guard case .documents(let path) = notebook.source else { return nil }
        return documentsDirectory.appendingPathComponent(path)
    }

    /// Publishes the notebook's edits and lets go of it, closing any section of it shown,
    /// then runs `act`; refuses with why while an edit can't be published, as moving the
    /// folder then would strand it.
    private func close(_ notebook: Notebook, refusing title: String, then act: @escaping () -> Void) {
        for scene in UIApplication.shared.connectedScenes {
            (scene.delegate as? SceneDelegate)?.close(notebook)
        }
        guard let handle = notebook.handle else { return act() }
        let pointer = Int(bitPattern: handle)
        background({ () -> String? in
            var error: UnsafeMutablePointer<CChar>?
            let closed = sb_library_close(OpaquePointer(bitPattern: pointer), 10, &error)
            let problem = take(error)
            return closed ? nil : problem ?? ""
        }) { [weak self] problem in
            if let problem {
                self?.alert(title, problem)
                return
            }
            notebook.pause()
            act()
        }
    }

    private func rename(_ folder: URL, of notebook: Notebook) {
        let alert = UIAlertController(title: "Rename", message: nil, preferredStyle: .alert)
        let isFile = folder.pathExtension.lowercased() == "one"
        let rename = UIAlertAction(title: "Rename", style: .default) { [weak self, weak alert] _ in
            let name = alert?.textFields?.first?.text?.trimmingCharacters(in: .whitespaces) ?? ""
            let target = folder.deletingLastPathComponent().appendingPathComponent(
                isFile ? name + "." + folder.pathExtension : name)
            guard target != folder else { return }
            guard !FileManager.default.fileExists(atPath: target.path) else {
                self?.alert("“\(name)” Already Exists", "Choose a different name.")
                return
            }
            self?.close(notebook, refusing: "Can’t Rename") {
                do {
                    try FileManager.default.moveItem(at: folder, to: target)
                    _ = sb_notebook_moved(cacheDirectory.path, folder.path, target.path, nil)
                } catch {
                    self?.alert("Can’t Rename", error.localizedDescription)
                }
                self?.rescan()
            }
        }
        alert.addTextField { field in
            field.text = folder.deletingPathExtension().lastPathComponent
            field.autocapitalizationType = .words
            field.clearButtonMode = .whileEditing
            NotificationCenter.default.addObserver(
                forName: UITextField.textDidChangeNotification, object: field, queue: .main
            ) { _ in rename.isEnabled = Self.namesFile(field) }
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(rename)
        alert.preferredAction = rename
        present(alert, animated: true)
    }

    /// Whether `field` holds a name a notebook's folder can take.
    private static func namesFile(_ field: UITextField) -> Bool {
        let name = (field.text ?? "").trimmingCharacters(in: .whitespaces)
        return !name.isEmpty && !name.hasPrefix(".") && !name.contains("/")
    }

    /// Copies the notebook beside itself, named as Files names a duplicate.
    private func duplicate(_ folder: URL, of notebook: Notebook) {
        // The copy takes what the open sections have stored.
        for section in Section.all where section.notebook === notebook && !section.flush(10) {
            return alert("Can’t Duplicate", "Some changes haven’t been saved to the notebook yet.")
        }
        let base = folder.deletingPathExtension().lastPathComponent
        let ext = folder.pathExtension
        var copy = folder
        for n in 2... {
            let name = base + " \(n)"
            copy = folder.deletingLastPathComponent().appendingPathComponent(ext.isEmpty ? name : name + "." + ext)
            if !FileManager.default.fileExists(atPath: copy.path) { break }
        }
        let target = copy
        background({ () -> String? in
            do { try FileManager.default.copyItem(at: folder, to: target) } catch { return error.localizedDescription }
            return nil
        }) { [weak self] problem in
            if let problem { self?.alert("Can’t Duplicate", problem) }
            self?.rescan()
        }
    }

    private func share(_ folder: URL, from notebook: Notebook) {
        let sheet = UIActivityViewController(activityItems: [folder], applicationActivities: nil)
        if let popover = sheet.popoverPresentationController {
            if let indexPath = dataSource.indexPath(for: .notebook(notebook.id)),
                let cell = collectionView.cellForItem(at: indexPath)
            {
                popover.sourceView = cell
                popover.sourceRect = cell.bounds
            } else {
                popover.sourceView = view
            }
        }
        present(sheet, animated: true)
    }

    /// Moves the notebook to Recently Deleted, or, where the system keeps none, removes it
    /// once confirmed.
    private func delete(_ folder: URL, of notebook: Notebook) {
        close(notebook, refusing: "Can’t Delete") { [weak self] in self?.remove(folder, of: notebook) }
    }

    private func remove(_ folder: URL, of notebook: Notebook) {
        if (try? FileManager.default.trashItem(at: folder, resultingItemURL: nil)) != nil {
            return rescan()
        }
        let name = folder.deletingPathExtension().lastPathComponent
        let alert = UIAlertController(
            title: "Delete “\(name)”?", message: "This notebook will be deleted immediately. You can’t undo this action.",
            preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel) { [weak self] _ in
            notebook.open { self?.reload() }
        })
        alert.addAction(UIAlertAction(title: "Delete", style: .destructive) { [weak self] _ in
            do { try FileManager.default.removeItem(at: folder) } catch {
                self?.alert("Can’t Delete", error.localizedDescription)
            }
            self?.rescan()
        })
        present(alert, animated: true)
    }

    override func collectionView(
        _ collectionView: UICollectionView, contextMenuConfigurationForItemsAt indexPaths: [IndexPath], point: CGPoint
    ) -> UIContextMenuConfiguration? {
        guard indexPaths.count == 1, let item = dataSource.itemIdentifier(for: indexPaths[0]) else { return nil }
        guard case .notebook(let id) = item, let notebook = notebook(id) else {
            let moves = arrangeMenu(item)
            return moves.isEmpty ? nil : UIContextMenuConfiguration(actionProvider: { _ in UIMenu(children: moves) })
        }
        return UIContextMenuConfiguration(actionProvider: { [weak self] _ in
            UIMenu(children: (self?.menu(for: notebook).children ?? []) + (self?.arrangeMenu(item) ?? []))
        })
    }

    override func collectionView(_ collectionView: UICollectionView, shouldSelectItemAt indexPath: IndexPath) -> Bool {
        guard let item = dataSource.itemIdentifier(for: indexPath) else { return false }
        switch item {
        case .section(let id, let path):
            let tab = notebook(id)?.tabs.first(where: { $0.path == path })
            return tab?.readable == true || tab?.locked == true
        case .status(let id):
            guard let notebook = notebook(id), notebook.problem != nil else { return false }
            let spinner = UIActivityIndicatorView(style: .medium)
            spinner.startAnimating()
            (collectionView.cellForItem(at: indexPath) as? UICollectionViewListCell)?.accessories = [
                .customView(configuration: .init(customView: spinner, placement: .trailing()))
            ]
            notebook.open { [weak self] in self?.reload() }
        case .notebook, .group:
            // The row opens and closes, as a folder of Files' sidebar does.
            guard let location = dataSource.sectionIdentifier(for: indexPath.section) else { return false }
            var list = dataSource.snapshot(for: location)
            if list.isExpanded(item) {
                list.collapse([item])
                collapsed.insert(item)
            } else {
                list.expand([item])
                collapsed.remove(item)
            }
            dataSource.apply(list, to: location)
        case .location: break
        }
        return false
    }

    override func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        guard case .section(let id, let path) = dataSource.itemIdentifier(for: indexPath), let notebook = notebook(id),
            let tab = notebook.tabs.first(where: { $0.path == path })
        else { return }
        if tab.locked { return unlock(tab, of: notebook, wrong: false) }
        onOpen?(tab, notebook)
    }

    /// Asks for a protected section's password, as OneNote's Protected Section dialog does,
    /// and opens it once unlocked.
    private func unlock(_ tab: Tab, of notebook: Notebook, wrong: Bool) {
        let alert = UIAlertController(
            title: "Protected Section",
            message: wrong ? "Password is incorrect." : "Section “\(tab.name)” is password protected.",
            preferredStyle: .alert)
        alert.addTextField { field in
            field.isSecureTextEntry = true
            field.placeholder = "Password"
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "OK", style: .default) { [weak self, weak alert] _ in
            let password = alert?.textFields?.first?.text ?? ""
            notebook.unlock(tab.path, password: password) { unlocked in
                self?.reload()
                if unlocked, let opened = notebook.tabs.first(where: { $0.path == tab.path }) {
                    self?.onOpen?(opened, notebook)
                } else {
                    self?.unlock(tab, of: notebook, wrong: true)
                }
            }
        })
        present(alert, animated: true)
    }

    private func add(_ notebook: Notebook) {
        Notebooks.add(notebook)
        openFirstSection(of: notebook)
    }

    private func openFirstSection(of notebook: Notebook) {
        reload()
        notebook.open { [weak self] in
            self?.reload()
            if let tab = notebook.tabs.first(where: \.readable) { self?.onOpen?(tab, notebook) }
        }
    }

    /// Creates a notebook in iCloud Drive or On My iPhone, as OneNote's New Notebook does, and
    /// opens it.
    private func newNotebook(inCloud: Bool) {
        guard let author = Author.name else {
            return Author.ask(from: self) { [weak self] in self?.newNotebook(inCloud: inCloud) }
        }
        let alert = UIAlertController(
            title: inCloud ? "New iCloud Notebook" : "New Notebook", message: nil, preferredStyle: .alert)
        let create = UIAlertAction(title: "Create", style: .default) { [weak self, weak alert] _ in
            let name = alert?.textFields?.first?.text?.trimmingCharacters(in: .whitespaces) ?? ""
            self?.create(name, by: author, inCloud: inCloud)
        }
        alert.addTextField { field in
            field.placeholder = "Name"
            // A notebook in iCloud Drive starts named for it, so the list tells it apart.
            if inCloud {
                let taken = Set(Notebooks.inCloud.map(\.name))
                field.text = (1...).lazy.map { $0 == 1 ? "iCloud Notebook" : "iCloud Notebook \($0)" }
                    .first { !taken.contains($0) }
            }
            create.isEnabled = field.text?.isEmpty == false
            field.autocapitalizationType = .words
            NotificationCenter.default.addObserver(
                forName: UITextField.textDidChangeNotification, object: field, queue: .main
            ) { _ in create.isEnabled = Self.namesFile(field) }
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(create)
        alert.preferredAction = create
        present(alert, animated: true)
    }

    private func create(_ name: String, by author: String, inCloud: Bool) {
        let place = inCloud ? ICloud.documents : documentsDirectory
        guard let folder = place?.appendingPathComponent(name) else { return }
        guard !FileManager.default.fileExists(atPath: folder.path) else {
            return alert("“\(name)” Already Exists", "Choose a different name.")
        }
        let (date, time) = titleDate()
        background({ () -> String? in
            var error: UnsafeMutablePointer<CChar>?
            let made = sb_notebook_create(folder.path, cacheDirectory.path, author, date, time, &error)
            let problem = take(error)
            return made ? nil : problem ?? ""
        }) { [weak self] problem in
            guard let self else { return }
            if let problem { return alert("Can’t Create Notebook", problem) }
            if !inCloud && !Notebooks.showsOnDevice { Notebooks.showsOnDevice = true }
            rescan(opening: inCloud ? .icloud(path: name) : .documents(path: name))
        }
    }

    /// Copies the guide the app carries to On My iPhone and opens the copy, so the carried one
    /// stays as shipped.
    private func openGuide() {
        let copy = documentsDirectory.appendingPathComponent(Notebooks.guide)
        background({ () -> String? in
            guard !FileManager.default.fileExists(atPath: copy.path) else { return nil }
            guard let carried = Bundle.main.url(forResource: Notebooks.guide, withExtension: nil) else {
                return "The guide is missing from this copy of Snowbound."
            }
            do { try FileManager.default.copyItem(at: carried, to: copy) } catch { return error.localizedDescription }
            return nil
        }) { [weak self] problem in
            guard let self else { return }
            if let problem { return alert("Can’t Open the Guide", problem) }
            if !Notebooks.showsOnDevice { Notebooks.showsOnDevice = true }
            rescan(opening: .documents(path: Notebooks.guide))
        }
    }

    private func openFolder() {
        let picker = UIDocumentPickerViewController(
            forOpeningContentTypes: [.folder, UTType(filenameExtension: "one") ?? .data])
        picker.delegate = self
        present(picker, animated: true)
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        if let url = urls.first { open(document: url) }
    }

    /// Opens the folder or section file at `url`, chosen here or opened from Files, at its first
    /// section, listing it unless it is listed.
    func open(document url: URL) {
        let accessing = url.startAccessingSecurityScopedResource()
        defer { if accessing { url.stopAccessingSecurityScopedResource() } }
        let listed = Notebooks.all.first { notebook in
            guard case .files(let bookmark) = notebook.source else { return false }
            var stale = false
            let location = try? URL(resolvingBookmarkData: bookmark, bookmarkDataIsStale: &stale)
            return location?.standardizedFileURL == url.standardizedFileURL
        }
        if let listed {
            // A notebook still opening at launch lists no sections yet.
            if let tab = listed.tabs.first(where: \.readable) { onOpen?(tab, listed) } else { openFirstSection(of: listed) }
            return
        }
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
}

extension UIFont {
    func withTraits(_ traits: UIFontDescriptor.SymbolicTraits) -> UIFont {
        fontDescriptor.withSymbolicTraits(traits).map { UIFont(descriptor: $0, size: 0) } ?? self
    }
}

/// A section's pages, subpages indented, each page's conflict pages beneath it.
final class PagesViewController: UITableViewController {
    struct Item {
        let row: Row
        let version: Row.Version?
        var id: String { version?.id ?? row.id }
    }

    private(set) var section: Section?
    private(set) var items: [Item] = []
    private var selected: String?
    private lazy var search = SearchViewController.controller { [weak self] in self?.section }
    /// Opens a page of the section.
    var onOpen: ((Section, String) -> Void)?

    /// The section strip prototype, over the pages.
    private lazy var strip = SectionStrip { [weak self] tab, notebook in
        (self?.view.window?.windowScene?.delegate as? SceneDelegate)?.open(tab, of: notebook)
    }

    init() {
        super.init(style: .plain)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "page")
        navigationItem.searchController = search
        navigationItem.hidesSearchBarWhenScrolling = false
        let add = UIBarButtonItem(
            title: "New Page", image: UIImage(systemName: "plus"),
            primaryAction: UIAction { [weak self] _ in self?.newPage(under: nil) })
        if #available(iOS 26, *) {
            navigationItem.preferredSearchBarPlacement = .integrated
            toolbarItems = [navigationItem.searchBarPlacementBarButtonItem, .fixedSpace(), add]
        } else {
            toolbarItems = [.flexibleSpace(), add]
        }
        NotificationCenter.default.addObserver(
            self, selector: #selector(changed), name: Section.changed, object: nil)
        setUpArranging()
        if Prototype.sectionStrip {
            for direction in [UISwipeGestureRecognizer.Direction.left, .right] {
                let swipe = UISwipeGestureRecognizer(target: self, action: #selector(swiped))
                swipe.direction = direction
                tableView.addGestureRecognizer(swipe)
            }
        }
    }

    @objc private func swiped(_ swipe: UISwipeGestureRecognizer) {
        if let tab = strip.neighbour(swipe.direction == .left ? 1 : -1) { strip.pick(tab) }
    }

    override func tableView(_ tableView: UITableView, viewForHeaderInSection section: Int) -> UIView? {
        Prototype.sectionStrip && strip.notebook != nil ? strip : nil
    }

    override func tableView(_ tableView: UITableView, heightForHeaderInSection section: Int) -> CGFloat {
        Prototype.sectionStrip && strip.notebook != nil ? 50 : 0
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        navigationController?.setToolbarHidden(section == nil, animated: false)
    }

    func loading(_ tab: Tab) {
        section = nil
        items = []
        title = tab.name
        if Prototype.sectionStrip, let notebook = strip.notebook {
            title = notebook.name
            strip.select(tab.path)
        }
        tableView.reloadData()
        var loading = UIContentUnavailableConfiguration.loading()
        loading.text = "Opening…"
        contentUnavailableConfiguration = loading
        navigationController?.setToolbarHidden(true, animated: false)
    }

    /// Lets go of the section shown, as when its notebook goes.
    func clear() {
        section = nil
        items = []
        title = nil
        tableView.reloadData()
        contentUnavailableConfiguration = nil
        navigationController?.setToolbarHidden(true, animated: false)
    }

    func failed(_ problem: String?) {
        var empty = UIContentUnavailableConfiguration.empty()
        empty.text = "Can’t Open Section"
        empty.secondaryText = problem
        contentUnavailableConfiguration = empty
    }

    func load(_ section: Section) {
        self.section = section
        title = section.tab.name
        if Prototype.sectionStrip {
            title = section.notebook.name
            strip.show(section.notebook, selected: section.tab.path)
        }
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
        section?.delete(item.id)
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
        cell.accessibilityCustomActions = arrangeActions(item)
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        guard let section else { return }
        selected = items[indexPath.row].id
        onOpen?(section, items[indexPath.row].id)
    }

    override func tableView(
        _ tableView: UITableView, trailingSwipeActionsConfigurationForRowAt indexPath: IndexPath
    ) -> UISwipeActionsConfiguration? {
        // A swipe across the list changes section instead; the context menu deletes.
        if Prototype.sectionStrip { return nil }
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
                actions += self?.arrangeMenu(item) ?? []
            }
            actions.append(
                UIAction(
                    title: item.version == nil ? "Delete Page" : "Delete Conflict Page",
                    image: UIImage(systemName: "trash"), attributes: .destructive
                ) { _ in self?.delete(item) })
            return UIMenu(children: actions)
        })
    }
}

/// A page `sb_search` found.
struct Found: Decodable {
    let notebook: Int
    let section: String
    let page: String
    let title: String
    let inTitle: Bool
    let titleHits: [[Int]]
    let snippet: String
    let snippetHits: [[Int]]
}

/// The desktop's search over every open notebook's pages: title matches, then body matches,
/// each with its notebook and section. Searched from a section, it offers OneNote's scopes,
/// starting at This Section.
final class SearchViewController: UITableViewController, UISearchResultsUpdating, UISearchBarDelegate,
    UISearchControllerDelegate
{
    private enum Scope: Int, CaseIterable {
        case section, notebook, all

        var title: String {
            switch self {
            case .section: "This Section"
            case .notebook: "This Notebook"
            case .all: "All Notebooks"
            }
        }
    }

    /// The section searched from, which the scopes narrow to.
    private let place: () -> Section?
    private weak var search: UISearchController?
    private var found: [(found: Found, notebook: Notebook)] = []
    private var query = ""
    private var searching: DispatchWorkItem?

    private init(place: @escaping () -> Section?) {
        self.place = place
        super.init(style: .insetGrouped)
    }

    required init?(coder: NSCoder) { fatalError() }

    /// A search bar whose results this shows, with scopes where `place` gives the section
    /// it is searched from.
    static func controller(place: (() -> Section?)? = nil) -> UISearchController {
        let results = SearchViewController(place: place ?? { nil })
        let search = UISearchController(searchResultsController: results)
        results.search = search
        search.searchResultsUpdater = results
        search.delegate = results
        search.searchBar.delegate = results
        search.searchBar.placeholder = "Search"
        if place != nil { search.searchBar.scopeButtonTitles = Scope.allCases.map(\.title) }
        return search
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "found")
    }

    func searchBar(_ searchBar: UISearchBar, selectedScopeButtonIndexDidChange selectedScope: Int) {
        if let search { updateSearchResults(for: search) }
    }

    /// A scope chosen lasts for the search; the next starts at This Section again.
    func didDismissSearchController(_ searchController: UISearchController) {
        searchController.searchBar.selectedScopeButtonIndex = Scope.section.rawValue
    }

    func updateSearchResults(for controller: UISearchController) {
        searching?.cancel()
        let query = controller.searchBar.text?.trimmingCharacters(in: .whitespaces) ?? ""
        guard !query.isEmpty else { return show([], query: query) }
        let open = place()
        let scope = open == nil ? .all : Scope(rawValue: controller.searchBar.selectedScopeButtonIndex) ?? .all
        let notebooks = scope == .all ? Notebooks.all : open.map { [$0.notebook] } ?? []
        let searched = notebooks.filter { $0.handle != nil }
        let libraries = searched.map { Int(bitPattern: $0.handle) }
        let section = scope == .section ? open?.tab.path : nil
        // Each keystroke waits for the next before the notebooks are read.
        let work = DispatchWorkItem { [weak self] in
            if self?.found.isEmpty == true {
                var loading = UIContentUnavailableConfiguration.loading()
                loading.text = "Searching…"
                self?.contentUnavailableConfiguration = loading
            }
            background({
                let pointers = libraries.map { OpaquePointer(bitPattern: $0) }
                return decode([Found].self, sb_search(pointers, pointers.count, section, query)) ?? []
            }) { [weak self] found in
                let now = controller.searchBar.text?.trimmingCharacters(in: .whitespaces)
                guard now == query, open == nil || controller.searchBar.selectedScopeButtonIndex == scope.rawValue
                else { return }
                self?.show(found.map { ($0, searched[$0.notebook]) }, query: query)
            }
        }
        searching = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: work)
    }

    private func show(_ found: [(found: Found, notebook: Notebook)], query: String) {
        self.found = found
        self.query = query
        tableView.reloadData()
        contentUnavailableConfiguration =
            found.isEmpty && !query.isEmpty ? UIContentUnavailableConfiguration.search() : nil
    }

    /// The results in the title's group or the body's, as the desktop's headings split them.
    private func group(_ section: Int) -> ArraySlice<(found: Found, notebook: Notebook)> {
        let titled = found.prefix { $0.found.inTitle }
        return titled.isEmpty || section == 1 ? found.dropFirst(titled.count) : titled
    }

    override func numberOfSections(in tableView: UITableView) -> Int {
        (found.first?.found.inTitle == true ? 1 : 0) + (found.last?.found.inTitle == false ? 1 : 0)
    }

    override func tableView(_ tableView: UITableView, titleForHeaderInSection section: Int) -> String? {
        let pages = group(section)
        return "\(pages.first?.found.inTitle == true ? "Title" : "Body") contains: \(query) (\(pages.count))"
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        group(section).count
    }

    private func result(_ indexPath: IndexPath) -> (found: Found, notebook: Notebook) {
        let pages = group(indexPath.section)
        return pages[pages.startIndex + indexPath.row]
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let (page, notebook) = result(indexPath)
        let cell = tableView.dequeueReusableCell(withIdentifier: "found", for: indexPath)
        var content = UIListContentConfiguration.subtitleCell()
        let font = UIFont.preferredFont(forTextStyle: .body)
        content.attributedText =
            page.title.isEmpty
            ? NSAttributedString(string: "Untitled Page", attributes: [.font: font])
            : marked(page.title, page.titleHits, font: font, color: .label)
        let tab = notebook.tabs.first { $0.path == page.section }
        content.image = UIImage(systemName: "rectangle.portrait.fill")
        content.imageProperties.tintColor = tab?.uiColor ?? .systemGray
        // Where the page is, as the desktop names it: notebook, groups and section.
        let groups: [String] = tab?.group.split(separator: "/").map(String.init) ?? []
        let place = ([notebook.name] + groups + [tab?.name ?? ""]).filter { !$0.isEmpty }.joined(separator: " › ")
        let detail = NSMutableAttributedString(
            string: place + "\n",
            attributes: [.font: UIFont.preferredFont(forTextStyle: .footnote), .foregroundColor: UIColor.secondaryLabel])
        detail.append(
            marked(
                page.snippet, page.snippetHits, font: .preferredFont(forTextStyle: .subheadline),
                color: .secondaryLabel))
        content.secondaryAttributedText = detail
        content.secondaryTextProperties.numberOfLines = 3
        cell.contentConfiguration = content
        return cell
    }

    /// `text` with `hits`, UTF-16 ranges, marked in OneNote's yellow as the desktop marks them.
    private func marked(_ text: String, _ hits: [[Int]], font: UIFont, color: UIColor) -> NSAttributedString {
        let marked = NSMutableAttributedString(string: text, attributes: [.font: font, .foregroundColor: color])
        for hit in hits where hit.count == 2 && hit[1] <= marked.length {
            marked.addAttributes(
                [.backgroundColor: UIColor(red: 1, green: 0.82, blue: 0, alpha: 0.45), .foregroundColor: UIColor.label],
                range: NSRange(location: hit[0], length: hit[1] - hit[0]))
        }
        return marked
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        let (page, notebook) = result(indexPath)
        (view.window?.windowScene?.delegate as? SceneDelegate)?
            .open(page.section, of: notebook, page: page.page, reveal: .text(query))
    }
}
