import UIKit

/// Design prototypes, off unless a launch argument turns one on, as
/// `-SnowboundReading YES`, `-SnowboundWelcome YES`, `-SnowboundSectionStrip YES` or
/// `-SnowboundQuickNote YES` (Xcode's scheme, `simctl launch`, or `devicectl … --arguments`).
enum Prototype {
    /// Page menu: Reading View, the page reflowed into the screen's width, read-only.
    static var reading: Bool { UserDefaults.standard.bool(forKey: "SnowboundReading") }
}

extension Prototype {
    /// What the page menu says under Reading View for a verdict from `sb_view_reading`.
    static func readingNote(_ verdict: String) -> String? {
        let notes = [
            ("Fits", "Page already fits"),
            ("RightToLeft", "Right-to-left pages not yet"),
            ("Drawn", "Mostly drawings"),
            ("Arranged", "Laid out as a canvas"),
            ("Asides", "Side notes follow their lines"),
            ("SideBySide", "Columns read left to right"),
            ("Group", "Overlapping parts stay together"),
            ("Wide", "Some parts stay wide"),
        ]
        let found = notes.filter { verdict.contains($0.0) }.map(\.1)
        return found.isEmpty ? nil : found.joined(separator: " · ")
    }
}

extension Prototype {
    /// First launch: a welcome where the list would be empty, the first notebook found opened
    /// at its first page, and the name asked at the first edit rather than the first open.
    static var welcome: Bool { UserDefaults.standard.bool(forKey: "SnowboundWelcome") }

    /// The notebook list's welcome while there are no notebooks, nil once there are or while
    /// iCloud Drive is still being looked up.
    static func welcome(
        new: @escaping () -> Void, openFolder: @escaping () -> Void, connect: @escaping () -> Void
    ) -> UIContentUnavailableConfiguration? {
        guard Notebooks.all.isEmpty, !ICloud.signedIn || ICloud.documents != nil else { return nil }
        var welcome = UIContentUnavailableConfiguration.empty()
        welcome.image = .notebook(nil, side: 64)
        welcome.text = "No Notebooks Yet"
        welcome.secondaryText =
            ICloud.documents != nil
            ? "New notebooks go in iCloud Drive, where OneNote and your other devices can open them."
            : "New notebooks stay on this iPhone. Turn on iCloud Drive to use them on all your devices."
        var button = UIButton.Configuration.borderedProminent()
        button.title = "New Notebook"
        welcome.button = button
        welcome.buttonProperties.primaryAction = UIAction { _ in new() }
        var open = UIButton.Configuration.plain()
        open.title = "Open Existing Notebook"
        welcome.secondaryButton = open
        var elsewhere = [
            UIAction(title: "Open Folder", image: UIImage(systemName: "folder")) { _ in openFolder() },
            UIAction(title: "Connect to Server", image: UIImage(systemName: "server.rack")) { _ in connect() },
        ]
        if !ICloud.signedIn {
            elsewhere.append(
                UIAction(title: "Turn On iCloud Drive", image: UIImage(systemName: "icloud")) { _ in
                    if let settings = URL(string: UIApplication.openSettingsURLString) {
                        UIApplication.shared.open(settings)
                    }
                })
        }
        welcome.secondaryButtonProperties.menu = UIMenu(children: elsewhere)
        return welcome
    }

    /// Asks for the name edits are stored under, then runs `then`.
    static func askName(from controller: UIViewController, then: @escaping () -> Void) {
        let alert = UIAlertController(
            title: "Your Name", message: "OneNote shows it beside the pages and changes you make.",
            preferredStyle: .alert)
        let save = UIAlertAction(title: "Continue", style: .default) { [weak alert] _ in
            Author.name = alert?.textFields?.first?.text?.trimmingCharacters(in: .whitespaces)
            then()
        }
        save.isEnabled = false
        alert.addTextField { field in
            field.placeholder = "Name"
            field.textContentType = .name
            field.autocapitalizationType = .words
            NotificationCenter.default.addObserver(
                forName: UITextField.textDidChangeNotification, object: field, queue: .main
            ) { _ in save.isEnabled = !(field.text ?? "").trimmingCharacters(in: .whitespaces).isEmpty }
        }
        alert.addAction(UIAlertAction(title: "Not Now", style: .cancel))
        alert.addAction(save)
        alert.preferredAction = save
        (controller.presentedViewController ?? controller).present(alert, animated: true)
    }
}

