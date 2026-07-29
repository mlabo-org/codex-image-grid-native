import AppKit
import Foundation
import SwiftUI
import UniformTypeIdentifiers

enum ImageGridContract {
    static let defaultPrompt = "うちのBLOGのマスコットキャラとして魅力的に描く。デスクまわりの落ち着いた空気感、親しみやすい笑顔、透明感、やわらかい可愛らしさを大切にする。"
    static let defaultBatchPrompts = [
        defaultPrompt,
        "白背景のミニマルなサムネイルとして描く。透明感、清潔感、親しみやすい笑顔を重視する。",
        "夜のデスク環境で、落ち着いた雰囲気のBLOGマスコットとして描く。",
    ]
    static let counts = [1, 2, 3, 4, 6]
    static let maxPrompts = 12
    static let maxJobs = 24

    static func batchJobCount(prompts: [String], count: Int) -> Int {
        prompts.filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }.count * count
    }

    static func batchIsValid(prompts: [String], count: Int) -> Bool {
        let filled = prompts.filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        return !filled.isEmpty
            && prompts.count <= maxPrompts
            && filled.count * count <= maxJobs
    }
}

enum PromptMode: String, CaseIterable, Identifiable {
    case single
    case batch

    var id: String { rawValue }
}

enum ImageMood: String, CaseIterable, Identifiable {
    case warmMascot = "warm-mascot"
    case cleanThumbnail = "clean-thumbnail"
    case editorialSoft = "editorial-soft"
    case cinematic
    case minimalProduct = "minimal-product"

    var id: String { rawValue }
}

enum ImageEngine: String, CaseIterable, Identifiable {
    case appServerImage = "app-server-image"
    case codexSvg = "codex-svg"

    var id: String { rawValue }
}

enum AspectRatio: String, CaseIterable, Identifiable {
    case widescreen = "16:9"
    case landscape = "4:3"
    case square = "1:1"
    case portrait = "3:4"
    case tall = "9:16"

    var id: String { rawValue }
}

enum ResultLimit: String, CaseIterable, Identifiable {
    case six = "6"
    case twelve = "12"
    case twentyFour = "24"
    case fortyEight = "48"
    case ninetySix = "96"
    case all

    var id: String { rawValue }

    var completedLimit: Int? {
        self == .all ? nil : Int(rawValue)
    }
}

private enum ReferenceInteractionState: Equatable {
    case empty
    case preparing
    case ready
    case analyzing
    case analyzed
    case preparationError(ImageGridReferencePreparationError)
    case displayError(String)
}

struct ImageGridView: View {
    @Environment(\.appShellLanguage) private var language
    @AppStorage(AppShellPreferenceKeys.language) private var selectedLanguage =
        AppShellLanguage.system
    @AppStorage(AppShellPreferenceKeys.theme) private var selectedTheme = AppShellTheme.system
    @AppStorage("imageGrid.resultLimit") private var resultLimit = ResultLimit.twentyFour
    @AppStorage("imageGrid.showFailed") private var showFailed = false
    @AppStorage("imageGrid.promptHistory") private var promptHistoryData = "[]"

    @StateObject private var store = ImageGridStore()
    @StateObject private var draftPersistence = ImageGridDraftPersistence()
    @State private var referencePremise = ""
    @State private var prompt = ImageGridContract.defaultPrompt
    @State private var promptMode = PromptMode.single
    @State private var batchPrompts = ImageGridContract.defaultBatchPrompts
    @State private var mood = ImageMood.warmMascot
    @State private var engine = ImageEngine.appServerImage
    @State private var count = 1
    @State private var aspectRatio = AspectRatio.widescreen
    @State private var referenceImage: ImageGridReference?
    @State private var referenceState = ReferenceInteractionState.empty
    @State private var referenceLoadGeneration = 0
    @State private var formError: String?
    @State private var draftReady = false

