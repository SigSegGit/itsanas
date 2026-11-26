import XCTest

/// Rename and Versions from the iPhone's Files screen. `scripts/iphone-e2e.sh`
/// puts `two-versions.txt` twice and `to-rename.txt` once on the computer, and
/// checks there afterwards that the rename reached it. Android's RenameTest and
/// VersionsTest, on the iPhone.
final class FileEditsTest: E2eCase {
    func testAnEarlierVersionComesBackFromTheVersionsScreen() throws {
        try logIn()
        try syncUntilListed("two-versions.txt")
        let row = app.cells["file:two-versions.txt"]
        row.swipeLeft()
        let versions = app.buttons["Versions"]
        try waitFor("a file offers no Versions") { versions.exists }
        versions.tap()
        let earlier = app.cells["version:1"]
        try waitFor("a file put twice on the computer lists no earlier version") { earlier.exists }
        earlier.tap()
        // Bringing a version back writes it again: a third version, the file now.
        try waitFor("bringing the earlier version back wrote nothing", seconds: 60) {
            app.cells["version:2"].exists
        }
        let newest = app.cells["version:0"]
        XCTAssertTrue(
            newest.label.contains("the file now") || newest.staticTexts.allElementsBoundByIndex
                .contains { $0.label.contains("the file now") },
            "the version brought back is not the file now")
        back()
    }

    func testARenamedFileLeavesItsOldName() throws {
        try logIn()
        try syncUntilListed("to-rename.txt")
        app.cells["file:to-rename.txt"].swipeLeft()
        let rename = app.buttons["Rename"]
        try waitFor("a file offers no Rename") { rename.exists }
        rename.tap()
        let field = app.alerts.firstMatch.textFields["rename-name"]
        try waitFor("Rename asked for no new name") { field.exists }
        // A touch past the short name puts the cursor at its end: the
        // deletes clear it, then the new name goes in.
        field.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5)).tap()
        let clear = String(repeating: XCUIKeyboardKey.delete.rawValue, count: "to-rename.txt".count + 4)
        field.typeText(clear + "renamed.txt")
        app.alerts.firstMatch.buttons["Rename"].tap()
        try waitFor("the file never took its new name", seconds: 60) { app.cells["file:renamed.txt"].exists }
        XCTAssertFalse(
            app.cells["file:to-rename.txt"].exists,
            "the old name is still listed beside the new: a rename doubled the file")
    }
}