extension Prototype {
    /// Pages list: the notebook's sections as OneNote's coloured tabs over the pages; a tab or
    /// a swipe across the list changes section.
    static var sectionStrip: Bool { UserDefaults.standard.bool(forKey: "SnowboundSectionStrip") }
}

/// A notebook's sections as a row of coloured tabs, the open one raised; a section group is a
/// tab whose menu holds its sections.
final class SectionStrip: UIView {
    private let scroll = UIScrollView()
    private let row = UIStackView()
    private let rule = UIView()
    private(set) var notebook: Notebook?
    private var selected = ""
    private let onPick: (Tab, Notebook) -> Void

    init(onPick: @escaping (Tab, Notebook) -> Void) {
        self.onPick = onPick
        super.init(frame: CGRect(x: 0, y: 0, width: 320, height: 50))
        backgroundColor = .systemBackground
        scroll.showsHorizontalScrollIndicator = false
        row.axis = .horizontal
        row.spacing = 2
        row.alignment = .bottom
        for view in [scroll, rule] as [UIView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        row.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(row)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.topAnchor.constraint(equalTo: topAnchor, constant: 6),
            scroll.bottomAnchor.constraint(equalTo: rule.topAnchor),
            rule.leadingAnchor.constraint(equalTo: leadingAnchor),
            rule.trailingAnchor.constraint(equalTo: trailingAnchor),
            rule.bottomAnchor.constraint(equalTo: bottomAnchor),
            rule.heightAnchor.constraint(equalToConstant: 3),
            row.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 12),
            row.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -12),
            row.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            row.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            row.heightAnchor.constraint(equalTo: scroll.frameLayoutGuide.heightAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    /// The readable sections in the notebook's order, groups' sections where the group is.
    private var readable: [Tab] { notebook?.tabs.filter(\.readable) ?? [] }

    func show(_ notebook: Notebook, selected path: String) {
        let first = self.notebook == nil
        self.notebook = notebook
        select(path)
        #if DEBUG
        // `SNOWBOUND_STRIP_TOUR=N` steps N sections on, one each 1.5 s, for recordings.
        if first, let steps = ProcessInfo.processInfo.environment["SNOWBOUND_STRIP_TOUR"].flatMap(Int.init) {
            for step in 1...max(1, steps) {
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.5 * Double(step)) { [weak self] in
                    if let tab = self?.neighbour(1) { self?.pick(tab) }
                }
            }
        }
        #endif
    }

    func select(_ path: String) {
        selected = path
        guard let notebook else { return }
        for view in row.arrangedSubviews { view.removeFromSuperview() }
        var groups: Set<String> = []
        for tab in notebook.tabs where tab.readable {
            let group = tab.group.split(separator: "/").first.map(String.init)
            if let group {
                guard groups.insert(group).inserted else { continue }
                let inside = notebook.tabs.filter { $0.readable && $0.group.hasPrefix(group) }
                let current = inside.first { $0.path == path }
                row.addArrangedSubview(
                    button(current?.name ?? group, color: current?.uiColor ?? .systemGray, raised: current != nil,
                           folder: true, menu: UIMenu(title: group, children: inside.map { tab in
                               UIAction(title: tab.name, image: swatch(tab.uiColor),
                                        state: tab.path == path ? .on : .off) { [weak self] _ in self?.pick(tab) }
                           })))
            } else {
                let tab = tab
                let button = button(tab.name, color: tab.uiColor, raised: tab.path == path, folder: false, menu: nil)
                button.addAction(UIAction { [weak self] _ in self?.pick(tab) }, for: .primaryActionTriggered)
                row.addArrangedSubview(button)
            }
        }
        rule.backgroundColor = notebook.tabs.first { $0.path == path }?.uiColor ?? .separator
        layoutIfNeeded()
        if let raised = row.arrangedSubviews.first(where: { $0.tag == 1 }) {
            scroll.scrollRectToVisible(raised.frame.insetBy(dx: -40, dy: 0), animated: false)
        }
    }

    /// The readable section `step` places after the open one, if any.
    func neighbour(_ step: Int) -> Tab? {
        let tabs = readable
        guard let at = tabs.firstIndex(where: { $0.path == selected }), tabs.indices.contains(at + step) else {
            return nil
        }
        return tabs[at + step]
    }

    func pick(_ tab: Tab) {
        guard let notebook else { return }
        select(tab.path)
        onPick(tab, notebook)
    }