    private var strings: ImageGridStrings {
        ImageGridStrings(language: language)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                header
                generationPanel
                resultsPanel
            }
            .padding(.horizontal, 32)
            .padding(.vertical, 28)
            .frame(maxWidth: 1480)
            .frame(maxWidth: .infinity)
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .frame(minHeight: 560)
        .onAppear {
            store.start()
        }
        .onDisappear {
            store.stop()
            if draftReady {
                draftPersistence.flush(draftMetadata)
            }
            let referenceToRemove = referenceImage
            Task {
                await draftPersistence.drain()
                referenceToRemove?.removeOwnedTemporaryFile()
            }
        }
        .task {
            await restoreDraft()
        }
        .onChange(of: draftMetadata) { _, metadata in
            guard draftReady else { return }
            draftPersistence.schedule(metadata)
        }
        .onChange(of: resultLimit) { _, value in
            if value == .all {
                Task {
                    await store.hydrateRuns()
                }
            }
        }
        .onPasteCommand(of: [.fileURL, .png, .jpeg, .webP]) { providers in
            pasteReference(providers: providers)
        }
    }

    private var header: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .center, spacing: 24) {
                brand
                Spacer(minLength: 20)
                headerControls
            }

            VStack(alignment: .leading, spacing: 16) {
                brand
                headerControls
            }
        }
    }

    private var brand: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text("CODEX APP SERVER")
                .appFont(.caption, weight: .bold)
                .foregroundStyle(Color.accentColor)
            Text("Image Grid")
                .appFont(.largeTitle, weight: .semibold)
        }
    }

    private var headerControls: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .center, spacing: 14) {
                languageSegment
                    .frame(minWidth: 180, idealWidth: 220)
                themeSegment
                    .frame(minWidth: 180, idealWidth: 220)
                RuntimePill(state: store.runtimeState, strings: strings)
            }

            VStack(alignment: .leading, spacing: 12) {
                languageSegment
                    .frame(maxWidth: .infinity)
                themeSegment
                    .frame(maxWidth: .infinity)
                RuntimePill(state: store.runtimeState, strings: strings)
            }
        }
    }

    private var languageSegment: some View {
        PreferenceSegment(
            title: strings.language,
            selection: $selectedLanguage,
            values: [.japanese, .english, .system],
            label: { strings.languageName($0) }
        )
    }

    private var themeSegment: some View {
        PreferenceSegment(
            title: strings.theme,
            selection: $selectedTheme,
            values: [.light, .dark, .system],
            label: { strings.themeName($0) }
        )
    }

    private var generationPanel: some View {
        Panel {
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .top, spacing: 16) {
                    generationForm
                        .frame(minWidth: 520, maxWidth: .infinity)
                    referencePanel
                        .frame(minWidth: 280, idealWidth: 380)
                }

                VStack(alignment: .leading, spacing: 18) {
                    generationForm
                    referencePanel
                }
            }
        }
    }

    private var generationForm: some View {
        VStack(alignment: .leading, spacing: 16) {
            LabeledControl(strings.referencePremise) {
                PlaceholderTextEditor(
                    text: $referencePremise,
                    placeholder: strings.referencePremisePlaceholder,
                    minHeight: 78
                )
            }

            promptSection

            LabeledControl(strings.mood) {
                Picker(strings.mood, selection: $mood) {
                    ForEach(ImageMood.allCases) { value in
                        Text(strings.moodName(value)).tag(value)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: .infinity)
            }

            ViewThatFits(in: .horizontal) {
                HStack(alignment: .top, spacing: 12) {
                    optionControls
                    Spacer(minLength: 0)
                }

                VStack(alignment: .leading, spacing: 12) {
                    optionControls
                }
            }
        }
    }

    @ViewBuilder
    private var optionControls: some View {
        LabeledControl(strings.engine) {
            Picker(strings.engine, selection: $engine) {
                ForEach(ImageEngine.allCases) { value in
                    Text(strings.engineName(value)).tag(value)
                }
            }
            .labelsHidden()
        }

        LabeledControl(strings.count) {
            Picker(strings.count, selection: $count) {
                ForEach(ImageGridContract.counts, id: \.self) { value in
                    Text(String(value)).tag(value)
                }
            }
            .labelsHidden()
        }

        LabeledControl(strings.aspectRatio) {
            Picker(strings.aspectRatio, selection: $aspectRatio) {
                ForEach(AspectRatio.allCases) { value in
                    Text(value.rawValue).tag(value)
                }
            }
            .labelsHidden()
            .disabled(engine == .codexSvg)
        }
    }

    private var promptSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .center, spacing: 10) {
                    promptModeControl
                    Spacer()
                    promptActions
                }

                VStack(alignment: .leading, spacing: 8) {
                    promptModeControl
                    promptActions
                }
            }

            if promptMode == .single {
                PlaceholderTextEditor(text: $prompt, placeholder: "", minHeight: 82)
            } else {
                batchEditor
            }
        }
    }

    private var promptModeControl: some View {
        HStack(alignment: .center, spacing: 10) {
            Text("Prompt")
                .appFont(.caption, weight: .semibold)
                .foregroundStyle(.secondary)
            Picker(strings.promptMode, selection: $promptMode) {
                Text(strings.singlePrompt).tag(PromptMode.single)
                Text(strings.batchPrompts).tag(PromptMode.batch)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(idealWidth: 132)
        }
    }

    private var promptActions: some View {
        HStack(spacing: 8) {
            Menu(strings.promptHistory) {
                if promptHistory.isEmpty {
                    Text(strings.noPromptHistory)
                } else {
                    ForEach(promptHistory, id: \.self) { entry in
                        Button(entry) {
                            applyPromptHistory(entry)
                        }
                    }
                }
            }
            .disabled(promptHistory.isEmpty)

            Button(strings.clearInput) {
                if promptMode == .single {
                    prompt = ""
                } else {
                    batchPrompts = [""]
                }
            }

            Button(strings.clearHistory) {
                promptHistoryData = "[]"
            }
            .disabled(promptHistory.isEmpty)
        }
    }

    private var batchEditor: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(strings.batchSummary(prompts: batchPrompts, count: count))
                    .appFont(.caption)
                    .foregroundStyle(.secondary)
                Spacer()
                Button(strings.addPrompt) {
                    if batchPrompts.count < ImageGridContract.maxPrompts {
                        batchPrompts.append("")
                    }
                }
                .disabled(batchPrompts.count >= ImageGridContract.maxPrompts)
            }

            ForEach(batchPrompts.indices, id: \.self) { index in
                VStack(alignment: .leading, spacing: 6) {
                    HStack {
                        Text("Prompt \(index + 1)")
                            .appFont(.caption, weight: .semibold)
                            .foregroundStyle(.secondary)
                        Spacer()
                        Button(strings.delete) {
                            batchPrompts.remove(at: index)
                        }
                        .disabled(batchPrompts.count == 1)
                    }
                    PlaceholderTextEditor(
                        text: Binding(
                            get: { batchPrompts[index] },
                            set: { batchPrompts[index] = $0 }
                        ),
                        placeholder: "",
                        minHeight: 64
                    )
                }
            }

            if !ImageGridContract.batchIsValid(prompts: batchPrompts, count: count) {
                Text(strings.batchLimitError)
                    .appFont(.caption)
                    .foregroundStyle(.red)
            }
        }
    }

    private var referencePanel: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(strings.referenceImage)
                .appFont(.caption, weight: .semibold)
                .foregroundStyle(.secondary)
            Text(strings.referenceImageHelp)
                .appFont(.caption)
                .foregroundStyle(.secondary)

            Text(referenceStatusText)
                .appFont(.caption, weight: .semibold)
                .foregroundStyle(referenceStatusIsError ? Color.red : Color.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityLabel(referenceStatusText)

            ReferenceDropZone(
                url: referenceImage?.url,
                isProcessing: referenceState == .preparing,
                strings: strings
            )
                .dropDestination(for: URL.self) { urls, _ in
                    guard let url = urls.first else {
                        return false
                    }
                    return selectReference(url)
                }

            ViewThatFits(in: .horizontal) {
                HStack(alignment: .center, spacing: 8) {
                    referenceSelectionStatus
                    Spacer(minLength: 8)
                    referenceActions
                }

                VStack(alignment: .leading, spacing: 8) {
                    referenceSelectionStatus
                    referenceActions
                }
            }

            if let message = formError ?? store.generationMessage {
                Text(message)
                    .appFont(.caption)
                    .foregroundStyle(.red)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }

            Button {
                submitGeneration()
            } label: {
                Label(strings.generate, systemImage: "play.fill")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .disabled(!generationIsValid || store.isSubmitting)
            .padding(.top, 8)
        }
    }

    private var referenceSelectionStatus: some View {
        Text(referenceImage.map { $0.url.lastPathComponent }
            ?? strings.noReferenceSelected)
            .appFont(.caption)
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .truncationMode(.middle)
    }

    private var referenceActions: some View {
        HStack(spacing: 8) {
            Button(strings.analyze) {
                analyzeReference()
            }
            .disabled(referenceImage == nil || referenceIsBusy)

            Button(strings.choose) {
                if let url = NativeFilePicker.chooseImageURL() {
                    _ = selectReference(url)
                }
            }
            .buttonStyle(.borderedProminent)

            Button(strings.paste) {
                pasteReference()
            }

            Button(strings.clear) {
                clearReference()
            }
        }
    }

    private var generationIsValid: Bool {
        if promptMode == .single {
            return !prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
        return ImageGridContract.batchIsValid(prompts: batchPrompts, count: count)
    }

    private func submitGeneration() {
        let prompts = promptMode == .single
            ? [prompt.trimmingCharacters(in: .whitespacesAndNewlines)].filter { !$0.isEmpty }
            : batchPrompts.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .filter { !$0.isEmpty }
        guard !prompts.isEmpty else {
            formError = strings.promptRequired
            return
        }
        guard promptMode != .batch
            || ImageGridContract.batchIsValid(prompts: batchPrompts, count: count)
        else {
            formError = strings.batchLimitError
            return
        }
        formError = nil
        store.generationMessage = nil
        let request = ImageGridGenerationRequest(
            prompt: prompts[0],
            prompts: promptMode == .batch ? prompts : nil,
            referencePremise: referencePremise,
            mood: mood.rawValue,
            engine: engine.rawValue,
            count: count,
            aspectRatio: aspectRatio.rawValue,
            referenceImagePath: referenceImage?.url.path
        )
        Task {
            if await store.generate(request: request, batch: promptMode == .batch) {
                savePromptHistory(prompts)
            }
        }
    }

    private var promptHistory: [String] {
        guard let data = promptHistoryData.data(using: .utf8),
              let decoded = try? JSONDecoder().decode([String].self, from: data)
        else {
            return []
        }
        return decoded.filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    }

    private func applyPromptHistory(_ entry: String) {
        if promptMode == .single {
            prompt = entry
        } else if batchPrompts.count < ImageGridContract.maxPrompts {
            batchPrompts.append(entry)
        }
    }

    private func savePromptHistory(_ prompts: [String]) {
        var next: [String] = []
        for value in prompts + promptHistory {
            let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmed.isEmpty, !next.contains(trimmed) {
                next.append(trimmed)
            }
        }
        next = Array(next.prefix(ImageGridContract.maxPrompts))
        if let data = try? JSONEncoder().encode(next),
           let encoded = String(data: data, encoding: .utf8)
        {
            promptHistoryData = encoded
        }
    }

    private func analyzeReference() {
        guard let referenceImage else { return }
        formError = nil
        referenceState = .analyzing
        let analyzedURL = referenceImage.url
        Task {
            if let premise = await store.analyze(reference: referenceImage) {
                guard self.referenceImage?.url == analyzedURL else { return }
                referencePremise = premise
                referenceState = .analyzed
            } else if self.referenceImage?.url == analyzedURL {
                referenceState = .displayError(
                    store.referenceAnalysisMessage ?? strings.analysisFailed
                )
            }
        }
    }

    private func pasteReference() {
        do {
            guard let pasted = try NativeReferencePasteboard.candidate() else {
                formError = strings.noPastedImage
                return
            }
            beginReferencePreparation(pasted)
        } catch let error as ImageGridReferencePreparationError {
            referenceState = .preparationError(error)
        } catch {
            referenceState = .preparationError(.preparationFailed)
        }
    }

    private func pasteReference(providers: [NSItemProvider]) {
        let generation = nextReferenceLoadGeneration()
        referenceState = .preparing
        Task {
            do {
                guard let candidate = try await NativeReferencePasteboard.candidate(
                    from: providers
                ) else {
                    guard generation == referenceLoadGeneration else { return }
                    referenceState = .displayError(strings.noPastedImage)
                    return
                }
                prepareReference(candidate, generation: generation)
            } catch let error as ImageGridReferencePreparationError {
                guard generation == referenceLoadGeneration else { return }
                referenceState = .preparationError(error)
            } catch {
                guard generation == referenceLoadGeneration else { return }
                referenceState = .preparationError(.preparationFailed)
            }
        }
    }

    private func selectReference(_ url: URL) -> Bool {
        guard url.isFileURL,
              ImageGridReference.supportedExtensions.contains(
                  url.pathExtension.lowercased()
              )
        else {
            referenceState = .preparationError(.unsupportedType)
            return false
        }
        beginReferencePreparation(
            ImageGridReferenceCandidate(url: url, ownsTemporaryFile: false)
        )
        return true
    }

    private func beginReferencePreparation(_ candidate: ImageGridReferenceCandidate) {
        let generation = nextReferenceLoadGeneration()
        referenceState = .preparing
        prepareReference(candidate, generation: generation)
    }

    private func prepareReference(
        _ candidate: ImageGridReferenceCandidate,
        generation: Int
    ) {
        Task {
            let result = await Task.detached(priority: .userInitiated) {
                do {
                    return Result<ImageGridReference, ImageGridReferencePreparationError>.success(
                        try ImageGridReference.prepare(candidate: candidate)
                    )
                } catch let error as ImageGridReferencePreparationError {
                    return .failure(error)
                } catch {
                    return .failure(.preparationFailed)
                }
            }.value
            guard generation == referenceLoadGeneration else {
                if case let .success(staleReference) = result {
                    staleReference.removeOwnedTemporaryFile()
                }
                return
            }
            switch result {
            case let .success(next):
                replaceReference(with: next)
            case let .failure(error):
                referenceState = .preparationError(error)
            }
        }
    }

    private func nextReferenceLoadGeneration() -> Int {
        referenceLoadGeneration += 1
        return referenceLoadGeneration
    }

    private func replaceReference(with next: ImageGridReference) {
        referenceImage?.removeOwnedTemporaryFile()
        referenceImage = next
        referenceState = .ready
        store.referenceAnalysisMessage = nil
        formError = nil
        draftPersistence.persistReference(next, metadata: draftMetadata)
    }

    private func clearReference() {
        referenceLoadGeneration += 1
        referenceImage?.removeOwnedTemporaryFile()
        referenceImage = nil
        referenceState = .empty
        referencePremise = ""
        store.referenceAnalysisMessage = nil
        formError = nil
        draftPersistence.clearReference(metadata: draftMetadata)
    }

    private var referenceIsBusy: Bool {
        referenceState == .preparing || referenceState == .analyzing
    }

    private var referenceStatusIsError: Bool {
        if case .preparationError = referenceState {
            return true
        }
        if case .displayError = referenceState {
            return true
        }
        return false
    }

    private var referenceStatusText: String {
        switch referenceState {
        case .empty:
            strings.referenceEmpty
        case .preparing:
            strings.referencePreparing
        case .ready:
            strings.referenceReady
        case .analyzing:
            strings.referenceAnalyzing
        case .analyzed:
            strings.referenceAnalyzed
        case let .preparationError(error):
            strings.referencePreparationError(error)
        case let .displayError(message):
            message
        }
    }

    private var draftMetadata: ImageGridDraftMetadata {
        ImageGridDraftMetadata(
            referencePremise: referencePremise,
            prompt: prompt,
            promptMode: promptMode.rawValue,
            batchPrompts: batchPrompts,
            mood: mood.rawValue,
            engine: engine.rawValue,
            count: count,
            aspectRatio: aspectRatio.rawValue,
            hasReferenceImage: referenceImage != nil,
            referenceStatusKey: draftReferenceStatusKey.rawValue
        )
    }

    private var draftReferenceStatusKey: ImageGridDraftReferenceStatusKey {
        switch referenceState {
        case .empty:
            .empty
        case .preparing:
            .preparing
        case .ready:
            .ready
        case .analyzing:
            .analyzing
        case .analyzed:
            .analyzed
        case .preparationError, .displayError:
            referenceImage == nil ? .empty : .ready
        }
    }

    private func restoreDraft() async {
        guard !draftReady else { return }
        let restoration = await draftPersistence.restore()
        guard !Task.isCancelled else {
            restoration.referenceImage?.removeOwnedTemporaryFile()
            return
        }
        let state = restoration.state
        referencePremise = state.referencePremise
        prompt = state.prompt
        promptMode = state.promptMode
        batchPrompts = state.batchPrompts
        mood = state.mood
        engine = state.engine
        count = state.count
        aspectRatio = state.aspectRatio
        referenceImage?.removeOwnedTemporaryFile()
        referenceImage = restoration.referenceImage
        switch state.referenceStatusKey {
        case .analyzed:
            referenceState = restoration.referenceImage == nil ? .empty : .analyzed
        case .empty:
            referenceState = .empty
        case .preparing, .ready, .analyzing:
            referenceState = restoration.referenceImage == nil ? .empty : .ready
        }
        draftReady = true
    }

    private var resultsPanel: some View {
        Panel {
            VStack(alignment: .leading, spacing: 16) {
                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .center, spacing: 12) {
                        resultsHeading
                        Spacer()
                        resultActions
                    }

                    VStack(alignment: .leading, spacing: 12) {
                        resultsHeading
                        resultActions
                    }
                }

                resultGrid
            }
        }
    }

    @ViewBuilder
    private var resultGrid: some View {
        let visible = store.visibleJobs(resultLimit: resultLimit, showFailed: showFailed)
        if visible.isEmpty {
            RoundedRectangle(cornerRadius: 8)
                .fill(Color(nsColor: .controlBackgroundColor).opacity(0.55))
                .frame(minHeight: 180)
                .overlay {
                    VStack(spacing: 8) {
                        Image(systemName: "photo.on.rectangle.angled")
                            .foregroundStyle(.secondary)
                            .accessibilityHidden(true)
                        Text(strings.readyMessage(engine: engine))
                            .appFont(.body)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                    }
                    .padding(24)
                }
        } else {
            ResponsiveResultGrid(minimumColumnWidth: 320, maximumColumns: 3, spacing: 16) {
                ForEach(visible) { job in
                    ResultCardView(
                        job: job,
                        imageURL: store.client.resolvedURL(job.imageUrl),
                        onCopy: { store.copyPrompt(job.prompt) },
                        onReveal: { store.reveal(job) },
                        onManifest: { store.openArtifact(job.manifestViewUrl) },
                        onHandoff: { store.openArtifact(job.handoffViewUrl) }
                    )
                    .frame(maxWidth: .infinity, alignment: .top)
                }
            }
            .frame(maxWidth: .infinity)
        }
    }

    private var resultsHeading: some View {
        let counts = store.counts
        return VStack(alignment: .leading, spacing: 4) {
            Text(strings.runs)
                .appFont(.title3, weight: .semibold)
            Text(strings.runSummary(
                done: counts.done,
                running: counts.running,
                failed: counts.failed
            ))
                .appFont(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var resultActions: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .center, spacing: 8) {
                resultLimitControl
                Toggle(strings.showFailed, isOn: $showFailed)
                    .toggleStyle(.checkbox)
                Button(strings.openInFinder) {
                    store.openGeneratedDirectory()
                }
                .disabled(store.generatedDirectory == nil)
                Button(strings.clearScreen) {
                    store.clearTerminalJobs()
                }
                .disabled(!store.hasTerminalJobs)
            }

            VStack(alignment: .leading, spacing: 8) {
                resultLimitControl
                Toggle(strings.showFailed, isOn: $showFailed)
                    .toggleStyle(.checkbox)
                HStack(spacing: 8) {
                    Button(strings.openInFinder) {
                        store.openGeneratedDirectory()
                    }
                    .disabled(store.generatedDirectory == nil)
                    Button(strings.clearScreen) {
                        store.clearTerminalJobs()
                    }
                    .disabled(!store.hasTerminalJobs)
                }
            }
        }
    }

    private var resultLimitControl: some View {
        HStack(spacing: 8) {
            Text(strings.latest)
                .appFont(.caption, weight: .semibold)
                .foregroundStyle(.secondary)
            Picker(strings.latest, selection: $resultLimit) {
                ForEach(ResultLimit.allCases) { value in
                    Text(strings.resultLimitName(value)).tag(value)
                }
            }
            .labelsHidden()
            .frame(idealWidth: 90)
        }
    }

}

