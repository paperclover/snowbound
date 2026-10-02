import UIKit

/// A page's place in its section's order, as `sb_section_arrange` takes it.
struct Placed: Codable, Equatable {
    let id: String
    var level: Int
}

/// Where `Arranging.move(page:)` took a page, from `sb_library_move_page`.
private struct MovedPage: Decodable {
    let page: String
    let added: String?
    let before: String?
    let level: Int
}

/// The catalog path of the folder holding the section or group at `path`; "" for the
/// notebook's top.
func parentFolder(of path: String) -> String {
    path.range(of: "/", options: .backwards).map { String(path[..<$0.lowerBound]) } ?? ""
}

private func json(_ value: some Encodable) -> String {
    (try? JSONEncoder().encode(value)).flatMap { String(data: $0, encoding: .utf8) } ?? ""
}

/// Moves of pages, sections, section groups and notebooks, each made as the desktop makes it,
/// as one edit through the notebook, and kept for Undo: shake, three-finger swipe or ⌘Z in
/// either list.
enum Arranging {
    static let history = UndoManager()
    private static let owner = NSObject()

    /// ⌘Z and ⇧⌘Z in a list, which, unlike a text view, has none of its own.
    static let keyCommands = [
        UIKeyCommand(title: "Undo", action: #selector(UIResponder.undoArranging), input: "z", modifierFlags: .command),
        UIKeyCommand(title: "Redo", action: #selector(UIResponder.redoArranging), input: "z", modifierFlags: [.command, .shift]),
    ]

    /// Keeps `undo` as Undo for `name`. Called as the change starts, so that a change Undo
    /// makes keeps its own undo as Redo.
    private static func done(_ name: String, undo: @escaping () -> Void) {
        history.registerUndo(withTarget: owner) { _ in undo() }
        history.setActionName(name)
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
    }

    /// Runs `work` off the main thread after the changes before it, then `then` with its result
    /// on the main thread.
    private static func later<T>(_ work: @escaping () -> T, then: @escaping (T) -> Void) {
        queue.async {
            let result = work()
            DispatchQueue.main.async { then(result) }
        }
    }
    private static let queue = DispatchQueue(label: "Arranging")

    private static func failed(_ title: String) {
        let window = UIApplication.shared.connectedScenes.lazy.compactMap { ($0 as? UIWindowScene)?.keyWindow }.first
        var top = window?.rootViewController
        while let presented = top?.presentedViewController { top = presented }
        top?.alert(title, "Check that the notebook can be reached, then try again.")
    }

    /// The pages `rows` lists with page `id` moved to `index` of them, at its level or under
    /// the page now above it, as the desktop drops a dragged page.
    static func order(_ rows: [Row], moving id: String, to index: Int) -> [Placed] {
        var order = rows.map { Placed(id: $0.id, level: $0.level) }
        guard let from = order.firstIndex(where: { $0.id == id }) else { return order }
        var page = order.remove(at: from)
        let at = min(max(index, 0), order.count)
        page.level = min(page.level, at == 0 ? 1 : order[at - 1].level + 1)
        order.insert(page, at: at)
        return order
    }

    /// Puts the pages of `section` in `order`, page `id` the one moved, as one edit.
    static func arrange(_ section: Section, _ order: [Placed], moving id: String) {
        let before = section.rows.map { Placed(id: $0.id, level: $0.level) }
        guard order != before, section.arrange(order, moving: id) else { return }
        let (notebook, path) = (section.notebook, section.tab.path)
        done("Move Page") {
            opening(path, of: notebook) { arrange($0, before, moving: id) }
        }
    }

    /// Gives page `id` of `section` `level` where it stands, as Make Subpage and Promote
    /// Subpage do.
    static func indent(_ section: Section, _ id: String, to level: Int) {
        var order = section.rows.map { Placed(id: $0.id, level: $0.level) }
        guard let at = order.firstIndex(where: { $0.id == id }), order[at].level != level else { return }
        let was = order[at].level
        order[at].level = level
        guard section.arrange(order, moving: id) else { return }
        let (notebook, path) = (section.notebook, section.tab.path)
        done(level > was ? "Make Subpage" : "Promote Subpage") {
            opening(path, of: notebook) { indent($0, id, to: was) }
        }
    }

    /// Hands `then` the section at `path` of `notebook`, opening it where it isn't open.
    private static func opening(_ path: String, of notebook: Notebook, then: @escaping (Section) -> Void) {
        guard let tab = notebook.tabs.first(where: { $0.path == path }) else { return }
        Section.open(tab, of: notebook) { section, _ in section.map(then) }
    }

    /// Moves page `id` of the section at `from` into the section at `to`, before `before` at
    /// `level` (last without), as OneNote moves a page dropped on a section's tab.
    static func move(
        page id: String, of notebook: Notebook, from: String, to: String, before: String? = nil, level: Int = 1,
        discard: String? = nil
    ) {
        guard let library = notebook.handle else { return }
        let pointer = Int(bitPattern: library)
        let author = Author.name ?? ""
        let (date, time) = titleDate()
        // Undo takes back what the move reports, once it has.
        var reported: MovedPage?
        done("Move Page") {
            guard let moved = reported else { return }
            move(
                page: moved.page, of: notebook, from: to, to: from, before: moved.before, level: moved.level,
                discard: moved.added)
        }
        later({
            decode(
                MovedPage.self,
                sb_library_move_page(
                    OpaquePointer(bitPattern: pointer), from, id, to, before, UInt32(level), discard, author, date,
                    time))
        }) { moved in
            reported = moved
            for section in Section.all where section.notebook === notebook && [from, to].contains(section.tab.path) {
                section.listed()
            }
            if moved == nil { failed("Can’t Move Page") }
        }
    }

    /// Puts the section or group at `path` of `notebook` into `folder`, ordered as `order`
    /// names its entries once moved; a section shown opens again where it went.
    static func place(_ path: String, of notebook: Notebook, in folder: String, order: [String] = []) {
        guard let library = notebook.handle else { return }
        let home = parentFolder(of: path)
        let name = path.split(separator: "/").last.map(String.init) ?? path
        let placed = home == folder ? path : folder.isEmpty ? name : folder + "/" + name
        let undo = notebook.entries(in: home)
        done("Move Section") { place(placed, of: notebook, in: home, order: undo) }
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0.delegate as? SceneDelegate }
        let released = home == folder ? [] : scenes.compactMap { scene in scene.release(path, of: notebook).map { (scene, $0) } }
        let pointer = Int(bitPattern: library)
        let paths = json(order)
        later({ take(sb_library_place(OpaquePointer(bitPattern: pointer), path, folder, paths)) != nil }) { moved in
            notebook.reload {
                NotificationCenter.default.post(name: Notebook.listed, object: notebook)
                for (scene, shown) in released {
                    let section = moved ? follow(shown.section, from: path, to: placed) : shown.section
                    if let tab = notebook.tabs.first(where: { $0.path == section }) {
                        scene.open(tab, of: notebook, page: shown.page)
                    }
                }
                if !moved { failed("Can’t Move Section") }
            }
        }
    }

    /// `current`, a section's path, after the section or group at `from` went to `to`.
    private static func follow(_ current: String, from: String, to: String) -> String {
        current == from ? to : current.hasPrefix(from + "/") ? to + current.dropFirst(from.count) : current
    }

    /// Puts `notebook` at `index` among the notebooks of its place in the list.
    static func move(_ notebook: Notebook, to index: Int) {
        guard let from = Notebooks.siblings(of: notebook).firstIndex(where: { $0 === notebook }), from != index else {
            return
        }
        Notebooks.move(notebook, to: index)
        NotificationCenter.default.post(name: Notebook.listed, object: notebook)
        done("Move Notebook") { move(notebook, to: from) }
    }
}

extension UIResponder {
    @objc func undoArranging() { Arranging.history.undo() }
    @objc func redoArranging() { Arranging.history.redo() }
}

extension Section {
    /// Puts page `id` where `order` places it, as one edit; false where it isn't listed.
    func arrange(_ order: [Placed], moving id: String) -> Bool {
        guard sb_section_arrange(handle, json(order), json([id])) else { return false }
        listed()
        return true
    }

