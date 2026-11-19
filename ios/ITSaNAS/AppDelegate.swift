import UIKit

/// iOS 14 is the floor (the first iPhone SE stops at 15), so the app keeps to
/// UIKit and one window, without scenes.
/// Files shared into the app, waiting for the account to be open.
enum Shared {
    static var waiting: [URL] = []
    static let arrived = Notification.Name("fr.ngas.itsanas.shared")
}

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        #if DEBUG
        TestLaunch.prepare()
        #endif
        Background.register()
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = UINavigationController(rootViewController: AppDelegate.first())
        window.makeKeyAndVisible()
        self.window = window
        return true
    }

    /// A file another app shares or opens in ITSaNAS ("Open in", the share
    /// sheet). Copied now: the address iOS gives may not outlive this call.
    /// The Files screen puts it into the account once the account is open.
    func application(
        _ app: UIApplication, open url: URL, options: [UIApplication.OpenURLOptionsKey: Any] = [:]
    ) -> Bool {
        guard url.isFileURL else { return false }
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("shared", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let copy = dir.appendingPathComponent(url.lastPathComponent)
        try? FileManager.default.removeItem(at: copy)
        guard (try? FileManager.default.copyItem(at: url, to: copy)) != nil else { return false }
        Shared.waiting.append(copy)
        NotificationCenter.default.post(name: Shared.arrived, object: nil)
        return true
    }

    /// Create an account, or unlock the one on this phone.
    static func first() -> UIViewController {
        Core.exists() ? UnlockViewController() : PageViewController(mode: "setup")
    }

    /// iOS gives no foreground service: the store closes when the app leaves
    /// the screen, so nothing holds it while the system may end the process.
    func applicationDidEnterBackground(_ application: UIApplication) {
        Core.pageStop()
        Core.close()
        Background.schedule()
        guard let navigation = window?.rootViewController as? UINavigationController else { return }
        navigation.setViewControllers([AppDelegate.first()], animated: false)
    }
}
