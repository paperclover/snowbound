import UIKit

/// A section's sync status from `sb_library_sync_status`.
struct SectionSync: Decodable {
    enum State: UInt8, Decodable, Comparable {
        case upToDate, syncing, inUse, notConnected, readOnly, protected, unreadable, failed

        static func < (a: State, b: State) -> Bool { a.rawValue < b.rawValue }
    }

    let path: String
    let state: State
    /// Seconds since 1970 when the section file was last reached.
    let synced: Double?
    let queued: Int
    let error: String?
}

/// OneNote's Work Offline and Sync Now over every open notebook, and their status.
enum Sync {
    /// Posted on the main thread when a notebook's closed sections report.
    static let changed = Notification.Name("SyncChanged")
    private static let key = "offline"

    static var offline: Bool {
        get { UserDefaults.standard.bool(forKey: key) }
        set {
            UserDefaults.standard.set(newValue, forKey: key)
            for notebook in Notebooks.all { notebook.follow() }
            NotificationCenter.default.post(name: changed, object: nil)
        }
    }

    static func now() {
        for notebook in Notebooks.all {
            if let handle = notebook.handle { sb_library_sync_now(handle) }
        }
    }

    /// Every open notebook's sections with their status, read off the main thread.
    static func status(done: @escaping ([(Notebook, [SectionSync])]) -> Void) {
        let notebooks = Notebooks.all.compactMap { notebook in notebook.handle.map { (notebook, Int(bitPattern: $0)) } }
        background({
            notebooks.map { notebook, pointer in
                (notebook, decode([SectionSync].self, sb_library_sync_status(OpaquePointer(bitPattern: pointer))) ?? [])
            }
        }, done: done)
    }

    /// What `state` comes to for the reader, as the desktop names it.
    static func label(_ state: SectionSync.State) -> String {
        if offline { return "Working offline" }
        return switch state {
        case .upToDate: "Up to date"
        case .syncing: "Syncing…"
        case .inUse: "Section in use"
        case .notConnected: "Not connected"
        case .protected: "Password protected"
        case .unreadable: "Can’t read this section"
        case .readOnly, .failed: "Unable to sync"
        }
    }

    static func symbol(_ state: SectionSync.State) -> String {
        if offline { return "icloud.slash" }
        return switch state {
        case .upToDate: "checkmark.circle"
        case .syncing, .inUse: "arrow.triangle.2.circlepath"
        case .notConnected: "wifi.slash"
        case .protected: "lock"
        case .readOnly, .unreadable, .failed: "exclamationmark.triangle"
        }
    }

    /// What the reader can do about `state`.
    static func advice(_ state: SectionSync.State) -> String? {
        let device = UIDevice.current.model
        if offline { return "Changes stay on this \(device) until you sync." }
        return switch state {
        case .readOnly: "You can’t change this notebook where it’s stored. Changes stay on this \(device)."
        case .inUse: "Someone else is saving this section. Sync continues when they finish."
        case .notConnected: "Changes stay on this \(device) and sync when the notebook is back."
        case .protected: "Snowbound can’t open password-protected sections yet."
        default: nil
        }
    }

    static func changes(_ count: Int) -> String { count == 1 ? "1 change" : "\(count) changes" }

    /// When a section was last reached: the time, and the date too before today.
    static func when(_ seconds: Double) -> String {
        let date = Date(timeIntervalSince1970: seconds)
        return Calendar.current.isDateInToday(date)
            ? DateFormatter.localizedString(from: date, dateStyle: .none, timeStyle: .short)
            : DateFormatter.localizedString(from: date, dateStyle: .short, timeStyle: .short)
    }
}