struct ImageGridStrings {
    private let resolvedLanguage: AppShellLanguage

    init(language: AppShellLanguage) {
        resolvedLanguage = language.resolved
    }

    private func localized(_ japanese: String, _ english: String) -> String {
        resolvedLanguage == .japanese ? japanese : english
    }

    var language: String { localized("言語", "Language") }
    var theme: String { localized("テーマ", "Theme") }
    var referencePremise: String { localized("参照画像の前提", "Reference premise") }
    var referencePremisePlaceholder: String {
        localized("参照画像を解析すると、ここへ前提が入ります。", "Analyze a reference image to fill this.")
    }
    var promptMode: String { localized("Prompt モード", "Prompt mode") }
    var singlePrompt: String { localized("単一", "Single") }
    var batchPrompts: String { localized("一括", "Batch") }
    var promptHistory: String { localized("Prompt履歴", "Prompt history") }
    var noPromptHistory: String { localized("Prompt履歴はありません", "No prompt history") }
    var clearInput: String { localized("入力をクリア", "Clear input") }
    var clearHistory: String { localized("クリア", "Clear") }
    var addPrompt: String { localized("Promptを追加", "Add prompt") }
    var delete: String { localized("削除", "Delete") }
    var batchLimitError: String {
        localized("1回の実行は最大24ジョブです。", "A run is limited to 24 total jobs.")
    }
    var mood: String { localized("雰囲気", "Mood") }
    var engine: String { localized("エンジン", "Engine") }
    var count: String { localized("生成枚数", "Count") }
    var aspectRatio: String { localized("縦横比", "Aspect ratio") }
    var referenceImage: String { localized("参照画像", "Reference image") }
    var referenceImageHelp: String {
        localized(
            "画像を貼り付け、ドロップ、または選択してください。",
            "Paste, drop, or choose an image."
        )
    }
    var dropImage: String { localized("ここに画像をドロップ", "Drop image here") }
    var fileTypes: String { localized("PNG、JPEG、WebP", "PNG, JPEG, or WebP") }
    var noReferenceSelected: String {
        localized("参照画像は選択されていません", "No reference selected")
    }
    var analyze: String { localized("解析", "Analyze") }
    var choose: String { localized("選択", "Choose") }
    var paste: String { localized("貼り付け", "Paste") }
    var clear: String { localized("クリア", "Clear") }
    var generate: String { localized("生成", "Generate") }
    var promptRequired: String {
        localized("Promptを入力してください。", "Enter a prompt.")
    }
    var noPastedImage: String {
        localized(
            "クリップボードにPNG、JPEG、WebP画像または画像ファイルがありません。",
            "The clipboard does not contain a PNG, JPEG, WebP image, or image file."
        )
    }
    var referenceEmpty: String {
        localized(
            "参照画像は選択されていません。",
            "No reference image is selected."
        )
    }
    var referencePreparing: String {
        localized("参照画像を準備しています...", "Preparing reference image...")
    }
    var referenceAnalyzing: String {
        localized("参照画像を解析中...", "Analyzing reference image...")
    }
    var referenceAnalyzed: String {
        localized("参照画像を解析しました。", "Reference image analyzed.")
    }
    var analysisFailed: String {
        localized("参照画像を解析できませんでした。", "Reference analysis failed.")
    }
    var referenceReady: String {
        localized("参照画像を追加しました。", "Reference image added.")
    }
    func referencePreparationError(_ error: ImageGridReferencePreparationError) -> String {
        switch error {
        case .unsupportedType:
            localized(
                "PNG、JPEG、WebP画像を選択してください。",
                "Choose a PNG, JPEG, or WebP image."
            )
        case .tooLarge:
            localized(
                "参照画像は100MB以下にしてください。",
                "The reference image must be 100 MB or smaller."
            )
        case .unsafeDimensions:
            localized(
                "参照画像の寸法が大きすぎるため、安全に処理できません。",
                "The reference image dimensions are too large to process safely."
            )
        case .decodeFailed:
            localized(
                "参照画像を読み取れませんでした。",
                "The reference image could not be decoded."
            )
        case .preparationFailed:
            localized(
                "参照画像を準備できませんでした。",
                "The reference image could not be prepared."
            )
        }
    }
    var generationUnavailable: String {
        localized(
            "生成経路は次の runnable slice で接続します。",
            "Generation will be connected in the next runnable slice."
        )
    }
    var runs: String { localized("実行結果", "Runs") }
    var latest: String { localized("表示件数", "Latest") }
    var showFailed: String { localized("失敗も表示", "Show failed") }
    var openInFinder: String { localized("Finderで開く", "Open in Finder") }
    var clearScreen: String { localized("画面をクリア", "Clear screen") }

