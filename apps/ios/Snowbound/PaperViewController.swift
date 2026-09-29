import UIKit

/// The page's paper from `sb_view_paper`, with the choices OneNote offers.
struct Paper: Decodable {
    struct Color: Decodable {
        let name: String
        let rgb: [UInt8]

        init(from decoder: Decoder) throws {
            var pair = try decoder.unkeyedContainer()
            name = try pair.decode(String.self)
            rgb = try pair.decode([UInt8].self)
        }
    }

    let color: [UInt8]?
    let ruled: Int?
    let colors: [Color]
    let rules: [String]
    let templates: [String]
}

extension UIColor {
    convenience init(rgb: [UInt8]) {
        self.init(red: CGFloat(rgb[0]) / 255, green: CGFloat(rgb[1]) / 255, blue: CGFloat(rgb[2]) / 255, alpha: 1)
    }

    var rgb: [UInt8] {
        var (red, green, blue, alpha): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
        getRed(&red, green: &green, blue: &blue, alpha: &alpha)
        return [red, green, blue].map { UInt8((min(1, max(0, $0)) * 255).rounded()) }
    }
}

/// OneNote's View, Page Color and Rule Lines, and a template's art behind the page, as a
/// sheet over the page, each choice taking effect at once.
final class PaperViewController: UITableViewController {
    private enum Part: Int, CaseIterable { case color, rules, background }

    private let canvas: CanvasView
    private var paper: Paper?
    private let swatches = UIStackView()
    private let well = UIColorWell()

    init(canvas: CanvasView) {
        self.canvas = canvas
        super.init(style: .insetGrouped)
        title = "Page Background"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "row")
        navigationItem.rightBarButtonItem = UIBarButtonItem(
            systemItem: .done, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        well.title = "Custom Color"
        well.supportsAlpha = false
        well.addAction(UIAction { [weak self] _ in
            guard let self, let color = well.selectedColor else { return }
            canvas.setPaper(color: color.rgb, ruled: paper?.ruled)
            reload()
        }, for: .valueChanged)
        reload()
    }

    private func reload() {
        paper = canvas.pagePaper
        showSwatches()
        tableView.reloadData()
    }

    private func showSwatches() {
        swatches.arrangedSubviews.forEach { $0.removeFromSuperview() }
        guard let paper else { return }
        let none = swatch(nil, name: "No Color", chosen: paper.color == nil)
        swatches.addArrangedSubview(none)
        for color in paper.colors {
            swatches.addArrangedSubview(swatch(color.rgb, name: color.name, chosen: paper.color == color.rgb))
        }
        let custom = paper.color.flatMap { color in paper.colors.contains { $0.rgb == color } ? nil : color }
        well.selectedColor = custom.map(UIColor.init(rgb:))
        swatches.addArrangedSubview(well)
    }

    private func swatch(_ rgb: [UInt8]?, name: String, chosen: Bool) -> UIButton {
        var configuration = UIButton.Configuration.plain()
        configuration.background.backgroundColor = rgb.map(UIColor.init(rgb:)) ?? .white
        configuration.background.cornerRadius = 16
        configuration.background.strokeColor = chosen ? .tintColor : .separator
        configuration.background.strokeWidth = chosen ? 3 : 1
        if rgb == nil {
            configuration.image = UIImage(systemName: "line.diagonal")
            configuration.baseForegroundColor = .systemRed
        }
        let button = UIButton(configuration: configuration, primaryAction: UIAction { [weak self] _ in
            guard let self else { return }
            canvas.setPaper(color: rgb, ruled: paper?.ruled)
            reload()
        })
        button.accessibilityLabel = name
        button.accessibilityTraits.insert(chosen ? .selected : [])
        button.widthAnchor.constraint(equalToConstant: 32).isActive = true
        button.heightAnchor.constraint(equalToConstant: 32).isActive = true
        return button
    }

    private let parts = Part.allCases

    override func numberOfSections(in tableView: UITableView) -> Int { parts.count }

    override func tableView(_ tableView: UITableView, titleForHeaderInSection section: Int) -> String? {
        switch parts[section] {
        case .color: "Page Color"
        case .rules: "Rule Lines"
        case .background: "Background"
        }
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        guard let paper else { return 0 }
        return switch parts[section] {
        case .color: 1
        case .rules: 1 + paper.rules.count
        case .background: 1 + paper.templates.count
        }
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "row", for: indexPath)
        cell.contentView.subviews.filter { $0 is UIScrollView }.forEach { $0.removeFromSuperview() }
        cell.accessoryType = .none
        cell.contentConfiguration = nil
        guard let paper else { return cell }
        switch parts[indexPath.section] {
        case .color:
            cell.selectionStyle = .none
            let scroll = UIScrollView()
            scroll.showsHorizontalScrollIndicator = false
            swatches.spacing = 12
            swatches.alignment = .center
            swatches.translatesAutoresizingMaskIntoConstraints = false
            scroll.translatesAutoresizingMaskIntoConstraints = false
            scroll.addSubview(swatches)
            cell.contentView.addSubview(scroll)
            NSLayoutConstraint.activate([
                scroll.leadingAnchor.constraint(equalTo: cell.contentView.leadingAnchor),
                scroll.trailingAnchor.constraint(equalTo: cell.contentView.trailingAnchor),
                scroll.topAnchor.constraint(equalTo: cell.contentView.topAnchor),
                scroll.bottomAnchor.constraint(equalTo: cell.contentView.bottomAnchor),
                scroll.heightAnchor.constraint(equalToConstant: 56),
                swatches.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 16),
                swatches.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -16),
                swatches.centerYAnchor.constraint(equalTo: scroll.frameLayoutGuide.centerYAnchor),
            ])
        case .rules:
            cell.selectionStyle = .default
            var content = cell.defaultContentConfiguration()
            content.text = indexPath.row == 0 ? "None" : paper.rules[indexPath.row - 1]
            cell.contentConfiguration = content
            let chosen = indexPath.row == 0 ? paper.ruled == nil : paper.ruled == indexPath.row - 1
            cell.accessoryType = chosen ? .checkmark : .none
        case .background:
            cell.selectionStyle = .default
            var content = cell.defaultContentConfiguration()
            content.text = indexPath.row == 0 ? "None" : paper.templates[indexPath.row - 1]
            cell.contentConfiguration = content
        }
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        guard let paper else { return }
        switch parts[indexPath.section] {
        case .color: return
        case .rules: canvas.setPaper(color: paper.color, ruled: indexPath.row == 0 ? nil : indexPath.row - 1)
        case .background: canvas.setArt(indexPath.row == 0 ? nil : paper.templates[indexPath.row - 1])
        }
        reload()
    }
}
