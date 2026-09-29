import UIKit

/// OneNote's tags in `sb_view_apply`'s order from 16, drawn with the page's artwork.
enum Tags {
    struct Tag {
        let name: String
        /// The symbol `sb_tag_icon` draws; 0 for a highlighting tag.
        let shape: UInt16
        let highlight: UIColor?
    }

    static let all: [Tag] = {
        guard let text = take(sb_tags()),
            let rows = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [[Any]]
        else { return [] }
        return rows.compactMap { row in
            guard row.count == 3, let name = row[0] as? String, let shape = row[1] as? NSNumber else { return nil }
            let highlight = (row[2] as? [NSNumber]).map { UIColor(rgb: $0.map(\.uint8Value)) }
            return Tag(name: name, shape: shape.uint16Value, highlight: highlight)
        }
    }()

    /// The first nine, which ⌘1 to ⌘9 apply, as Ctrl+1 to Ctrl+9 do in OneNote.
    static let keyed = 9

    private static var icons: [Int: UIImage] = [:]

    /// Tag symbol `shape` as OneNote draws it, a check box `checked`; a highlighter in
    /// `highlight` for a highlighting tag.
    static func icon(_ shape: UInt16, checked: Bool = false, highlight: UIColor? = nil) -> UIImage? {
        guard shape != 0 else {
            // Highlighted lines, as the desktop draws these tags.
            let fill = highlight ?? .systemYellow
            return UIGraphicsImageRenderer(size: CGSize(width: 18, height: 18)).image { context in
                let card = UIBezierPath(roundedRect: CGRect(x: 1.5, y: 3.5, width: 15, height: 11), cornerRadius: 1.5)
                fill.setFill()
                card.fill()
                UIColor(white: 0, alpha: 0.35).setStroke()
                card.stroke()
                UIColor(white: 0.25, alpha: 1).setFill()
                context.fill(CGRect(x: 4, y: 6.5, width: 10, height: 1.5))
                context.fill(CGRect(x: 4, y: 10, width: 7, height: 1.5))
            }
        }
        let key = Int(shape) << 1 | (checked ? 1 : 0)
        if let icon = icons[key] { return icon }
        let points: CGFloat = 18
        let scale = UIScreen.main.scale
        let pixels = Int(points * scale)
        var rgba = [UInt8](repeating: 0, count: pixels * pixels * 4)
        guard sb_tag_icon(shape, checked, UInt32(pixels), &rgba),
            let provider = CGDataProvider(data: Data(rgba) as CFData),
            let image = CGImage(
                width: pixels, height: pixels, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: pixels * 4,
                space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
        else { return UIImage(systemName: "tag") }
        let icon = UIImage(cgImage: image, scale: scale, orientation: .up).withRenderingMode(.alwaysOriginal)
        icons[key] = icon
        return icon
    }

    /// Every tag as a menu, those the selection has checked, then Remove Tag; `apply` takes
    /// the `sb_view_apply` command.
    static func menu(bits: UInt64, apply: @escaping (UInt8) -> Void) -> [UIMenuElement] {
        let actions = all.enumerated().map { index, tag in
            UIAction(
                title: tag.name, image: icon(tag.shape, highlight: tag.highlight),
                state: bits & (1 << UInt64(16 + index)) != 0 ? .on : .off
            ) { _ in apply(16 + UInt8(index)) }
        }
        let tagged = bits >> 16 != 0
        return [
            UIMenu(options: .displayInline, children: Array(actions.prefix(keyed))),
            UIMenu(title: "More Tags", image: UIImage(systemName: "tag"), children: Array(actions.dropFirst(keyed))),
            UIAction(
                title: "Remove Tag", image: UIImage(systemName: "tag.slash"),
                attributes: tagged ? [] : .disabled
            ) { _ in apply(8) },
        ]
    }
}

/// A tagged paragraph from `sb_library_tagged`.
struct Tagged: Decodable {
    let section: String
    let page: String
    let title: String
    let paragraph: String
    let name: String
    let shape: UInt16
    let checked: Bool
    let text: String
}

/// OneNote's Tags Summary: a notebook's tagged paragraphs by tag, each opening its page with
/// the paragraph selected.
final class TagsViewController: UITableViewController {
    private let notebook: Notebook
    private var groups: [(name: String, items: [Tagged])] = []
    private var unchecked = false
    private var all: [Tagged] = []
    /// Opens a page of the notebook: section path, page and paragraph.
    var onOpen: ((Notebook, String, String, String) -> Void)?