    func languageName(_ value: AppShellLanguage) -> String {
        switch value {
        case .japanese:
            "日本語"
        case .english:
            "English"
        case .system:
            localized("システム", "System")
        }
    }

    func themeName(_ value: AppShellTheme) -> String {
        switch value {
        case .light:
            localized("ライト", "Light")
        case .dark:
            localized("ダーク", "Dark")
        case .system:
            localized("システム", "System")
        }
    }

    func moodName(_ value: ImageMood) -> String {
        switch value {
        case .warmMascot:
            localized("あたたかいマスコット", "Warm mascot")
        case .cleanThumbnail:
            localized("すっきりしたサムネイル", "Clean thumbnail")
        case .editorialSoft:
            localized("やわらかい誌面風", "Editorial soft")
        case .cinematic:
            localized("シネマ調", "Cinematic")
        case .minimalProduct:
            localized("ミニマルな商品写真風", "Minimal product")
        }
    }

    func engineName(_ value: ImageEngine) -> String {
        switch value {
        case .appServerImage:
            "App Server Image"
        case .codexSvg:
            "Codex SVG"
        }
    }

    func batchSummary(prompts: [String], count: Int) -> String {
        let filled = prompts.filter {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }.count
        let jobs = filled * count
        return localized(
            "\(filled)件のPrompt × \(count)枚 = \(jobs)ジョブ",
            "\(filled) filled prompts × \(count) = \(jobs) jobs"
        )
    }

