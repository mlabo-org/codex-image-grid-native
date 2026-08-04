import AppKit
import Foundation
import SwiftUI

public enum AppShellLanguage: String, CaseIterable, Identifiable {
    case system
    case japanese
    case english

    public var id: String { rawValue }

    public var resolved: AppShellLanguage {
        guard self == .system else { return self }
        let preferred = Locale.preferredLanguages.first?.lowercased() ?? "en"
        return preferred.hasPrefix("ja") ? .japanese : .english
    }

    public var localeOverride: Locale? {
        switch self {
        case .system:
            return nil
        case .japanese:
            return Locale(identifier: "ja")
        case .english:
            return Locale(identifier: "en")
        }
    }
}

public enum AppShellTheme: String, CaseIterable, Identifiable {
    case system
    case light
    case dark

    public var id: String { rawValue }
}

public enum AppShellFontFamily: String, CaseIterable, Identifiable {
    case system
    case rounded
    case serif
    case monospaced

    public var id: String { rawValue }

    fileprivate var design: Font.Design {
        switch self {
        case .system:
            return .default
        case .rounded:
            return .rounded
        case .serif:
            return .serif
        case .monospaced:
            return .monospaced
        }
    }
}

public struct AppShellFontSize: RawRepresentable, Hashable, Identifiable {
    public static let minimumPoints = 10
    public static let maximumPoints = 32
    public static let defaultValue = AppShellFontSize(points: 16)

    public let rawValue: Int

    public var id: Int { rawValue }
    public var points: Int { rawValue }

    public init?(rawValue: Int) {
        guard Self.range.contains(rawValue) else { return nil }
        self.rawValue = rawValue
    }

    public init(points: Int) {
        rawValue = min(max(points, Self.minimumPoints), Self.maximumPoints)
    }

    public static var range: ClosedRange<Int> {
        minimumPoints...maximumPoints
    }

    fileprivate var scale: CGFloat {
        CGFloat(points) / 17
    }
}

public enum AppShellTextStyle {
    case largeTitle
    case title
    case title2
    case title3
    case headline
    case body
    case callout
    case subheadline
    case footnote
    case caption

    fileprivate var baseSize: CGFloat {
        switch self {
        case .largeTitle:
            return 34
        case .title:
            return 28
        case .title2:
            return 22
        case .title3:
            return 20
        case .headline:
            return 17
        case .body:
            return 17
        case .callout:
            return 16
        case .subheadline:
            return 15
        case .footnote:
            return 13
        case .caption:
            return 12
        }
    }

    fileprivate var defaultWeight: Font.Weight {
        switch self {
        case .largeTitle, .title, .title2, .title3, .headline:
            return .semibold
        default:
            return .regular
        }
    }
}

public struct AppShellTypography {
    public let family: AppShellFontFamily
    public let size: AppShellFontSize

    public init(family: AppShellFontFamily, size: AppShellFontSize) {
        self.family = family
        self.size = size
    }

    public func font(
        _ style: AppShellTextStyle,
        weight: Font.Weight? = nil
    ) -> Font {
        Font.system(
            size: style.baseSize * size.scale,
            weight: weight ?? style.defaultWeight,
            design: family.design
        )
    }
}

private struct AppShellTypographyKey: EnvironmentKey {
    static let defaultValue = AppShellTypography(
        family: .system,
        size: .defaultValue
    )
}

private struct AppShellLanguageKey: EnvironmentKey {
    static let defaultValue = AppShellLanguage.system
}

public extension EnvironmentValues {
    var appShellTypography: AppShellTypography {
        get { self[AppShellTypographyKey.self] }
        set { self[AppShellTypographyKey.self] = newValue }
    }

    var appShellLanguage: AppShellLanguage {
        get { self[AppShellLanguageKey.self] }
        set { self[AppShellLanguageKey.self] = newValue }
    }
}

public extension View {
    func appFont(
        _ style: AppShellTextStyle,
        weight: Font.Weight? = nil
    ) -> some View {
        modifier(AppShellFontModifier(style: style, weight: weight))
    }
}

private struct AppShellFontModifier: ViewModifier {
    @Environment(\.appShellTypography) private var typography

    let style: AppShellTextStyle
    let weight: Font.Weight?

