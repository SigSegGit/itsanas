import XCTest

/// A scenario that cannot go on: the consequence for a person, and what was on
/// the screen. Thrown, so the test stops where it went wrong.
struct Failed: Error, CustomStringConvertible {
    let description: String
}

/// What every scenario of the hand test shares: a phone with nothing on it,
/// pinned to the coordinator `scripts/iphone-e2e.sh` started, the screens
/// driven by taps and keys, and a screenshot kept wherever one fails.
///
/// Nothing here talks to the official coordinator. A run without
/// `E2E_COORDINATOR` fails in `setUpWithError`, saying so, before it can.
class E2eCase: XCTestCase {
    let app = XCUIApplication()
    let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
    /// The account a computer made, with two files in it, before the run.
    var username = ""
    var passphrase = ""
    private var coordinator = ""
    private var device = ""

    override func setUpWithError() throws {
        continueAfterFailure = false
        let given = ProcessInfo.processInfo.environment
        coordinator = given["E2E_COORDINATOR"] ?? ""
        device = given["E2E_COORDINATOR_DEVICE"] ?? ""
        username = given["E2E_USERNAME"] ?? ""
        passphrase = given["E2E_PASSPHRASE"] ?? ""
        if coordinator.isEmpty || username.isEmpty || passphrase.isEmpty {
            throw Failed(description: "no E2E_COORDINATOR, E2E_USERNAME or E2E_PASSPHRASE: run "
                + "scripts/iphone-e2e.sh, which starts a coordinator and a computer of its own. "
                + "Without them this test would register names at the official coordinator.")
        }
        // The Face ID question, the first time the app asks for a face.
        addUIInterruptionMonitor(withDescription: "biometric permission") { alert in
            guard alert.buttons["OK"].exists, alert.label.contains("Face ID") else { return false }
            alert.buttons["OK"].tap()
            return true
        }
    }

    override func tearDown() {
        app.terminate()
        super.tearDown()
    }

    // ------------------------------------------------------------ launching

    /// Open the app; `wipe` empties the phone first, as a first install.
    func launch(wipe: Bool) {
        app.launchEnvironment = [
            "E2E_COORDINATOR": coordinator,
            "E2E_COORDINATOR_DEVICE": device,
            "E2E_WIPE": wipe ? "1" : "0",
        ]
        app.launch()
    }

    /// Home screen, then back: what iOS calls leaving the app.
    func leaveAndComeBack() {
        XCUIDevice.shared.press(.home)
        Thread.sleep(forTimeInterval: 2)
        app.activate()
        Thread.sleep(forTimeInterval: 2)
    }

    // -------------------------------------------------------------- waiting

    /// Wait until `condition` holds; otherwise stop with `consequence` and
    /// what the screen showed.
    func waitFor(_ consequence: String, seconds: Double = 30, _ condition: () -> Bool) throws {
        let end = Date().addingTimeInterval(seconds)
        while Date() < end {
            if condition() { return }
            Thread.sleep(forTimeInterval: 0.5)
        }
        try fail(consequence)
    }

    /// Stop here, with a screenshot and what was on the screen.
    func fail(_ consequence: String) throws -> Never {
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = "FAILED " + consequence
        shot.lifetime = .keepAlways
        add(shot)
        throw Failed(description: "\(consequence) (on screen: \(onScreen()))")
    }

    /// The alert's words, the bar's title and the first texts: enough to tell
    /// which screen a failure happened on.
    /// Read from one snapshot of each app: an element queried by index
    /// vanishes when the screen changes, and the query then fails the test
    /// in place of the sentence that says why (seen as "No matches found
    /// for Element at index 10").
    func onScreen() -> String {
        var said: [String] = []
        for source in [app, springboard] {
            guard let root = try? source.snapshot() else { continue }
            for alert in Self.all(.alert, in: root) {
                let words = Self.all(.staticText, in: alert).map(\.label).joined(separator: " / ")
                said.append("alert \"\(alert.label)\": \(words)")
            }
        }
        guard let root = try? app.snapshot() else { return said.joined(separator: "; ") }
        if let bar = Self.all(.navigationBar, in: root).first { said.append("bar \"\(bar.identifier)\"") }
        let texts = Self.all(.staticText, in: root).map(\.label).filter { !$0.isEmpty }.prefix(20)
        said.append("texts: " + texts.joined(separator: " | "))
        let buttons = Self.all(.button, in: root).map(\.label).filter { !$0.isEmpty }.prefix(12)
        said.append("buttons: " + buttons.joined(separator: " | "))
        return said.joined(separator: "; ")
    }

