import Foundation
import SwiftUI

enum ImageGridDraftReferenceStatusKey: String, Codable, Equatable, Sendable {
    case empty = "referenceEmpty"
    case preparing = "referenceProcessing"
    case ready = "referenceAttached"
    case analyzing = "referenceAnalyzing"
    case analyzed = "referenceAnalyzed"

    func normalizedForRestoration(hasReferenceImage: Bool) -> Self {
        guard hasReferenceImage else {
            return .empty
        }
        return self == .analyzed ? .analyzed : .ready
    }
}

struct ImageGridDraftMetadata: Codable, Equatable, Sendable {
    static let schemaVersion = 1

    var version = Self.schemaVersion
    var referencePremise: String
    var prompt: String
    var promptMode: String
    var batchPrompts: [String]
    var mood: String
    var engine: String
    var count: Int
    var aspectRatio: String
    var hasReferenceImage: Bool
    var referenceStatusKey: String
    var referenceFileName: String?

    init(
        referencePremise: String,
        prompt: String,
        promptMode: String,
        batchPrompts: [String],
        mood: String,
        engine: String,
        count: Int,
        aspectRatio: String,
        hasReferenceImage: Bool,
        referenceStatusKey: String,
        referenceFileName: String? = nil
    ) {
        self.referencePremise = referencePremise
        self.prompt = prompt
        self.promptMode = promptMode
        self.batchPrompts = batchPrompts
        self.mood = mood
        self.engine = engine
        self.count = count
        self.aspectRatio = aspectRatio
        self.hasReferenceImage = hasReferenceImage
        self.referenceStatusKey = referenceStatusKey
        self.referenceFileName = referenceFileName
    }

    static var defaults: Self {
        Self(
            referencePremise: "",
            prompt: ImageGridContract.defaultPrompt,
            promptMode: PromptMode.single.rawValue,
            batchPrompts: ImageGridContract.defaultBatchPrompts,
            mood: ImageMood.warmMascot.rawValue,
            engine: ImageEngine.appServerImage.rawValue,
            count: 1,
            aspectRatio: AspectRatio.widescreen.rawValue,
            hasReferenceImage: false,
            referenceStatusKey: ImageGridDraftReferenceStatusKey.empty.rawValue
        )
    }

    func validated() -> ImageGridDraftState {
        var prompts = Array(batchPrompts.prefix(ImageGridContract.maxPrompts))
        if prompts.isEmpty {
            prompts = [""]
        }
        let hasReference = hasReferenceImage
        let status = (
            ImageGridDraftReferenceStatusKey(rawValue: referenceStatusKey)
                ?? (hasReference ? .ready : .empty)
        ).normalizedForRestoration(hasReferenceImage: hasReference)

        return ImageGridDraftState(
            referencePremise: referencePremise,
            prompt: prompt,
            promptMode: PromptMode(rawValue: promptMode) ?? .single,
            batchPrompts: prompts,
            mood: ImageMood(rawValue: mood) ?? .warmMascot,
            engine: ImageEngine(rawValue: engine) ?? .appServerImage,
            count: ImageGridContract.counts.contains(count) ? count : 1,
            aspectRatio: AspectRatio(rawValue: aspectRatio) ?? .widescreen,
            hasReferenceImage: hasReference,
            referenceStatusKey: status
        )
    }
}

struct ImageGridDraftState: Equatable, Sendable {
    var referencePremise: String
    var prompt: String
    var promptMode: PromptMode
    var batchPrompts: [String]
    var mood: ImageMood
    var engine: ImageEngine
    var count: Int
    var aspectRatio: AspectRatio
    var hasReferenceImage: Bool
    var referenceStatusKey: ImageGridDraftReferenceStatusKey

    static var defaults: Self {
        ImageGridDraftMetadata.defaults.validated()
    }

    func metadata(referenceFileName: String? = nil) -> ImageGridDraftMetadata {
        ImageGridDraftMetadata(
            referencePremise: referencePremise,
            prompt: prompt,
            promptMode: promptMode.rawValue,
            batchPrompts: batchPrompts,
            mood: mood.rawValue,
            engine: engine.rawValue,
            count: count,
            aspectRatio: aspectRatio.rawValue,
            hasReferenceImage: hasReferenceImage,
            referenceStatusKey: referenceStatusKey.rawValue,
            referenceFileName: referenceFileName
        )
    }

    func withoutReference() -> Self {
        var copy = self
        copy.hasReferenceImage = false
        copy.referenceStatusKey = .empty
        return copy
    }
}

struct ImageGridDraftRestoration: Sendable {
    let state: ImageGridDraftState
    let referenceImage: ImageGridReference?
}

@MainActor
final class ImageGridDraftPersistence: ObservableObject {
    static let debounceMilliseconds = 250

