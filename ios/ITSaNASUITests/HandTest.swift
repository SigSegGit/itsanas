import XCTest

/// The hand test of docs/IOS.md, steps 1 to 5, 8 and 9, as a person does them
/// on a simulator: a computer of the account (a node `scripts/iphone-e2e.sh`
/// runs on the Mac) holds `from-computer.txt` and `to-trash.txt`, and every
/// scenario starts from a phone with nothing on it. Steps 6 and 7 (iOS choosing
/// when a background round runs) and 10 (Windows Hello) stay with a person.
///
/// XCTest runs these in the order of their names, which is the hand test's.
final class HandTest: E2eCase {
    /// 1. Open the account: an existing account, by its name and passphrase.
    func test1_anExistingAccountOpensOnThePhone() throws {
        try logIn(fresh: true)
        XCTAssertTrue(app.buttons["files-trash"].exists, "the file list opened without its trash")
    }

    /// 2. A file from a computer: listed after Sync, and it opens.
    func test2_aFileFromAComputerIsListedAndOpens() throws {
        try logIn()
        try syncUntilListed("from-computer.txt")
        app.cells["file:from-computer.txt"].tap()
        let shared = app.otherElements["ActivityListView"]
        try waitFor("a file from the computer is listed but does not open", seconds: 60) {
            shared.exists || app.alerts.count > 0
        }
        if app.alerts.count > 0 { try fail("opening a file from the computer was refused") }
    }

    /// 3. The trash: a delete waits there with its days, Restore brings it
    /// back, and a delete from every machine asks who is there first, which
    /// the script then checks on the computer.
    func test3_theTrashRestoresAndDeletesForGoodOnceTheOwnerConfirms() throws {
        try logIn()
        try syncUntilListed("to-trash.txt")
        let row = app.cells["file:to-trash.txt"]
        let trashed = app.cells["trash:to-trash.txt"]

        try deleteFromList(row)
        app.buttons["files-trash"].tap()
        try waitFor("a deleted file is not in the trash, so it cannot be put back") { trashed.exists }
        XCTAssertTrue(trashed.label.contains(" days)"), "the trash shows no days left: \(trashed.label)")
        trashed.swipeLeft()
        app.buttons["Restore"].tap()
        try waitFor("restoring left the file in the trash") { !trashed.exists }
        back()
        try waitFor("a restored file did not come back to the list") { row.exists }

        try deleteFromList(row)
        app.buttons["files-trash"].tap()
        try waitFor("a file deleted again is not in the trash") { trashed.exists }
        trashed.swipeLeft()
        app.tables.buttons["Delete"].tap()
        let sure = app.alerts.firstMatch.buttons["Delete"]
        try waitFor("deleting from every machine asked nothing") { sure.exists }
        sure.tap()
        try proveIdentity()
        try waitFor("deleting from every machine left the file in the trash", seconds: 60) { !trashed.exists }
        back()
        // The round that tells the computer.
        app.buttons["files-sync"].tap()
        try waitFor("a file deleted for good came back after a round", seconds: 60) {
            app.alerts.count == 0 && !row.exists
        }
        Thread.sleep(forTimeInterval: 5)
        XCTAssertFalse(row.exists, "a file deleted for good came back after a round")
    }

    /// Who is there, as the trash asks before a delete from every machine:
    /// the phone's own lock when it has one (a finger, on the simulator), the
    /// account's passphrase on a phone without a passcode.
    private func proveIdentity() throws {
        let typed = app.alerts.firstMatch.secureTextFields["identity-pass"]
        try waitFor("deleting from every machine never asked who is there", seconds: 15) {
            typed.exists || springboard.buttons["Cancel"].exists || app.buttons["Cancel"].exists
        }
        if typed.exists {
            typed.tap()
            typed.typeText(passphrase)
            app.alerts.firstMatch.buttons["Confirm"].tap()
        } else {
            touch(matching: true)
        }
    }

