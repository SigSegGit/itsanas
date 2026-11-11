import UIKit

/// iOS 14 is the floor (the first iPhone SE stops at 15), so the app keeps to
/// UIKit and one window, without scenes.
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
