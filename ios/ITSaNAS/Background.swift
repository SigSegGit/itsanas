import BackgroundTasks
import Network
import UIKit

/// The few minutes iOS grants when it chooses: the same round as the refresh
/// button, with the account closed again afterwards. A short refresh brings the
/// listing; a long task, on the charger, moves contents, unless the network is
/// metered.
enum Background {
    static let refresh = "fr.ngas.itsanas.refresh"
    static let processing = "fr.ngas.itsanas.sync"

    /// Touched on the main queue only: two grants at once would share a node
    /// the first one closes.
    private static var running = false

    /// Before launching ends, or iOS refuses the identifiers. Once per process.
    static func register() {
        for identifier in [refresh, processing] {
            BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: .main) { run($0) }
        }
    }

    /// Ask for the next grants. Refused on a simulator, and iOS owes nothing.
    static func schedule() {
        let quick = BGAppRefreshTaskRequest(identifier: refresh)
        quick.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        try? BGTaskScheduler.shared.submit(quick)
        let long = BGProcessingTaskRequest(identifier: processing)
        long.requiresNetworkConnectivity = true
        long.requiresExternalPower = true
        try? BGTaskScheduler.shared.submit(long)
    }

    private static func run(_ task: BGTask) {
        schedule()
        guard !running else {
            task.setTaskCompleted(success: true)
            return
        }
        running = true
        var finished = false
        let finish = { (success: Bool) in
            guard !finished else { return }
            finished = true
            task.setTaskCompleted(success: success)
        }
        task.expirationHandler = { DispatchQueue.main.async { finish(false) } }
        onNetworkKnown { metered in
            let metadataOnly = task.identifier == refresh || metered
            DispatchQueue.global(qos: .utility).async {
                let outcome = Result { try Core.backgroundPass(metadataOnly: metadataOnly) }
                // What the hand test reads from the PC (docs/IOS.md): no secret, a round's counts.
                NSLog("ITSaNAS background %@: %@", task.identifier, String(describing: outcome))
                DispatchQueue.main.async {
                    running = false
                    // Still away: nothing may hold the store when iOS ends the
                    // process. Back on the screen, the node is the person's now.
                    if UIApplication.shared.applicationState == .background { Core.close() }
                    if case .success = outcome { finish(true) } else { finish(false) }
                }
            }
        }
    }

    private static func onNetworkKnown(_ body: @escaping (Bool) -> Void) {
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { path in
            monitor.pathUpdateHandler = nil
            monitor.cancel()
            body(path.isExpensive)
        }
        monitor.start(queue: .main)
    }
}
