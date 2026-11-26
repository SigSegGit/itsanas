import UIKit

/// Every version of one file the account knows, newest first. A tap on one
/// that is not the file now brings it back, on every machine: fetched first
/// from a machine of the account when this phone never held it. Android's
/// Versions dialog, on the iPhone.
final class VersionsViewController: UITableViewController {
    private let path: String
    private var versions: [[String: Any]] = []
    private var busy = false

    init(path: String) {
        self.path = path
        super.init(style: .insetGrouped)
    }

    required init?(coder: NSCoder) { fatalError("not from a storyboard") }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = (path as NSString).lastPathComponent
        tableView.accessibilityIdentifier = "versions"
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "version")
        reload()
    }

    private func reload() {
        do {
            versions = try Core.versions(path)
            tableView.reloadData()
        } catch {
            show(error.localizedDescription)
        }
    }

    override func tableView(_ tableView: UITableView, titleForFooterInSection section: Int) -> String? {
        NSLocalizedString("Tap an earlier version to make it the file again, on every machine.", comment: "")
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        versions.count
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = UITableViewCell(style: .subtitle, reuseIdentifier: "version")
        let version = versions[indexPath.row]
        let when = Date(timeIntervalSince1970: TimeInterval(version["modified"] as? Int64 ?? 0))
        let size = ByteCountFormatter.string(fromByteCount: version["size"] as? Int64 ?? 0, countStyle: .file)
        cell.textLabel?.text = DateFormatter.localizedString(from: when, dateStyle: .medium, timeStyle: .medium)
        var notes = [size]
        if version["current"] as? Bool ?? false { notes.append(NSLocalizedString("the file now", comment: "")) }
        if version["mine"] as? Bool ?? false { notes.append(NSLocalizedString("written here", comment: "")) }
        if !(version["here"] as? Bool ?? false) { notes.append(NSLocalizedString("on another machine", comment: "")) }
        cell.detailTextLabel?.text = notes.joined(separator: " · ")
        cell.accessibilityIdentifier = "version:\(indexPath.row)"
        cell.selectionStyle = version["current"] as? Bool ?? false ? .none : .default
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        let at = indexPath.row
        guard !busy, !(versions[at]["current"] as? Bool ?? false) else { return }
        busy = true
        let path = self.path
        // Off the main thread: a version not here is fetched from another machine.
        DispatchQueue.global(qos: .userInitiated).async {
            let outcome = Result { try Core.bringBack(path, at: at) }
            DispatchQueue.main.async {
                self.busy = false
                if case .failure(let error) = outcome { self.show(error.localizedDescription) }
                self.reload()
            }
        }
    }

    private func show(_ message: String) {
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default))
        present(alert, animated: true)
    }
}