    /// Lists the pages again and tells the views.
    func listed() {
        reloadRows()
        NotificationCenter.default.post(name: Self.changed, object: self, userInfo: ["flags": Self.listed])
    }
}

extension Notebook {
    /// The section groups, by catalog path, in the order the list shows them.
    var groups: [String] {
        var groups: [String] = []
        for tab in tabs {
            var path = ""
            for part in tab.group.split(separator: "/") {
                path += (path.isEmpty ? "" : "/") + part
                if !groups.contains(path) { groups.append(path) }
            }
        }
        return groups
    }

    /// The sections and groups directly in `folder`, by catalog path, as the list shows them.
    func entries(in folder: String) -> [String] {
        tabs.filter { $0.group == folder }.map(\.path) + groups.filter { parentFolder(of: $0) == folder }
    }

    /// Where the section or group at `path` can move: the notebook's top and its groups, by
    /// catalog path, leaving out its own folder, itself and its groups.
    func destinations(of path: String) -> [String] {
        ([""] + groups).filter { $0 != parentFolder(of: path) && $0 != path && !$0.hasPrefix(path + "/") }
    }

    /// The sections a page of the section at `path` can move to.
    func sections(besides path: String) -> [Tab] {
        tabs.filter { $0.path != path && $0.readable && !$0.locked }
    }
}

/// A page dragged out of the page list.
final class PageDrag {
    let section: Section
    let id: String

