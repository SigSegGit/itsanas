#if DEBUG
import Foundation
import Security

/// What the UI tests (ios/ITSaNASUITests, run by scripts/iphone-e2e.sh) ask
/// of the app at launch, in a debug build only: a phone with nothing on it,
/// and the coordinator the script started instead of the official one. The
/// core refuses the second to a release build whatever this file says.
enum TestLaunch {
    static func prepare() {
        let asked = ProcessInfo.processInfo.environment
        if asked["E2E_WIPE"] == "1" {
            Core.close()
            try? FileManager.default.removeItem(atPath: Core.home)
            let kept: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: "fr.ngas.itsanas",
            ]
            SecItemDelete(kept as CFDictionary)
            UserDefaults.standard.removeObject(forKey: "trashDays")
        }
        guard let address = asked["E2E_COORDINATOR"], !address.isEmpty else { return }
        do {
            try Core.useCoordinatorForTests(address, device: asked["E2E_COORDINATOR_DEVICE"] ?? "")
        } catch {
            // Going on would register the test's names at the official coordinator.
            fatalError("the UI tests' coordinator was refused: \(error.localizedDescription)")
        }
    }
}
#endif
