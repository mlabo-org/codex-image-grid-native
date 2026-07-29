import AppKit
import Foundation
import Testing
@testable import CodexImageGridNative

@Test @MainActor
func draftMetadataRoundTripAndInvalidValuesUseFrozenDefaults() async throws {
    let supportDirectory = try isolatedDraftSupportDirectory()
    defer {
        try? FileManager.default.removeItem(at: supportDirectory)
    }
    let persistence = ImageGridDraftPersistence(
        applicationSupportDirectory: supportDirectory
    )
    #expect(ImageGridDraftPersistence.debounceMilliseconds == 250)

    let first = ImageGridDraftMetadata(
        referencePremise: "first premise",
        prompt: "first prompt",
        promptMode: PromptMode.batch.rawValue,
        batchPrompts: ["one", "two"],
        mood: ImageMood.cinematic.rawValue,
        engine: ImageEngine.codexSvg.rawValue,
        count: 4,
        aspectRatio: AspectRatio.square.rawValue,
        hasReferenceImage: false,
        referenceStatusKey: ImageGridDraftReferenceStatusKey.empty.rawValue
    )
    var second = first
    second.prompt = "latest prompt"
    persistence.schedule(first)
    persistence.schedule(second)
    await persistence.drain()

    let roundTrip = await persistence.restore().state
    #expect(roundTrip.referencePremise == "first premise")
    #expect(roundTrip.prompt == "latest prompt")
    #expect(roundTrip.promptMode == .batch)
    #expect(roundTrip.batchPrompts == ["one", "two"])
    #expect(roundTrip.mood == .cinematic)
    #expect(roundTrip.engine == .codexSvg)
    #expect(roundTrip.count == 4)
    #expect(roundTrip.aspectRatio == .square)
    #expect(!roundTrip.hasReferenceImage)

    let invalid = ImageGridDraftMetadata(
        referencePremise: "keep premise",
        prompt: "keep prompt",
        promptMode: "invalid-mode",
        batchPrompts: [],
        mood: "invalid-mood",
        engine: "invalid-engine",
        count: 5,
        aspectRatio: "invalid-ratio",
        hasReferenceImage: false,
        referenceStatusKey: "invalid-status"
    )
    persistence.flush(invalid)
    await persistence.drain()

    let validated = await persistence.restore().state
    #expect(validated.referencePremise == "keep premise")
    #expect(validated.prompt == "keep prompt")
    #expect(validated.promptMode == .single)
    #expect(validated.batchPrompts == [""])
    #expect(validated.mood == .warmMascot)
    #expect(validated.engine == .appServerImage)
    #expect(validated.count == 1)
    #expect(validated.aspectRatio == .widescreen)
    #expect(validated.referenceStatusKey == .empty)
}

@Test @MainActor
func draftReferenceCopyDeleteAndInvalidRestorationAreIsolated() async throws {
    let supportDirectory = try isolatedDraftSupportDirectory()
    defer {
        try? FileManager.default.removeItem(at: supportDirectory)
    }
    let persistence = ImageGridDraftPersistence(
        applicationSupportDirectory: supportDirectory
    )
    let sourceDirectory = supportDirectory.appendingPathComponent(
        "source",
        isDirectory: true
    )
    try FileManager.default.createDirectory(
        at: sourceDirectory,
        withIntermediateDirectories: true
    )
    let validSource = sourceDirectory.appendingPathComponent("selected.png")
    try testPNGData().write(to: validSource)
    let attached = ImageGridDraftMetadata(
        referencePremise: "preserved premise",
        prompt: "preserved prompt",
        promptMode: PromptMode.single.rawValue,
        batchPrompts: ["batch"],
        mood: ImageMood.editorialSoft.rawValue,
        engine: ImageEngine.appServerImage.rawValue,
        count: 2,
        aspectRatio: AspectRatio.landscape.rawValue,
        hasReferenceImage: true,
        referenceStatusKey: ImageGridDraftReferenceStatusKey.analyzing.rawValue
    )

    persistence.persistReference(at: validSource, metadata: attached)
    await persistence.drain()
    let persistedReference = persistence.draftDirectory
        .appendingPathComponent("reference.png")
    #expect(FileManager.default.fileExists(atPath: persistedReference.path))

    let restored = await persistence.restore()
    #expect(restored.state.prompt == "preserved prompt")
    #expect(restored.state.referenceStatusKey == .ready)
    #expect(restored.referenceImage != nil)
    restored.referenceImage?.removeOwnedTemporaryFile()

    var cleared = attached
    cleared.referencePremise = ""
    cleared.hasReferenceImage = false
    cleared.referenceStatusKey = ImageGridDraftReferenceStatusKey.empty.rawValue
    persistence.clearReference(metadata: cleared)
    await persistence.drain()
    #expect(!FileManager.default.fileExists(atPath: persistedReference.path))
    let clearedRestoration = await persistence.restore()
    #expect(clearedRestoration.referenceImage == nil)

    let invalidSource = sourceDirectory.appendingPathComponent("broken.png")
    try Data([0x89, 0x50, 0x4e, 0x47]).write(to: invalidSource)
    persistence.persistReference(at: invalidSource, metadata: attached)
    await persistence.drain()
    #expect(FileManager.default.fileExists(atPath: persistedReference.path))

    let invalidRestoration = await persistence.restore()
    #expect(invalidRestoration.state.referencePremise == "preserved premise")
    #expect(invalidRestoration.state.prompt == "preserved prompt")
    #expect(!invalidRestoration.state.hasReferenceImage)
    #expect(invalidRestoration.state.referenceStatusKey == .empty)
    #expect(invalidRestoration.referenceImage == nil)
    #expect(!FileManager.default.fileExists(atPath: persistedReference.path))
}

private func isolatedDraftSupportDirectory() throws -> URL {
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent(
            "codex-image-grid-native-draft-tests-\(UUID().uuidString)",
            isDirectory: true
        )
    try FileManager.default.createDirectory(
        at: directory,
        withIntermediateDirectories: true
    )
    return directory
}

private func testPNGData() throws -> Data {
    guard let bitmap = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: 1,
        pixelsHigh: 1,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bitmapFormat: [],
        bytesPerRow: 4,
        bitsPerPixel: 32
    ), let data = bitmap.representation(using: .png, properties: [:])
    else {
        throw ImageGridReferencePreparationError.preparationFailed
    }
    return data
}