    static func all(_ type: XCUIElement.ElementType, in root: XCUIElementSnapshot) -> [XCUIElementSnapshot] {
        (root.elementType == type ? [root] : []) + root.children.flatMap { all(type, in: $0) }
    }

    // ----------------------------------------------------------------- page

    /// An element of the page whose accessible name is `label`.
    func web(_ label: String, _ type: XCUIElement.ElementType = .any) -> XCUIElement {
        app.webViews.descendants(matching: type)
            .matching(NSPredicate(format: "label == %@", label)).firstMatch
    }

    func webContaining(_ words: String, _ type: XCUIElement.ElementType = .any) -> XCUIElement {
        app.webViews.descendants(matching: type)
            .matching(NSPredicate(format: "label CONTAINS %@", words)).firstMatch
    }

    /// Scroll the page until `element` can be touched, then touch it.
    func tapOnPage(_ element: XCUIElement, _ what: String) throws {
        try waitFor("\(what) is not on the page") { element.exists }
        for direction in [true, false] {
            var swipes = 0
            while !element.isHittable && swipes < 15 {
                if direction { app.webViews.firstMatch.swipeUp() } else { app.webViews.firstMatch.swipeDown() }
                swipes += 1
            }
        }
        guard element.isHittable else { try fail("\(what) cannot be reached on the page") }
        element.tap()
    }

    /// Where `element` stands, once it stands still inside the window: two
    /// snapshots a moment apart agree. A screen sliding in or out (the unlock
    /// field was seen at x = -96) is not touched; `isHittable` said yes to it
    /// and the touch then found nothing. Snapshots throw where `isHittable`
    /// and `tap` fail the test.
    func settledCentre(_ element: XCUIElement) -> CGPoint? {
        guard let first = try? element.snapshot().frame else { return nil }
        Thread.sleep(forTimeInterval: 0.3)
        guard let second = try? element.snapshot().frame, second == first, !second.isEmpty else { return nil }
        let centre = CGPoint(x: second.midX, y: second.midY)
        return app.windows.firstMatch.frame.contains(centre) ? centre : nil
    }

