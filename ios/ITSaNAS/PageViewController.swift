import UIKit
import WebKit

/// The shared page (account creation, then Settings), served by the core on
/// 127.0.0.1 and shown in a web view, as the Android app does. Its secrets
/// are never typed in the page: the engine asks for them, and this screen
/// answers from a native alert.
final class PageViewController: UIViewController, WKUIDelegate {
    private let mode: String
    private let web = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
    private var poll: Timer?
    private var asking = false

    init(mode: String) {
        self.mode = mode
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("not from a storyboard") }

    override func loadView() { view = web }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "ITSaNAS"
        web.accessibilityIdentifier = "page"
        // Without a delegate a web view answers every window.confirm with
        // false, unseen: Sign out and every "are you sure" of the page did
        // nothing on an iPhone.
        web.uiDelegate = self
        do {
            web.load(URLRequest(url: try Core.pageStart(mode: mode)))
        } catch {
            show(error.localizedDescription)
        }
        poll = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in self?.look() }
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        if isMovingFromParent {
            poll?.invalidate()
            Core.pageStop()
        }
    }

    /// One question at a time: the engine waits for each answer.
    private func look() {
        guard !asking, let request = Core.nextRequest() else { return }
        let id = (request["id"] as? NSNumber)?.uint64Value ?? 0
        let kind = request["kind"] as? String ?? ""
        let args = request["args"] as? [String] ?? []
        switch kind {
        case "event": event(args)
        case "askNewPassphrase": ask(id, NSLocalizedString("Choose a passphrase", comment: ""), nil, secure: true)
        case "askPassphrase": ask(id, NSLocalizedString("Passphrase", comment: ""), args.first, secure: true)
        case "askRecoveryPhrase": ask(id, NSLocalizedString("Your 24 words", comment: ""), nil, secure: false)
        case "showWords": tell(id, NSLocalizedString("Write these 24 words down", comment: ""), args.first ?? "")
        case "showAndConfirm":
            let at = args.dropFirst().joined(separator: ", ")
            let format = NSLocalizedString("Type words %@, in order", comment: "")
            ask(id, String(format: format, at), nil, secure: false)
        default:
            Core.reply(id, nil)
        }
    }

    private func event(_ args: [String]) {
        // The page's end, leaving the account, or removing it: this screen goes.
        if args.first == "biometric_failed" {
            show(NSLocalizedString("Touch ID could not be turned on. Unlock with your passphrase once, then try again.", comment: ""))
            return
        }
        guard let name = args.first, ["closed", "signedOut", "account_removed", "removed"].contains(name)
        else { return }
        poll?.invalidate()
        // Off the main thread: the engine may hold the node, and closing waits for it.
        DispatchQueue.global(qos: .userInitiated).async {
            Core.close()
            DispatchQueue.main.async {
                self.navigationController?.setViewControllers([AppDelegate.first()], animated: true)
            }
        }
    }

    private func ask(_ id: UInt64, _ title: String, _ detail: String?, secure: Bool) {
        asking = true
        let alert = UIAlertController(title: title, message: detail, preferredStyle: .alert)
        alert.addTextField { field in
            field.isSecureTextEntry = secure
            field.autocapitalizationType = .none
            field.autocorrectionType = .no
        }
        alert.addAction(UIAlertAction(title: NSLocalizedString("Back", comment: ""), style: .cancel) { _ in
            Core.reply(id, nil)
            self.asking = false
        })
        alert.addAction(UIAlertAction(title: "OK", style: .default) { _ in
            Core.reply(id, alert.textFields?.first?.text ?? "")
            self.asking = false
        })
        present(alert, animated: true)
    }

    private func tell(_ id: UInt64, _ title: String, _ text: String) {
        asking = true
        let alert = UIAlertController(title: title, message: text, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default) { _ in
            Core.reply(id, "")
            self.asking = false
        })
        present(alert, animated: true)
    }

    /// The page's window.confirm, as a native alert.
    func webView(
        _ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String,
        initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping (Bool) -> Void
    ) {
        // Something already on screen (a question of the engine): no.
        guard presentedViewController == nil else {
            completionHandler(false)
            return
        }
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: NSLocalizedString("Cancel", comment: ""), style: .cancel) { _ in
            completionHandler(false)
        })
        alert.addAction(UIAlertAction(title: "OK", style: .default) { _ in completionHandler(true) })
        present(alert, animated: true)
    }

    private func show(_ message: String) {
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default))
        present(alert, animated: true)
    }
}