    init(notebook: Notebook) {
        self.notebook = notebook
        super.init(style: .insetGrouped)
        title = "Tags Summary"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "tagged")
        navigationItem.rightBarButtonItem = UIBarButtonItem(
            systemItem: .done, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        navigationItem.leftBarButtonItem = UIBarButtonItem(
            title: "Filter", image: UIImage(systemName: "line.3.horizontal.decrease.circle"),
            menu: UIMenu(children: [
                UIDeferredMenuElement.uncached { [weak self] done in
                    done([
                        UIAction(title: "Unchecked Only", state: self?.unchecked == true ? .on : .off) { _ in
                            self?.unchecked.toggle()
                            self?.show()
                        }
                    ])
                }
            ]))
        var loading = UIContentUnavailableConfiguration.loading()
        loading.text = "Finding Tags…"
        contentUnavailableConfiguration = loading
        guard let library = notebook.handle else { return }
        let open = Section.all.first { $0.notebook === notebook }
        let (pointer, section, path) = (Int(bitPattern: library), open.map { Int(bitPattern: $0.handle) } ?? 0, open?.tab.path)
        background({
            decode([Tagged].self, sb_library_tagged(OpaquePointer(bitPattern: pointer), OpaquePointer(bitPattern: section), path))
                ?? []
        }) { [weak self] tagged in
            self?.all = tagged
            self?.show()
        }
    }

    private func show() {
        var order: [String] = []
        var byName: [String: [Tagged]] = [:]
        for tagged in all where !unchecked || !tagged.checked {
            if byName[tagged.name] == nil { order.append(tagged.name) }
            byName[tagged.name, default: []].append(tagged)
        }
        groups = order.map { ($0, byName[$0] ?? []) }
        tableView.reloadData()
        var empty = UIContentUnavailableConfiguration.empty()
        empty.image = UIImage(systemName: "tag")
        empty.text = unchecked ? "No Unchecked Tags" : "No Tags"
        empty.secondaryText = unchecked ? nil : "Tag a paragraph from the format bar or its menu."
        contentUnavailableConfiguration = groups.isEmpty ? empty : nil
    }

    override func numberOfSections(in tableView: UITableView) -> Int { groups.count }

    override func tableView(_ tableView: UITableView, titleForHeaderInSection section: Int) -> String? {
        groups[section].name
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        groups[section].items.count
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let tagged = groups[indexPath.section].items[indexPath.row]
        let cell = tableView.dequeueReusableCell(withIdentifier: "tagged", for: indexPath)
        var content = UIListContentConfiguration.subtitleCell()
        content.image = Tags.icon(tagged.shape, checked: tagged.checked)
        content.text = tagged.text.isEmpty ? " " : tagged.text
        content.textProperties.numberOfLines = 3
        let section = notebook.tabs.first { $0.path == tagged.section }?.name
        content.secondaryText = [tagged.title.isEmpty ? "Untitled Page" : tagged.title, section]
            .compactMap { $0 }.joined(separator: " · ")
        content.secondaryTextProperties.color = .secondaryLabel
        cell.contentConfiguration = content
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        let tagged = groups[indexPath.section].items[indexPath.row]
        dismiss(animated: true)
        onOpen?(notebook, tagged.section, tagged.page, tagged.paragraph)
    }
}
