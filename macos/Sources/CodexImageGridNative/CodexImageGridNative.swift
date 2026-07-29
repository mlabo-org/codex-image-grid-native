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
    @NSApplicationDelegateAdaptor(CodexImageGridApplicationDelegate.self)
    private var applicationDelegate

    var body: some Scene {
        WindowGroup("Codex Image Grid") {
            ImageGridView()
                .environmentObject(applicationDelegate.runtimeLifecycle)
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
final class CodexImageGridApplicationDelegate: NSObject, NSApplicationDelegate {
    let runtimeLifecycle = NativeRuntimeLifecycle()
    private var terminationReplyTask: Task<Void, Never>?

    func applicationDidFinishLaunching(_ notification: Notification) {
        runtimeLifecycle.start()
    }

    func applicationShouldTerminate(
        _ sender: NSApplication
    ) -> NSApplication.TerminateReply {
        if terminationReplyTask == nil {
            terminationReplyTask = Task { @MainActor [runtimeLifecycle] in
                await runtimeLifecycle.stop()
                sender.reply(toApplicationShouldTerminate: true)
            }
        }
        return .terminateLater
    }

    func applicationWillTerminate(_ notification: Notification) {
        // applicationShouldTerminate owns the one shared graceful-stop completion.
    }
}

@MainActor
enum NativeFilePicker {
    static func chooseImageURL() -> URL? {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.png, .jpeg, .webP]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        return panel.runModal() == .OK ? panel.url : nil
    }
}
