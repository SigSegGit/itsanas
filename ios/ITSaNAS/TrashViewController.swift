import UIKit

/// What was deleted on this phone and still waits: put it back, or delete it
/// from every machine now.
final class TrashViewController: UITableViewController {
    private var rows: [[String: Any]] = []
    private var now: Double = 0

    override func viewDidLoad() {
        super.viewDidLoad()
        title = NSLocalizedString("Trash", comment: "")
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "entry")
        reload()
    }

    private func reload() {
        DispatchQueue.global(qos: .userInitiated).async {
            let outcome = Result { try Core.trashList() }
            DispatchQueue.main.async {
                switch outcome {
                case .success(let list):
                    self.rows = list["trashed"] as? [[String: Any]] ?? []
                    self.now = (list["now"] as? NSNumber)?.doubleValue ?? 0
                    self.tableView.reloadData()
                case .failure(let error):
                    self.show(error.localizedDescription)
                }
            }
        }
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        rows.count
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "entry", for: indexPath)
        let due = (rows[indexPath.row]["due"] as? NSNumber)?.doubleValue ?? 0
        let days = max(0, Int(((due - now) / 86_400).rounded(.up)))
        let format = NSLocalizedString("%@ (%d days)", comment: "")
        cell.textLabel?.text = String(format: format, path(indexPath), days)
        return cell
    }

    private func path(_ indexPath: IndexPath) -> String {
        rows[indexPath.row]["path"] as? String ?? ""
    }

    override func tableView(
        _ tableView: UITableView, trailingSwipeActionsConfigurationForRowAt indexPath: IndexPath
    ) -> UISwipeActionsConfiguration? {
        let file = path(indexPath)
        let restore = UIContextualAction(style: .normal, title: NSLocalizedString("Restore", comment: "")) { _, _, done in
            self.act { try Core.trashRestore(file) }
            done(true)
        }
        let delete = UIContextualAction(style: .destructive, title: NSLocalizedString("Delete", comment: "")) { _, _, done in
            self.confirmDelete(file)
            done(false)
        }
        return UISwipeActionsConfiguration(actions: [delete, restore])
    }

    private func confirmDelete(_ file: String) {
        let alert = UIAlertController(
            title: NSLocalizedString("Delete from every machine?", comment: ""),
            message: file, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: NSLocalizedString("Back", comment: ""), style: .cancel))
        alert.addAction(UIAlertAction(title: NSLocalizedString("Delete", comment: ""), style: .destructive) { _ in
            self.act { try Core.trashDelete(file) }
        })
        present(alert, animated: true)
    }

    private func act(_ body: @escaping () throws -> Void) {
        DispatchQueue.global(qos: .userInitiated).async {
            let outcome = Result { try body() }
            DispatchQueue.main.async {
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