    private func deleteFromList(_ row: XCUIElement) throws {
        row.swipeLeft()
        // The row's own Delete: the bar's trash button once read "Delete" too.
        let delete = app.tables["files"].buttons["Delete"]
        if row.exists && delete.exists { delete.tap() }
        try waitFor("a deleted file is still in the list") { !row.exists }
    }

    /// 4. Touch ID: on in Settings; back from the home screen a finger is
    /// asked; a finger that is not enrolled leaves the passphrase field, which
    /// opens; the right finger opens.
    func test4_touchIDOpensForTheRightFingerOnly() throws {
        enrol(true)
        try logIn(fresh: true)
        try setFinger(true)

        leaveAndComeBack()
        touch(matching: false)
        Thread.sleep(forTimeInterval: 3)
        if app.tables["files"].exists { try fail("a finger that is not enrolled opened the account") }
        cancelFingerSheet()
        try unlockWithPassphrase()

        leaveAndComeBack()
        touch(matching: true)
        try waitFor("the enrolled finger did not open the account") { app.tables["files"].exists }
    }

    /// 5. A finger enrolled later: once the enrolment changed, no finger opens,
    /// the passphrase does; off and on again in Settings, a finger opens.
    func test5_aFingerEnrolledLaterOpensNothingUntilTouchIDIsTurnedOnAgain() throws {
        enrol(true)
        try logIn(fresh: true)
        try setFinger(true)
        leaveAndComeBack()
        touch(matching: true)
        try waitFor("the enrolled finger did not open the account") { app.tables["files"].exists }

        // As adding a fingerprint in the iPhone's settings does.
        enrol(false)
        enrol(true)
        leaveAndComeBack()
        touch(matching: true)
        Thread.sleep(forTimeInterval: 3)
        if app.tables["files"].exists {
            try fail("a finger enrolled after Touch ID was turned on opened the account")
        }
        cancelFingerSheet()
        try unlockWithPassphrase()

        try setFinger(false)
        try setFinger(true)
        leaveAndComeBack()
        touch(matching: true)
        try waitFor("Touch ID turned on again does not open with a finger") { app.tables["files"].exists }
    }

    /// 8. Settings hides what an iPhone does not do.
    func test8_settingsHidesWhatAnIPhoneDoesNotDo() throws {
        try logIn()
        try openSettings()
        Thread.sleep(forTimeInterval: 2)
        // A closed section hides its cards from this test as well as from a
        // person: open the ones that would hold what an iPhone cannot do.
        try openSection("This machine", for: web("Appearance"))
        try openSection("Security", for: webContaining("Unlock with my fingerprint or face"))
        for heading in ["Disks", "Copy this account to a USB key", "Contribute to the network", "Advanced"] {
            XCTAssertFalse(
                web(heading).exists,
                "Settings offers \"\(heading)\" on an iPhone, which cannot do it")
        }
    }

    /// 9. Sign out, from Settings: back on the first screen, and the app
    /// opened again asks to log in.
    func test9_signingOutLocksTheAppUntilThePassphrase() throws {
        try logIn()
        try openSettings()
        try openSection("Account", for: web("Sign out", .button))
        try tapOnPage(web("Sign out", .button), "Sign out")
        let ok = app.alerts.firstMatch.buttons["OK"]
        try waitFor("Sign out asked nothing, and did nothing") { ok.exists }
        ok.tap()
        let field = app.secureTextFields["unlock-pass"]
        try waitFor("signing out did not lock the app, so the account stays open", seconds: 60) {
            field.exists && !app.tables["files"].exists
        }

        app.terminate()
        launch(wipe: false)
        try waitFor("the app opened again shows no way in") { field.exists }
        Thread.sleep(forTimeInterval: 3)
        if app.tables["files"].exists { try fail("the app opened again after Sign out without the passphrase") }
        try unlockWithPassphrase()
    }
}
