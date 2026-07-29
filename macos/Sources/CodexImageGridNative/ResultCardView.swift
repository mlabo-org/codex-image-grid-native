import SwiftUI

struct ResponsiveResultGrid {
    let minimumColumnWidth: CGFloat
    let spacing: CGFloat

    init(minimumColumnWidth: CGFloat, spacing: CGFloat) {
        self.minimumColumnWidth = minimumColumnWidth
        self.spacing = spacing
    }

    func columnCount(for width: CGFloat, itemCount: Int) -> Int {
        max(
            1,
            min(itemCount, Int((width + spacing) / (minimumColumnWidth + spacing)))
        )
    }

    func gridItems(for width: CGFloat, itemCount: Int) -> [GridItem] {
        gridItems(count: columnCount(for: width, itemCount: itemCount))
    }

    func gridItems(count: Int) -> [GridItem] {
        Array(
            repeating: GridItem(
                .flexible(minimum: 0),
                spacing: spacing,
                alignment: .top
            ),
            count: max(1, count)
        )
    }
}

struct ResultCardView: View {
    @Environment(\.appShellLanguage) private var language

    let job: ImageGridJob
    let imageURL: URL?
    let onCopy: () -> Void
    let onReveal: () -> Void
    let onManifest: () -> Void
    let onHandoff: () -> Void

    @State private var showsPreview = false

    private var strings: ResultCardStrings {
        ResultCardStrings(language: language)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            image
            VStack(alignment: .leading, spacing: 10) {
                heading
                Divider()
                prompt
                settings
                artifactActions
                if let log = job.log, !log.isEmpty {
                    DisclosureGroup(strings.generationLog) {
                        Text(log)
                            .appFont(.caption)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.top, 6)
                    }
                }
            }
            .padding(14)
        }
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(Color(nsColor: .separatorColor), lineWidth: 1)
        }
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .sheet(isPresented: $showsPreview) {
            preview
        }
    }

    private var image: some View {
        ZStack {
            Color(nsColor: .underPageBackgroundColor)
            if let imageURL {
                AsyncImage(url: imageURL) { phase in
                    switch phase {
                    case .empty:
                        ProgressView()
                    case let .success(image):
                        image
                            .resizable()
                            .scaledToFill()
                    case .failure:
                        Image(systemName: "photo.badge.exclamationmark")
                            .foregroundStyle(.secondary)
                            .accessibilityLabel(strings.imageUnavailable)
                    @unknown default:
                        EmptyView()
                    }
                }
            } else if job.isActive {
                VStack(spacing: 8) {
                    ProgressView()
                    Text(job.statusText ?? job.status)
                        .appFont(.caption)
                }
                .foregroundStyle(.secondary)
            } else {
                Image(systemName: "photo.badge.exclamationmark")
                    .foregroundStyle(.secondary)
                    .accessibilityLabel(strings.imageUnavailable)
            }
        }
        .aspectRatio(aspectRatio, contentMode: .fit)
        .clipped()
        .accessibilityElement(children: .combine)
    }

    private var heading: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .appFont(.headline)
                    .lineLimit(2)
                Label(job.statusText ?? job.status, systemImage: statusSymbol)
                    .appFont(.caption, weight: .semibold)
                    .foregroundStyle(statusColor)
            }
            Spacer(minLength: 8)
            Button(strings.preview) {
                showsPreview = true
            }
            .disabled(imageURL == nil)
        }
    }

    private var prompt: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(strings.usedPrompt)
                    .appFont(.caption, weight: .semibold)
                    .foregroundStyle(.secondary)
                Spacer()
                Button(strings.copy, action: onCopy)
                    .disabled(job.prompt?.isEmpty != false)
            }
            Text(job.prompt?.isEmpty == false ? job.prompt! : strings.promptUnavailable)
                .appFont(.caption)
                .textSelection(.enabled)
                .lineLimit(5)
        }
    }

    private var settings: some View {
        Text(strings.settings(job))
            .appFont(.caption)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var artifactActions: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 10) {
                artifactButtons
            }
            VStack(alignment: .leading, spacing: 8) {
                artifactButtons
            }
        }
    }

    @ViewBuilder
    private var artifactButtons: some View {
        Button("manifest", action: onManifest)
            .disabled(job.manifestViewUrl == nil)
        Button("handoff", action: onHandoff)
            .disabled(job.handoffViewUrl == nil)
        Button(strings.reveal, action: onReveal)
            .disabled(job.outputPath == nil || job.status != "done")
    }

    private var preview: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text(strings.preview)
                        .appFont(.title3)
                    Text("\(job.filename ?? strings.image) · Run \(job.runId ?? "—")")
                        .appFont(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button(strings.close) {
                    showsPreview = false
                }
                .keyboardShortcut(.cancelAction)
            }
            if let imageURL {
                AsyncImage(url: imageURL) { phase in
                    switch phase {
                    case .empty:
                        ProgressView()
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    case let .success(image):
                        image
                            .resizable()
                            .scaledToFit()
                    case .failure:
                        ContentUnavailableView(
                            strings.imageUnavailable,
                            systemImage: "photo.badge.exclamationmark"
                        )
                    @unknown default:
                        EmptyView()
                    }
                }
            }
        }
        .padding(18)
        .frame(minWidth: 520, idealWidth: 900, minHeight: 420, idealHeight: 680)
    }

    private var title: String {
        let promptPart = (job.promptTotal ?? 1) > 1
            ? "Prompt \(job.promptIndex ?? 1)/\(job.promptTotal ?? 1) · "
            : ""
        return "\(promptPart)Variant \(job.variant ?? 1)/\(job.total ?? 1) · \(job.model ?? job.engine ?? "job")"
    }

    private var aspectRatio: CGFloat {
        guard let value = job.aspectRatio?.split(separator: ":"),
              value.count == 2,
              let width = Double(value[0]),
              let height = Double(value[1]),
              height > 0
        else {
            return 16 / 9
        }
        return CGFloat(width / height)
    }

    private var statusSymbol: String {
        switch job.status {
        case "done": "checkmark.circle.fill"
        case "error": "exclamationmark.triangle.fill"
        case "queued": "clock.fill"
        default: "arrow.triangle.2.circlepath"
        }
    }

    private var statusColor: Color {
        switch job.status {
        case "done": .green
        case "error": .red
        default: .secondary
        }
    }
}

