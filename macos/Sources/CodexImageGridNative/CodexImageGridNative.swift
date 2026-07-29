import AppKit
import SwiftUI
import UniformTypeIdentifiers

extension AppShellLanguage: @unchecked Sendable {}
extension AppShellFontFamily: @unchecked Sendable {}
extension AppShellFontSize: @unchecked Sendable {}
extension AppShellTypography: @unchecked Sendable {}

@main
struct CodexImageGridNative {
    static func main() {
        CodexImageGridApp.main()
    }
}

struct CodexImageGridApp: App {
    var body: some Scene {
        WindowGroup("Codex Image Grid") {
            AppShellRoot {
                ImageGridView()
            }
        }
        .defaultSize(width: 1180, height: 820)
        .commands {
            AppShellSettingsMenu()
        }

        Settings {
            AppShellRoot {
                AppShellPreferencesView()
            }
        }
    }
}

@MainActor
enum NativeFilePicker {
    static func chooseImagePath() -> String? {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.png, .jpeg, .webP]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        return panel.runModal() == .OK ? panel.url?.path : nil
    }
}