    private func button(_ title: String, color: UIColor, raised: Bool, folder: Bool, menu: UIMenu?) -> UIButton {
        var configuration = UIButton.Configuration.filled()
        configuration.title = title
        configuration.image = folder ? UIImage(systemName: "folder") : nil
        configuration.imagePadding = 4
        configuration.preferredSymbolConfigurationForImage = UIImage.SymbolConfiguration(textStyle: .caption1)
        configuration.baseBackgroundColor = raised ? color : color.withAlphaComponent(0.28)
        configuration.baseForegroundColor = raised ? .white : .label
        configuration.background.cornerRadius = 9
        configuration.contentInsets = NSDirectionalEdgeInsets(
            top: raised ? 11 : 8, leading: 14, bottom: raised ? 9 : 7, trailing: 14)
        configuration.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer {
            var attributes = $0
            attributes.font = .preferredFont(forTextStyle: .subheadline).withTraits(raised ? .traitBold : [])
            return attributes
        }
        let button = UIButton(configuration: configuration)
        // Tabs: only the top corners round, the open one meeting the rule beneath.
        button.layer.maskedCorners = [.layerMinXMinYCorner, .layerMaxXMinYCorner]
        button.layer.cornerRadius = 9
        button.clipsToBounds = true
        button.tag = raised ? 1 : 0
        if let menu {
            button.menu = menu
            button.showsMenuAsPrimaryAction = true
        }
        return button
    }

    private func swatch(_ color: UIColor) -> UIImage? {
        UIImage(systemName: "rectangle.portrait.fill")?.withTintColor(color, renderingMode: .alwaysOriginal)
    }
}

extension Prototype {
    /// The notebook list's bottom corner: Quick Note, a new page in a section chosen once.
    static var quickNote: Bool { UserDefaults.standard.bool(forKey: "SnowboundQuickNote") }
}

/// OneNote's Quick Notes, which go to its Unfiled Notes section: a page made from the notebook
/// list goes to the section picked the first time.
enum QuickNote {
    private struct Place: Codable, Equatable {
        let notebook: String
        let section: String
    }

    private static let key = "quickNoteSection"

    private static var place: Place? {
        get { UserDefaults.standard.data(forKey: key).flatMap { try? JSONDecoder().decode(Place.self, from: $0) } }
        set { UserDefaults.standard.set(try? JSONEncoder().encode(newValue), forKey: key) }
    }

    /// The chosen section, while its notebook lists it.
    private static var target: (Tab, Notebook)? {
        guard let place, let notebook = Notebooks.all.first(where: { $0.id == place.notebook }),
            let tab = notebook.tabs.first(where: { $0.path == place.section && $0.readable })
        else { return nil }
        return (tab, notebook)
    }

    /// Makes a page in the chosen section and opens it, its title ready for typing; until a
    /// section is chosen, a tap asks where Quick Notes go.
    static func item() -> UIBarButtonItem {
        let item = UIBarButtonItem(title: "Quick Note", image: UIImage(systemName: "square.and.pencil"))
        let create = UIAction(title: "Quick Note") { _ in
            guard let (tab, notebook) = target else {
                scene?.window?.rootViewController?.alert(
                    "Can’t Open the Quick Notes Section", "Touch and hold Quick Note to choose another section.")
                return
            }
            write(in: tab, of: notebook)
        }
        item.primaryAction = place == nil ? nil : create
        item.menu = UIMenu(children: [
            UIDeferredMenuElement.uncached { [weak item] provide in
                let notebooks = Notebooks.all.filter { $0.tabs.contains(where: \.readable) }
                let choices = notebooks.map { notebook in
                    UIMenu(
                        title: notebook.name, image: .notebook(notebook.color),
                        children: notebook.tabs.filter(\.readable).map { tab in
                            let chosen = Place(notebook: notebook.id, section: tab.path)
                            let action = UIAction(
                                title: tab.name,
                                image: UIImage(systemName: "rectangle.portrait.fill")?
                                    .withTintColor(tab.uiColor, renderingMode: .alwaysOriginal)
                            ) { _ in
                                let first = item?.primaryAction == nil
                                place = chosen
                                item?.primaryAction = create
                                if first { write(in: tab, of: notebook) }
                            }
                            action.state = place == chosen ? .on : .off
                            return action
                        })
                }
                provide([UIMenu(title: "Quick Notes Go To", options: .displayInline, children: choices)])
            }
        ])
        return item
    }

    private static var scene: SceneDelegate? {
        UIApplication.shared.connectedScenes.lazy.compactMap { $0.delegate as? SceneDelegate }.first
    }

    private static func write(in tab: Tab, of notebook: Notebook) {
        scene?.open(tab, of: notebook) { _ in scene?.newPage() }
    }
}
