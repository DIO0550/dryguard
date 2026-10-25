import Testing

@Test func discountOfThreeIsFour() {
    let value = discount(of: 3)
    let expected = (3 + 1) * (3 - 1) / 2
    check(value, expected)
}