    let draftDirectory: URL
    let metadataURL: URL
    private let fileStore: ImageGridDraftFileStore
    private var pendingMetadata: ImageGridDraftMetadata?
    private var debounceTask: Task<Void, Never>?
    private var operationTail: Task<Void, Never>?
    private(set) var lastErrorDescription: String?

    init(applicationSupportDirectory: URL? = nil) {
        let supportDirectory = applicationSupportDirectory
            ?? FileManager.default.urls(
                for: .applicationSupportDirectory,
                in: .userDomainMask
            ).first!
        let directory = supportDirectory
            .appendingPathComponent("codex-image-grid", isDirectory: true)
            .appendingPathComponent("ui-draft", isDirectory: true)
        draftDirectory = directory
        metadataURL = directory.appendingPathComponent("draft.json", isDirectory: false)
        fileStore = ImageGridDraftFileStore(draftDirectory: directory)
    }

    func schedule(_ metadata: ImageGridDraftMetadata) {
        pendingMetadata = metadata
        debounceTask?.cancel()
        debounceTask = Task { @MainActor [weak self] in
            do {
                try await Task.sleep(
                    for: .milliseconds(Self.debounceMilliseconds)
                )
            } catch {
                return
            }
            guard let self, !Task.isCancelled, let snapshot = pendingMetadata else {
                return
            }
            pendingMetadata = nil
            enqueue { store in
                try await store.writeMetadata(snapshot)
            }
        }
    }

    func persistReference(
        _ referenceImage: ImageGridReference,
        metadata: ImageGridDraftMetadata
    ) {
        persistReference(at: referenceImage.url, metadata: metadata)
    }

    func persistReference(at sourceURL: URL, metadata: ImageGridDraftMetadata) {
        cancelPendingDebounce()
        enqueue { store in
            try await store.replaceReference(from: sourceURL, metadata: metadata)
        }
    }

    func clearReference(metadata: ImageGridDraftMetadata) {
        cancelPendingDebounce()
        enqueue { store in
            try await store.clearReference(metadata: metadata)
        }
    }

    func flush(_ metadata: ImageGridDraftMetadata) {
        cancelPendingDebounce()
        enqueue { store in
            try await store.writeMetadata(metadata)
        }
    }

    func restore() async -> ImageGridDraftRestoration {
        await drain()
        do {
            let restoration = try await fileStore.restore()
            lastErrorDescription = nil
            return restoration
        } catch {
            lastErrorDescription = error.localizedDescription
            return ImageGridDraftRestoration(
                state: .defaults,
                referenceImage: nil
            )
        }
    }

    func drain() async {
        if let debounceTask {
            await debounceTask.value
        }
        if let operationTail {
            await operationTail.value
        }
    }

    private func cancelPendingDebounce() {
        debounceTask?.cancel()
        debounceTask = nil
        pendingMetadata = nil
    }

    private func enqueue(
        _ operation: @escaping @Sendable (ImageGridDraftFileStore) async throws -> Void
    ) {
        let previous = operationTail
        let store = fileStore
        operationTail = Task { @MainActor [weak self] in
            if let previous {
                await previous.value
            }
            do {
                try await operation(store)
                self?.lastErrorDescription = nil
            } catch {
                self?.lastErrorDescription = error.localizedDescription
            }
        }
    }
}

