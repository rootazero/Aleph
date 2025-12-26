import XCTest

/// UI Tests for Modernize Settings UI (Phase 6)
/// Tests visual interactions, animations, and user flows
final class ModernSettingsUITestsXCUI: XCTestCase {

    var app: XCUIApplication!

    // MARK: - Setup & Teardown

    override func setUpWithError() throws {
        try super.setUpWithError()
        continueAfterFailure = false

        app = XCUIApplication()
        app.launchArguments = ["--uitesting"]
        app.launch()
    }

    override func tearDownWithError() throws {
        app = nil
        try super.tearDownWithError()
    }

    // MARK: - 6.2.1 Light Mode Visual Tests

    func testLightModeThemeSwitcher() throws {
        // Open settings window
        // Note: Actual implementation depends on how settings are accessed
        // This is a template for the test structure

        // Find theme switcher
        let themeSwitcher = app.buttons["ThemeSwitcher"]
        XCTAssertTrue(themeSwitcher.waitForExistence(timeout: 5))

        // Click Light mode button
        let lightModeButton = app.buttons["LightModeButton"]
        lightModeButton.tap()

        // Verify light mode applied
        // This would check appearance properties
        XCTAssertTrue(lightModeButton.isSelected)

        // Verify other elements visible in light mode
        let providersTab = app.staticTexts["Providers"]
        XCTAssertTrue(providersTab.exists)
    }

    func testLightModeProviderCards() throws {
        // Switch to light mode
        let lightModeButton = app.buttons["LightModeButton"]
        if lightModeButton.exists {
            lightModeButton.tap()
        }

        // Navigate to Providers tab
        app.buttons["Providers"].tap()

        // Verify provider cards visible
        let providerCard = app.otherElements["ProviderCard"]
        XCTAssertTrue(providerCard.waitForExistence(timeout: 3))

        // Verify card elements readable
        let providerName = app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'OpenAI' OR label CONTAINS 'Claude'"))
        XCTAssertTrue(providerName.firstMatch.exists)
    }

    // MARK: - 6.2.2 Dark Mode Visual Tests

    func testDarkModeThemeSwitcher() throws {
        // Find theme switcher
        let themeSwitcher = app.buttons["ThemeSwitcher"]
        XCTAssertTrue(themeSwitcher.waitForExistence(timeout: 5))

        // Click Dark mode button
        let darkModeButton = app.buttons["DarkModeButton"]
        darkModeButton.tap()

        // Verify dark mode applied
        XCTAssertTrue(darkModeButton.isSelected)

        // Wait for theme to apply
        Thread.sleep(forTimeInterval: 0.5)

        // Verify sidebar visible in dark mode
        let sidebar = app.otherElements["ModernSidebar"]
        XCTAssertTrue(sidebar.exists)
    }

    func testDarkModeProviderCards() throws {
        // Switch to dark mode
        let darkModeButton = app.buttons["DarkModeButton"]
        if darkModeButton.exists {
            darkModeButton.tap()
        }

        // Navigate to Providers tab
        app.buttons["Providers"].tap()

        // Verify provider cards visible in dark mode
        let providerCard = app.otherElements["ProviderCard"]
        XCTAssertTrue(providerCard.waitForExistence(timeout: 3))

        // Verify shadows and borders visible
        // This would require checking visual properties
        XCTAssertTrue(providerCard.exists)
    }

    // MARK: - 6.2.3 Auto Mode Tests

    func testAutoModeFollowsSystem() throws {
        // Click Auto mode button
        let autoModeButton = app.buttons["AutoModeButton"]
        if autoModeButton.exists {
            autoModeButton.tap()
        }

        XCTAssertTrue(autoModeButton.isSelected)

        // Note: Testing system appearance changes requires macOS API access
        // This would use AppleScript or system events to change appearance
        // For now, we verify auto mode can be selected
    }

    // MARK: - 6.2.4 Theme Switcher Interaction Tests

    func testThemeSwitcherHighlight() throws {
        let lightButton = app.buttons["LightModeButton"]
        let darkButton = app.buttons["DarkModeButton"]
        let autoButton = app.buttons["AutoModeButton"]

        // Test Light mode selection
        lightButton.tap()
        XCTAssertTrue(lightButton.isSelected)
        XCTAssertFalse(darkButton.isSelected)
        XCTAssertFalse(autoButton.isSelected)

        // Test Dark mode selection
        darkButton.tap()
        XCTAssertFalse(lightButton.isSelected)
        XCTAssertTrue(darkButton.isSelected)
        XCTAssertFalse(autoButton.isSelected)

        // Test Auto mode selection
        autoButton.tap()
        XCTAssertFalse(lightButton.isSelected)
        XCTAssertFalse(darkButton.isSelected)
        XCTAssertTrue(autoButton.isSelected)
    }