    init(section: Section, id: String) {
        self.section = section
        self.id = id
    }
}

/// A sheet offering `actions`, for VoiceOver’s Move actions, from `view`.
private func sheet(_ title: String, _ actions: [UIAction], from view: UIView, in controller: UIViewController) {
    let sheet = UIAlertController(title: title, message: nil, preferredStyle: .actionSheet)
    for action in actions {
        sheet.addAction(UIAlertAction(title: action.title, style: .default) { _ in action.performWithSender(nil, target: nil) })
    }
    sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
    sheet.popoverPresentationController?.sourceView = view
    sheet.popoverPresentationController?.sourceRect = view.bounds
    controller.present(sheet, animated: true)
}

// MARK: Pages

extension PagesViewController: UITableViewDragDelegate, UITableViewDropDelegate {
    func setUpArranging() {
        tableView.dragDelegate = self
        tableView.dropDelegate = self
        tableView.dragInteractionEnabled = true
        navigationItem.rightBarButtonItem = editButtonItem
    }

    override var canBecomeFirstResponder: Bool { true }
    override var undoManager: UndoManager? { Arranging.history }
    override var keyCommands: [UIKeyCommand]? { Arranging.keyCommands }

    /// Undo in the list takes back its moves.
    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        becomeFirstResponder()
    }

    /// Page `id` moved to `index` of the section's pages.
    private func move(_ id: String, to index: Int) {
        guard let section else { return }
        Arranging.arrange(section, Arranging.order(section.rows, moving: id, to: index), moving: id)
        becomeFirstResponder()
    }

    /// Page `id` a level deeper or shallower, where it stays.
    private func indent(_ id: String, by change: Int) {
        guard let section, let row = section.rows.first(where: { $0.id == id }) else { return }
        Arranging.indent(section, id, to: row.level + change)
        becomeFirstResponder()
    }

    /// Whether page `at` of `rows` can become a subpage, as the desktop allows: under a page
    /// at its level or deeper, at most three deep.
    private func indentable(_ rows: [Row], _ at: Int) -> Bool {
        at > 0 && rows[at].level < 3 && rows[at - 1].level >= rows[at].level
    }

    private func moveActions(_ item: Item) -> [UIAction] {
        guard let section else { return [] }
        return section.notebook.sections(besides: section.tab.path).map { tab in
            UIAction(
                title: tab.name, subtitle: tab.group.isEmpty ? nil : tab.group,
                image: UIImage(systemName: "rectangle.portrait.fill")?.withTintColor(tab.uiColor, renderingMode: .alwaysOriginal)
            ) { [weak self] _ in
                Arranging.move(page: item.row.id, of: section.notebook, from: section.tab.path, to: tab.path)
                self?.becomeFirstResponder()
            }
        }
    }