/// The sync status in a list's toolbar, as Mail shows when it last checked; opens the sync
/// sheet.
final class SyncIndicator: UIButton {
    init() {
        var configuration = UIButton.Configuration.plain()
        configuration.imagePadding = 4
        configuration.preferredSymbolConfigurationForImage = UIImage.SymbolConfiguration(textStyle: .caption1)
        configuration.baseForegroundColor = .secondaryLabel
        configuration.titleLineBreakMode = .byTruncatingTail
        configuration.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer {
            var attributes = $0
            attributes.font = .preferredFont(forTextStyle: .caption1)
            return attributes
        }
        super.init(frame: .zero)
        self.configuration = configuration
        addAction(UIAction { [weak self] _ in self?.present() }, for: .primaryActionTriggered)
        for name in [Sync.changed, Section.changed] {
            NotificationCenter.default.addObserver(self, selector: #selector(refresh), name: name, object: nil)
        }
        refresh()
    }

    required init?(coder: NSCoder) { fatalError() }

    @objc private func refresh() {
        Sync.status { [weak self] notebooks in
            let sections = notebooks.flatMap(\.1)
            guard let self, let worst = sections.map(\.state).max() else {
                self?.isHidden = true
                return
            }
            isHidden = false
            let queued = sections.map(\.queued).reduce(0, +)
            let label = Sync.label(worst)
            configuration?.title = queued > 0 && worst != .syncing ? "\(label), \(Sync.changes(queued))" : label
            configuration?.image = UIImage(systemName: Sync.symbol(worst))
            configuration?.baseForegroundColor = worst >= .notConnected && !Sync.offline ? .systemOrange : .secondaryLabel
            accessibilityLabel = "Sync status: \(configuration?.title ?? label)"
            sizeToFit()
        }
    }

    private func present() {
        guard let controller = window?.rootViewController else { return }
        let navigation = UINavigationController(rootViewController: SyncViewController())
        navigation.sheetPresentationController?.detents = [.medium(), .large()]
        navigation.sheetPresentationController?.prefersGrabberVisible = true
        (controller.presentedViewController ?? controller).present(navigation, animated: true)
    }
}

/// OneNote's Notebook Sync Status: Work Offline, Sync Now, and each notebook's sections.
final class SyncViewController: UITableViewController {
    private var notebooks: [(Notebook, [SectionSync])] = []
    private var timer: Timer?
    private let offline = UISwitch()

    init() {
        super.init(style: .insetGrouped)
        title = "Sync Status"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "row")
        navigationItem.rightBarButtonItem = UIBarButtonItem(
            systemItem: .done, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        offline.addAction(UIAction { [weak self] _ in
            Sync.offline = self?.offline.isOn ?? false
        }, for: .valueChanged)
        for name in [Sync.changed, Section.changed] {
            NotificationCenter.default.addObserver(self, selector: #selector(refresh), name: name, object: nil)
        }
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        refresh()
        // The last sync time follows each round, which reports only when something changes.
        timer = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in self?.refresh() }
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        timer?.invalidate()
    }

    @objc private func refresh() {
        Sync.status { [weak self] notebooks in
            self?.notebooks = notebooks.filter { !$0.1.isEmpty }
            self?.tableView.reloadData()
        }
    }

    override func numberOfSections(in tableView: UITableView) -> Int { 1 + notebooks.count }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        section == 0 ? 2 : notebooks[section - 1].1.count
    }

    override func tableView(_ tableView: UITableView, titleForHeaderInSection section: Int) -> String? {
        section == 0 ? nil : notebooks[section - 1].0.name
    }

    override func tableView(_ tableView: UITableView, titleForFooterInSection section: Int) -> String? {
        guard section > 0 else { return Sync.offline ? Sync.advice(.upToDate) : nil }
        let (notebook, sections) = notebooks[section - 1]
        let worst = sections.map(\.state).max() ?? .upToDate
        let synced = sections.map(\.synced)
        let oldest = synced.contains(where: { $0 == nil }) ? nil : synced.compactMap { $0 }.min()
        return [
            notebook.location,
            oldest.map { "Last sync \(Sync.when($0))" },
            Sync.offline ? nil : Sync.advice(worst),
        ].compactMap { $0 }.joined(separator: "\n")
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "row", for: indexPath)
        cell.accessoryView = nil
        cell.selectionStyle = .none
        if indexPath.section == 0 {
            var content = UIListContentConfiguration.cell()
            if indexPath.row == 0 {
                content.text = "Work Offline"
                offline.isOn = Sync.offline
                cell.accessoryView = offline
            } else {
                content.text = "Sync Now"
                content.textProperties.color = .tintColor
                cell.selectionStyle = .default
            }
            cell.contentConfiguration = content
            return cell
        }
        let (notebook, sections) = notebooks[indexPath.section - 1]
        let sync = sections[indexPath.row]
        var content = UIListContentConfiguration.valueCell()
        let tab = notebook.tabs.first { $0.path == sync.path }
        content.text = tab?.name ?? URL(fileURLWithPath: sync.path).deletingPathExtension().lastPathComponent
        content.image = UIImage(systemName: "rectangle.portrait.fill")
        content.imageProperties.tintColor = tab?.uiColor ?? .systemGray
        let label = Sync.label(sync.state)
        content.secondaryText = sync.queued > 0 ? "\(label), \(Sync.changes(sync.queued))" : label
        if let error = sync.error {
            content = UIListContentConfiguration.subtitleCell()
            content.text = tab?.name ?? sync.path
            content.image = UIImage(systemName: "rectangle.portrait.fill")
            content.imageProperties.tintColor = tab?.uiColor ?? .systemGray
            content.secondaryText = "\(label): \(error)"
            content.secondaryTextProperties.color = .secondaryLabel
        }
        cell.contentConfiguration = content
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        guard indexPath == IndexPath(row: 1, section: 0) else { return }
        Sync.now()
        refresh()
    }
}