private actor ImageGridDraftFileStore {
    private static let referenceFileNames = [
        "reference.png",
        "reference.jpg",
        "reference.webp",
    ]

    private let draftDirectory: URL
    private let metadataURL: URL
    private var activeReferenceFileName: String?

    init(draftDirectory: URL) {
        self.draftDirectory = draftDirectory
        metadataURL = draftDirectory.appendingPathComponent("draft.json", isDirectory: false)
    }

    func writeMetadata(_ metadata: ImageGridDraftMetadata) throws {
        var snapshot = metadata
        if snapshot.hasReferenceImage {
            snapshot.referenceFileName = validReferenceFileName(snapshot.referenceFileName)
                ?? activeReferenceFileName
                ?? discoverReferenceFileName()
            if snapshot.referenceFileName == nil {
                snapshot.hasReferenceImage = false
                snapshot.referenceStatusKey = ImageGridDraftReferenceStatusKey.empty.rawValue
            }
        } else {
            snapshot.referenceFileName = nil
        }
        try writeMetadataFile(snapshot)
    }

    func replaceReference(
        from sourceURL: URL,
        metadata: ImageGridDraftMetadata
    ) throws {
        let fileName = try referenceFileName(for: sourceURL)
        try FileManager.default.createDirectory(
            at: draftDirectory,
            withIntermediateDirectories: true
        )
        let destinationURL = draftDirectory.appendingPathComponent(fileName)
        let temporaryURL = draftDirectory.appendingPathComponent(
            ".reference-\(UUID().uuidString).tmp"
        )
        do {
            try FileManager.default.copyItem(at: sourceURL, to: temporaryURL)
            let values = try temporaryURL.resourceValues(
                forKeys: [.isRegularFileKey, .fileSizeKey]
            )
            guard values.isRegularFile == true, (values.fileSize ?? 0) > 0 else {
                throw ImageGridReferencePreparationError.preparationFailed
            }
            if FileManager.default.fileExists(atPath: destinationURL.path) {
                _ = try FileManager.default.replaceItemAt(
                    destinationURL,
                    withItemAt: temporaryURL
                )
            } else {
                try FileManager.default.moveItem(at: temporaryURL, to: destinationURL)
            }
        } catch {
            try? FileManager.default.removeItem(at: temporaryURL)
            throw error
        }

        activeReferenceFileName = fileName
        var snapshot = metadata
        snapshot.hasReferenceImage = true
        snapshot.referenceFileName = fileName
        snapshot.referenceStatusKey = (
            ImageGridDraftReferenceStatusKey(rawValue: snapshot.referenceStatusKey) ?? .ready
        ).normalizedForRestoration(hasReferenceImage: true).rawValue
        try writeMetadataFile(snapshot)
        try removeReferenceFiles(except: fileName)
    }

    func clearReference(metadata: ImageGridDraftMetadata) throws {
        try removeReferenceFiles(except: nil)
        activeReferenceFileName = nil
        var snapshot = metadata
        snapshot.hasReferenceImage = false
        snapshot.referenceFileName = nil
        snapshot.referenceStatusKey = ImageGridDraftReferenceStatusKey.empty.rawValue
        try writeMetadataFile(snapshot)
    }

    func restore() throws -> ImageGridDraftRestoration {
        guard let data = try? Data(contentsOf: metadataURL),
              let metadata = try? JSONDecoder().decode(
                  ImageGridDraftMetadata.self,
                  from: data
              )
        else {
            return ImageGridDraftRestoration(
                state: .defaults,
                referenceImage: nil
            )
        }

        let state = metadata.validated()
        guard state.hasReferenceImage else {
            try removeReferenceFiles(except: nil)
            activeReferenceFileName = nil
            let normalized = state.withoutReference()
            try writeMetadataFile(normalized.metadata())
            return ImageGridDraftRestoration(
                state: normalized,
                referenceImage: nil
            )
        }

        guard let fileName = validReferenceFileName(metadata.referenceFileName) else {
            return try removeInvalidReference(keeping: state)
        }
        let referenceURL = draftDirectory.appendingPathComponent(fileName)
        do {
            let reference = try ImageGridReference.prepare(url: referenceURL)
            activeReferenceFileName = fileName
            var normalized = state
            normalized.hasReferenceImage = true
            normalized.referenceStatusKey = state.referenceStatusKey
                .normalizedForRestoration(hasReferenceImage: true)
            try writeMetadataFile(normalized.metadata(referenceFileName: fileName))
            try removeReferenceFiles(except: fileName)
            return ImageGridDraftRestoration(
                state: normalized,
                referenceImage: reference
            )
        } catch {
            return try removeInvalidReference(keeping: state)
        }
    }

    private func removeInvalidReference(
        keeping state: ImageGridDraftState
    ) throws -> ImageGridDraftRestoration {
        try removeReferenceFiles(except: nil)
        activeReferenceFileName = nil
        let normalized = state.withoutReference()
        try writeMetadataFile(normalized.metadata())
        return ImageGridDraftRestoration(
            state: normalized,
            referenceImage: nil
        )
    }

    private func referenceFileName(for sourceURL: URL) throws -> String {
        switch sourceURL.pathExtension.lowercased() {
        case "png":
            "reference.png"
        case "jpg", "jpeg":
            "reference.jpg"
        case "webp":
            "reference.webp"
        default:
            throw ImageGridReferencePreparationError.unsupportedType
        }
    }

    private func validReferenceFileName(_ value: String?) -> String? {
        guard let value, Self.referenceFileNames.contains(value) else {
            return nil
        }
        return value
    }

    private func discoverReferenceFileName() -> String? {
        Self.referenceFileNames.first { fileName in
            FileManager.default.fileExists(
                atPath: draftDirectory.appendingPathComponent(fileName).path
            )
        }
    }

    private func removeReferenceFiles(except retainedFileName: String?) throws {
        for fileName in Self.referenceFileNames where fileName != retainedFileName {
            let url = draftDirectory.appendingPathComponent(fileName)
            if FileManager.default.fileExists(atPath: url.path) {
                try FileManager.default.removeItem(at: url)
            }
        }
    }

    private func writeMetadataFile(_ metadata: ImageGridDraftMetadata) throws {
        try FileManager.default.createDirectory(
            at: draftDirectory,
            withIntermediateDirectories: true
        )
        let data = try JSONEncoder().encode(metadata)
        try data.write(to: metadataURL, options: .atomic)
    }
}