    /// Make Subpage, Promote Subpage and Move to Section, for a page's menu.
    func arrangeMenu(_ item: Item) -> [UIMenuElement] {
        guard let rows = section?.rows, let at = rows.firstIndex(where: { $0.id == item.row.id }) else { return [] }
        let id = item.row.id
        var actions: [UIMenuElement] = [
            UIAction(
                title: "Make Subpage", image: UIImage(systemName: "increase.indent"),
                attributes: indentable(rows, at) ? [] : .disabled
            ) { [weak self] _ in self?.indent(id, by: 1) },
            UIAction(
                title: "Promote Subpage", image: UIImage(systemName: "decrease.indent"),
                attributes: rows[at].level > 1 ? [] : .disabled
            ) { [weak self] _ in self?.indent(id, by: -1) },
        ]
        let moves = moveActions(item)
        if !moves.isEmpty {
            actions.append(UIMenu(title: "Move to Section", image: UIImage(systemName: "arrow.right.doc.on.clipboard"), children: moves))
        }
        return [UIMenu(options: .displayInline, children: actions)]
    }

    /// VoiceOver's ways to move a page without dragging it.
    func arrangeActions(_ item: Item) -> [UIAccessibilityCustomAction]? {
        guard item.version == nil, let rows = section?.rows, let at = rows.firstIndex(where: { $0.id == item.row.id })
        else { return nil }
        let id = item.row.id
        var actions: [UIAccessibilityCustomAction] = []
        let add = { (name: String, act: @escaping () -> Void) in
            actions.append(UIAccessibilityCustomAction(name: name) { _ in act(); return true })
        }
        if at > 0 { add("Move Up") { [weak self] in self?.move(id, to: at - 1) } }
        if at < rows.count - 1 { add("Move Down") { [weak self] in self?.move(id, to: at + 1) } }
        if indentable(rows, at) { add("Make Subpage") { [weak self] in self?.indent(id, by: 1) } }
        if rows[at].level > 1 { add("Promote Subpage") { [weak self] in self?.indent(id, by: -1) } }
        if !moveActions(item).isEmpty {
            add("Move to Section…") { [weak self] in
                guard let self, let row = items.firstIndex(where: { $0.id == id }),
                    let cell = tableView.cellForRow(at: IndexPath(row: row, section: 0))
                else { return }
                sheet("Move to Section", moveActions(item), from: cell, in: self)
            }
        }
        return actions
    }

    override func tableView(
        _ tableView: UITableView, leadingSwipeActionsConfigurationForRowAt indexPath: IndexPath
    ) -> UISwipeActionsConfiguration? {
        // A swipe across the list changes section instead.
        guard !Prototype.sectionStrip, let rows = section?.rows, items[indexPath.row].version == nil,
            let at = rows.firstIndex(where: { $0.id == items[indexPath.row].row.id })
        else { return nil }
        let id = rows[at].id
        var actions: [UIContextualAction] = []
        if indentable(rows, at) {
            let indent = UIContextualAction(style: .normal, title: "Make Subpage") { [weak self] _, _, done in
                self?.indent(id, by: 1)
                done(true)
            }
            indent.image = UIImage(systemName: "increase.indent")
            indent.backgroundColor = .systemIndigo
            actions.append(indent)
        }
        if rows[at].level > 1 {
            let promote = UIContextualAction(style: .normal, title: "Promote Subpage") { [weak self] _, _, done in
                self?.indent(id, by: -1)
                done(true)
            }
            promote.image = UIImage(systemName: "decrease.indent")
            promote.backgroundColor = .systemTeal
            actions.append(promote)
        }
        return UISwipeActionsConfiguration(actions: actions)
    }

    override func tableView(_ tableView: UITableView, canMoveRowAt indexPath: IndexPath) -> Bool {
        items[indexPath.row].version == nil
    }

    override func tableView(_ tableView: UITableView, moveRowAt from: IndexPath, to: IndexPath) {
        var moved = items
        let item = moved.remove(at: from.row)
        moved.insert(item, at: to.row)
        guard let index = moved.filter({ $0.version == nil }).firstIndex(where: { $0.id == item.id }) else { return }
        // The table finishes its move before the list reloads.
        DispatchQueue.main.async { [weak self] in self?.move(item.id, to: index) }
    }