    func tapAt(_ point: CGPoint) {
        app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: point.x, dy: point.y)).tap()
    }

    /// The native passphrase field, once it stands still: typed into, then Open.
    /// Whether it was typed. A touch that gave the field no keyboard (the
    /// screen still settling) types nothing and is tried again by the caller:
    /// typing with no focus fails the test.
    @discardableResult
    func typePassphraseIfAsked() -> Bool {
        guard let centre = settledCentre(app.secureTextFields["unlock-pass"]) else { return false }
        tapAt(centre)
        guard app.keyboards.firstMatch.waitForExistence(timeout: 3) else { return false }
        app.typeText(passphrase)
        if let open = settledCentre(app.buttons["unlock-open"]) { tapAt(open) }
        return true
    }

    enum Reach { case tapped, scrolled, absent }

    /// Touch `element` if it is inside the page's visible part, or scroll the
    /// page one swipe toward it. Read from a snapshot, which throws when the
    /// page changed under it, never from `isHittable` or `frame`, which fail
    /// the test instead: a step that moves on between the look and the touch
    /// is the page working, not a fault. A phone's setup steps are longer than
    /// its screen, so a step's Next is often below the fold.
    @discardableResult
    func scrollToward(_ element: XCUIElement) -> Reach {
        let page = app.webViews.firstMatch
        guard let shot = try? element.snapshot(), let view = try? page.snapshot() else { return .absent }
        let frame = shot.frame
        var visible = view.frame.intersection(app.windows.firstMatch.frame)
        // The keyboard covers the bottom of the page and takes a touch meant for it.
        if let keys = try? app.keyboards.firstMatch.snapshot().frame, !keys.isEmpty {
            visible.size.height = max(0, min(visible.maxY, keys.minY) - visible.minY)
        }
        guard !frame.isEmpty, !visible.isEmpty else { return .absent }
        let centre = CGPoint(x: frame.midX, y: frame.midY)
        if visible.insetBy(dx: 0, dy: 8).contains(centre) {
            app.coordinate(withNormalizedOffset: .zero)
                .withOffset(CGVector(dx: centre.x, dy: centre.y)).tap()
            return .tapped
        }
        if centre.y > visible.midY { page.swipeUp() } else { page.swipeDown() }
        return .scrolled
    }

    // -------------------------------------------------------------- answers

    /// Answer the question on screen, if there is one: the passphrase, or OK.
    /// Answers whether there was one.
    @discardableResult
    func answerAlert() -> Bool {
        guard app.alerts.count > 0 else { return false }
        let alert = app.alerts.firstMatch
        let field = alert.secureTextFields.count > 0
            ? alert.secureTextFields.firstMatch : alert.textFields.firstMatch
        if field.exists {
            field.tap()
            field.typeText(passphrase)
        }
        if alert.buttons["OK"].exists { alert.buttons["OK"].tap() }
        return true
    }

    // ---------------------------------------------------------- the account

    /// Whether the last scenario left the phone logged in to the account.
    private static var phoneHoldsTheAccount = false

    /// A phone with nothing on it logs in to the computer's account: the
    /// page's steps, the passphrase in the native question, then the file list.
    ///
    /// A login fetches the account's recovery copy, which the coordinator
    /// gives one username five times in 15 minutes against guessing
    /// (`ESCROW_ATTEMPTS`); the sixth login of a run stopped setup. So a
    /// scenario that is not about the first login (`fresh` false) opens the
    /// account the phone kept from the one before, when there is one.
    func logIn(fresh: Bool = false) throws {
        if !fresh && Self.phoneHoldsTheAccount && openKept() { return }
        Self.phoneHoldsTheAccount = false
        launch(wipe: true)
        let start = web("Start", .button)
        try waitFor("Welcome never appeared on a phone with no account", seconds: 60) { start.exists }
        try tapOnPage(start, "Start on Welcome")
        try tapOnPage(webContaining("already have an account"), "\"No, I already have an account\"")
        let named = app.webViews.textFields["Your username"]
        let name = named.exists ? named : app.webViews.textFields.firstMatch
        try tapOnPage(name, "the username field")
        name.typeText(username)
        let escrow = webContaining("recovery copy is kept by the network")
        if escrow.exists { try tapOnPage(escrow, "the recovery copy box") }
        try tapOnPage(web("Next", .button), "Next on the account step")
        try throughSetup()
        Self.phoneHoldsTheAccount = true
    }

    /// The account the phone kept, opened again: from the Keychain, or with
    /// the passphrase when a finger is asked and put away. False when the
    /// phone shows setup instead.
    private func openKept() -> Bool {
        launch(wipe: false)
        let files = app.tables["files"]
        let start = web("Start", .button)
        let end = Date().addingTimeInterval(30)
        while Date() < end {
            if files.exists { return true }
            if start.exists { return false }
            cancelFingerSheet()
            typePassphraseIfAsked()
            Thread.sleep(forTimeInterval: 0.5)
        }
        return false
    }

    /// From the account step to the file list, whatever steps the page shows
    /// a phone and whatever the engine asks.
    private func throughSetup() throws {
        let files = app.tables["files"]
        // The last page's Close also closes a setup that stopped, and the
        // phone then starts over at Welcome: the reason would be lost.
        let stopped = web("Setup stopped")
        var lastTap = Date.distantPast
        try waitFor("the file list never opened after logging in, so the account is unusable", seconds: 240) {
            if files.exists || stopped.exists { return true }
            if answerAlert() { return false }
            if typePassphraseIfAsked() { return false }
            guard Date().timeIntervalSince(lastTap) > 3 else { return false }
            for label in ["Close", "Start setting up", "Next"] {
                switch scrollToward(web(label, .button)) {
                case .tapped: lastTap = Date()
                case .scrolled: break
                case .absent: continue
                }
                break
            }
            return false
        }
        if stopped.exists { try fail("setup stopped while logging in, so the account is unusable") }
    }

    // ------------------------------------------------------------ the files

    /// Sync until `name` is listed: a round brings what the computer holds.
    func syncUntilListed(_ name: String) throws {
        let row = app.cells["file:" + name]
        var lastSync = Date.distantPast
        try waitFor("\(name), put on a computer of the account, never reached the phone's list", seconds: 120) {
            if row.exists { return true }
            if app.alerts.count > 0 { return false }
            if Date().timeIntervalSince(lastSync) > 10 {
                app.buttons["files-sync"].tap()
                lastSync = Date()
            }
            return false
        }
    }

    func back() {
        app.navigationBars.firstMatch.buttons.element(boundBy: 0).tap()
    }

    func openSettings() throws {
        app.buttons["files-settings"].tap()
        try waitFor("Settings never opened", seconds: 60) { web("Security").exists }
    }

    /// Settings' section titled `title` opened, unless `inside` is already
    /// there: a closed section keeps its cards out of reach, and a second
    /// touch would close it again.
    func openSection(_ title: String, for inside: XCUIElement) throws {
        if inside.exists { return }
        try tapOnPage(web(title), "the \(title) section of Settings")
        try waitFor("the \(title) section did not open") { inside.exists }
    }

    // ----------------------------------------------------------- the finger

    /// The simulator's enrolment, as the iPhone's settings would change it.
    func enrol(_ enrolled: Bool) {
        var token: Int32 = 0
        notify_register_check("com.apple.BiometricKit.enrollmentChanged", &token)
        notify_set_state(token, enrolled ? 1 : 0)
        notify_post("com.apple.BiometricKit.enrollmentChanged")
        Thread.sleep(forTimeInterval: 1)
    }

    /// A finger on the sensor, or a face before the camera, enrolled or not.
    func touch(matching: Bool) {
        let outcome = matching ? "match" : "nomatch"
        notify_post("com.apple.BiometricKit_Sim.fingerTouch." + outcome)
        notify_post("com.apple.BiometricKit_Sim.pearl." + outcome)
        Thread.sleep(forTimeInterval: 1)
    }

    /// The system's finger sheet, wherever iOS draws it, put away.
    func cancelFingerSheet() {
        for source in [springboard, app] where source.buttons["Cancel"].exists {
            source.buttons["Cancel"].tap()
            return
        }
    }

    /// Settings, Security, the fingerprint box, until it says `on`; then back
    /// to the file list.
    func setFinger(_ on: Bool) throws {
        try openSettings()
        let box = webContaining("Unlock with my fingerprint or face")
        try openSection("Security", for: box)
        // A checkbox's value reads "1" or 1, by iOS version.
        let isOn = { String(describing: box.value ?? "") == "1" }
        if isOn() != on { try tapOnPage(box, "the fingerprint box") }
        try waitFor("the fingerprint box did not turn \(on ? "on" : "off")") { isOn() == on }
        Thread.sleep(forTimeInterval: 1)
        if app.alerts.count > 0 { try fail("turning the fingerprint \(on ? "on" : "off") was refused") }
        back()
        try waitFor("Settings did not close") { app.tables["files"].exists }
    }

    func unlockWithPassphrase() throws {
        try waitFor("the passphrase field is not there to open the account") { typePassphraseIfAsked() }
        try waitFor("the passphrase did not open the account") { app.tables["files"].exists }
    }
}
