import Testing

@Suite struct SmokeTests {
    @Test("test target builds and runs")
    func smoke() {
        #expect(1 + 1 == 2)
    }
}