    override func tableView(
        _ tableView: UITableView, editingStyleForRowAt indexPath: IndexPath
    ) -> UITableViewCell.EditingStyle {
        tableView.isEditing ? .none : .delete
    }

    override func tableView(_ tableView: UITableView, shouldIndentWhileEditingRowAt indexPath: IndexPath) -> Bool {
        false
    }

    func tableView(
        _ tableView: UITableView, itemsForBeginning session: UIDragSession, at indexPath: IndexPath
    ) -> [UIDragItem] {
        let item = items[indexPath.row]
        guard let section, item.version == nil else { return [] }
        let drag = UIDragItem(itemProvider: NSItemProvider())
        drag.localObject = PageDrag(section: section, id: item.row.id)
        return [drag]
    }

    func tableView(_ tableView: UITableView, dragSessionIsRestrictedToDraggingApplication session: UIDragSession) -> Bool {
        true
    }

    /// A wide window shows the notebooks beside the pages while a page is dragged, so it can
    /// be dropped on a section there.
    func tableView(_ tableView: UITableView, dragSessionWillBegin session: UIDragSession) {
        guard let split = splitViewController, !split.isCollapsed, split.displayMode != .twoBesideSecondary else { return }
        session.localContext = split.preferredDisplayMode
        split.preferredDisplayMode = .twoBesideSecondary
    }

    func tableView(_ tableView: UITableView, dragSessionDidEnd session: UIDragSession) {
        guard let mode = session.localContext as? UISplitViewController.DisplayMode else { return }
        splitViewController?.preferredDisplayMode = mode
    }

    func tableView(
        _ tableView: UITableView, dropSessionDidUpdate session: UIDropSession, withDestinationIndexPath: IndexPath?
    ) -> UITableViewDropProposal {
        let page = session.localDragSession?.items.first?.localObject as? PageDrag
        guard let page, page.section === section else {
            return UITableViewDropProposal(operation: .forbidden)
        }
        return UITableViewDropProposal(operation: .move, intent: .insertAtDestinationIndexPath)
    }

    /// Drops within the list reorder through `moveRowAt`.
    func tableView(_ tableView: UITableView, performDropWith coordinator: UITableViewDropCoordinator) {}
}

// MARK: Notebooks, sections and groups

extension NotebooksViewController: UICollectionViewDragDelegate, UICollectionViewDropDelegate {
    func setUpArranging() {
        collectionView.dragDelegate = self
        collectionView.dropDelegate = self
        collectionView.dragInteractionEnabled = true
        // A sidebar's trailing edge has no room left for it.
        if UIDevice.current.userInterfaceIdiom == .pad {
            navigationItem.leftBarButtonItem = editButtonItem
        } else {
            navigationItem.rightBarButtonItems?.append(editButtonItem)
        }
        // Rows dragged between rows, and by Edit's reorder handles, move through the data source.
        dataSource.reorderingHandlers.canReorderItem = { [weak self] item in self?.movable(item) == true }
        // The data source is still applying the move when this runs.
        dataSource.reorderingHandlers.didReorder = { [weak self] transaction in
            DispatchQueue.main.async { self?.reordered(transaction) }
        }
    }

    override var canBecomeFirstResponder: Bool { true }
    override var undoManager: UndoManager? { Arranging.history }
    override var keyCommands: [UIKeyCommand]? { Arranging.keyCommands }

