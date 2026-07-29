import Testing
@testable import CodexImageGridNative

@Test func frozenUiChoicesAndDefaults() {
    #expect(ImageGridContract.counts == [1, 2, 3, 4, 6])
    #expect(ImageMood.allCases.map(\.rawValue) == [
        "warm-mascot",
        "clean-thumbnail",
        "editorial-soft",
        "cinematic",
        "minimal-product",
    ])
    #expect(AspectRatio.allCases.map(\.rawValue) == ["16:9", "4:3", "1:1", "3:4", "9:16"])
    #expect(ResultLimit.allCases.map(\.rawValue) == ["6", "12", "24", "48", "96", "all"])
    #expect(ImageGridContract.defaultBatchPrompts.count == 3)
}

@Test func batchJobLimitMatchesTheFrozenRenderer() {
    let fourPrompts = Array(repeating: "prompt", count: 4)
    let fivePrompts = Array(repeating: "prompt", count: 5)

    #expect(ImageGridContract.batchJobCount(prompts: fourPrompts, count: 6) == 24)
    #expect(ImageGridContract.batchIsValid(prompts: fourPrompts, count: 6))
    #expect(!ImageGridContract.batchIsValid(prompts: fivePrompts, count: 6))
}