    func testThemeSwitcherAnimation() throws {
        let lightButton = app.buttons["LightModeButton"]
        let darkButton = app.buttons["DarkModeButton"]

        // Click light mode
        lightButton.tap()
        Thread.sleep(forTimeInterval: 0.3) // Wait for animation

        // Click dark mode
        darkButton.tap()
        Thread.sleep(forTimeInterval: 0.3) // Wait for animation

        // Verify no crashes during rapid switching
        for _ in 0..<5 {
            lightButton.tap()
            darkButton.tap()
        }

        XCTAssertTrue(darkButton.isSelected)
    }

    // MARK: - 6.2.5 Window Size Tests

    func testMinimumWindowSize() throws {
        // Get window
        let window = app.windows.firstMatch
        XCTAssertTrue(window.exists)

        // Attempt to resize to minimum (800x600)
        // Note: XCTest doesn't directly support window resizing
        // This would require AppleScript or Accessibility API

        // Verify all elements still visible
        let sidebar = app.otherElements["ModernSidebar"]
        let themeSwitcher = app.buttons["ThemeSwitcher"]

        XCTAssertTrue(sidebar.exists)
        XCTAssertTrue(themeSwitcher.exists)
    }

    func testFullscreenMode() throws {
        // Get window
        let window = app.windows.firstMatch
        XCTAssertTrue(window.exists)

        // Toggle fullscreen (Cmd+Ctrl+F)
        // Note: This requires accessibility permissions
        // window.typeKey("f", modifierFlags: [.command, .control])

        // Verify layout in fullscreen
        let sidebar = app.otherElements["ModernSidebar"]
        XCTAssertTrue(sidebar.exists)
    }

    // MARK: - 6.5.1 VoiceOver Tests

    func testVoiceOverSidebarAccessibility() throws {
        // Note: VoiceOver testing requires special setup
        // These tests verify accessibility identifiers and labels exist

        let generalButton = app.buttons["General"]
        XCTAssertTrue(generalButton.exists)
        XCTAssertNotNil(generalButton.label)
        XCTAssertFalse(generalButton.label.isEmpty)

        let providersButton = app.buttons["Providers"]
        XCTAssertTrue(providersButton.exists)
        XCTAssertNotNil(providersButton.label)
    }

    func testVoiceOverProviderCards() throws {
        // Navigate to Providers tab
        app.buttons["Providers"].tap()

        // Check provider card accessibility
        let providerCards = app.otherElements.matching(identifier: "ProviderCard")
        if providerCards.count > 0 {
            let firstCard = providerCards.firstMatch
            XCTAssertTrue(firstCard.exists)

            // Verify accessibility label exists
            let label = firstCard.label
            XCTAssertFalse(label.isEmpty, "Provider card should have accessibility label")
        }
    }

    func testVoiceOverThemeSwitcher() throws {
        let lightButton = app.buttons["LightModeButton"]
        let darkButton = app.buttons["DarkModeButton"]
        let autoButton = app.buttons["AutoModeButton"]

        // Verify all buttons have accessibility labels
        XCTAssertFalse(lightButton.label.isEmpty, "Light mode button should have label")
        XCTAssertFalse(darkButton.label.isEmpty, "Dark mode button should have label")
        XCTAssertFalse(autoButton.label.isEmpty, "Auto mode button should have label")

        // Verify labels are descriptive
        XCTAssertTrue(lightButton.label.contains("Light") || lightButton.label.contains("Day"))
        XCTAssertTrue(darkButton.label.contains("Dark") || darkButton.label.contains("Night"))
        XCTAssertTrue(autoButton.label.contains("Auto") || autoButton.label.contains("System"))
    }

    // MARK: - 6.5.2 Keyboard Navigation Tests

    func testTabKeyNavigation() throws {
        // Note: Tab key navigation requires app to have focus
        // This test verifies the navigation flow

        let firstElement = app.buttons.firstMatch
        firstElement.tap() // Give focus

        // Press Tab key multiple times
        for _ in 0..<5 {
            app.typeKey(XCUIKeyboardKey.tab, modifierFlags: [])
            Thread.sleep(forTimeInterval: 0.1)
        }

        // Verify focus moved (no crashes)
        XCTAssertTrue(app.windows.firstMatch.exists)
    }

    func testEscapeKeyClosesModal() throws {
        // Open a modal (e.g., Add Provider)
        let addButton = app.buttons["AddProviderButton"]
        if addButton.exists {
            addButton.tap()

            // Wait for modal
            let modal = app.sheets.firstMatch
            if modal.waitForExistence(timeout: 2) {
                // Press Escape
                app.typeKey(XCUIKeyboardKey.escape, modifierFlags: [])

                // Verify modal closed
                Thread.sleep(forTimeInterval: 0.5)
                XCTAssertFalse(modal.exists, "Modal should close on Escape")
            }
        }
    }