    func body(content: Content) -> some View {
        content.font(typography.font(style, weight: weight))
    }
}

public enum AppShellPreferenceKeys {
    public static let language = "appShell.language"
    public static let theme = "appShell.theme"
    public static let fontFamily = "appShell.fontFamily"
    public static let fontSize = "appShell.fontSizePoints"
}

public enum AppShellPreferencesLayout {
    public static let width: CGFloat = 520
    public static let height: CGFloat = 320
}

@MainActor
public enum AppShellAppearance {
    public static func apply(_ theme: AppShellTheme) {
        switch theme {
        case .system:
            NSApplication.shared.appearance = nil
        case .light:
            NSApplication.shared.appearance = NSAppearance(named: .aqua)
        case .dark:
            NSApplication.shared.appearance = NSAppearance(named: .darkAqua)
        }
    }
}

public struct AppShellRoot<Content: View>: View {
    @AppStorage(AppShellPreferenceKeys.language) private var language = AppShellLanguage.system
    @AppStorage(AppShellPreferenceKeys.theme) private var theme = AppShellTheme.system
    @AppStorage(AppShellPreferenceKeys.fontFamily) private var fontFamily = AppShellFontFamily.system
    @AppStorage(AppShellPreferenceKeys.fontSize) private var fontSize =
        AppShellFontSize.defaultValue

    private let content: Content

    public init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    public var body: some View {
        localizedContent
            .environment(
                \.appShellTypography,
                AppShellTypography(family: fontFamily, size: fontSize)
            )
            .environment(\.appShellLanguage, language)
            .font(
                AppShellTypography(family: fontFamily, size: fontSize)
                    .font(.body)
            )
            .onAppear {
                AppShellAppearance.apply(theme)
            }
            .onChange(of: theme) { newTheme in
                AppShellAppearance.apply(newTheme)
            }
    }

    @ViewBuilder
    private var localizedContent: some View {
        if let locale = language.localeOverride {
            content.environment(\.locale, locale)
        } else {
            content
        }
    }
}

public struct AppShellPreferencesView: View {
    @AppStorage(AppShellPreferenceKeys.language) private var language = AppShellLanguage.system
    @AppStorage(AppShellPreferenceKeys.theme) private var theme = AppShellTheme.system
    @AppStorage(AppShellPreferenceKeys.fontFamily) private var fontFamily = AppShellFontFamily.system
    @AppStorage(AppShellPreferenceKeys.fontSize) private var fontSize =
        AppShellFontSize.defaultValue

    public init() {}

    public var body: some View {
        let strings = AppShellStrings(language: language)

        Form {
            Picker(strings.language, selection: $language) {
                ForEach(AppShellLanguage.allCases) { choice in
                    Text(strings.name(for: choice)).tag(choice)
                }
            }

            Picker(strings.theme, selection: $theme) {
                ForEach(AppShellTheme.allCases) { choice in
                    Text(strings.name(for: choice)).tag(choice)
                }
            }

            Picker(strings.fontFamily, selection: $fontFamily) {
                ForEach(AppShellFontFamily.allCases) { choice in
                    Text(strings.name(for: choice)).tag(choice)
                }
            }

            LabeledContent(strings.fontSize) {
                HStack(spacing: 6) {
                    TextField(
                        strings.fontSize,
                        value: fontSizePoints,
                        format: .number
                    )
                    .labelsHidden()
                    .multilineTextAlignment(.trailing)
                    .frame(width: 54)

                    Text(strings.pointUnit)
                        .foregroundStyle(.secondary)
                        .accessibilityHidden(true)

                    Stepper(
                        strings.fontSize,
                        value: fontSizePoints,
                        in: AppShellFontSize.range,
                        step: 1
                    )
                    .labelsHidden()
                }
            }

            Text(strings.preview)
                .appFont(.body)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 8)
        }
        .formStyle(.grouped)
        .padding()
        .frame(
            width: AppShellPreferencesLayout.width,
            height: AppShellPreferencesLayout.height
        )
    }

    private var fontSizePoints: Binding<Int> {
        Binding(
            get: { fontSize.points },
            set: { fontSize = AppShellFontSize(points: $0) }
        )
    }
}

