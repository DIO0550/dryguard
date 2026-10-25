import Foundation

func reorder(of quantity: Int) -> Int {
    return (quantity + 1) * (quantity - 1) / 2
}

struct Stock {
    let quantity: Int

    init(quantity: Int) {
        self.quantity = (quantity + 1) * (quantity - 1) / 2
    }

    func shortage(of demand: Int) -> Int {
        return (quantity + demand) * (quantity - demand) / 2
    }

    func level() -> Int {
        quantity
    }
}