private struct ResultCardStrings {
    private let language: AppShellLanguage

    init(language: AppShellLanguage) {
        self.language = language.resolved
    }

    private func localized(_ japanese: String, _ english: String) -> String {
        language == .japanese ? japanese : english
    }

    var preview: String { localized("プレビュー", "Preview") }
    var close: String { localized("閉じる", "Close") }
    var copy: String { localized("コピー", "Copy") }
    var reveal: String { localized("Finderに表示", "Reveal") }
    var usedPrompt: String { localized("使用Prompt", "Prompt used") }
    var promptUnavailable: String {
        localized("この生成のPromptは残っていません。", "The prompt for this generation is unavailable.")
    }
    var generationLog: String { localized("生成ログ", "Generation log") }
    var imageUnavailable: String { localized("画像を読み込めません", "Image unavailable") }
    var image: String { localized("画像", "Image") }

    func settings(_ job: ImageGridJob) -> String {
        let mood = job.mood ?? localized("未指定", "Not specified")
        let ratio = job.aspectRatio ?? localized("未指定", "Not specified")
        let reference = job.referenceImagePath == nil
            ? localized("なし", "No")
            : localized("あり", "Yes")
        return [
            localized("雰囲気: \(mood)", "Mood: \(mood)"),
            localized("比率: \(ratio)", "Ratio: \(ratio)"),
            localized("参照画像: \(reference)", "Reference: \(reference)"),
        ].joined(separator: " / ")
    }
}