    func resultLimitName(_ value: ResultLimit) -> String {
        value == .all ? localized("すべて", "All") : value.rawValue
    }

    func runSummary(done: Int, running: Int, failed: Int) -> String {
        localized(
            "完了 \(done) · 実行中 \(running) · 失敗 \(failed)",
            "\(done) done · \(running) running · \(failed) failed"
        )
    }

    func readyMessage(engine: ImageEngine) -> String {
        if engine == .codexSvg {
            return localized(
                "Codex App Server経由でSVGサムネイルを並列生成できます。",
                "Ready to generate parallel SVG thumbnails through Codex App Server."
            )
        }
        return localized(
            "Codex App Server経由で画像を並列生成できます。",
            "Ready to generate parallel images through Codex App Server."
        )
    }

    func runtimeStatus(_ state: RuntimeConnectionState) -> String {
        switch state {
        case .idle:
            localized("App Server: 待機中", "App Server: Idle")
        case .starting:
            localized("App Server: 接続中...", "App Server: Starting...")
        case .ready:
            localized("App Server: 接続済み", "App Server: Ready")
        case .error:
            localized("App Server: エラー", "App Server: Error")
        case .disconnected:
            localized("Image Grid: 切断", "Image Grid: Disconnected")
        }
    }
}

private struct RuntimePill: View {
    let state: RuntimeConnectionState
    let strings: ImageGridStrings