public struct AppShellSettingsMenu: Commands {
    @AppStorage(AppShellPreferenceKeys.language) private var language = AppShellLanguage.system
    @AppStorage(AppShellPreferenceKeys.theme) private var theme = AppShellTheme.system
    @AppStorage(AppShellPreferenceKeys.fontFamily) private var fontFamily = AppShellFontFamily.system
    @AppStorage(AppShellPreferenceKeys.fontSize) private var fontSize =
        AppShellFontSize.defaultValue

    public init() {}

    public var body: some Commands {
        CommandMenu(AppShellStrings(language: language).display) {
            Picker(AppShellStrings(language: language).language, selection: $language) {
                ForEach(AppShellLanguage.allCases) { choice in
                    Text(AppShellStrings(language: language).name(for: choice)).tag(choice)
                }
            }

            Picker(AppShellStrings(language: language).theme, selection: $theme) {
                ForEach(AppShellTheme.allCases) { choice in
                    Text(AppShellStrings(language: language).name(for: choice)).tag(choice)
                }
            }

            Picker(AppShellStrings(language: language).fontFamily, selection: $fontFamily) {
                ForEach(AppShellFontFamily.allCases) { choice in
                    Text(AppShellStrings(language: language).name(for: choice)).tag(choice)
                }
            }

            Menu(
                "\(AppShellStrings(language: language).fontSize): "
                    + "\(fontSize.points) "
                    + AppShellStrings(language: language).pointUnit
            ) {
                Button(AppShellStrings(language: language).decreaseFontSize) {
                    fontSize = AppShellFontSize(points: fontSize.points - 1)
                }
                .disabled(fontSize.points == AppShellFontSize.minimumPoints)

                Button(AppShellStrings(language: language).increaseFontSize) {
                    fontSize = AppShellFontSize(points: fontSize.points + 1)
                }
                .disabled(fontSize.points == AppShellFontSize.maximumPoints)

                Divider()

                Button(AppShellStrings(language: language).resetFontSize) {
                    fontSize = .defaultValue
                }
                .disabled(fontSize == .defaultValue)
            }
        }
    }
}

public struct AppShellStrings {
    private let resolvedLanguage: AppShellLanguage

    public init(language: AppShellLanguage) {
        resolvedLanguage = language.resolved
    }

    public func localized(japanese: String, english: String) -> String {
        resolvedLanguage == .japanese ? japanese : english
    }

    public var display: String {
        localized(japanese: "表示", english: "Display")
    }

    public var language: String {
        localized(japanese: "言語", english: "Language")
    }

    public var theme: String {
        localized(japanese: "テーマ", english: "Theme")
    }

    public var fontFamily: String {
        localized(japanese: "フォント", english: "Font")
    }

    public var fontSize: String {
        localized(japanese: "文字サイズ", english: "Text size")
    }

    public var pointUnit: String { "pt" }

    public var decreaseFontSize: String {
        localized(japanese: "文字を1 pt小さく", english: "Decrease by 1 pt")
    }

    public var increaseFontSize: String {
        localized(japanese: "文字を1 pt大きく", english: "Increase by 1 pt")
    }

    public var resetFontSize: String {
        localized(japanese: "16 ptに戻す", english: "Reset to 16 pt")
    }

    public var preview: String {
        localized(
            japanese: "表示設定のプレビューです。",
            english: "This is a preview of the display settings."
        )
    }

    public func name(for value: AppShellLanguage) -> String {
        switch value {
        case .system:
            return localized(japanese: "システム", english: "System")
        case .japanese:
            return localized(japanese: "日本語", english: "Japanese")
        case .english:
            return localized(japanese: "英語", english: "English")
        }
    }

    public func name(for value: AppShellTheme) -> String {
        switch value {
        case .system:
            return localized(japanese: "システム", english: "System")
        case .light:
            return localized(japanese: "ライト", english: "Light")
        case .dark:
            return localized(japanese: "ダーク", english: "Dark")
        }
    }

    public func name(for value: AppShellFontFamily) -> String {
        switch value {
        case .system:
            return localized(japanese: "システム", english: "System")
        case .rounded:
            return localized(japanese: "丸ゴシック", english: "Rounded")
        case .serif:
            return localized(japanese: "明朝", english: "Serif")
        case .monospaced:
            return localized(japanese: "等幅", english: "Monospaced")
        }
    }

}
