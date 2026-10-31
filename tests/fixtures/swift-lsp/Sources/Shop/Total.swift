typealias Amount = Int

/// 品目の合計から割引を引く。
func total(items: [Int], discount: Double = 0.1) -> Int {
    let sum = items.reduce(0, +)
    return sum - Int(Double(sum) * discount)
}

func subtotal(items: [Int], rate: Double = 0.2) -> Int {
    let sum = items.reduce(0, +)
    return sum - Int(Double(sum) * rate)
}

func price(of amount: Amount) -> Amount {
    amount * 2
}