    var body: some View {
        Text(strings.runtimeStatus(state))
            .appFont(.caption, weight: .semibold)
            .foregroundStyle(foregroundColor)
            .padding(.horizontal, 12)
            .frame(height: 32)
            .background(backgroundColor, in: RoundedRectangle(cornerRadius: 6))
            .overlay {
                RoundedRectangle(cornerRadius: 6)
                    .stroke(foregroundColor.opacity(0.5), lineWidth: 1)
            }
    }

    private var foregroundColor: Color {
        switch state {
        case .ready:
            .green
        case .starting:
            .accentColor
        case .error, .disconnected:
            .red
        case .idle:
            .secondary
        }
    }

    private var backgroundColor: Color {
        foregroundColor.opacity(0.08)
    }
}

private struct PreferenceSegment<Value: Hashable>: View {
    let title: String
    @Binding var selection: Value
    let values: [Value]
    let label: (Value) -> String

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title)
                .appFont(.caption, weight: .semibold)
                .foregroundStyle(.secondary)
            Picker(title, selection: $selection) {
                ForEach(values, id: \.self) { value in
                    Text(label(value)).tag(value)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
        }
    }
}

private struct LabeledControl<Content: View>: View {
    let title: String
    let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            Text(title)
                .appFont(.caption, weight: .semibold)
                .foregroundStyle(.secondary)
            content
                .frame(maxWidth: .infinity)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct PlaceholderTextEditor: View {
    @Binding var text: String
    let placeholder: String
    let minHeight: CGFloat

    var body: some View {
        ZStack(alignment: .topLeading) {
            TextEditor(text: $text)
                .scrollContentBackground(.hidden)
                .padding(6)
            if text.isEmpty, !placeholder.isEmpty {
                Text(placeholder)
                    .appFont(.caption)
                    .foregroundStyle(.tertiary)
                    .padding(.horizontal, 11)
                    .padding(.vertical, 13)
                    .allowsHitTesting(false)
            }
        }
        .frame(minHeight: minHeight)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
        .overlay {
            RoundedRectangle(cornerRadius: 6)
                .stroke(Color(nsColor: .separatorColor), lineWidth: 1)
        }
    }
}

private struct ReferenceDropZone: View {
    let url: URL?
    let isProcessing: Bool
    let strings: ImageGridStrings

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 6)
                .fill(Color(nsColor: .controlBackgroundColor))
            RoundedRectangle(cornerRadius: 6)
                .stroke(
                    Color(nsColor: .separatorColor),
                    style: StrokeStyle(lineWidth: 1, dash: [5, 4])
                )

            if let url, let image = NSImage(contentsOf: url) {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFit()
                    .padding(8)
            } else {
                VStack(spacing: 7) {
                    Image(systemName: "photo.badge.plus")
                        .foregroundStyle(.secondary)
                        .accessibilityHidden(true)
                    Text(strings.dropImage)
                        .appFont(.body, weight: .semibold)
                    Text(strings.fileTypes)
                        .appFont(.caption)
                        .foregroundStyle(.secondary)
                }
                .padding(16)
            }
        }
        .aspectRatio(16 / 9, contentMode: .fit)
        .frame(minHeight: 210)
        .overlay {
            if isProcessing {
                ZStack {
                    Color(nsColor: .windowBackgroundColor).opacity(0.72)
                    ProgressView()
                        .controlSize(.large)
                        .accessibilityLabel(strings.referencePreparing)
                }
                .clipShape(RoundedRectangle(cornerRadius: 6))
            }
        }
    }
}

private struct Panel<Content: View>: View {
    let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        content
            .padding(18)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
            .overlay {
                RoundedRectangle(cornerRadius: 8)
                    .stroke(Color(nsColor: .separatorColor).opacity(0.8), lineWidth: 1)
            }
    }
}
