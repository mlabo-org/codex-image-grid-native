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

enum ResultCardImageLoadFailurePresentation: Equatable {
    case progress
    case generationFailure
    case imageUnavailable

    static func resolve(for job: ImageGridJob) -> Self {
        if job.isActive {
            return .progress
        }
        return job.status == "error" ? .generationFailure : .imageUnavailable
    }
}

struct ResultCardImageRequestIdentity: Hashable {
    let jobID: String
    let jobStatus: String
    let imageURL: URL
}

struct ResultCardView: View {
    @Environment(\.appShellLanguage) private var language

    let job: ImageGridJob
    let imageURL: URL?
    let isSelected: Bool
    let isSelectable: Bool
    let onSelectionToggle: () -> Void
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
                .stroke(
                    isSelected
                        ? Color.accentColor
                        : (job.status == "error"
                            ? Color.red.opacity(0.55)
                            : Color(nsColor: .separatorColor)),
                    lineWidth: isSelected ? 3 : (job.status == "error" ? 1.5 : 1)
                )
        }
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .sheet(isPresented: $showsPreview) {
            preview
        }
    }

    private var image: some View {
        ZStack(alignment: .topLeading) {
            Color(nsColor: .underPageBackgroundColor)
            if let imageURL {
                AsyncImage(url: imageURL) { phase in
                    switch phase {
                    case .empty:
                        if job.isActive {
                            activeImagePlaceholder
                        } else {
                            ProgressView()
                        }
                    case let .success(image):
                        image
                            .resizable()
                            .scaledToFill()
                    case .failure:
                        switch ResultCardImageLoadFailurePresentation.resolve(for: job) {
                        case .progress:
                            activeImagePlaceholder
                        case .generationFailure:
                            generationFailurePlaceholder
                        case .imageUnavailable:
                            imageUnavailablePlaceholder
                        }
                    @unknown default:
                        EmptyView()
                    }
                }
                .id(imageRequestIdentity(imageURL))
            } else if job.isActive {
                activeImagePlaceholder
            } else if job.status == "error" {
                generationFailurePlaceholder
            } else {
                imageUnavailablePlaceholder
            }

            if isSelectable {
                Button(action: onSelectionToggle) {
                    Label(
                        isSelected ? strings.selectedForDeletion : strings.markForDeletion,
                        systemImage: isSelected ? "checkmark.circle.fill" : "circle"
                    )
                    .appFont(.caption, weight: .semibold)
                    .foregroundStyle(isSelected ? Color.white : Color.primary)
                    .padding(.horizontal, 10)
                    .frame(height: 30)
                    .background(
                        isSelected
                            ? Color.accentColor
                            : Color(nsColor: .windowBackgroundColor).opacity(0.92),
                        in: Capsule()
                    )
                    .overlay {
                        Capsule()
                            .stroke(
                                isSelected
                                    ? Color.accentColor
                                    : Color(nsColor: .separatorColor),
                                lineWidth: 1
                            )
                    }
                }
                .buttonStyle(.plain)
                .padding(10)
                .accessibilityLabel(
                    isSelected ? strings.removeFromDeletion : strings.addToDeletion
                )
            }
        }
        .aspectRatio(aspectRatio, contentMode: .fit)
        .clipped()
        .accessibilityElement(children: .combine)
    }

    private var activeImagePlaceholder: some View {
        VStack(spacing: 8) {
            ProgressView()
            Text(activeStatusText)
                .appFont(.caption, weight: .semibold)
        }
        .padding(20)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .foregroundStyle(.secondary)
        .multilineTextAlignment(.center)
        .accessibilityElement(children: .combine)
    }

    private var generationFailurePlaceholder: some View {
        VStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 28, weight: .semibold))
                .foregroundStyle(.red)
                .accessibilityHidden(true)
            Text(strings.generationFailed)
                .appFont(.body, weight: .semibold)
            if let errorMessage = job.errorMessage, !errorMessage.isEmpty {
                Text(errorMessage)
                    .appFont(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
                    .textSelection(.enabled)
            }
        }
        .padding(20)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .multilineTextAlignment(.center)
        .accessibilityElement(children: .combine)
    }

    private var imageUnavailablePlaceholder: some View {
        VStack(spacing: 8) {
            Image(systemName: "photo.badge.exclamationmark")
                .font(.system(size: 24, weight: .medium))
                .accessibilityHidden(true)
            Text(strings.imageUnavailable)
                .appFont(.caption, weight: .semibold)
        }
        .padding(20)
        .foregroundStyle(.secondary)
        .multilineTextAlignment(.center)
        .accessibilityElement(children: .combine)
    }

    private var heading: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .appFont(.headline)
                    .lineLimit(2)
                Label(displayStatusText, systemImage: statusSymbol)
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
                .id(imageRequestIdentity(imageURL))
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

    private func imageRequestIdentity(_ imageURL: URL) -> ResultCardImageRequestIdentity {
        ResultCardImageRequestIdentity(
            jobID: job.id,
            jobStatus: job.status,
            imageURL: imageURL
        )
    }

    private var activeStatusText: String {
        imageURL == nil
            ? (job.statusText ?? strings.generatingImage)
            : strings.finalizingImage
    }

    private var displayStatusText: String {
        job.isActive ? activeStatusText : (job.statusText ?? job.status)
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
    var generatingImage: String { localized("画像を生成しています...", "Generating image...") }
    var finalizingImage: String { localized("画像生成を完了しています...", "Finalizing image...") }
    var generationFailed: String { localized("画像生成に失敗しました", "Image generation failed") }
    var imageUnavailable: String { localized("画像を読み込めません", "Image unavailable") }
    var image: String { localized("画像", "Image") }
    var markForDeletion: String { localized("削除対象にする", "Mark for deletion") }
    var selectedForDeletion: String { localized("削除対象", "Selected for deletion") }
    var addToDeletion: String { localized("このrunを削除対象に追加", "Add this run to deletion") }
    var removeFromDeletion: String {
        localized("このrunを削除対象から外す", "Remove this run from deletion")
    }

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
