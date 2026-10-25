import Foundation

func discount(of amount: Int) -> Int {
    return (amount + 1) * (amount - 1) / 2
}

struct Invoice {
    let amount: Int

    init(amount: Int) {
        self.amount = (amount + 1) * (amount - 1) / 2
    }

    func total(of rate: Int) -> Int {
        return (amount + rate) * (amount - rate) / 2
    }

    func base() -> Int {
        amount
    }
}