    /// Undo in the list takes back its moves.
    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        becomeFirstResponder()
    }

    override func setEditing(_ editing: Bool, animated: Bool) {
        super.setEditing(editing, animated: animated)
        collectionView.isEditing = editing
        collectionView.dragInteractionEnabled = !editing
    }

    private func movable(_ item: Item) -> Bool {
        switch item {
        case .notebook, .group, .section: true
        default: false
        }
    }

    /// The notebook and catalog path of a section or group row.
    private func entry(_ item: Item) -> (notebook: Notebook, path: String)? {
        switch item {
        case .group(let id, let path), .section(let id, let path): notebook(id).map { ($0, path) }
        default: nil
        }
    }

    /// The group or notebook top a row stands for, as a folder an entry of `notebook` can go into.
    private func folder(_ item: Item, in notebook: Notebook) -> String? {
        switch item {
        case .notebook(let id) where id == notebook.id: ""
        case .group(let id, let path) where id == notebook.id: path
        default: nil
        }
    }

    /// Moves the section or group at `path` within its folder by `offset`.
    private func shift(_ path: String, of notebook: Notebook, by offset: Int) {
        var order = notebook.entries(in: parentFolder(of: path))
        guard let at = order.firstIndex(of: path), order.indices.contains(at + offset) else { return }
        order.swapAt(at, at + offset)
        Arranging.place(path, of: notebook, in: parentFolder(of: path), order: order)
        becomeFirstResponder()
    }

    private func moveActions(_ path: String, of notebook: Notebook) -> [UIAction] {
        notebook.destinations(of: path).map { folder in
            UIAction(
                title: folder.isEmpty ? notebook.name : folder.split(separator: "/").joined(separator: " › "),
                image: UIImage(systemName: folder.isEmpty ? "book.closed" : "folder")
            ) { [weak self] _ in
                Arranging.place(path, of: notebook, in: folder)
                self?.becomeFirstResponder()
            }
        }
    }

    /// Move, into the notebook's top or a group, for a section's or group's menu; Move Up and
    /// Move Down for a notebook's, as the desktop's.
    func arrangeMenu(_ item: Item) -> [UIMenuElement] {
        if case .notebook(let id) = item, let notebook = notebook(id) {
            let siblings = Notebooks.siblings(of: notebook)
            guard let at = siblings.firstIndex(where: { $0 === notebook }), siblings.count > 1 else { return [] }
            return [
                UIMenu(options: .displayInline, children: [
                    UIAction(title: "Move Up", image: UIImage(systemName: "arrow.up"), attributes: at > 0 ? [] : .disabled) {
                        _ in Arranging.move(notebook, to: at - 1)
                    },
                    UIAction(
                        title: "Move Down", image: UIImage(systemName: "arrow.down"),
                        attributes: at < siblings.count - 1 ? [] : .disabled
                    ) { _ in Arranging.move(notebook, to: at + 1) },
                ])
            ]
        }
        guard let (notebook, path) = entry(item) else { return [] }
        let moves = moveActions(path, of: notebook)
        return moves.isEmpty ? [] : [UIMenu(title: "Move", image: UIImage(systemName: "folder"), children: moves)]
    }

    /// Adds Edit's reorder handle and VoiceOver's ways to move to a row.
    func arranging(_ cell: UICollectionViewListCell, _ item: Item) {
        if movable(item) { cell.accessories.append(.reorder(displayed: .whenEditing)) }
        var actions: [UIAccessibilityCustomAction] = []
        let add = { (name: String, act: @escaping () -> Void) in
            actions.append(UIAccessibilityCustomAction(name: name) { _ in act(); return true })
        }
        if case .notebook(let id) = item, let notebook = notebook(id) {
            let siblings = Notebooks.siblings(of: notebook)
            if let at = siblings.firstIndex(where: { $0 === notebook }) {
                if at > 0 { add("Move Up") { Arranging.move(notebook, to: at - 1) } }
                if at < siblings.count - 1 { add("Move Down") { Arranging.move(notebook, to: at + 1) } }
            }
        } else if let (notebook, path) = entry(item) {
            let order = notebook.entries(in: parentFolder(of: path))
            if let at = order.firstIndex(of: path) {
                if at > 0 { add("Move Up") { [weak self] in self?.shift(path, of: notebook, by: -1) } }
                if at < order.count - 1 { add("Move Down") { [weak self] in self?.shift(path, of: notebook, by: 1) } }
            }
            if !notebook.destinations(of: path).isEmpty {
                add("Move…") { [weak self, weak cell] in
                    guard let self, let cell else { return }
                    sheet("Move", moveActions(path, of: notebook), from: cell, in: self)
                }
            }
        }
        cell.accessibilityCustomActions = actions
    }

    /// Carries out a move the data source made: a notebook among its place's, or a
    /// section or group into the folder it landed in; anywhere else it goes back.
    private func reordered(_ transaction: NSDiffableDataSourceTransaction<Location, Item>) {
        becomeFirstResponder()
        let moved = transaction.difference.insertions.lazy.compactMap { change -> Item? in
            if case .insert(_, let item, _) = change { return item }
            return nil
        }.first
        guard let moved, let list = transaction.sectionTransactions.first?.finalSnapshot,
            let parent = list.parent(of: moved)
        else { return reload() }
        let siblings = list.snapshot(of: parent).rootItems
        if case .notebook(let id) = moved, let notebook = notebook(id), case .location = parent {
            let notebooks = siblings.filter { if case .notebook = $0 { true } else { false } }
            return Arranging.move(notebook, to: notebooks.firstIndex(of: moved) ?? 0)
        }
        guard let (notebook, path) = entry(moved), let folder = folder(parent, in: notebook),
            folder != path, !folder.hasPrefix(path + "/")
        else { return reload() }
        let name = path.split(separator: "/").last.map(String.init) ?? path
        let order = siblings.compactMap { sibling in
            sibling == moved ? (folder.isEmpty ? name : folder + "/" + name) : entry(sibling)?.path
        }
        Arranging.place(path, of: notebook, in: folder, order: order)
    }

    func collectionView(
        _ collectionView: UICollectionView, itemsForBeginning session: UIDragSession, at indexPath: IndexPath
    ) -> [UIDragItem] {
        guard let item = dataSource.itemIdentifier(for: indexPath), movable(item) else { return [] }
        let drag = UIDragItem(itemProvider: NSItemProvider())
        drag.localObject = item
        return [drag]
    }

    func collectionView(
        _ collectionView: UICollectionView, dragSessionIsRestrictedToDraggingApplication session: UIDragSession
    ) -> Bool {
        true
    }

    func collectionView(_ collectionView: UICollectionView, canHandle session: UIDropSession) -> Bool {
        session.localDragSession != nil
    }

    /// The row under `session`'s touch.
    private func row(under session: UIDropSession) -> (IndexPath, Item, UICollectionViewCell)? {
        let point = session.location(in: collectionView)
        guard let indexPath = collectionView.indexPathForItem(at: point),
            let item = dataSource.itemIdentifier(for: indexPath), let cell = collectionView.cellForItem(at: indexPath)
        else { return nil }
        return (indexPath, item, cell)
    }

    /// The section a dragged page would move to, under the touch.
    private func section(for page: PageDrag, under session: UIDropSession) -> Tab? {
        guard let (_, item, _) = row(under: session), case .section(let id, let path) = item,
            id == page.section.notebook.id
        else { return nil }
        return page.section.notebook.sections(besides: page.section.tab.path).first { $0.path == path }
    }

    func collectionView(
        _ collectionView: UICollectionView, dropSessionDidUpdate session: UIDropSession,
        withDestinationIndexPath destinationIndexPath: IndexPath?
    ) -> UICollectionViewDropProposal {
        let dragged = session.localDragSession?.items.first?.localObject
        if let page = dragged as? PageDrag {
            guard section(for: page, under: session) != nil else { return UICollectionViewDropProposal(operation: .forbidden) }
            return UICollectionViewDropProposal(operation: .move, intent: .insertIntoDestinationIndexPath)
        }
        guard dragged is Item else { return UICollectionViewDropProposal(operation: .forbidden) }
        // A row dropped between rows moves through the data source's reordering handlers.
        return UICollectionViewDropProposal(operation: .move, intent: .insertAtDestinationIndexPath)
    }

    func collectionView(_ collectionView: UICollectionView, performDropWith coordinator: UICollectionViewDropCoordinator) {
        guard let drop = coordinator.items.first else { return }
        let session = coordinator.session
        guard let page = drop.dragItem.localObject as? PageDrag, let tab = section(for: page, under: session),
            let (indexPath, _, cell) = row(under: session)
        else { return }
        coordinator.drop(drop.dragItem, intoItemAt: indexPath, rect: cell.bounds)
        Arranging.move(page: page.id, of: page.section.notebook, from: page.section.tab.path, to: tab.path)
        becomeFirstResponder()
    }
}
