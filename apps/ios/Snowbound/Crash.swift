import UIKit

enum Crash {
    private static var offered = false

    static func start() {
        guard let folder = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else { return }
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "development"
        _ = sb_crash_start(folder.appendingPathComponent("crash.txt").path, build, "iOS \(UIDevice.current.systemVersion)")
    }

    static func offer(from host: UIViewController) {
        guard !offered, let saved = sb_crash_report() else { return }
        let report = String(cString: saved)
        sb_string_free(saved)
        offered = true
        let alert = UIAlertController(
            title: "Snowbound quit unexpectedly", message: "Send a report to help fix the problem. You can review the report before sending it.", preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "Send Report", style: .default) { _ in send(report, from: host) })
        alert.addAction(UIAlertAction(title: "Show Report", style: .default) { _ in show(report, from: host) })
        alert.addAction(UIAlertAction(title: "Don’t Send", style: .cancel) { _ in sb_crash_forget() })
        host.present(alert, animated: true)
    }

    private static func show(_ report: String, from host: UIViewController) {
        let page = UIViewController()
        page.title = "Crash Report"
        let text = UITextView()
        text.text = report
        text.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        text.isEditable = false
        text.backgroundColor = .systemBackground
        page.view = text
        let navigation = UINavigationController(rootViewController: page)
        page.navigationItem.leftBarButtonItem = UIBarButtonItem(title: "Don’t Send", primaryAction: UIAction { [weak navigation] _ in
            sb_crash_forget()
            navigation?.dismiss(animated: true)
        })
        page.navigationItem.rightBarButtonItem = UIBarButtonItem(title: "Send Report", primaryAction: UIAction { [weak navigation] _ in
            navigation?.dismiss(animated: true) { send(report, from: host) }
        })
        host.present(navigation, animated: true)
    }

    private static func send(_ report: String, from host: UIViewController) {
        let session = URLSession(configuration: .ephemeral)
        var request = URLRequest(url: URL(string: String(cString: sb_crash_address()))!)
        request.httpMethod = "POST"
        request.setValue("text/plain; charset=utf-8", forHTTPHeaderField: "Content-Type")
        request.httpBody = Data(report.utf8)
        session.dataTask(with: request) { _, response, error in
            session.finishTasksAndInvalidate()
            let sent = error == nil && (response as? HTTPURLResponse).map { (200..<300).contains($0.statusCode) } == true
            DispatchQueue.main.async {
                if sent {
                    sb_crash_forget()
                } else {
                    let alert = UIAlertController(title: "Report wasn’t sent", message: "You can try again.", preferredStyle: .alert)
                    alert.addAction(UIAlertAction(title: "Try Again", style: .default) { _ in send(report, from: host) })
                    alert.addAction(UIAlertAction(title: "Don’t Send", style: .cancel) { _ in sb_crash_forget() })
                    host.present(alert, animated: true)
                }
            }
        }.resume()
    }
}