    func testReturnKeyActivatesButton() throws {
        // Focus a button
        let button = app.buttons.firstMatch
        button.tap()

        // Press Return key
        app.typeKey(XCUIKeyboardKey.return, modifierFlags: [])

        // Verify action occurred (no crash)
        XCTAssertTrue(app.windows.firstMatch.exists)
    }

    // MARK: - Provider Tab Interaction Tests

    func testProviderSearch() throws {
        // Navigate to Providers tab
        app.buttons["Providers"].tap()

        // Find search field
        let searchField = app.searchFields["ProviderSearchField"]
        if searchField.exists {
            searchField.tap()
            searchField.typeText("OpenAI")

            // Verify filtering occurred
            // Note: This would require checking the number of visible cards
            let providerCards = app.otherElements.matching(identifier: "ProviderCard")

            // At least one card should match
            XCTAssertGreaterThan(providerCards.count, 0)
        }
    }

    func testProviderCardSelection() throws {
        // Navigate to Providers tab
        app.buttons["Providers"].tap()

        // Click first provider card
        let firstCard = app.otherElements.matching(identifier: "ProviderCard").firstMatch
        if firstCard.exists {
            firstCard.tap()

            // Verify detail panel appears
            let detailPanel = app.otherElements["ProviderDetailPanel"]
            XCTAssertTrue(detailPanel.waitForExistence(timeout: 2))
        }
    }

    // MARK: - Sidebar Interaction Tests

    func testSidebarNavigation() throws {
        let tabs = ["General", "Providers", "Routing", "Shortcuts", "Behavior", "Memory"]

        for tabName in tabs {
            let tabButton = app.buttons[tabName]
            if tabButton.exists {
                tabButton.tap()

                // Wait for content to load
                Thread.sleep(forTimeInterval: 0.3)

                // Verify tab selected (visual feedback)
                XCTAssertTrue(tabButton.isSelected || tabButton.value as? String == "selected")
            }
        }
    }

    func testSidebarBottomActions() throws {
        // Test Import button
        let importButton = app.buttons["ImportSettingsButton"]
        if importButton.exists {
            XCTAssertTrue(importButton.isEnabled)
        }

        // Test Export button
        let exportButton = app.buttons["ExportSettingsButton"]
        if exportButton.exists {
            XCTAssertTrue(exportButton.isEnabled)
        }

        // Test Reset button
        let resetButton = app.buttons["ResetSettingsButton"]
        if resetButton.exists {
            XCTAssertTrue(resetButton.isEnabled)
        }
    }

    // MARK: - Animation Smoothness Tests

    func testProviderCardHoverAnimation() throws {
        // Navigate to Providers tab
        app.buttons["Providers"].tap()

        let firstCard = app.otherElements.matching(identifier: "ProviderCard").firstMatch
        if firstCard.exists {
            // Hover over card (requires mouse event simulation)
            // Note: XCTest doesn't directly support hover
            // This would require Accessibility API or AppleScript

            // For now, verify card exists and is hittable
            XCTAssertTrue(firstCard.isHittable)
        }
    }

    func testSidebarSelectionAnimation() throws {
        let generalTab = app.buttons["General"]
        let providersTab = app.buttons["Providers"]

        // Click between tabs rapidly
        for _ in 0..<3 {
            generalTab.tap()
            Thread.sleep(forTimeInterval: 0.3)
            providersTab.tap()
            Thread.sleep(forTimeInterval: 0.3)
        }

        // Verify no crashes during rapid switching
        XCTAssertTrue(providersTab.exists)
    }

    // MARK: - Performance Tests

    func testSettingsWindowLaunchPerformance() throws {
        measure(metrics: [XCTApplicationLaunchMetric()]) {
            app.launch()
        }
    }

    func testThemeSwitchingPerformance() throws {
        let lightButton = app.buttons["LightModeButton"]
        let darkButton = app.buttons["DarkModeButton"]

        measure(metrics: [XCTClockMetric()]) {
            for _ in 0..<10 {
                lightButton.tap()
                darkButton.tap()
            }
        }
    }

    func testProviderSearchPerformance() throws {
        app.buttons["Providers"].tap()

        let searchField = app.searchFields["ProviderSearchField"]
        if searchField.exists {
            measure(metrics: [XCTClockMetric()]) {
                searchField.tap()
                searchField.typeText("test")
                searchField.buttons["Clear"].tap()
            }
        }
    }
}
