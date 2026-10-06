import Foundation
import Testing
@testable import CodexImageGridNative

@Test func editingRequestPreservesOperationAndInstructionsDuringReferenceLease() throws {
    let original = ImageGridGenerationRequest(
        prompt: "ハンバーガーだけクレープに変更して",
        prompts: ["ハンバーガーだけクレープに変更して", "飲み物だけ変更して"],
        referencePremise: "generation character notes",
        mood: "warm-mascot",
        engine: "app-server-image",
        count: 2,
        aspectRatio: "16:9",
        referenceImagePath: "/tmp/original.png",
        operation: .edit
    )
    let leased = original.usingReferenceImagePath("/tmp/leased-source.png")
    let object = try #require(JSONSerialization.jsonObject(
        with: JSONEncoder().encode(leased)
    ) as? [String: Any])
    #expect(object["operation"] as? String == "edit")
    #expect(object["referenceImagePath"] as? String == "/tmp/leased-source.png")
    #expect(object["prompt"] as? String == original.prompt)
    #expect(object["prompts"] as? [String] == original.prompts)
    #expect(object["count"] as? Int == 2)
    #expect(original.referenceImagePath == "/tmp/original.png")
}

@Test func editingRequiresSourceAndSupportedEngineWhileGenerationIsUnchanged() {
    #expect(ImageGridOperation.edit.canSubmit(engine: "app-server-image", referenceImagePath: "/tmp/source.png"))
    #expect(!ImageGridOperation.edit.canSubmit(engine: "app-server-image", referenceImagePath: nil))
    #expect(!ImageGridOperation.edit.canSubmit(engine: "app-server-image", referenceImagePath: "  "))
    #expect(!ImageGridOperation.edit.canSubmit(engine: "codex-svg", referenceImagePath: "/tmp/source.png"))
    #expect(ImageGridOperation.edit.canSubmit(engine: "grok-imagine", referenceImagePath: "/tmp/source.png"))
    #expect(ImageGridOperation.generate.canSubmit(engine: "codex-svg", referenceImagePath: nil))
}

@Test func editingDraftRoundTripAndLegacyDraftMigrationPreserveGenerationDefaults() throws {
    var metadata = ImageGridDraftMetadata.defaults
    metadata.operation = "edit"
    metadata.referencePremise = "preserve character notes for generation"
    let edited = try JSONDecoder().decode(ImageGridDraftMetadata.self, from: JSONEncoder().encode(metadata))
    #expect(edited.validated().operation == .edit)
    #expect(edited.validated().metadata().operation == "edit")
    #expect(edited.validated().referencePremise == metadata.referencePremise)
    var legacyObject = try #require(JSONSerialization.jsonObject(with: JSONEncoder().encode(metadata)) as? [String: Any])
    legacyObject.removeValue(forKey: "operation")
    let legacy = try JSONDecoder().decode(ImageGridDraftMetadata.self, from: JSONSerialization.data(withJSONObject: legacyObject))
    #expect(legacy.validated().operation == .generate)
    #expect(legacy.validated().mood == .warmMascot)
    #expect(legacy.validated().aspectRatio == .widescreen)
}

@Test func oldAndEditedJobsDecodeWithCorrectOperation() throws {
    let old = try JSONDecoder().decode(ImageGridJob.self, from: Data(#"{"id":"old","status":"done"}"#.utf8))
    let edited = try JSONDecoder().decode(ImageGridJob.self, from: Data(#"{"id":"new","status":"done","operation":"edit"}"#.utf8))
    #expect(old.resolvedOperation == .generate)
    #expect(edited.resolvedOperation == .edit)
}
